use std::net::IpAddr;

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::capabilities::{
        context_export::{build_context_sync_snapshot, normalize_context_export_payload},
        repo_sync::{PeerMessageBlock, SyncDirection, SyncMapping, SyncMode},
    },
};

#[derive(Debug, Deserialize)]
struct UpsertMappingRequest {
    #[serde(default)]
    id: String,
    workflow_run_id: String,
    peer_ipv4: IpAddr,
    direction: SyncDirection,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    sync_mode: SyncMode,
}

#[derive(Debug, Deserialize)]
struct StartPairingRequest {
    passphrase: String,
}

#[derive(Debug, Deserialize)]
struct StatusQuery {
    workflow_run_id: String,
}

#[derive(Debug, Deserialize)]
struct ManualSyncRequest {
    workflow_run_id: String,
    #[serde(default)]
    preview: bool,
}

#[derive(Debug, Deserialize)]
struct SendPeerMessageRequest {
    blocks: Vec<PeerMessageBlock>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/repo-sync/status", get(status))
        .route("/api/repo-sync/mappings", put(upsert_mapping))
        .route(
            "/api/repo-sync/mappings/:mapping_id/pairing/start",
            post(start_pairing),
        )
        .route(
            "/api/repo-sync/pairing/:session_id/confirm",
            post(confirm_pairing),
        )
        .route(
            "/api/repo-sync/mappings/:mapping_id/reconnect",
            post(reconnect),
        )
        .route(
            "/api/repo-sync/mappings/:mapping_id",
            delete(unpair),
        )
        .route("/api/repo-sync/manual-sync", post(manual_sync))
        .route(
            "/api/repo-sync/mappings/:mapping_id/messages",
            get(peer_messages)
                .post(send_peer_message)
                .layer(DefaultBodyLimit::max(24 * 1024 * 1024)),
        )
}

async fn status(
    State(state): State<AppState>,
    Query(query): Query<StatusQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state
        .repo_sync
        .listen_for_reconnects(&state.db)
        .await
        .map_err(internal)?;
    let identity = state
        .repo_sync
        .load_identity_if_present()
        .await
        .map_err(internal)?;

    let mut status = serde_json::to_value(
        state
            .repo_sync
            .status(query.workflow_run_id.trim())
            .await
            .map_err(internal)?,
    )
    .map_err(internal)?;
    if let Some(object) = status.as_object_mut() {
        object.insert(
            "local_certificate_pem".to_string(),
            identity
                .map(|identity| Value::String(identity.certificate_pem))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "local_ipv4".to_string(),
            state
                .runtime_endpoints
                .local_lan_ipv4()
                .map(|address| Value::String(address.to_string()))
                .unwrap_or(Value::Null),
        );
    }
    Ok(Json(status))
}

async fn upsert_mapping(
    State(state): State<AppState>,
    Json(req): Json<UpsertMappingRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mapping = state
        .repo_sync
        .upsert_mapping(SyncMapping {
            id: req.id,
            workflow_run_id: req.workflow_run_id,
            peer_ipv4: req.peer_ipv4,
            peer_port: None,
            direction: req.direction,
            peer_certificate_pem: String::new(),
            enabled: req.enabled,
            sync_mode: req.sync_mode,
            connected: false,
        })
        .await
        .map_err(internal)?;

    Ok(Json(serde_json::to_value(mapping).map_err(internal)?))
}

async fn start_pairing(
    State(state): State<AppState>,
    Path(mapping_id): Path<String>,
    Json(req): Json<StartPairingRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let session = state
        .repo_sync
        .start_pairing(
            &state.db,
            &state.runtime_endpoints,
            mapping_id.as_str(),
            req.passphrase.as_str(),
        )
        .await
        .map_err(internal)?;

    Ok(Json(serde_json::to_value(session).map_err(internal)?))
}

async fn confirm_pairing(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let session = state
        .repo_sync
        .confirm_pairing(session_id.as_str())
        .await
        .map_err(internal)?;

    Ok(Json(serde_json::to_value(session).map_err(internal)?))
}

async fn reconnect(
    State(state): State<AppState>,
    Path(mapping_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mapping = state
        .repo_sync
        .reconnect(&state.db, &state.runtime_endpoints, mapping_id.as_str())
        .await
        .map_err(internal)?;
    Ok(Json(serde_json::to_value(mapping).map_err(internal)?))
}

async fn unpair(
    State(state): State<AppState>,
    Path(mapping_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state
        .repo_sync
        .unpair(mapping_id.as_str())
        .await
        .map_err(internal)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn manual_sync(
    State(state): State<AppState>,
    Json(req): Json<ManualSyncRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let workflow_run_id = req.workflow_run_id.trim();
    if workflow_run_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "workflow_run_id is required".to_string(),
        ));
    }

    let row = sqlx::query("SELECT repo_ref, context_json FROM workflow_runs WHERE id = ?")
        .bind(workflow_run_id)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "workflow run not found".to_string()))?;

    let repo_ref: String = row.get("repo_ref");
    let context_json: String = row.get("context_json");
    let context: Value = serde_json::from_str(&context_json).unwrap_or_else(|_| json!({}));
    let context_export = context
        .get("workflow_engine")
        .and_then(|value| value.get("global_state"))
        .and_then(|value| value.get("capabilities"))
        .and_then(|value| value.get("context_export"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let payload = normalize_context_export_payload(
        context_export,
        Some(json!({
            "repo_ref": repo_ref,
            "git_ref": "WORKTREE"
        })),
        repo_ref.as_str(),
    );
    let snapshot = build_context_sync_snapshot(payload).map_err(internal)?;
    let file_count = snapshot.files.len();
    let total_bytes = snapshot
        .files
        .iter()
        .map(|file| file.contents.as_bytes().len())
        .sum::<usize>();

    if req.preview {
        return Ok(Json(json!({
            "ok": true,
            "file_count": file_count,
            "total_bytes": total_bytes
        })));
    }

    let mapping = state
        .repo_sync
        .manual_send_mapping(workflow_run_id)
        .await
        .map_err(|error| (StatusCode::CONFLICT, error.to_string()))?;
    let sync_id = Uuid::new_v4().to_string();
    let snapshot_json = serde_json::to_string(&snapshot).map_err(internal)?;
    let remote_response = state
        .repo_sync
        .send_manual_snapshot(&mapping, sync_id.as_str(), snapshot_json.as_str())
        .await
        .map_err(internal)?;

    Ok(Json(json!({
        "ok": true,
        "sync_id": sync_id,
        "file_count": file_count,
        "total_bytes": total_bytes,
        "remote_response": remote_response
    })))
}

async fn peer_messages(
    State(state): State<AppState>,
    Path(mapping_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let messages = state
        .repo_sync
        .peer_messages(mapping_id.as_str())
        .await
        .map_err(internal)?;
    Ok(Json(serde_json::to_value(messages).map_err(internal)?))
}

async fn send_peer_message(
    State(state): State<AppState>,
    Path(mapping_id): Path<String>,
    Json(req): Json<SendPeerMessageRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let message = state
        .repo_sync
        .send_peer_message(mapping_id.as_str(), req.blocks)
        .await
        .map_err(|error| (StatusCode::CONFLICT, format!("{:#}", error)))?;
    Ok(Json(serde_json::to_value(message).map_err(internal)?))
}

fn internal<E>(error: E) -> (StatusCode, String)
where
    E: Into<anyhow::Error>,
{
    let error: anyhow::Error = error.into();
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{:#}", error))
}
