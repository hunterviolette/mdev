use std::{
    collections::{HashMap, HashSet},
    fs,
    io::BufReader,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener as StdTcpListener},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use anyhow::{bail, Context, Result};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use rcgen::generate_simple_self_signed;
use reqwest::{Certificate, Client, ClientBuilder, Identity};
use axum_server::tls_rustls::RustlsConfig;
use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer},
    server::WebPkiClientVerifier,
    RootCertStore, ServerConfig,
};
use rustls_pki_types::pem::PemObject;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use tokio::sync::RwLock;
use tokio::{
    net::{TcpListener, TcpStream},
    time::sleep,
};
use uuid::Uuid;

use crate::engine::{
    capabilities::{
        changeset::apply::execute_changeset_apply,
        context_export::{build_existing_context_sync_snapshot, ContextSyncSnapshot},
        filesystem,
        registry::{find_result, CapabilityContext, CapabilityInvocationRequest, CapabilityResult},
    },
    runtime_endpoints::{NetworkExposure, RuntimeEndpointManager},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncDirection {
    Send,
    Receive,
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    Manual,
    AutoApply,
}

impl Default for SyncMode {
    fn default() -> Self {
        Self::Manual
    }
}

impl SyncDirection {
    pub fn can_send(&self) -> bool {
        matches!(self, Self::Send | Self::Both)
    }

    pub fn can_receive(&self) -> bool {
        matches!(self, Self::Receive | Self::Both)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncMapping {
    pub id: String,
    pub workflow_run_id: String,
    #[serde(default)]
    pub link_id: String,
    pub peer_ipv4: IpAddr,
    #[serde(default)]
    pub peer_port: Option<u16>,
    pub direction: SyncDirection,
    #[serde(default)]
    pub peer_certificate_pem: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub sync_mode: SyncMode,
    #[serde(default)]
    pub state_revision: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub connected: bool,
}

const PAIRING_CONTROL_PORT: u16 = 47831;
const PAIRING_TTL_SECONDS: u64 = 300;
const PEER_MESSAGE_TTL_MS: u64 = 15 * 60 * 1000;
const PEER_MESSAGE_MAX_BYTES: usize = 1536 * 1024 * 1024;
const PEER_MESSAGE_MAX_RETAINED_BYTES: usize = 2048 * 1024 * 1024;
const PEER_MESSAGE_MAX_COUNT: usize = 200;
const CONNECTION_LEASE_MS: u64 = 4 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingSession {
    pub id: String,
    pub mapping_id: String,
    #[serde(default)]
    pub peer_pairing_id: String,
    pub verification_code: String,
    pub peer_ipv4: IpAddr,
    pub local_port: u16,
    pub peer_port: Option<u16>,
    pub expires_at_unix_ms: u128,
    pub local_certificate_pem: String,
    #[serde(default)]
    pub peer_certificate_pem: String,
    #[serde(default)]
    pub local_confirmed: bool,
    #[serde(default)]
    pub remote_confirmed: bool,
    #[serde(default)]
    pub complete: bool,
    #[serde(skip)]
    pairing_proof: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoSyncStatus {
    pub identity_ready: bool,
    pub certificate_fingerprint: String,
    pub local_ipv4: Option<IpAddr>,
    pub pairing_listener_running: bool,
    pub pairing_listener_port: Option<u16>,
    pub pairings: Vec<PairingSession>,
    pub mappings: Vec<SyncMapping>,
}

#[derive(Debug, Clone)]
pub struct LocalSyncIdentity {
    pub certificate_pem: String,
    pub private_key_pem: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PairingJoinRequest {
    proof: String,
    #[serde(default)]
    pairing_id: String,
    certificate_pem: String,
    sync_port: u16,
    confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PairingJoinResponse {
    matched: bool,
    #[serde(default)]
    pairing_id: String,
    #[serde(default)]
    certificate_pem: String,
    #[serde(default)]
    sync_port: Option<u16>,
    #[serde(default)]
    confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReconnectRequest {
    certificate_pem: String,
    #[serde(default)]
    link_id: String,
    sync_port: u16,
    sync_mode: SyncMode,
    state_revision: u64,
    attempt_id: String,
    attempt_started_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReconnectResponse {
    certificate_pem: String,
    sync_port: u16,
    sync_mode: SyncMode,
    state_revision: u64,
    attempt_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SyncStateEnvelope {
    sync_mode: SyncMode,
    state_revision: u64,
}

#[derive(Debug, Clone)]
struct ReconnectAttempt {
    id: String,
    mapping_id: String,
    initiator_fingerprint: String,
    started_at_unix_ms: u64,
}

fn reconnect_attempt_wins(incoming: &ReconnectAttempt, current: &ReconnectAttempt) -> bool {
    if incoming.initiator_fingerprint != current.initiator_fingerprint {
        return incoming.initiator_fingerprint < current.initiator_fingerprint;
    }
    if incoming.started_at_unix_ms != current.started_at_unix_ms {
        return incoming.started_at_unix_ms > current.started_at_unix_ms;
    }
    incoming.id > current.id
}

fn reconnect_peer_session_key(
    link_id: &str,
    local_fingerprint: &str,
    peer_fingerprint: &str,
) -> String {
    if local_fingerprint <= peer_fingerprint {
        format!("{}:{}:{}", link_id, local_fingerprint, peer_fingerprint)
    } else {
        format!("{}:{}:{}", link_id, peer_fingerprint, local_fingerprint)
    }
}

fn pairing_link_id(
    local_pairing_id: &str,
    peer_pairing_id: &str,
    local_certificate_pem: &str,
    peer_certificate_pem: &str,
) -> String {
    let mut pairing_ids = [local_pairing_id, peer_pairing_id];
    pairing_ids.sort();

    let mut fingerprints = [
        certificate_fingerprint(local_certificate_pem),
        certificate_fingerprint(peer_certificate_pem),
    ];
    fingerprints.sort();

    let mut hasher = Sha256::new();
    hasher.update(b"mdev-repo-sync-link-v1\0");
    hasher.update(pairing_ids[0].as_bytes());
    hasher.update(b"\0");
    hasher.update(pairing_ids[1].as_bytes());
    hasher.update(b"\0");
    hasher.update(fingerprints[0].as_bytes());
    hasher.update(b"\0");
    hasher.update(fingerprints[1].as_bytes());
    hex::encode(hasher.finalize())
}

fn incoming_sync_state_wins(
    incoming_revision: u64,
    local_revision: u64,
    incoming_owner_fingerprint: &str,
    local_owner_fingerprint: &str,
) -> bool {
    incoming_revision > local_revision
        || (incoming_revision == local_revision
            && incoming_owner_fingerprint > local_owner_fingerprint)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PeerMessageBlock {
    Text { text: String },
    Code {
        #[serde(default)]
        language: String,
        text: String,
    },
    File {
        name: String,
        mime_type: String,
        data_base64: String,
    },
    Image {
        name: String,
        mime_type: String,
        data_base64: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerMessage {
    pub id: String,
    pub sent_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub author: String,
    pub blocks: Vec<PeerMessageBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerMessageEnvelope {
    id: String,
    sent_at_unix_ms: u64,
    blocks: Vec<PeerMessageBlock>,
}

#[derive(Clone)]
struct RepoSyncServerState {
    runtime: RepoSyncRuntime,
    db: SqlitePool,
    mapping_id: String,
    workflow_run_id: String,
}

#[derive(Debug, Default)]
struct RepoSyncState {
    identity: Option<LocalSyncIdentity>,
    pairings: HashMap<String, PairingSession>,
    mappings: HashMap<String, SyncMapping>,
    active_sessions: HashSet<String>,
    reconnect_ports: HashMap<String, u16>,
    reconnect_attempts_by_peer: HashMap<String, ReconnectAttempt>,
    reserved_sync_listeners: HashMap<String, StdTcpListener>,
    sync_server_ports: HashMap<String, u16>,
    sync_server_abort_handles: HashMap<String, tokio::task::AbortHandle>,
    peer_messages: HashMap<String, Vec<PeerMessage>>,
    connection_expires_at_unix_ms: HashMap<String, u64>,
    db: Option<SqlitePool>,
    mappings_loaded: bool,
    local_ipv4: Option<IpAddr>,
    pairing_server_started: bool,
    pairing_server_port: Option<u16>,
}

#[derive(Debug, Clone, Default)]
pub struct RepoSyncRuntime {
    state: Arc<RwLock<RepoSyncState>>,
}

impl RepoSyncRuntime {
    pub async fn load_identity_if_present(&self) -> Result<Option<LocalSyncIdentity>> {
        {
            let state = self.state.read().await;
            if let Some(identity) = &state.identity {
                return Ok(Some(identity.clone()));
            }
        }

        let Some(identity) = load_identity_if_present()? else {
            return Ok(None);
        };

        self.state.write().await.identity = Some(identity.clone());
        Ok(Some(identity))
    }

    pub async fn ensure_identity(&self) -> Result<LocalSyncIdentity> {
        {
            let state = self.state.read().await;
            if let Some(identity) = &state.identity {
                return Ok(identity.clone());
            }
        }

        let identity = load_or_create_identity()?;
        self.state.write().await.identity = Some(identity.clone());
        Ok(identity)
    }

    async fn ensure_mappings_loaded(&self) -> Result<()> {
        {
            let state = self.state.read().await;
            if state.mappings_loaded {
                return Ok(());
            }
        }

        let mappings = load_persisted_mappings()?;
        let mut state = self.state.write().await;
        if state.mappings_loaded {
            return Ok(());
        }

        for mut mapping in mappings {
            mapping.connected = false;
            mapping.peer_port = None;
            state.mappings.insert(mapping.id.clone(), mapping);
        }
        state.mappings_loaded = true;
        Ok(())
    }

    async fn bind_db(&self, db: &SqlitePool) {
        let mut state = self.state.write().await;
        if state.db.is_none() {
            state.db = Some(db.clone());
        }
    }

    fn clear_session_locked(state: &mut RepoSyncState, mapping_id: &str) {
        state.active_sessions.remove(mapping_id);
        state.connection_expires_at_unix_ms.remove(mapping_id);
        state.peer_messages.remove(mapping_id);
        state.reconnect_ports.remove(mapping_id);
        state
            .reconnect_attempts_by_peer
            .retain(|_, attempt| attempt.mapping_id != mapping_id);
        state.reserved_sync_listeners.remove(mapping_id);
        state.sync_server_ports.remove(mapping_id);

        if let Some(abort_handle) = state.sync_server_abort_handles.remove(mapping_id) {
            abort_handle.abort();
        }

        if let Some(mapping) = state.mappings.get_mut(mapping_id) {
            mapping.connected = false;
            mapping.peer_port = None;
        }
    }

    async fn reconnect_attempt_is_current(
        &self,
        peer_session_key: &str,
        attempt_id: &str,
    ) -> bool {
        let state = self.state.read().await;
        state
            .reconnect_attempts_by_peer
            .get(peer_session_key)
            .map(|attempt| attempt.id.as_str() == attempt_id)
            .unwrap_or(false)
    }

    async fn wait_for_peer_active_session(
        &self,
        peer_fingerprint: &str,
        timeout: Duration,
    ) -> Result<SyncMapping> {
        let deadline = SystemTime::now()
            .checked_add(timeout)
            .unwrap_or(SystemTime::now());

        loop {
            {
                let state = self.state.read().await;
                if let Some(mapping) = state
                    .mappings
                    .values()
                    .find(|mapping| {
                        state.active_sessions.contains(mapping.id.as_str())
                            && !mapping.peer_certificate_pem.trim().is_empty()
                            && certificate_fingerprint(mapping.peer_certificate_pem.as_str())
                                == peer_fingerprint
                    })
                    .cloned()
                {
                    return Ok(mapping);
                }
            }

            if SystemTime::now() >= deadline {
                bail!("timed out waiting for the winning Repo Sync peer session");
            }

            sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn activate_workflow(
        &self,
        db: &SqlitePool,
        workflow_run_id: &str,
    ) -> Result<()> {
        self.bind_db(db).await;
        self.ensure_mappings_loaded().await?;
        self.expire_connection_leases().await?;

        let has_trusted_mapping = {
            let state = self.state.read().await;
            state.mappings.values().any(|mapping| {
                mapping.workflow_run_id == workflow_run_id
                    && !mapping.peer_certificate_pem.trim().is_empty()
            })
        };

        if has_trusted_mapping {
            self.ensure_pairing_server().await?;
        }

        Ok(())
    }

    async fn expire_connection_leases(&self) -> Result<()> {
        let now = unix_ms_now();
        let mut state = self.state.write().await;
        let expired = state
            .connection_expires_at_unix_ms
            .iter()
            .filter_map(|(mapping_id, expires_at)| {
                (*expires_at <= now).then_some(mapping_id.clone())
            })
            .collect::<Vec<_>>();

        if expired.is_empty() {
            return Ok(());
        }

        for mapping_id in &expired {
            Self::clear_session_locked(&mut state, mapping_id);
        }

        for mapping_id in expired {
            tracing::info!(
                mapping_id = %mapping_id,
                "Repo Sync four-hour connection lease expired"
            );
        }

        Ok(())
    }

    async fn mark_disconnected(&self, mapping_id: &str, reason: &str) -> Result<()> {
        let mut state = self.state.write().await;
        Self::clear_session_locked(&mut state, mapping_id);

        tracing::info!(
            mapping_id = %mapping_id,
            reason,
            "Repo Sync session destroyed"
        );

        Ok(())
    }

    async fn ensure_sync_server(&self, mapping_id: &str) -> Result<()> {
        self.ensure_mappings_loaded().await?;

        let (mapping, port, db, already_started) = {
            let state = self.state.read().await;
            let mapping = state
                .mappings
                .get(mapping_id)
                .cloned()
                .context("Repo Sync mapping not found")?;
            let port = *state
                .reconnect_ports
                .get(mapping_id)
                .context("local Repo Sync data port is missing")?;
            let db = state
                .db
                .clone()
                .context("Repo Sync database binding is missing")?;
            let already_started = state.sync_server_ports.get(mapping_id) == Some(&port);
            (mapping, port, db, already_started)
        };

        if already_started {
            return Ok(());
        }
        if mapping.peer_certificate_pem.trim().is_empty() {
            bail!("cannot start Repo Sync data server before peer trust is established");
        }

        let identity = self.ensure_identity().await?;
        let tls = build_mtls_server_config(&identity, mapping.peer_certificate_pem.as_str())?;
        let listener = self
            .state
            .write()
            .await
            .reserved_sync_listeners
            .remove(mapping_id)
            .context("reserved Repo Sync data listener is missing")?;
        listener
            .set_nonblocking(true)
            .context("failed to configure Repo Sync data listener")?;
        let listener_addr = listener
            .local_addr()
            .context("failed to read Repo Sync data listener address")?;

        let server_state = RepoSyncServerState {
            runtime: self.clone(),
            db,
            mapping_id: mapping.id.clone(),
            workflow_run_id: mapping.workflow_run_id.clone(),
        };
        let app = Router::new()
            .route("/sync/v1/ping", post(sync_ping))
            .route("/sync/v1/state", post(sync_state))
            .route("/sync/v1/changeset", post(sync_changeset))
            .route("/sync/v1/hard-sync", post(sync_hard_sync))
            .route("/sync/v1/message", post(sync_peer_message))
            .layer(DefaultBodyLimit::max(1536 * 1024 * 1024))
            .with_state(server_state);

        tracing::info!(
            mapping_id = %mapping.id,
            workflow_run_id = %mapping.workflow_run_id,
            bind_address = %listener_addr,
            port,
            "Repo Sync TLS data listener ready"
        );

        let task = tokio::spawn(async move {
            let server = match axum_server::from_tcp_rustls(listener, tls) {
                Ok(server) => server,
                Err(error) => {
                    tracing::error!(
                        port,
                        error = %format!("{:#}", error),
                        "failed to create Repo Sync TLS data server"
                    );
                    return;
                }
            };

            if let Err(error) = server.serve(app.into_make_service()).await {
                tracing::error!(
                    port,
                    error = %format!("{:#}", error),
                    "Repo Sync data server stopped"
                );
            }
        });
        let abort_handle = task.abort_handle();

        let mut state = self.state.write().await;
        state.sync_server_ports.insert(mapping.id.clone(), port);
        state
            .sync_server_abort_handles
            .insert(mapping.id.clone(), abort_handle);

        Ok(())
    }

    async fn probe_peer_once(&self, mapping_id: &str) -> Result<()> {
        let mapping = {
            let state = self.state.read().await;
            state
                .mappings
                .get(mapping_id)
                .cloned()
                .context("trusted Repo Sync mapping not found")?
        };

        let peer_port = mapping.peer_port.context("paired peer has no sync port")?;
        let peer_address = SocketAddr::new(mapping.peer_ipv4, peer_port);

        let tcp_stream = tokio::time::timeout(
            Duration::from_secs(2),
            TcpStream::connect(peer_address),
        )
        .await
        .with_context(|| {
            format!(
                "Repo Sync TCP connection to {} timed out before TLS; check the peer listener and firewall",
                peer_address
            )
        })?
        .with_context(|| {
            format!(
                "Repo Sync TCP connection to {} failed before TLS",
                peer_address
            )
        })?;

        tracing::info!(
            mapping_id = %mapping.id,
            peer_address = %peer_address,
            "Repo Sync raw TCP connection to peer data listener succeeded"
        );
        drop(tcp_stream);

        let identity = self.ensure_identity().await?;
        let client = build_mtls_client(&identity, &mapping)?;
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            client
                .post(format!("https://mdev-sync:{}/sync/v1/ping", peer_port))
                .header("x-mdev-mapping-id", mapping.id.as_str())
                .send(),
        )
        .await
        .with_context(|| {
            format!(
                "Repo Sync TCP connection to {} succeeded but the TLS/HTTP readiness request timed out",
                peer_address
            )
        })?
        .with_context(|| {
            format!(
                "Repo Sync TCP connection to {} succeeded but the TLS/HTTP readiness request failed",
                peer_address
            )
        })?;

        if !response.status().is_success() {
            bail!(
                "Repo Sync TLS connection succeeded but peer readiness endpoint returned {}",
                response.status()
            );
        }

        tracing::info!(
            mapping_id = %mapping.id,
            peer_address = %peer_address,
            "Repo Sync mTLS readiness request succeeded"
        );

        Ok(())
    }

    async fn wait_for_peer_ready(&self, mapping_id: &str, timeout: Duration) -> Result<()> {
        let deadline = SystemTime::now()
            .checked_add(timeout)
            .unwrap_or(SystemTime::now());
        let mut last_error = None;

        loop {
            match self.probe_peer_once(mapping_id).await {
                Ok(()) => return Ok(()),
                Err(error) => last_error = Some(error),
            }

            if SystemTime::now() >= deadline {
                let detail = last_error
                    .map(|error| format!("{:#}", error))
                    .unwrap_or_else(|| "peer readiness probe failed".to_string());
                bail!("timed out waiting for Repo Sync peer data listener: {}", detail);
            }

            sleep(Duration::from_millis(300)).await;
        }
    }

    async fn mark_connected(&self, mapping_id: &str) -> Result<SyncMapping> {
        let mut state = self.state.write().await;
        state.peer_messages.remove(mapping_id);
        state.active_sessions.insert(mapping_id.to_string());
        let mapping = state
            .mappings
            .get_mut(mapping_id)
            .context("trusted Repo Sync mapping not found")?;
        mapping.connected = true;
        let mapping = mapping.clone();
        let expires_at_unix_ms = unix_ms_now().saturating_add(CONNECTION_LEASE_MS);
        state
            .connection_expires_at_unix_ms
            .insert(mapping_id.to_string(), expires_at_unix_ms);

        tracing::info!(
            mapping_id = %mapping.id,
            workflow_run_id = %mapping.workflow_run_id,
            peer_ipv4 = %mapping.peer_ipv4,
            peer_port = ?mapping.peer_port,
            expires_at_unix_ms,
            "Repo Sync ephemeral session established"
        );

        Ok(mapping)
    }

    fn verify_peer_in_background(&self, mapping_id: String) {
        let runtime = self.clone();
        tokio::spawn(async move {
            let endpoint = {
                let state = runtime.state.read().await;
                state
                    .mappings
                    .get(mapping_id.as_str())
                    .map(|mapping| (mapping.peer_ipv4, mapping.peer_port))
            };

            tracing::info!(
                mapping_id = %mapping_id,
                peer_endpoint = ?endpoint,
                "Repo Sync starting peer TLS readiness verification"
            );

            match runtime
                .wait_for_peer_ready(mapping_id.as_str(), Duration::from_secs(30))
                .await
            {
                Ok(()) => {
                    tracing::info!(
                        mapping_id = %mapping_id,
                        peer_endpoint = ?endpoint,
                        "Repo Sync peer TLS readiness verification succeeded"
                    );

                    if let Err(error) = runtime.mark_connected(mapping_id.as_str()).await {
                        tracing::warn!(
                            mapping_id = %mapping_id,
                            error = %format!("{:#}", error),
                            "Repo Sync peer became reachable but connected state could not be saved"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        mapping_id = %mapping_id,
                        peer_endpoint = ?endpoint,
                        error = %format!("{:#}", error),
                        "Repo Sync peer TLS readiness verification failed"
                    );
                }
            }
        });
    }

    pub async fn listen_for_reconnects(&self, db: &SqlitePool) -> Result<()> {
        self.bind_db(db).await;
        self.ensure_mappings_loaded().await?;

        let has_trusted_mapping = {
            let state = self.state.read().await;
            state
                .mappings
                .values()
                .any(|mapping| !mapping.peer_certificate_pem.trim().is_empty())
        };

        if has_trusted_mapping {
            self.ensure_pairing_server().await?;
        }

        Ok(())
    }

    pub async fn status(
        &self,
        workflow_run_id: &str,
        endpoints: &RuntimeEndpointManager,
    ) -> Result<RepoSyncStatus> {
        self.ensure_mappings_loaded().await?;

        if let Some(address) = endpoints.local_lan_ipv4() {
            let mut state = self.state.write().await;
            state.local_ipv4 = Some(IpAddr::V4(address));
        }

        let state = self.state.read().await;
        Ok(RepoSyncStatus {
            identity_ready: state.identity.is_some(),
            certificate_fingerprint: state
                .identity
                .as_ref()
                .map(|identity| identity.fingerprint.clone())
                .unwrap_or_default(),
            local_ipv4: state.local_ipv4,
            pairing_listener_running: state.pairing_server_started,
            pairing_listener_port: state.pairing_server_port,
            pairings: state
                .pairings
                .values()
                .filter(|session| !session.complete && !pairing_expired(session))
                .cloned()
                .collect(),
            mappings: state
                .mappings
                .values()
                .filter(|mapping| mapping.workflow_run_id == workflow_run_id)
                .cloned()
                .map(|mut mapping| {
                    mapping.connected = state.active_sessions.contains(mapping.id.as_str());
                    if !mapping.connected {
                        mapping.peer_port = None;
                    }
                    mapping
                })
                .collect(),
        })
    }

    async fn accept_sync_state(
        &self,
        mapping_id: &str,
        incoming: SyncStateEnvelope,
    ) -> Result<SyncStateEnvelope> {
        self.ensure_mappings_loaded().await?;
        let identity = self.ensure_identity().await?;

        let mut state = self.state.write().await;
        let mapping = state
            .mappings
            .get_mut(mapping_id)
            .context("trusted Repo Sync mapping not found")?;
        let peer_fingerprint = certificate_fingerprint(mapping.peer_certificate_pem.as_str());

        if incoming_sync_state_wins(
            incoming.state_revision,
            mapping.state_revision,
            peer_fingerprint.as_str(),
            identity.fingerprint.as_str(),
        ) {
            mapping.sync_mode = incoming.sync_mode;
            mapping.state_revision = incoming.state_revision;
            persist_mappings(&state.mappings)?;
        }

        let mapping = state
            .mappings
            .get(mapping_id)
            .context("trusted Repo Sync mapping not found")?;

        Ok(SyncStateEnvelope {
            sync_mode: mapping.sync_mode.clone(),
            state_revision: mapping.state_revision,
        })
    }

    async fn reconcile_sync_state(
        &self,
        mapping_id: &str,
        incoming: SyncStateEnvelope,
    ) -> Result<SyncMapping> {
        self.ensure_mappings_loaded().await?;
        let identity = self.ensure_identity().await?;

        let mut state = self.state.write().await;
        let mapping = state
            .mappings
            .get_mut(mapping_id)
            .context("trusted Repo Sync mapping not found")?;
        let peer_fingerprint = certificate_fingerprint(mapping.peer_certificate_pem.as_str());

        if incoming_sync_state_wins(
            incoming.state_revision,
            mapping.state_revision,
            peer_fingerprint.as_str(),
            identity.fingerprint.as_str(),
        ) {
            mapping.sync_mode = incoming.sync_mode;
            mapping.state_revision = incoming.state_revision;
            persist_mappings(&state.mappings)?;
        }

        state
            .mappings
            .get(mapping_id)
            .cloned()
            .context("trusted Repo Sync mapping not found")
    }

    async fn push_sync_state(&self, mapping_id: &str) -> Result<()> {
        let mapping = {
            let state = self.state.read().await;
            state
                .mappings
                .get(mapping_id)
                .cloned()
                .context("trusted Repo Sync mapping not found")?
        };

        if !mapping.connected {
            return Ok(());
        }

        let identity = self.ensure_identity().await?;
        let client = build_mtls_client(&identity, &mapping)?;
        let peer_port = mapping.peer_port.context("paired peer has no sync port")?;
        let response = match client
            .post(format!("https://mdev-sync:{}/sync/v1/state", peer_port))
            .header("x-mdev-mapping-id", mapping.id.as_str())
            .json(&SyncStateEnvelope {
                sync_mode: mapping.sync_mode.clone(),
                state_revision: mapping.state_revision,
            })
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                let _ = self
                    .mark_disconnected(mapping_id, "sync state transport failed")
                    .await;
                return Err(error).context("failed to synchronize Repo Sync state");
            }
        };

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("remote Repo Sync state update failed: {} {}", status, body);
        }

        let remote = response
            .json::<SyncStateEnvelope>()
            .await
            .context("failed to decode remote Repo Sync state")?;

        self.reconcile_sync_state(mapping_id, remote).await?;
        Ok(())
    }

    pub async fn upsert_mapping(&self, mut mapping: SyncMapping) -> Result<SyncMapping> {
        self.ensure_mappings_loaded().await?;

        if mapping.workflow_run_id.trim().is_empty() {
            bail!("workflow_run_id is required");
        }
        if mapping.id.trim().is_empty() {
            mapping.id = Uuid::new_v4().to_string();
        }

        let (mapping, should_push) = {
            let mut state = self.state.write().await;
            let mut should_push = false;

            if let Some(existing) = state.mappings.get(mapping.id.as_str()) {
                let sync_mode_changed = mapping.sync_mode != existing.sync_mode;

                if mapping.peer_certificate_pem.trim().is_empty() {
                    mapping.peer_certificate_pem = existing.peer_certificate_pem.clone();
                }
                if mapping.link_id.trim().is_empty() {
                    mapping.link_id = existing.link_id.clone();
                }
                if mapping.peer_port.is_none() {
                    mapping.peer_port = existing.peer_port;
                }

                let session_active = state.active_sessions.contains(mapping.id.as_str());
                mapping.connected = session_active;
                mapping.state_revision = if sync_mode_changed {
                    existing.state_revision.saturating_add(1)
                } else {
                    existing.state_revision
                };
                should_push = sync_mode_changed && session_active;
            } else {
                mapping.state_revision = 1;
            }

            state.mappings.insert(mapping.id.clone(), mapping.clone());
            persist_mappings(&state.mappings)?;
            (mapping, should_push)
        };

        if should_push {
            self.push_sync_state(mapping.id.as_str()).await?;
        }

        let state = self.state.read().await;
        Ok(state
            .mappings
            .get(mapping.id.as_str())
            .cloned()
            .unwrap_or(mapping))
    }

    async fn ensure_pairing_server(&self) -> Result<()> {
        let mut state = self.state.write().await;
        if state.pairing_server_started {
            return Ok(());
        }

        let listener = StdTcpListener::bind((Ipv4Addr::UNSPECIFIED, PAIRING_CONTROL_PORT))
            .with_context(|| {
                format!(
                    "failed to bind Repo Sync pairing control port {}",
                    PAIRING_CONTROL_PORT
                )
            })?;
        listener
            .set_nonblocking(true)
            .context("failed to configure Repo Sync pairing control listener")?;
        let listener = TcpListener::from_std(listener)
            .context("failed to create async Repo Sync pairing control listener")?;

        state.pairing_server_started = true;
        state.pairing_server_port = Some(PAIRING_CONTROL_PORT);
        drop(state);

        let app = Router::new()
            .route("/pair/v1/join", post(pairing_join))
            .route("/pair/v1/reconnect", post(reconnect_join))
            .with_state(self.clone());

        tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, app).await {
                tracing::error!(
                    error = %format!("{:#}", error),
                    "Repo Sync pairing control server stopped"
                );
            }
        });

        Ok(())
    }

    pub async fn reconnect(
        &self,
        db: &SqlitePool,
        endpoints: &RuntimeEndpointManager,
        mapping_id: &str,
    ) -> Result<SyncMapping> {
        self.bind_db(db).await;
        self.ensure_mappings_loaded().await?;
        self.ensure_pairing_server().await?;
        let identity = self.ensure_identity().await?;

        let mapping = {
            let state = self.state.read().await;
            state
                .mappings
                .get(mapping_id)
                .cloned()
                .context("trusted Repo Sync peer not found")?
        };

        if mapping.peer_certificate_pem.trim().is_empty() {
            bail!("peer is not trusted; pair this workflow first");
        }

        if mapping.link_id.trim().is_empty() {
            bail!("this Repo Sync mapping predates workflow link identities; unpair and pair this workflow again before reconnecting");
        }

        {
            let state = self.state.read().await;
            if state.active_sessions.contains(mapping_id) {
                return state
                    .mappings
                    .get(mapping_id)
                    .cloned()
                    .context("trusted Repo Sync mapping not found");
            }
        }

        let peer_fingerprint =
            certificate_fingerprint(mapping.peer_certificate_pem.as_str());
        let peer_session_key = reconnect_peer_session_key(
            mapping.link_id.as_str(),
            identity.fingerprint.as_str(),
            peer_fingerprint.as_str(),
        );
        let attempt = ReconnectAttempt {
            id: Uuid::new_v4().to_string(),
            mapping_id: mapping.id.clone(),
            initiator_fingerprint: identity.fingerprint.clone(),
            started_at_unix_ms: unix_ms_now(),
        };

        {
            let mut state = self.state.write().await;

            if let Some(previous) = state
                .reconnect_attempts_by_peer
                .get(peer_session_key.as_str())
                .cloned()
            {
                if previous.mapping_id != mapping.id {
                    Self::clear_session_locked(&mut state, previous.mapping_id.as_str());
                }
            }

            Self::clear_session_locked(&mut state, mapping.id.as_str());
            state
                .reconnect_attempts_by_peer
                .insert(peer_session_key.clone(), attempt.clone());
        }

        let (endpoint, listener) = endpoints.reserve(NetworkExposure::Lan, None, None, &[])?;
        let local_port = endpoint.port;

        {
            let mut state = self.state.write().await;
            let current = state
                .reconnect_attempts_by_peer
                .get(peer_session_key.as_str())
                .map(|current| current.id.as_str() == attempt.id.as_str())
                .unwrap_or(false);

            if !current {
                drop(state);
                drop(listener);
                return self
                    .wait_for_peer_active_session(
                        peer_fingerprint.as_str(),
                        Duration::from_secs(15),
                    )
                    .await;
            }

            state
                .reconnect_ports
                .insert(mapping.id.clone(), local_port);
            state
                .reserved_sync_listeners
                .insert(mapping.id.clone(), listener);
        }

        self.ensure_sync_server(mapping.id.as_str()).await?;

        if !self
            .reconnect_attempt_is_current(peer_session_key.as_str(), attempt.id.as_str())
            .await
        {
            return self
                .wait_for_peer_active_session(
                    peer_fingerprint.as_str(),
                    Duration::from_secs(15),
                )
                .await;
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .context("failed to build Repo Sync reconnect client")?;
        let url = format!(
            "http://{}:{}/pair/v1/reconnect",
            mapping.peer_ipv4,
            PAIRING_CONTROL_PORT
        );
        let request = ReconnectRequest {
            certificate_pem: identity.certificate_pem.clone(),
            link_id: mapping.link_id.clone(),
            sync_port: local_port,
            sync_mode: mapping.sync_mode.clone(),
            state_revision: mapping.state_revision,
            attempt_id: attempt.id.clone(),
            attempt_started_at_unix_ms: attempt.started_at_unix_ms,
        };

        let deadline = SystemTime::now()
            .checked_add(Duration::from_secs(15))
            .unwrap_or(SystemTime::now());

        loop {
            if !self
                .reconnect_attempt_is_current(
                    peer_session_key.as_str(),
                    attempt.id.as_str(),
                )
                .await
            {
                return self
                    .wait_for_peer_active_session(
                        peer_fingerprint.as_str(),
                        Duration::from_secs(15),
                    )
                    .await;
            }

            match client.post(url.as_str()).json(&request).send().await {
                Ok(response) if response.status().is_success() => {
                    let response: ReconnectResponse = response
                        .json()
                        .await
                        .context("failed to decode Repo Sync reconnect response")?;

                    if response.attempt_id != attempt.id {
                        tracing::debug!(
                            mapping_id = %mapping_id,
                            expected_attempt_id = %attempt.id,
                            response_attempt_id = %response.attempt_id,
                            "Repo Sync ignored stale reconnect response"
                        );
                        continue;
                    }

                    if certificate_fingerprint(response.certificate_pem.as_str())
                        != certificate_fingerprint(mapping.peer_certificate_pem.as_str())
                    {
                        bail!("reconnect peer certificate does not match the trusted certificate");
                    }

                    if !self
                        .reconnect_attempt_is_current(
                            peer_session_key.as_str(),
                            attempt.id.as_str(),
                        )
                        .await
                    {
                        return self
                            .wait_for_peer_active_session(
                                peer_fingerprint.as_str(),
                                Duration::from_secs(15),
                            )
                            .await;
                    }

                    tracing::info!(
                        mapping_id = %mapping_id,
                        attempt_id = %attempt.id,
                        peer_ipv4 = %mapping.peer_ipv4,
                        local_port,
                        peer_port = response.sync_port,
                        "Repo Sync reconnect control exchange succeeded"
                    );

                    {
                        let mut state = self.state.write().await;
                        let current = state
                            .reconnect_attempts_by_peer
                            .get(peer_session_key.as_str())
                            .map(|current| current.id.as_str() == attempt.id.as_str())
                            .unwrap_or(false);

                        if !current {
                            drop(state);
                            return self
                                .wait_for_peer_active_session(
                                    peer_fingerprint.as_str(),
                                    Duration::from_secs(15),
                                )
                                .await;
                        }

                        let updated = state
                            .mappings
                            .get_mut(mapping_id)
                            .context("trusted Repo Sync peer disappeared during reconnect")?;
                        updated.peer_port = Some(response.sync_port);
                        persist_mappings(&state.mappings)?;
                    }

                    self.reconcile_sync_state(
                        mapping_id,
                        SyncStateEnvelope {
                            sync_mode: response.sync_mode,
                            state_revision: response.state_revision,
                        },
                    )
                    .await?;

                    if let Err(error) = self
                        .wait_for_peer_ready(mapping_id, Duration::from_secs(15))
                        .await
                    {
                        if self
                            .reconnect_attempt_is_current(
                                peer_session_key.as_str(),
                                attempt.id.as_str(),
                            )
                            .await
                        {
                            let _ = self
                                .mark_disconnected(mapping_id, "reconnect peer readiness failed")
                                .await;
                            return Err(error);
                        }

                        return self
                            .wait_for_peer_active_session(
                                peer_fingerprint.as_str(),
                                Duration::from_secs(15),
                            )
                            .await;
                    }

                    if !self
                        .reconnect_attempt_is_current(
                            peer_session_key.as_str(),
                            attempt.id.as_str(),
                        )
                        .await
                    {
                        return self
                            .wait_for_peer_active_session(
                                peer_fingerprint.as_str(),
                                Duration::from_secs(15),
                            )
                            .await;
                    }

                    return self.mark_connected(mapping_id).await;
                }
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    tracing::debug!(
                        mapping_id = %mapping_id,
                        attempt_id = %attempt.id,
                        peer_ipv4 = %mapping.peer_ipv4,
                        status = %status,
                        response = %body,
                        "Repo Sync reconnect peer rejected control request"
                    );
                }
                Err(error) => {
                    tracing::debug!(
                        mapping_id = %mapping_id,
                        attempt_id = %attempt.id,
                        peer_ipv4 = %mapping.peer_ipv4,
                        error = %format!("{:#}", error),
                        "Repo Sync reconnect control request failed"
                    );
                }
            }

            if !self
                .reconnect_attempt_is_current(
                    peer_session_key.as_str(),
                    attempt.id.as_str(),
                )
                .await
            {
                return self
                    .wait_for_peer_active_session(
                        peer_fingerprint.as_str(),
                        Duration::from_secs(15),
                    )
                    .await;
            }

            if SystemTime::now() >= deadline {
                let _ = self
                    .mark_disconnected(mapping_id, "reconnect control exchange timed out")
                    .await;
                bail!("timed out waiting for the trusted peer; inspect Repo Sync logs on both computers");
            }

            sleep(Duration::from_millis(750)).await;
        }
    }

    async fn accept_reconnect(&self, request: ReconnectRequest) -> Result<ReconnectResponse> {
        self.ensure_mappings_loaded().await?;
        let identity = self.ensure_identity().await?;
        Certificate::from_pem(request.certificate_pem.as_bytes())
            .context("invalid reconnect certificate")?;

        if request.attempt_id.trim().is_empty() {
            bail!("reconnect attempt id is required");
        }

        if request.link_id.trim().is_empty() {
            bail!("reconnect workflow link id is required; re-pair this workflow on both machines");
        }

        let fingerprint = certificate_fingerprint(request.certificate_pem.as_str());
        let incoming_attempt = ReconnectAttempt {
            id: request.attempt_id.clone(),
            mapping_id: String::new(),
            initiator_fingerprint: fingerprint.clone(),
            started_at_unix_ms: request.attempt_started_at_unix_ms,
        };

        let mapping_id = {
            let state = self.state.read().await;
            state
                .mappings
                .values()
                .find(|mapping| {
                    mapping.link_id == request.link_id
                        && !mapping.peer_certificate_pem.trim().is_empty()
                        && certificate_fingerprint(mapping.peer_certificate_pem.as_str())
                            == fingerprint
                })
                .map(|mapping| mapping.id.clone())
                .context("no trusted Repo Sync workflow link matches this certificate and link id")?
        };

        let peer_session_key = reconnect_peer_session_key(
            request.link_id.as_str(),
            identity.fingerprint.as_str(),
            fingerprint.as_str(),
        );
        let incoming_attempt = ReconnectAttempt {
            mapping_id: mapping_id.clone(),
            ..incoming_attempt
        };

        let existing_port = {
            let mut state = self.state.write().await;
            match state
                .reconnect_attempts_by_peer
                .get(peer_session_key.as_str())
                .cloned()
            {
                Some(current) if current.id == incoming_attempt.id => state
                    .reconnect_ports
                    .get(current.mapping_id.as_str())
                    .copied(),
                Some(current) => {
                    if !reconnect_attempt_wins(&incoming_attempt, &current) {
                        bail!("simultaneous reconnect resolved in favor of the existing peer attempt");
                    }

                    tracing::info!(
                        peer_session_key = %peer_session_key,
                        previous_mapping_id = %current.mapping_id,
                        winning_mapping_id = %mapping_id,
                        previous_attempt_id = %current.id,
                        winning_attempt_id = %incoming_attempt.id,
                        "Repo Sync replaced losing simultaneous peer reconnect"
                    );

                    Self::clear_session_locked(&mut state, current.mapping_id.as_str());
                    if current.mapping_id != mapping_id {
                        Self::clear_session_locked(&mut state, mapping_id.as_str());
                    }
                    state
                        .reconnect_attempts_by_peer
                        .insert(peer_session_key.clone(), incoming_attempt.clone());
                    None
                }
                None => {
                    Self::clear_session_locked(&mut state, mapping_id.as_str());
                    state
                        .reconnect_attempts_by_peer
                        .insert(peer_session_key.clone(), incoming_attempt.clone());
                    None
                }
            }
        };

        let local_port = if let Some(local_port) = existing_port {
            local_port
        } else {
            let endpoints = RuntimeEndpointManager::default();
            let (endpoint, listener) =
                endpoints.reserve(NetworkExposure::Lan, None, None, &[])?;
            let local_port = endpoint.port;

            let mut state = self.state.write().await;
            let current = state
                .reconnect_attempts_by_peer
                .get(peer_session_key.as_str())
                .map(|current| {
                    current.id.as_str() == incoming_attempt.id.as_str()
                        && current.mapping_id.as_str() == mapping_id.as_str()
                })
                .unwrap_or(false);

            if !current {
                drop(state);
                drop(listener);
                bail!("reconnect attempt was superseded before listener activation");
            }

            state
                .reconnect_ports
                .insert(mapping_id.clone(), local_port);
            state
                .reserved_sync_listeners
                .insert(mapping_id.clone(), listener);
            local_port
        };

        {
            let mut state = self.state.write().await;
            let current = state
                .reconnect_attempts_by_peer
                .get(peer_session_key.as_str())
                .map(|current| {
                    current.id.as_str() == incoming_attempt.id.as_str()
                        && current.mapping_id.as_str() == mapping_id.as_str()
                })
                .unwrap_or(false);

            if !current {
                bail!("reconnect attempt was superseded before peer state update");
            }

            let mapping = state
                .mappings
                .get_mut(mapping_id.as_str())
                .context("trusted Repo Sync mapping not found")?;
            mapping.peer_port = Some(request.sync_port);
            if incoming_sync_state_wins(
                request.state_revision,
                mapping.state_revision,
                fingerprint.as_str(),
                identity.fingerprint.as_str(),
            ) {
                mapping.sync_mode = request.sync_mode.clone();
                mapping.state_revision = request.state_revision;
            }
            persist_mappings(&state.mappings)?;
        }

        self.ensure_sync_server(mapping_id.as_str()).await?;

        if !self
            .reconnect_attempt_is_current(
                peer_session_key.as_str(),
                incoming_attempt.id.as_str(),
            )
            .await
        {
            bail!("reconnect attempt was superseded during listener activation");
        }

        tracing::info!(
            mapping_id = %mapping_id,
            attempt_id = %incoming_attempt.id,
            peer_port = request.sync_port,
            local_port,
            "Repo Sync accepted trusted peer reconnect"
        );

        if let Err(error) = self
            .wait_for_peer_ready(mapping_id.as_str(), Duration::from_secs(15))
            .await
        {
            if self
                .reconnect_attempt_is_current(
                    peer_session_key.as_str(),
                    incoming_attempt.id.as_str(),
                )
                .await
            {
                let _ = self
                    .mark_disconnected(
                        mapping_id.as_str(),
                        "inbound reconnect peer readiness failed",
                    )
                    .await;
                return Err(error);
            }

            bail!("reconnect attempt was superseded during peer readiness verification");
        }

        if !self
            .reconnect_attempt_is_current(
                peer_session_key.as_str(),
                incoming_attempt.id.as_str(),
            )
            .await
        {
            bail!("reconnect attempt was superseded before session activation");
        }

        let mapping = self.mark_connected(mapping_id.as_str()).await?;

        Ok(ReconnectResponse {
            certificate_pem: identity.certificate_pem,
            sync_port: local_port,
            sync_mode: mapping.sync_mode,
            state_revision: mapping.state_revision,
            attempt_id: incoming_attempt.id,
        })
    }

    pub async fn unpair(&self, mapping_id: &str) -> Result<()> {
        self.ensure_mappings_loaded().await?;
        let mut state = self.state.write().await;
        Self::clear_session_locked(&mut state, mapping_id);
        state.mappings.remove(mapping_id);
        state
            .pairings
            .retain(|_, session| session.mapping_id != mapping_id);
        persist_mappings(&state.mappings)?;
        Ok(())
    }

    pub async fn start_pairing(
        &self,
        db: &SqlitePool,
        endpoints: &RuntimeEndpointManager,
        mapping_id: &str,
        passphrase: &str,
    ) -> Result<PairingSession> {
        self.bind_db(db).await;
        let passphrase = passphrase.trim();
        if passphrase.len() < 6 {
            bail!("pairing passphrase must be at least 6 characters");
        }

        self.ensure_pairing_server().await?;

        let identity = self.ensure_identity().await?;
        let state = self.state.read().await;
        let mapping = state
            .mappings
            .get(mapping_id)
            .cloned()
            .context("mapping not found")?;
        drop(state);

        let (endpoint, listener) = endpoints.reserve(NetworkExposure::Lan, None, None, &[])?;
        let id = Uuid::new_v4().to_string();
        let expires_at_unix_ms = SystemTime::now()
            .checked_add(Duration::from_secs(PAIRING_TTL_SECONDS))
            .unwrap_or(SystemTime::now())
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();

        let session = PairingSession {
            id: id.clone(),
            mapping_id: mapping.id,
            peer_pairing_id: String::new(),
            verification_code: String::new(),
            peer_ipv4: mapping.peer_ipv4,
            local_port: endpoint.port,
            peer_port: None,
            expires_at_unix_ms,
            local_certificate_pem: identity.certificate_pem,
            peer_certificate_pem: String::new(),
            local_confirmed: false,
            remote_confirmed: false,
            complete: false,
            pairing_proof: pairing_proof(passphrase),
        };

        {
            let mut state = self.state.write().await;

            state.pairings.retain(|existing_id, existing| {
                let keep = existing.mapping_id != session.mapping_id;
                if !keep {
                    tracing::info!(
                        pairing_id = %existing_id,
                        mapping_id = %existing.mapping_id,
                        "Repo Sync removed previous pairing session before starting a new one"
                    );
                }
                keep
            });

            state
                .reconnect_ports
                .insert(session.mapping_id.clone(), endpoint.port);
            state
                .reserved_sync_listeners
                .insert(session.mapping_id.clone(), listener);
            state.pairings.insert(id.clone(), session.clone());
        }

        tracing::info!(
            pairing_id = %session.id,
            mapping_id = %session.mapping_id,
            peer_ipv4 = %session.peer_ipv4,
            local_port = session.local_port,
            "Repo Sync pairing session started"
        );

        let runtime = self.clone();
        tokio::spawn(async move {
            runtime.run_pairing_exchange(id).await;
        });

        Ok(session)
    }

    async fn run_pairing_exchange(&self, session_id: String) {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        loop {
            let session = {
                let state = self.state.read().await;
                state.pairings.get(session_id.as_str()).cloned()
            };

            let Some(session) = session else {
                return;
            };

            if pairing_expired(&session) || session.complete {
                return;
            }

            let request = PairingJoinRequest {
                proof: session.pairing_proof.clone(),
                pairing_id: session.id.clone(),
                certificate_pem: session.local_certificate_pem.clone(),
                sync_port: session.local_port,
                confirmed: session.local_confirmed,
            };

            let url = format!(
                "http://{}:{}/pair/v1/join",
                session.peer_ipv4,
                PAIRING_CONTROL_PORT
            );

            match client.post(url.as_str()).json(&request).send().await {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        match response.json::<PairingJoinResponse>().await {
                            Ok(response) => {
                                if response.matched {
                                    if let Err(error) = self
                                        .accept_pairing_response(session_id.as_str(), response)
                                        .await
                                    {
                                        tracing::warn!(
                                            pairing_id = %session_id,
                                            peer_ipv4 = %session.peer_ipv4,
                                            local_port = session.local_port,
                                            error = %format!("{:#}", error),
                                            "Repo Sync failed to accept pairing response"
                                        );
                                    }
                                }
                            }
                            Err(error) => {
                                tracing::warn!(
                                    pairing_id = %session_id,
                                    peer_ipv4 = %session.peer_ipv4,
                                    status = %status,
                                    error = %format!("{:#}", error),
                                    "Repo Sync failed to decode pairing response"
                                );
                            }
                        }
                    } else {
                        tracing::debug!(
                            pairing_id = %session_id,
                            peer_ipv4 = %session.peer_ipv4,
                            status = %status,
                            "Repo Sync pairing peer returned non-success status"
                        );
                    }
                }
                Err(error) => {
                    tracing::debug!(
                        pairing_id = %session_id,
                        peer_ipv4 = %session.peer_ipv4,
                        error = %format!("{:#}", error),
                        "Repo Sync pairing request did not reach peer"
                    );
                }
            }

            sleep(Duration::from_millis(750)).await;
        }
    }

    async fn accept_pairing_response(
        &self,
        session_id: &str,
        response: PairingJoinResponse,
    ) -> Result<()> {
        if response.certificate_pem.trim().is_empty() {
            return Ok(());
        }

        if response.pairing_id.trim().is_empty() {
            bail!("peer pairing response is missing pairing id");
        }

        Certificate::from_pem(response.certificate_pem.as_bytes())
            .context("invalid peer certificate")?;

        let identity = self.ensure_identity().await?;
        let mut state = self.state.write().await;
        let session = state
            .pairings
            .get_mut(session_id)
            .context("pairing session not found")?;

        if pairing_expired(session) {
            bail!("pairing session expired");
        }

        session.verification_code = verification_code(
            identity.certificate_pem.as_str(),
            response.certificate_pem.as_str(),
        );
        session.peer_certificate_pem = response.certificate_pem;
        session.peer_pairing_id = response.pairing_id;
        session.peer_port = response.sync_port;
        session.remote_confirmed = response.confirmed;

        let finalized = finalize_pairing_if_ready(&mut state, session_id)?;
        drop(state);

        tracing::info!(
            pairing_id = %session_id,
            remote_confirmed = response.confirmed,
            peer_port = ?response.sync_port,
            finalized = finalized.is_some(),
            "Repo Sync processed outbound pairing response"
        );
        if let Some(mapping) = finalized {
            self.ensure_sync_server(mapping.id.as_str()).await?;
            self.verify_peer_in_background(mapping.id);
        }
        Ok(())
    }

    async fn accept_pairing_join(
        &self,
        request: PairingJoinRequest,
    ) -> Result<PairingJoinResponse> {
        Certificate::from_pem(request.certificate_pem.as_bytes())
            .context("invalid peer certificate")?;

        if request.pairing_id.trim().is_empty() {
            bail!("peer pairing request is missing pairing id");
        }

        let identity = self.ensure_identity().await?;
        let mut state = self.state.write().await;

        state
            .pairings
            .retain(|_, session| !pairing_expired(session));

        let matching_session_ids = state
            .pairings
            .iter()
            .filter(|(_, session)| session.pairing_proof == request.proof)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();

        let session_id = match matching_session_ids.as_slice() {
            [session_id] => session_id.clone(),
            [] => {
                tracing::debug!(
                    peer_sync_port = request.sync_port,
                    "Repo Sync inbound pairing request did not match an active pairing session"
                );
                return Ok(PairingJoinResponse {
                    matched: false,
                    pairing_id: String::new(),
                    certificate_pem: String::new(),
                    sync_port: None,
                    confirmed: false,
                });
            }
            matches => {
                tracing::warn!(
                    matching_sessions = matches.len(),
                    peer_sync_port = request.sync_port,
                    "Repo Sync rejected ambiguous inbound pairing request"
                );
                return Ok(PairingJoinResponse {
                    matched: false,
                    pairing_id: String::new(),
                    certificate_pem: String::new(),
                    sync_port: None,
                    confirmed: false,
                });
            }
        };

        let response = {
            let session = state
                .pairings
                .get_mut(session_id.as_str())
                .context("pairing session not found")?;

            session.verification_code = verification_code(
                identity.certificate_pem.as_str(),
                request.certificate_pem.as_str(),
            );
            session.peer_certificate_pem = request.certificate_pem;
            session.peer_pairing_id = request.pairing_id;
            session.peer_port = Some(request.sync_port);
            session.remote_confirmed = request.confirmed;

            PairingJoinResponse {
                matched: true,
                pairing_id: session.id.clone(),
                certificate_pem: identity.certificate_pem.clone(),
                sync_port: Some(session.local_port),
                confirmed: session.local_confirmed,
            }
        };

        let finalized = finalize_pairing_if_ready(&mut state, session_id.as_str())?;
        drop(state);
        tracing::info!(
            pairing_id = %session_id,
            peer_port = request.sync_port,
            remote_confirmed = request.confirmed,
            finalized = finalized.is_some(),
            "Repo Sync processed inbound pairing request"
        );
        if let Some(mapping) = finalized {
            self.ensure_sync_server(mapping.id.as_str()).await?;
            self.verify_peer_in_background(mapping.id);
        }
        Ok(response)
    }

    pub async fn confirm_pairing(&self, session_id: &str) -> Result<PairingSession> {
        let mut state = self.state.write().await;
        let session = state
            .pairings
            .get_mut(session_id)
            .context("pairing session not found")?;

        if pairing_expired(session) {
            bail!("pairing session expired");
        }
        if session.peer_certificate_pem.trim().is_empty() {
            bail!("peer has not been discovered yet");
        }

        session.local_confirmed = true;
        let snapshot = session.clone();
        let finalized = finalize_pairing_if_ready(&mut state, session_id)?;
        drop(state);
        if let Some(mapping) = finalized {
            self.ensure_sync_server(mapping.id.as_str()).await?;
            self.verify_peer_in_background(mapping.id);
        }
        Ok(snapshot)
    }

    pub async fn outbound_auto_apply_connected(
        &self,
        workflow_run_id: &str,
    ) -> Result<bool> {
        self.ensure_mappings_loaded().await?;
        self.expire_connection_leases().await?;

        let state = self.state.read().await;
        Ok(state.mappings.values().any(|mapping| {
            mapping.workflow_run_id == workflow_run_id
                && mapping.enabled
                && mapping.sync_mode == SyncMode::AutoApply
                && mapping.direction.can_send()
                && mapping.connected
        }))
    }

    pub async fn outbound_auto_apply_mapping(
        &self,
        workflow_run_id: &str,
    ) -> Result<Option<SyncMapping>> {
        self.ensure_mappings_loaded().await?;
        self.expire_connection_leases().await?;

        let state = self.state.read().await;
        let mapping = state
            .mappings
            .values()
            .find(|mapping| {
                mapping.workflow_run_id == workflow_run_id
                    && mapping.enabled
                    && mapping.sync_mode == SyncMode::AutoApply
                    && mapping.direction.can_send()
            })
            .cloned();

        let Some(mapping) = mapping else {
            return Ok(None);
        };

        if !mapping.connected {
            bail!("Repo Sync Auto Apply peer is disconnected");
        }
        if mapping.peer_port.is_none() {
            bail!("Repo Sync Auto Apply peer has no active data port");
        }
        if mapping.peer_certificate_pem.trim().is_empty() {
            bail!("Repo Sync Auto Apply peer has no trusted certificate");
        }

        Ok(Some(mapping))
    }

    async fn inbound_mapping(&self, mapping_id: &str) -> Result<SyncMapping> {
        self.ensure_mappings_loaded().await?;
        self.expire_connection_leases().await?;
        let state = self.state.read().await;
        let mapping = state
            .mappings
            .get(mapping_id)
            .cloned()
            .context("trusted Repo Sync mapping not found")?;

        if !mapping.enabled {
            bail!("Repo Sync is disabled for this workflow");
        }
        if !mapping.connected {
            bail!("trusted Repo Sync peer is disconnected");
        }
        if !mapping.direction.can_receive() {
            bail!("this Repo Sync mapping is configured as send only");
        }
        if mapping.peer_certificate_pem.trim().is_empty() {
            bail!("Repo Sync peer certificate is missing");
        }

        Ok(mapping)
    }

    pub async fn manual_send_mapping(&self, workflow_run_id: &str) -> Result<SyncMapping> {
        self.ensure_mappings_loaded().await?;
        self.expire_connection_leases().await?;
        let state = self.state.read().await;
        let mapping = state
            .mappings
            .values()
            .find(|mapping| mapping.workflow_run_id == workflow_run_id)
            .cloned()
            .context("this workflow has no trusted Repo Sync peer")?;

        if !mapping.enabled {
            bail!("Repo Sync is disabled for this workflow");
        }
        if !mapping.connected {
            bail!("trusted Repo Sync peer is disconnected; reconnect before sending a manual sync");
        }
        if mapping.sync_mode != SyncMode::Manual {
            bail!("manual hard sync is not selected for this workflow");
        }
        if !mapping.direction.can_send() {
            bail!("this Repo Sync mapping is configured as receive only");
        }
        if mapping.peer_port.is_none() {
            bail!("connected peer has no sync port");
        }
        if mapping.peer_certificate_pem.trim().is_empty() {
            bail!("connected peer has no trusted certificate");
        }

        Ok(mapping)
    }

    pub async fn send_manual_snapshot(
        &self,
        mapping: &SyncMapping,
        sync_id: &str,
        snapshot_json: &str,
    ) -> Result<String> {
        if mapping.sync_mode != SyncMode::Manual {
            bail!("manual hard sync is not selected for this workflow");
        }
        if !mapping.connected {
            bail!("trusted Repo Sync peer is disconnected");
        }
        if !mapping.direction.can_send() {
            bail!("this Repo Sync mapping is configured as receive only");
        }

        let identity = self.ensure_identity().await?;
        let client = build_mtls_client(&identity, mapping)?;
        let peer_port = mapping.peer_port.context("paired peer has no sync port")?;
        let url = format!("https://mdev-sync:{}/sync/v1/hard-sync", peer_port);
        let response = match client
            .post(url.as_str())
            .header("x-mdev-mapping-id", mapping.id.as_str())
            .header("x-mdev-sync-id", sync_id)
            .header("content-type", "application/json")
            .body(snapshot_json.to_string())
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                let _ = self
                    .mark_disconnected(mapping.id.as_str(), "manual sync transport failed")
                    .await;
                return Err(error).with_context(|| {
                    format!(
                        "failed to send manual repository sync to {}:{} via {}",
                        mapping.peer_ipv4,
                        peer_port,
                        url
                    )
                });
            }
        };

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("remote manual sync failed: {} {}", status, body);
        }

        Ok(body)
    }


    pub async fn send_changeset(
        &self,
        mapping: &SyncMapping,
        sync_id: &str,
        changeset_json: &str,
    ) -> Result<String> {
        let identity = self.ensure_identity().await?;
        let client = build_mtls_client(&identity, mapping)?;
        let peer_port = mapping.peer_port.context("paired peer has no sync port")?;
        let response = match client
            .post(format!(
                "https://mdev-sync:{}/sync/v1/changeset",
                peer_port
            ))
            .header("x-mdev-mapping-id", mapping.id.as_str())
            .header("x-mdev-sync-id", sync_id)
            .header("content-type", "application/json")
            .body(changeset_json.to_string())
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                let _ = self
                    .mark_disconnected(mapping.id.as_str(), "changeset transport failed")
                    .await;
                return Err(error).context("failed to send changeset");
            }
        };

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("remote changeset apply failed: {} {}", status, body);
        }

        Ok(body)
    }

    async fn connected_mapping(&self, mapping_id: &str) -> Result<SyncMapping> {
        self.ensure_mappings_loaded().await?;
        self.expire_connection_leases().await?;
        let state = self.state.read().await;
        let mapping = state
            .mappings
            .get(mapping_id)
            .cloned()
            .context("trusted Repo Sync mapping not found")?;

        if !mapping.enabled {
            bail!("Repo Sync is disabled for this workflow");
        }
        if !mapping.connected {
            bail!("trusted Repo Sync peer is disconnected");
        }
        if mapping.peer_port.is_none() {
            bail!("connected peer has no sync port");
        }
        if mapping.peer_certificate_pem.trim().is_empty() {
            bail!("connected peer has no trusted certificate");
        }

        Ok(mapping)
    }

    pub async fn peer_messages(&self, mapping_id: &str) -> Result<Vec<PeerMessage>> {
        self.ensure_mappings_loaded().await?;
        let mut state = self.state.write().await;
        prune_peer_messages_locked(&mut state);
        let connected = state
            .mappings
            .get(mapping_id)
            .map(|mapping| mapping.connected)
            .unwrap_or(false);
        if !connected {
            state.peer_messages.remove(mapping_id);
            return Ok(Vec::new());
        }
        Ok(state
            .peer_messages
            .get(mapping_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn store_peer_message(&self, mapping_id: &str, message: PeerMessage) {
        let mut state = self.state.write().await;
        prune_peer_messages_locked(&mut state);
        let messages = state.peer_messages.entry(mapping_id.to_string()).or_default();
        messages.push(message);
        while messages.len() > PEER_MESSAGE_MAX_COUNT
            || messages.iter().map(peer_message_size).sum::<usize>() > PEER_MESSAGE_MAX_RETAINED_BYTES
        {
            if messages.is_empty() {
                break;
            }
            messages.remove(0);
        }
    }

    pub async fn send_peer_message(
        &self,
        mapping_id: &str,
        blocks: Vec<PeerMessageBlock>,
    ) -> Result<PeerMessage> {
        validate_peer_message_blocks(&blocks)?;
        let mapping = self.connected_mapping(mapping_id).await?;
        let now = unix_ms_now();
        let envelope = PeerMessageEnvelope {
            id: Uuid::new_v4().to_string(),
            sent_at_unix_ms: now,
            blocks: blocks.clone(),
        };

        let identity = self.ensure_identity().await?;
        let client = build_mtls_client(&identity, &mapping)?;
        let peer_port = mapping.peer_port.context("paired peer has no sync port")?;
        let response = match client
            .post(format!("https://mdev-sync:{}/sync/v1/message", peer_port))
            .json(&envelope)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                let _ = self
                    .mark_disconnected(mapping_id, "peer message transport failed")
                    .await;
                return Err(error).context("failed to send peer message");
            }
        };

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("remote peer message failed: {} {}", status, body);
        }

        let message = PeerMessage {
            id: envelope.id,
            sent_at_unix_ms: envelope.sent_at_unix_ms,
            expires_at_unix_ms: now.saturating_add(PEER_MESSAGE_TTL_MS),
            author: "self".to_string(),
            blocks,
        };
        self.store_peer_message(mapping_id, message.clone()).await;
        Ok(message)
    }

    async fn accept_peer_message(
        &self,
        mapping_id: &str,
        envelope: PeerMessageEnvelope,
    ) -> Result<PeerMessage> {
        self.connected_mapping(mapping_id).await?;
        validate_peer_message_blocks(&envelope.blocks)?;
        let now = unix_ms_now();
        let message = PeerMessage {
            id: envelope.id,
            sent_at_unix_ms: envelope.sent_at_unix_ms,
            expires_at_unix_ms: now.saturating_add(PEER_MESSAGE_TTL_MS),
            author: "peer".to_string(),
            blocks: envelope.blocks,
        };
        self.store_peer_message(mapping_id, message.clone()).await;
        Ok(message)
    }
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn peer_message_block_size(block: &PeerMessageBlock) -> usize {
    match block {
        PeerMessageBlock::Text { text } => text.len(),
        PeerMessageBlock::Code { language, text } => language.len().saturating_add(text.len()),
        PeerMessageBlock::File {
            name,
            mime_type,
            data_base64,
        }
        | PeerMessageBlock::Image {
            name,
            mime_type,
            data_base64,
        } => name
            .len()
            .saturating_add(mime_type.len())
            .saturating_add(data_base64.len()),
    }
}

fn peer_message_size(message: &PeerMessage) -> usize {
    message
        .blocks
        .iter()
        .map(peer_message_block_size)
        .sum::<usize>()
}

fn validate_peer_message_blocks(blocks: &[PeerMessageBlock]) -> Result<()> {
    if blocks.is_empty() {
        bail!("peer message requires at least one block");
    }
    if blocks.len() > 32 {
        bail!("peer message has too many blocks");
    }

    let mut total = 0usize;
    for block in blocks {
        total = total.saturating_add(peer_message_block_size(block));
        match block {
            PeerMessageBlock::Text { text } | PeerMessageBlock::Code { text, .. } => {
                if text.trim().is_empty() {
                    bail!("text and code blocks may not be empty");
                }
            }
            PeerMessageBlock::File {
                name,
                data_base64,
                ..
            } => {
                if name.trim().is_empty() || data_base64.is_empty() {
                    bail!("file blocks require a name and data");
                }
            }
            PeerMessageBlock::Image {
                name,
                mime_type,
                data_base64,
            } => {
                if name.trim().is_empty() || data_base64.is_empty() {
                    bail!("image blocks require a name and data");
                }
                if !mime_type.starts_with("image/") {
                    bail!("image block MIME type must start with image/");
                }
            }
        }
    }

    if total > PEER_MESSAGE_MAX_BYTES {
        bail!("peer message exceeds the 1536 MiB limit");
    }
    Ok(())
}

fn prune_peer_messages_locked(state: &mut RepoSyncState) {
    let now = unix_ms_now();
    let active_sessions = state.active_sessions.clone();
    state.peer_messages.retain(|mapping_id, messages| {
        if !active_sessions.contains(mapping_id) {
            return false;
        }
        messages.retain(|message| message.expires_at_unix_ms > now);
        !messages.is_empty()
    });
}

async fn sync_ping() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

async fn sync_state(
    State(server): State<RepoSyncServerState>,
    Json(incoming): Json<SyncStateEnvelope>,
) -> Result<Json<SyncStateEnvelope>, (StatusCode, String)> {
    server
        .runtime
        .accept_sync_state(server.mapping_id.as_str(), incoming)
        .await
        .map(Json)
        .map_err(sync_forbidden)
}

async fn sync_peer_message(
    State(server): State<RepoSyncServerState>,
    Json(envelope): Json<PeerMessageEnvelope>,
) -> Result<Json<PeerMessage>, (StatusCode, String)> {
    let message = server
        .runtime
        .accept_peer_message(server.mapping_id.as_str(), envelope)
        .await
        .map_err(sync_forbidden)?;
    Ok(Json(message))
}

async fn sync_changeset(
    State(server): State<RepoSyncServerState>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_sync_id(&headers).map_err(sync_bad_request)?;
    server
        .runtime
        .inbound_mapping(server.mapping_id.as_str())
        .await
        .map_err(sync_forbidden)?;
    validate_remote_changeset_paths(body.as_str()).map_err(sync_bad_request)?;

    let repo_ref = workflow_repo_ref(&server.db, server.workflow_run_id.as_str())
        .await
        .map_err(sync_internal)?;
    let result = execute_changeset_apply(Path::new(repo_ref.as_str()), body.as_str(), "WORKTREE")
        .map_err(sync_bad_request)?;

    if !result.get("ok").and_then(serde_json::Value::as_bool).unwrap_or(false) {
        return Err((StatusCode::CONFLICT, result.to_string()));
    }

    Ok(Json(result))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncNewlineStyle {
    None,
    Lf,
    CrLf,
    MixedOrOther,
}

fn sync_newline_style(value: &str) -> SyncNewlineStyle {
    let bytes = value.as_bytes();
    let mut saw_lf = false;
    let mut saw_crlf = false;
    let mut saw_lone_cr = false;
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                if index + 1 < bytes.len() && bytes[index + 1] == b'\n' {
                    saw_crlf = true;
                    index += 2;
                } else {
                    saw_lone_cr = true;
                    index += 1;
                }
            }
            b'\n' => {
                saw_lf = true;
                index += 1;
            }
            _ => {
                index += 1;
            }
        }
    }

    if saw_lone_cr || (saw_lf && saw_crlf) {
        SyncNewlineStyle::MixedOrOther
    } else if saw_crlf {
        SyncNewlineStyle::CrLf
    } else if saw_lf {
        SyncNewlineStyle::Lf
    } else {
        SyncNewlineStyle::None
    }
}

fn normalize_crlf_for_sync_compare(value: &str) -> String {
    value.replace("\r\n", "\n")
}

fn sync_text_equivalent(existing: &str, incoming: &str) -> bool {
    if existing == incoming {
        return true;
    }

    if matches!(sync_newline_style(existing), SyncNewlineStyle::MixedOrOther)
        || matches!(sync_newline_style(incoming), SyncNewlineStyle::MixedOrOther)
    {
        return false;
    }

    normalize_crlf_for_sync_compare(existing) == normalize_crlf_for_sync_compare(incoming)
}

fn preserve_sync_newline_style(existing: &str, incoming: &str) -> String {
    let existing_style = sync_newline_style(existing);
    let incoming_style = sync_newline_style(incoming);

    match (existing_style, incoming_style) {
        (SyncNewlineStyle::CrLf, SyncNewlineStyle::Lf) => {
            incoming.replace('\n', "\r\n")
        }
        (SyncNewlineStyle::Lf, SyncNewlineStyle::CrLf) => {
            incoming.replace("\r\n", "\n")
        }
        _ => incoming.to_string(),
    }
}

async fn sync_hard_sync(
    State(server): State<RepoSyncServerState>,
    headers: HeaderMap,
    Json(snapshot): Json<ContextSyncSnapshot>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let sync_id = require_sync_id(&headers).map_err(sync_bad_request)?;
    server
        .runtime
        .inbound_mapping(server.mapping_id.as_str())
        .await
        .map_err(sync_forbidden)?;

    let (repo_ref, _) = workflow_repo_context(&server.db, server.workflow_run_id.as_str())
        .await
        .map_err(sync_internal)?;
    let repo = PathBuf::from(repo_ref.as_str());

    let local_scope = serde_json::json!({
        "repo_ref": repo_ref,
        "git_ref": "WORKTREE",
        "include_files": snapshot.include_files,
        "include_directories": snapshot.include_directories,
        "exclude_files": snapshot.exclude_files,
        "exclude_directories": snapshot.exclude_directories,
        "include_override_regex": snapshot.include_override_regex,
        "include_staged_diff": false,
        "include_unstaged_diff": false,
        "skip_binary": snapshot.skip_binary,
        "skip_gitignore": snapshot.skip_gitignore,
        "exclude_regex": snapshot.exclude_regex,
        "save_path": "",
        "inline_repo_context_in_prompt": false
    });
    let local_snapshot = match build_existing_context_sync_snapshot(local_scope) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            tracing::warn!(
                workflow_run_id = %server.workflow_run_id,
                repo = %repo.display(),
                repo_exists = repo.exists(),
                repo_is_dir = repo.is_dir(),
                error = %format!("{:#}", error),
                "failed to build receiver Context Exporter snapshot; continuing with empty hard-sync baseline"
            );
            ContextSyncSnapshot {
                files: Vec::new(),
                include_files: snapshot.include_files.clone(),
                include_directories: snapshot.include_directories.clone(),
                exclude_files: snapshot.exclude_files.clone(),
                exclude_directories: snapshot.exclude_directories.clone(),
                include_override_regex: snapshot.include_override_regex.clone(),
                skip_binary: snapshot.skip_binary,
                skip_gitignore: snapshot.skip_gitignore,
                exclude_regex: snapshot.exclude_regex.clone(),
            }
        }
    };

    let mut incoming = HashMap::<String, String>::new();
    for file in snapshot.files {
        let path = validate_sync_path(file.path.as_str()).map_err(sync_bad_request)?;
        if incoming.insert(path.clone(), file.contents).is_some() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("duplicate hard-sync path {}", path),
            ));
        }
    }

    let incoming_paths = incoming.keys().cloned().collect::<HashSet<_>>();
    let mut deleted = 0usize;
    for file in local_snapshot.files {
        let path = validate_sync_path(file.path.as_str()).map_err(sync_bad_request)?;
        if !incoming_paths.contains(path.as_str()) {
            let full = repo.join(path.as_str());
            if full.exists() {
                fs::remove_file(&full)
                    .with_context(|| format!("failed to delete {}", full.display()))
                    .map_err(sync_internal)?;
                deleted += 1;
            }
        }
    }

    let mut written = 0usize;
    let mut unchanged = 0usize;
    for (path, contents) in incoming {
        let normalized = validate_sync_path(path.as_str()).map_err(sync_bad_request)?;
        let mut full = repo.clone();
        for component in normalized.split('/') {
            full.push(component);
        }

        let contents_to_write = if full.is_file() {
            match fs::read(&full) {
                Ok(existing_bytes) => match String::from_utf8(existing_bytes) {
                    Ok(existing) => {
                        if sync_text_equivalent(existing.as_str(), contents.as_str()) {
                            unchanged += 1;
                            continue;
                        }
                        preserve_sync_newline_style(existing.as_str(), contents.as_str())
                    }
                    Err(_) => contents,
                },
                Err(error) => {
                    return Err(sync_internal(anyhow::anyhow!(
                        "failed to read {} before hard sync: {}",
                        full.display(),
                        error
                    )));
                }
            }
        } else {
            contents
        };

        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))
                .map_err(sync_internal)?;
        }
        fs::write(&full, contents_to_write.as_bytes())
            .with_context(|| format!("failed to write {}", full.display()))
            .map_err(sync_internal)?;
        written += 1;
    }

    Ok(Json(serde_json::json!({
        "ok": true,
        "sync_id": sync_id,
        "written": written,
        "unchanged": unchanged,
        "deleted": deleted
    })))
}

async fn workflow_repo_ref(db: &SqlitePool, workflow_run_id: &str) -> Result<String> {
    workflow_repo_context(db, workflow_run_id)
        .await
        .map(|(repo_ref, _)| repo_ref)
}

async fn workflow_repo_context(
    db: &SqlitePool,
    workflow_run_id: &str,
) -> Result<(String, serde_json::Value)> {
    let row = sqlx::query("SELECT repo_ref, context_json FROM workflow_runs WHERE id = ?")
        .bind(workflow_run_id)
        .fetch_optional(db)
        .await?
        .context("receiving workflow run not found")?;
    let repo_ref: String = row.get("repo_ref");
    let repo_ref = repo_ref.trim().to_string();
    let context_json: String = row.get("context_json");
    let context = serde_json::from_str(&context_json).unwrap_or_else(|_| serde_json::json!({}));
    Ok((repo_ref, context))
}

fn require_sync_id(headers: &HeaderMap) -> Result<String> {
    let value = headers
        .get("x-mdev-sync-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("x-mdev-sync-id is required")?;
    Ok(value.to_string())
}

fn validate_sync_path(path: &str) -> Result<String> {
    let normalized = filesystem::normalize_rel_path(path)?;
    if normalized.is_empty() {
        bail!("sync path is empty");
    }
    if normalized == ".git"
        || normalized.starts_with(".git/")
        || normalized == ".mdev"
        || normalized.starts_with(".mdev/")
    {
        bail!("protected path may not be synchronized: {}", normalized);
    }
    Ok(normalized)
}

fn validate_remote_changeset_paths(body: &str) -> Result<()> {
    let payload: serde_json::Value = serde_json::from_str(body)
        .context("failed to parse remote ChangeSet")?;
    let operations = payload
        .get("operations")
        .and_then(serde_json::Value::as_array)
        .context("remote ChangeSet operations are required")?;

    for operation in operations {
        match operation.get("op").and_then(serde_json::Value::as_str).unwrap_or("") {
            "write" | "delete" | "edit" => {
                let path = operation
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .context("remote ChangeSet operation path is required")?;
                validate_sync_path(path)?;
            }
            "move" => {
                let from = operation
                    .get("from")
                    .and_then(serde_json::Value::as_str)
                    .context("remote ChangeSet move.from is required")?;
                let to = operation
                    .get("to")
                    .and_then(serde_json::Value::as_str)
                    .context("remote ChangeSet move.to is required")?;
                validate_sync_path(from)?;
                validate_sync_path(to)?;
            }
            other => bail!("unsupported remote ChangeSet operation {}", other),
        }
    }

    Ok(())
}

fn sync_bad_request(error: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, error.to_string())
}

fn sync_forbidden(error: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::FORBIDDEN, error.to_string())
}

fn sync_internal(error: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

async fn pairing_join(
    State(runtime): State<RepoSyncRuntime>,
    Json(request): Json<PairingJoinRequest>,
) -> Result<Json<PairingJoinResponse>, (StatusCode, String)> {
    runtime
        .accept_pairing_join(request)
        .await
        .map(Json)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))
}

async fn reconnect_join(
    State(runtime): State<RepoSyncRuntime>,
    Json(request): Json<ReconnectRequest>,
) -> Result<Json<ReconnectResponse>, (StatusCode, String)> {
    runtime
        .accept_reconnect(request)
        .await
        .map(Json)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))
}

fn finalize_pairing_if_ready(
    state: &mut RepoSyncState,
    session_id: &str,
) -> Result<Option<SyncMapping>> {
    let Some(session) = state.pairings.get(session_id).cloned() else {
        return Ok(None);
    };

    if session.complete || !session.local_confirmed || !session.remote_confirmed {
        return Ok(None);
    }

    if session.peer_certificate_pem.trim().is_empty() {
        return Ok(None);
    }

    if session.peer_pairing_id.trim().is_empty() {
        return Ok(None);
    }

    let link_id = pairing_link_id(
        session.id.as_str(),
        session.peer_pairing_id.as_str(),
        session.local_certificate_pem.as_str(),
        session.peer_certificate_pem.as_str(),
    );

    let peer_port = session.peer_port.context("peer sync port is missing")?;
    let mapping = state
        .mappings
        .get_mut(session.mapping_id.as_str())
        .context("mapping not found")?;

    mapping.peer_certificate_pem = session.peer_certificate_pem;
    mapping.link_id = link_id;
    mapping.peer_port = Some(peer_port);
    mapping.enabled = true;
    mapping.connected = false;
    let mapping = mapping.clone();

    if let Some(session) = state.pairings.get_mut(session_id) {
        session.complete = true;
    }

    persist_mappings(&state.mappings)?;
    Ok(Some(mapping))
}

fn pairing_proof(passphrase: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mdev-repo-sync-pair-v1\0");
    hasher.update(passphrase.trim().as_bytes());
    hex::encode(hasher.finalize())
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    prior_results: &[CapabilityResult],
    _config: serde_json::Value,
) -> Result<CapabilityResult> {
    let changeset = find_result(prior_results, "changeset")
        .context("Repo Sync ChangeSet requires a successful ChangeSet result")?;

    let successful_actions = changeset
        .payload
        .get("stats")
        .and_then(|stats| stats.get("successful_actions"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    if successful_actions == 0 {
        return Ok(CapabilityResult {
            ok: true,
            capability: "repo_sync_changeset".to_string(),
            payload: serde_json::json!({
                "ok": true,
                "state": "skipped",
                "reason": "no_local_changes_applied"
            }),
            follow_ups: CapabilityInvocationRequest::None,
        });
    }

    let changeset_json = changeset
        .payload
        .get("applied_payload")
        .and_then(serde_json::Value::as_str)
        .context("ChangeSet result with applied actions is missing applied_payload")?;
    let workflow_run_id = ctx.run_id.to_string();

    let mapping = match ctx
        .state
        .repo_sync
        .outbound_auto_apply_mapping(workflow_run_id.as_str())
        .await
    {
        Ok(Some(mapping)) => mapping,
        Ok(None) => {
            return Ok(CapabilityResult {
                ok: true,
                capability: "repo_sync_changeset".to_string(),
                payload: serde_json::json!({
                    "ok": true,
                    "state": "skipped",
                    "reason": "auto_apply_not_configured"
                }),
                follow_ups: CapabilityInvocationRequest::None,
            });
        }
        Err(error) => {
            return Ok(CapabilityResult {
                ok: false,
                capability: "repo_sync_changeset".to_string(),
                payload: serde_json::json!({
                    "ok": false,
                    "state": "unavailable",
                    "error": format!("{:#}", error)
                }),
                follow_ups: CapabilityInvocationRequest::None,
            });
        }
    };

    let sync_id = Uuid::new_v4().to_string();
    match ctx
        .state
        .repo_sync
        .send_changeset(&mapping, sync_id.as_str(), changeset_json)
        .await
    {
        Ok(response) => Ok(CapabilityResult {
            ok: true,
            capability: "repo_sync_changeset".to_string(),
            payload: serde_json::json!({
                "ok": true,
                "state": "delivered",
                "sync_id": sync_id,
                "mapping_id": mapping.id,
                "peer_ipv4": mapping.peer_ipv4,
                "peer_port": mapping.peer_port,
                "remote_response": response
            }),
            follow_ups: CapabilityInvocationRequest::None,
        }),
        Err(error) => Ok(CapabilityResult {
            ok: false,
            capability: "repo_sync_changeset".to_string(),
            payload: serde_json::json!({
                "ok": false,
                "state": "rejected",
                "sync_id": sync_id,
                "mapping_id": mapping.id,
                "peer_ipv4": mapping.peer_ipv4,
                "peer_port": mapping.peer_port,
                "error": format!("{:#}", error)
            }),
            follow_ups: CapabilityInvocationRequest::None,
        }),
    }
}

fn identity_directory() -> PathBuf {
    if let Ok(value) = std::env::var("MDEV_DATA_DIR") {
        let value = value.trim();
        if !value.is_empty() {
            return PathBuf::from(value).join("repo-sync");
        }
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = if cwd.file_name().and_then(|value| value.to_str()) == Some("api") {
        cwd.parent().map(Path::to_path_buf).unwrap_or(cwd)
    } else {
        cwd
    };

    root.join(".mdev").join("repo-sync")
}

fn peers_path() -> PathBuf {
    identity_directory().join("peers.json")
}

fn load_persisted_mappings() -> Result<Vec<SyncMapping>> {
    let path = peers_path();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let text = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut mappings: Vec<SyncMapping> = serde_json::from_str(&text)
        .with_context(|| format!("failed to decode {}", path.display()))?;
    for mapping in &mut mappings {
        mapping.connected = false;
        mapping.peer_port = None;
    }
    Ok(mappings)
}

fn persist_mappings(mappings: &HashMap<String, SyncMapping>) -> Result<()> {
    let directory = identity_directory();
    fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;

    let mut saved = mappings.values().cloned().collect::<Vec<_>>();
    saved.sort_by(|left, right| left.workflow_run_id.cmp(&right.workflow_run_id));
    for mapping in &mut saved {
        mapping.connected = false;
        mapping.peer_port = None;
    }

    let path = peers_path();
    let text = serde_json::to_string_pretty(&saved)?;
    fs::write(&path, text)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn load_identity_if_present() -> Result<Option<LocalSyncIdentity>> {
    let directory = identity_directory();
    let certificate_path = directory.join("identity-cert.pem");
    let private_key_path = directory.join("identity-key.pem");

    if !certificate_path.exists() && !private_key_path.exists() {
        return Ok(None);
    }

    if !certificate_path.exists() || !private_key_path.exists() {
        bail!(
            "Repo Sync identity is incomplete under {}",
            directory.display()
        );
    }

    let certificate_pem = fs::read_to_string(&certificate_path)
        .with_context(|| format!("failed to read {}", certificate_path.display()))?;
    let private_key_pem = fs::read_to_string(&private_key_path)
        .with_context(|| format!("failed to read {}", private_key_path.display()))?;

    Ok(Some(LocalSyncIdentity {
        fingerprint: certificate_fingerprint(certificate_pem.as_str()),
        certificate_pem,
        private_key_pem,
    }))
}

fn load_or_create_identity() -> Result<LocalSyncIdentity> {
    if let Some(identity) = load_identity_if_present()? {
        return Ok(identity);
    }

    let directory = identity_directory();
    fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;

    let certificate_path = directory.join("identity-cert.pem");
    let private_key_path = directory.join("identity-key.pem");

    let certified = generate_simple_self_signed(vec![
        "mdev-sync".to_string(),
        "localhost".to_string(),
    ])
    .context("failed to generate sync identity")?;

    let certificate_pem = certified.cert.pem();
    let private_key_pem = certified.key_pair.serialize_pem();

    fs::write(&certificate_path, certificate_pem.as_bytes())
        .with_context(|| format!("failed to write {}", certificate_path.display()))?;
    fs::write(&private_key_path, private_key_pem.as_bytes())
        .with_context(|| format!("failed to write {}", private_key_path.display()))?;

    Ok(LocalSyncIdentity {
        fingerprint: certificate_fingerprint(certificate_pem.as_str()),
        certificate_pem,
        private_key_pem,
    })
}

fn build_mtls_client(
    identity: &LocalSyncIdentity,
    mapping: &SyncMapping,
) -> Result<Client> {
    let identity_pem = format!(
        "{}\n{}",
        identity.certificate_pem, identity.private_key_pem
    );
    let identity = Identity::from_pem(identity_pem.as_bytes())
        .context("failed to load local sync identity")?;
    let peer_certificate = Certificate::from_pem(mapping.peer_certificate_pem.as_bytes())
        .context("failed to load peer certificate")?;
    let peer_port = mapping.peer_port.context("paired peer has no sync port")?;

    ClientBuilder::new()
        .identity(identity)
        .add_root_certificate(peer_certificate)
        .resolve(
            "mdev-sync",
            SocketAddr::new(mapping.peer_ipv4, peer_port),
        )
        .https_only(true)
        .timeout(Duration::from_secs(30 * 60))
        .build()
        .context("failed to build sync client")
}

fn build_mtls_server_config(
    identity: &LocalSyncIdentity,
    peer_certificate_pem: &str,
) -> Result<RustlsConfig> {
    let certificates = CertificateDer::pem_slice_iter(identity.certificate_pem.as_bytes())
        .collect::<std::result::Result<Vec<CertificateDer<'static>>, _>>()
        .context("failed to parse Repo Sync server certificate")?;
    if certificates.is_empty() {
        bail!("Repo Sync server certificate is empty");
    }

    let private_key: PrivateKeyDer<'static> =
        PrivateKeyDer::from_pem_slice(identity.private_key_pem.as_bytes())
            .context("failed to parse Repo Sync private key")?;

    let peer_certificates = CertificateDer::pem_slice_iter(peer_certificate_pem.as_bytes())
        .collect::<std::result::Result<Vec<CertificateDer<'static>>, _>>()
        .context("failed to parse trusted peer certificate")?;
    if peer_certificates.is_empty() {
        bail!("trusted peer certificate is empty");
    }

    let mut roots = RootCertStore::empty();
    for certificate in peer_certificates {
        roots
            .add(certificate)
            .context("failed to add trusted Repo Sync peer certificate")?;
    }

    let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .context("failed to build Repo Sync client certificate verifier")?;
    let config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(certificates, private_key)
        .context("failed to build Repo Sync TLS server configuration")?;

    Ok(RustlsConfig::from_config(Arc::new(config)))
}

fn certificate_fingerprint(certificate_pem: &str) -> String {
    hex::encode(Sha256::digest(certificate_pem.as_bytes()))
}

fn verification_code(local_certificate_pem: &str, peer_certificate_pem: &str) -> String {
    let mut fingerprints = [
        certificate_fingerprint(local_certificate_pem),
        certificate_fingerprint(peer_certificate_pem),
    ];
    fingerprints.sort();

    let mut hasher = Sha256::new();
    hasher.update(fingerprints[0].as_bytes());
    hasher.update(fingerprints[1].as_bytes());
    let value = hex::encode(hasher.finalize());

    format!("{}-{}-{}", &value[0..4], &value[4..8], &value[8..12])
}

fn pairing_expired(session: &PairingSession) -> bool {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        > session.expires_at_unix_ms
}
