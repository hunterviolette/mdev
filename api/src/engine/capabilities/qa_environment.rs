use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    net::Ipv4Addr,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::engine::runtime_endpoints::{NetworkExposure, RuntimeEndpointManager};

use crate::engine::{
    capabilities::{
        registry::{
            find_result,
            CapabilityContext,
            CapabilityInvocationRequest,
            CapabilityResult,
        },
        terminal_runtime::{self, ProcessRecord, TerminalExecutionOwner},
    },
    runtime_tools::{
        QaDirectoryMapping,
        QaDirectoryMode,
        QaSymlinkBehavior,
        QaEnvironmentSpec,
        QaReadinessSpec,
        QaStageSpec,
        TerminalCommandMode,
    },
};

use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use axum::{
    body::{to_bytes, Body},
    extract::{ws::{Message, WebSocketUpgrade}, FromRequestParts, State},
    http::{header, HeaderMap, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use crate::app_state::AppState;

#[derive(Clone)]
pub struct QaProxyTarget {
    pub owner: String,
    pub port: u16,
}

#[derive(Clone, Default)]
pub struct QaReverseProxy {
    routes: Arc<Mutex<HashMap<String, QaProxyTarget>>>,
}

fn qa_hostname_slug(value: &str, max_length: usize) -> String {
    let mut slug = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if slug.len() >= max_length {
                break;
            }
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            if slug.len() >= max_length {
                break;
            }
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "qa".to_string()
    } else {
        slug.to_string()
    }
}

impl QaReverseProxy {
    pub fn reserve(&self, owner: &str, template: &str, workflow_title: &str, service: &str, primary: bool, port: u16) -> String {
        let workflow = qa_hostname_slug(workflow_title, 48);
        let service_slug = qa_hostname_slug(service, 24);
        let template = if template.trim() == "{run}.qa.localhost" {
            "{workflow}.qa.localhost"
        } else {
            template
        };
        let has_service = template.contains("{service}");
        let resolved = template
            .replace("{workflow}", &workflow)
            .replace("{run}", owner.split(':').next().unwrap_or(owner))
            .replace("{deployment}", owner.split(':').next().unwrap_or(owner))
            .replace("{service}", &service_slug);
        let base = resolved
            .trim()
            .trim_end_matches(".localhost")
            .split('.')
            .map(|label| qa_hostname_slug(label, 48))
            .collect::<Vec<_>>()
            .join(".");
        let base = if !primary && !has_service {
            format!("{}-{}", base, service_slug)
        } else {
            base
        };
        let mut routes = self.routes.lock().unwrap();
        let mut suffix = 1u64;
        loop {
            let candidate = if suffix == 1 {
                format!("{}.localhost", base)
            } else {
                format!("{}-{}.localhost", base, suffix)
            };
            match routes.get(&candidate) {
                Some(existing) if existing.owner == owner && existing.port == port => {
                    return candidate;
                }
                Some(_) => {
                    suffix += 1;
                }
                None => {
                    routes.insert(candidate.clone(), QaProxyTarget {
                        owner: owner.to_string(),
                        port,
                    });
                    return candidate;
                }
            }
        }
    }

    pub fn release_owner(&self, owner: &str) {
        self.routes.lock().unwrap().retain(|_, target| target.owner != owner);
    }

    pub fn lookup(&self, hostname: &str) -> Option<QaProxyTarget> {
        self.routes.lock().unwrap().get(&hostname.to_ascii_lowercase()).cloned()
    }
}

pub async fn proxy_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let hostname = request.headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    if !hostname.ends_with(".localhost") || hostname == "localhost" {
        return next.run(request).await;
    }

    let Some(target) = state.qa_reverse_proxy.lookup(&hostname) else {
        return (StatusCode::NOT_FOUND, "Unknown QA hostname").into_response();
    };

    let upgrade_requested = request.headers()
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));

    if upgrade_requested {
        let (mut parts, _) = request.into_parts();
        let websocket = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
            Ok(websocket) => websocket,
            Err(error) => return error.into_response(),
        };
        let path = parts.uri.path_and_query().map(|value| value.as_str()).unwrap_or("/").to_string();
        let upstream = format!("ws://127.0.0.1:{}{}", target.port, path);
        return websocket.on_upgrade(move |client| async move {
            let Ok((server, _)) = tokio_tungstenite::connect_async(upstream).await else { return; };
            let (mut client_tx, mut client_rx) = client.split();
            let (mut server_tx, mut server_rx) = server.split();
            let client_to_server = async {
                while let Some(Ok(message)) = client_rx.next().await {
                    let message = match message {
                        Message::Text(value) => tokio_tungstenite::tungstenite::Message::Text(value.to_string().into()),
                        Message::Binary(value) => tokio_tungstenite::tungstenite::Message::Binary(value.to_vec().into()),
                        Message::Ping(value) => tokio_tungstenite::tungstenite::Message::Ping(value.to_vec().into()),
                        Message::Pong(value) => tokio_tungstenite::tungstenite::Message::Pong(value.to_vec().into()),
                        Message::Close(_) => break,
                    };
                    if server_tx.send(message).await.is_err() { break; }
                }
            };
            let server_to_client = async {
                while let Some(Ok(message)) = server_rx.next().await {
                    let message = match message {
                        tokio_tungstenite::tungstenite::Message::Text(value) => Message::Text(value.to_string().into()),
                        tokio_tungstenite::tungstenite::Message::Binary(value) => Message::Binary(value.to_vec().into()),
                        tokio_tungstenite::tungstenite::Message::Ping(value) => Message::Ping(value.to_vec().into()),
                        tokio_tungstenite::tungstenite::Message::Pong(value) => Message::Pong(value.to_vec().into()),
                        tokio_tungstenite::tungstenite::Message::Close(_) => break,
                        _ => continue,
                    };
                    if client_tx.send(message).await.is_err() { break; }
                }
            };
            tokio::select! { _ = client_to_server => {}, _ = server_to_client => {} }
        }).into_response();
    }

    forward_qa_http(request, target.port).await
}

async fn forward_qa_http(request: Request<Body>, port: u16) -> Response {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build();
    let Ok(client) = client else { return StatusCode::BAD_GATEWAY.into_response(); };
    let (parts, body) = request.into_parts();
    let Ok(body) = to_bytes(body, 64 * 1024 * 1024).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let path = parts.uri.path_and_query().map(|value| value.as_str()).unwrap_or("/");
    let url = format!("http://127.0.0.1:{}{}", port, path);
    let mut upstream = client.request(parts.method, url).body(body);
    let mut headers = HeaderMap::new();
    for (name, value) in &parts.headers {
        if name != header::HOST && name != header::CONNECTION && name != header::UPGRADE && name != header::CONTENT_LENGTH {
            headers.append(name, value.clone());
        }
    }
    upstream = upstream.headers(headers);
    let Ok(response) = upstream.send().await else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let status = response.status();
    let headers = response.headers().clone();
    let mut result = Response::builder().status(status);
    for (name, value) in headers.iter() {
        if name != header::CONNECTION && name != header::TRANSFER_ENCODING && name != header::CONTENT_LENGTH {
            result = result.header(name, value);
        }
    }
    result.body(Body::from_stream(response.bytes_stream())).unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

fn relative_path(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if path.as_os_str().is_empty() || path.components().any(|component| !matches!(component, Component::Normal(_))) {
        bail!("mapping path must be a nonempty relative path: {}", value);
    }
    Ok(path.to_path_buf())
}

pub fn trusted_repository_root(workspace: &Path) -> Result<PathBuf> {
    let workspace = workspace.canonicalize()?;
    for ancestor in workspace.ancestors() {
        if ancestor.file_name().is_some_and(|name| name == ".mdev") {
            return Ok(ancestor.parent().context("missing repository root")?.to_path_buf());
        }
    }
    bail!("QA workspace is not located under a trusted .mdev workspace")
}

fn excluded_directory(relative: &Path, exclusions: &[String]) -> bool {
    let relative = relative.to_string_lossy().replace('\\', "/");
    let name = relative.rsplit('/').next().unwrap_or("");
    exclusions.iter().any(|pattern| {
        let pattern = pattern.trim().trim_end_matches('/');
        if let Some(name_pattern) = pattern.strip_prefix("**/") {
            !name_pattern.is_empty() && !name_pattern.contains('/') && name == name_pattern
        } else if pattern.contains('/') {
            relative == pattern
        } else {
            !pattern.is_empty() && name == pattern
        }
    })
}

#[cfg(unix)]
fn preserve_symlink(source: &Path, destination: &Path) -> Result<()> {
    std::os::unix::fs::symlink(fs::read_link(source)?, destination)?;
    Ok(())
}

#[cfg(windows)]
fn preserve_symlink(source: &Path, destination: &Path) -> Result<()> {
    let target = fs::read_link(source)?;
    let metadata = fs::metadata(source)?;
    if metadata.is_dir() {
        std::os::windows::fs::symlink_dir(target, destination)?;
    } else {
        std::os::windows::fs::symlink_file(target, destination)?;
    }
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path, relative: &Path, exclusions: &[String], excluded_workspace: &Path, excluded_staging: &Path, symlink_behavior: QaSymlinkBehavior) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        if source_path.starts_with(excluded_workspace) || source_path.starts_with(excluded_staging) {
            continue;
        }
        let destination_path = destination.join(entry.file_name());
        let child_relative = relative.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)?;
        let is_link = is_directory_link(&metadata);
        if (metadata.is_dir() || is_link)
            && excluded_directory(&child_relative, exclusions)
        {
            continue;
        }
        if is_link {
            match symlink_behavior {
                QaSymlinkBehavior::Reject => {
                    bail!("directory copy contains a symlink: {}", source_path.display());
                }
                QaSymlinkBehavior::Preserve => {
                    preserve_symlink(&source_path, &destination_path)?;
                }
                QaSymlinkBehavior::Skip => {}
            }
            continue;
        }
        if metadata.is_dir() {
            copy_directory(&source_path, &destination_path, &child_relative, exclusions, excluded_workspace, excluded_staging, symlink_behavior)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &destination_path)?;
        } else {
            bail!("unsupported filesystem entry: {}", source_path.display());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn create_directory_link(source: &Path, destination: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, destination)?;
    Ok(())
}

#[cfg(windows)]
fn create_directory_link(source: &Path, destination: &Path) -> Result<()> {
    use std::process::Command;

    let output = Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(destination)
        .arg(source)
        .output()
        .with_context(|| format!("failed to create QA junction {} -> {}", destination.display(), source.display()))?;

    if !output.status.success() {
        bail!(
            "failed to create QA junction {} -> {}: {} {}",
            destination.display(),
            source.display(),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(())
}

#[cfg(windows)]
fn is_directory_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_directory_link(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn remove_mapping_destination(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    if is_directory_link(metadata) {
        #[cfg(windows)]
        {
            if metadata.is_dir() || fs::metadata(path).is_ok_and(|target| target.is_dir()) {
                fs::remove_dir(path)?;
            } else {
                fs::remove_file(path)?;
            }
        }
        #[cfg(not(windows))]
        fs::remove_file(path)?;
    } else if metadata.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn apply_directory_mappings(workspace: &Path, mappings: &[QaDirectoryMapping]) -> Result<()> {
    if mappings.is_empty() {
        return Ok(());
    }
    let workspace = workspace.canonicalize()?;
    let trusted_root = trusted_repository_root(&workspace)?;
    for mapping in mappings {
        let source_relative = relative_path(&mapping.source)?;
        let destination_relative = relative_path(&mapping.destination)?;
        let source = trusted_root.join(source_relative).canonicalize().with_context(|| format!("missing source directory {}", mapping.source))?;
        if !source.starts_with(&trusted_root) || !source.is_dir() {
            bail!("mapping source escapes the trusted repository or is not a directory");
        }
        let destination = workspace.join(destination_relative);
        let parent = destination.parent().context("invalid directory destination")?;
        let mut existing_parent = parent;
        while !existing_parent.exists() {
            existing_parent = existing_parent.parent().context("invalid directory destination parent")?;
        }
        if !existing_parent.canonicalize()?.starts_with(&workspace) {
            bail!("mapping destination escapes the QA workspace");
        }
        fs::create_dir_all(parent)?;
        let parent = parent.canonicalize()?;
        if !parent.starts_with(&workspace) {
            bail!("mapping destination escapes the QA workspace");
        }
        let destination = parent.join(destination.file_name().context("invalid directory destination name")?);
        if source == destination
            || source.starts_with(&destination)
            || (destination.starts_with(&source) && matches!(mapping.mode, QaDirectoryMode::Symlink))
        {
            bail!("recursive or overlapping directory mapping: {} -> {}", mapping.source, mapping.destination);
        }
        if source.starts_with(trusted_root.join(".mdev"))
            && matches!(mapping.mode, QaDirectoryMode::Symlink)
            && !["cache", "browser-profile", "repo-sync"]
                .iter()
                .any(|name| source.starts_with(trusted_root.join(".mdev").join(name)))
        {
            bail!("MDEV directory symlinks are restricted to cache, browser-profile, and repo-sync");
        }
        let existing = match fs::symlink_metadata(&destination) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        match mapping.mode {
            QaDirectoryMode::Copy => {
                let default_behavior = if mapping.replace_existing { "replace" } else { "error" };
                let behavior = mapping.existing_destination.as_deref().unwrap_or(default_behavior);
                if !matches!(behavior, "error" | "replace" | "use_existing") {
                    bail!("invalid existing destination behavior: {}", behavior);
                }
                if let Some(metadata) = &existing {
                    match behavior {
                        "error" => bail!("mapping destination already exists: {}", destination.display()),
                        "use_existing" => {
                            if is_directory_link(metadata) || !metadata.is_dir() {
                                bail!("existing destination is not a real directory: {}", destination.display());
                            }
                            continue;
                        }
                        "replace" => {}
                        _ => unreachable!(),
                    }
                }
                let temporary = parent.join(format!(".qa-copy-{}", uuid::Uuid::new_v4()));
                let result = copy_directory(&source, &temporary, Path::new(""), &mapping.exclude_directories, &workspace, &temporary, mapping.symlink_behavior).and_then(|_| {
                    if let Some(metadata) = &existing {
                        if is_directory_link(metadata) || metadata.is_dir() || metadata.is_file() {
                            remove_mapping_destination(&destination, metadata)?;
                        } else {
                            bail!("unsupported existing mapping destination");
                        }
                    }
                    fs::rename(&temporary, &destination)?;
                    Ok(())
                });
                if result.is_err() {
                    let _ = fs::remove_dir_all(&temporary);
                }
                result?;
            }
            QaDirectoryMode::Symlink => {
                if let Some(metadata) = existing {
                    if !is_directory_link(&metadata) {
                        bail!("directory link destination already exists and is not a link: {}", destination.display());
                    }
                    if destination.canonicalize().ok().as_deref() != Some(source.as_path()) {
                        bail!("directory link destination points to a different source: {}", destination.display());
                    }
                } else {
                    create_directory_link(&source, &destination)?;
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllocatedQaService {
    pub id: String,
    pub label: String,
    pub port: u16,
    pub environment_variable: String,
    pub public: bool,
    pub public_url: Option<String>,
    pub execution: Option<ProcessRecord>,
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let qa = resolve_qa_spec(ctx, config)?;
    validate_environment(&qa.environment)?;

    let repo_ref = ctx
        .local_state
        .get("resources")
        .and_then(|value| value.get("repo"))
        .and_then(|value| value.get("repo_ref"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(ctx.repo_ref);
    let workspace = PathBuf::from(repo_ref);
    let session = qa_session_id(ctx);
    let proxy_owner = format!("{}:{}", ctx.run_id, ctx.step.id);
    ctx.state.qa_reverse_proxy.release_owner(&proxy_owner);

    let _ = ctx
        .state
        .process_registry
        .terminate_deployment(
            ctx.run_id.to_string().as_str(),
            ctx.step.id.as_str(),
            true,
        )
        .await;

    apply_directory_mappings(workspace.as_path(), &qa.environment.directory_mappings)?;
    let mut shared_environment = shared_dependency_environment(prior_results);
    let build_environment = super::shared_dependencies::build_cache_environment(workspace.as_path())?;
    shared_environment.extend(build_environment);
    let mut prepare_sequence = qa.environment.prepare.clone();

    for command in &mut prepare_sequence.commands {
        for (key, value) in &shared_environment {
            command
                .environment
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }

    let prepare_owner = TerminalExecutionOwner {
        run_id: ctx.run_id.to_string(),
        step_id: ctx.step.id.clone(),
        capability: "qa_environment.prepare".to_string(),
        service_id: None,
    };

    let prepare = terminal_runtime::run_sequence(
        &ctx.state.process_registry,
        workspace.as_path(),
        &prepare_sequence,
        &prepare_owner,
    )
    .await?;

    if !prepare.ok {
        return Ok(CapabilityResult {
            ok: false,
            capability: "qa_environment".to_string(),
            payload: json!({
                "ok": false,
                "status": "prepare_failed",
                "session_id": session,
                "prepare": prepare,
                "services": []
            }),
            follow_ups: CapabilityInvocationRequest::None,
        });
    }

    let workflow_title = crate::engine::load_run(ctx.state, ctx.run_id).await?.title;
    let proxy_port = crate::environment_file::workflow_api_bind_addr()?.port();
    let mut services = allocate_services(
        &ctx.state.runtime_endpoints,
        &ctx.state.qa_reverse_proxy,
        &qa.environment,
        &proxy_owner,
        &workflow_title,
        proxy_port,
    )?;

    let service_ports = services
        .iter()
        .map(|service| (service.id.clone(), service.port))
        .collect::<BTreeMap<_, _>>();
    let service_urls = services
        .iter()
        .map(|service| {
            (
                service.id.clone(),
                format!("http://127.0.0.1:{}", service.port),
            )
        })
        .collect::<BTreeMap<_, _>>();

    for (index, (allocated, service)) in services
        .iter_mut()
        .zip(qa.environment.services.iter())
        .enumerate()
    {
        let mut command = prepare_service_command(
            &service.command,
            workspace.as_path(),
            allocated.port,
            session.as_str(),
            service.id.as_str(),
            allocated.environment_variable.as_str(),
            allocated.public_url.as_deref(),
            index == 0,
            &service_ports,
            &service_urls,
        );

        for (key, value) in &shared_environment {
            command
                .environment
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }

        let execution = terminal_runtime::start_service(
            &ctx.state.process_registry,
            workspace.as_path(),
            &command,
            TerminalExecutionOwner {
                run_id: ctx.run_id.to_string(),
                step_id: ctx.step.id.clone(),
                capability: "qa_environment".to_string(),
                service_id: Some(service.id.clone()),
            },
        )
        .await?;

        let execution_id = execution.execution_id.clone();
        allocated.execution = Some(execution);

        let readiness = wait_for_service_readiness(
            &ctx.state.process_registry,
            execution_id.as_str(),
            service.id.as_str(),
            allocated.port,
            service.command.command.as_str(),
            &service.readiness,
        )
        .await;

        match readiness {
            Ok(current) => {
                allocated.execution = Some(current);
            }
            Err(error) => {
                let current = ctx
                    .state
                    .process_registry
                    .get(execution_id.as_str())
                    .await;

                if let Some(current) = current {
                    allocated.execution = Some(current);
                }

                let _ = ctx
                    .state
                    .process_registry
                    .terminate_deployment(
                        ctx.run_id.to_string().as_str(),
                        ctx.step.id.as_str(),
                        true,
                    )
                    .await;
                ctx.state.qa_reverse_proxy.release_owner(&proxy_owner);

                return Ok(CapabilityResult {
                    ok: false,
                    capability: "qa_environment".to_string(),
                    payload: json!({
                        "ok": false,
                        "status": "readiness_failed",
                        "session_id": session,
                        "failed_service": service.id,
                        "error": format!("{:#}", error),
                        "public_url": null,
                        "prepare": prepare,
                        "services": services,
                        "environment": qa.environment
                    }),
                    follow_ups: CapabilityInvocationRequest::None,
                });
            }
        }
    }

    let public_url = services
        .iter()
        .find_map(|service| service.public_url.clone());

    Ok(CapabilityResult {
        ok: true,
        capability: "qa_environment".to_string(),
        payload: json!({
            "ok": true,
            "status": "running",
            "session_id": session,
            "public_url": public_url,
            "prepare": prepare,
            "services": services,
            "environment": qa.environment,
            "actions": ["stop", "restart", "fail", "approve"]
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}

async fn wait_for_service_readiness(
    registry: &terminal_runtime::ProcessRegistry,
    execution_id: &str,
    service_id: &str,
    port: u16,
    command: &str,
    readiness: &QaReadinessSpec,
) -> Result<ProcessRecord> {
    let configured_timeout_seconds = match readiness {
        QaReadinessSpec::None => 0,
        QaReadinessSpec::Http {
            timeout_seconds, ..
        }
        | QaReadinessSpec::Tcp { timeout_seconds }
        | QaReadinessSpec::Log {
            timeout_seconds, ..
        } => *timeout_seconds,
    };
    let normalized_command = command.trim().to_ascii_lowercase();
    let timeout_seconds = if normalized_command == "cargo run"
        || normalized_command.starts_with("cargo run ")
    {
        configured_timeout_seconds.max(300)
    } else {
        configured_timeout_seconds
    };
    let deadline = Instant::now() + Duration::from_secs(timeout_seconds);
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;

    loop {
        let current = registry.get(execution_id).await.ok_or_else(|| {
            anyhow!(
                "QA service '{}' disappeared from the process registry during readiness checks",
                service_id
            )
        })?;

        if current.status != "running" && current.status != "starting" {
            bail!(
                "QA service '{}' exited during readiness checks with status '{}'.\ncommand: {}\nworking directory: {}\nstdout:\n{}\nstderr:\n{}",
                service_id,
                current.status,
                current.command,
                current.working_directory,
                current.stdout,
                current.stderr
            );
        }

        let ready = match readiness {
            QaReadinessSpec::None => true,
            QaReadinessSpec::Http {
                path,
                expected_status,
                ..
            } => {
                let normalized_path = if path.starts_with('/') {
                    path.clone()
                } else {
                    format!("/{}", path)
                };
                let url = format!("http://127.0.0.1:{}{}", port, normalized_path);
                match http_client.get(url).send().await {
                    Ok(response) => expected_status
                        .map(|status| response.status().as_u16() == status)
                        .unwrap_or_else(|| response.status().is_success()),
                    Err(_) => false,
                }
            }
            QaReadinessSpec::Tcp { .. } => {
                tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                    .await
                    .is_ok()
            }
            QaReadinessSpec::Log { pattern, .. } => {
                let expression = regex::Regex::new(pattern).map_err(|error| {
                    anyhow!(
                        "invalid readiness log pattern for QA service '{}': {}",
                        service_id,
                        error
                    )
                })?;
                expression.is_match(current.stdout.as_str())
                    || expression.is_match(current.stderr.as_str())
            }
        };

        if ready {
            return Ok(current);
        }

        if Instant::now() >= deadline {
            bail!(
                "QA service '{}' did not satisfy its readiness check within {} seconds.\ncommand: {}\nworking directory: {}\nstdout:\n{}\nstderr:\n{}",
                service_id,
                timeout_seconds,
                current.command,
                current.working_directory,
                current.stdout,
                current.stderr
            );
        }

        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn resolve_qa_spec(ctx: &CapabilityContext<'_>, _config: Value) -> Result<QaStageSpec> {
    let capability = ctx
        .local_state
        .get("capabilities")
        .and_then(|value| value.get("qa_environment"))
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "QA runtime capability state is missing for stage '{}'",
                ctx.step.id
            )
        })?;

    serde_json::from_value(capability)
        .map_err(|error| anyhow!("invalid QA environment configuration: {}", error))
}

fn shared_dependency_environment(
    prior_results: &[CapabilityResult],
) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();

    let Some(result) = find_result(prior_results, "shared_dependencies") else {
        return environment;
    };

    let Some(providers) = result.payload.get("providers").and_then(Value::as_array) else {
        return environment;
    };

    for provider in providers {
        let Some(values) = provider.get("environment").and_then(Value::as_object) else {
            continue;
        };

        for (key, value) in values {
            if let Some(value) = value.as_str() {
                environment.insert(key.clone(), value.to_string());
            }
        }
    }

    environment
}

fn root_scoped_service_command(command: &str, working_directory: &str) -> String {
    let directory = working_directory
        .trim()
        .trim_matches('/')
        .trim_matches('\\')
        .replace('\\', "/");

    if directory.is_empty() || directory == "." {
        return command.to_string();
    }

    let trimmed = command.trim();

    if trimmed == "cargo run" {
        return format!("cargo run --manifest-path {}/Cargo.toml", directory);
    }

    if let Some(remainder) = trimmed.strip_prefix("cargo run ") {
        return format!(
            "cargo run --manifest-path {}/Cargo.toml {}",
            directory,
            remainder
        );
    }

    if trimmed == "npm run" {
        return format!("npm --prefix {} run", directory);
    }

    if let Some(remainder) = trimmed.strip_prefix("npm run ") {
        return format!("npm --prefix {} run {}", directory, remainder);
    }

    if trimmed == "npm" {
        return format!("npm --prefix {}", directory);
    }

    if let Some(remainder) = trimmed.strip_prefix("npm ") {
        return format!("npm --prefix {} {}", directory, remainder);
    }

    command.to_string()
}

fn prepare_service_command(
    configured: &crate::engine::runtime_tools::TerminalCommandSpec,
    workspace: &Path,
    port: u16,
    deployment_id: &str,
    service_id: &str,
    configured_port_variable: &str,
    public_url: Option<&str>,
    primary_service: bool,
    service_ports: &BTreeMap<String, u16>,
    service_urls: &BTreeMap<String, String>,
) -> crate::engine::runtime_tools::TerminalCommandSpec {
    let mut command = configured.clone();
    let port_text = port.to_string();
    let workspace_text = workspace.to_string_lossy().to_string();

    command.command = replace_service_tokens(
        command.command.as_str(),
        port_text.as_str(),
        deployment_id,
        service_id,
        service_ports,
        service_urls,
    );
    command.arguments = command
        .arguments
        .iter()
        .map(|argument| {
            replace_service_tokens(
                argument,
                port_text.as_str(),
                deployment_id,
                service_id,
                service_ports,
                service_urls,
            )
        })
        .collect();
    let configured_working_directory = replace_service_tokens(
        command.working_directory.as_str(),
        port_text.as_str(),
        deployment_id,
        service_id,
        service_ports,
        service_urls,
    );
    command.command = root_scoped_service_command(
        command.command.as_str(),
        configured_working_directory.as_str(),
    );
    command.working_directory = ".".to_string();

    command.environment = command
        .environment
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                replace_service_tokens(
                    value,
                    port_text.as_str(),
                    deployment_id,
                    service_id,
                    service_ports,
                    service_urls,
                ),
            )
        })
        .collect();

    let port_variable = configured_port_variable.trim();
    if !port_variable.is_empty() {
        command
            .environment
            .insert(port_variable.to_string(), port_text.clone());
    }

    command
        .environment
        .entry("PORT".to_string())
        .or_insert_with(|| port_text.clone());
    command
        .environment
        .entry("HOST".to_string())
        .or_insert_with(|| "127.0.0.1".to_string());
    command
        .environment
        .insert("DEPLOYMENT_ID".to_string(), deployment_id.to_string());
    command
        .environment
        .insert("SERVICE_ID".to_string(), service_id.to_string());
    command
        .environment
        .insert("SERVICE_PORT".to_string(), port_text.clone());
    command.environment.insert(
        "SERVICE_PRIMARY".to_string(),
        primary_service.to_string(),
    );

    if let Some(public_url) = public_url {
        command
            .environment
            .insert("SERVICE_PUBLIC_URL".to_string(), public_url.to_string());
        command
            .environment
            .insert("MDEV_QA_PUBLIC_URL".to_string(), public_url.to_string());
    }

    command
        .environment
        .insert("MDEV_QA_SESSION".to_string(), deployment_id.to_string());
    command
        .environment
        .insert("MDEV_QA_SERVICE".to_string(), service_id.to_string());
    command
        .environment
        .insert("MDEV_QA_WORKSPACE".to_string(), workspace_text.clone());
    command
        .environment
        .insert("MDEV_REPO_ROOT".to_string(), workspace_text.clone());
    command
        .environment
        .insert("MDEV_QA_PORT".to_string(), port_text);

    if service_id == "web" {
        if let Some(api_url) = service_urls.get("api") {
            command.environment.insert(
                "VITE_API_BASE_URL".to_string(),
                format!("{}/api", api_url.trim_end_matches('/')),
            );
        }
    }

    command.mode = TerminalCommandMode::Service;
    command
}

fn replace_service_tokens(
    value: &str,
    port: &str,
    deployment_id: &str,
    service_id: &str,
    service_ports: &BTreeMap<String, u16>,
    service_urls: &BTreeMap<String, String>,
) -> String {
    let mut resolved = value
        .replace("{port}", port)
        .replace("{host}", "127.0.0.1")
        .replace("{run}", deployment_id)
        .replace("{deployment}", deployment_id)
        .replace("{service}", service_id);

    for (id, service_port) in service_ports {
        let port_text = service_port.to_string();
        let internal_url = format!("http://127.0.0.1:{}", service_port);

        resolved = resolved
            .replace(format!("{{service.{}.port}}", id).as_str(), port_text.as_str())
            .replace(
                format!("{{service.{}.internal_url}}", id).as_str(),
                internal_url.as_str(),
            )
            .replace(
                format!("{{service.{}.url}}", id).as_str(),
                internal_url.as_str(),
            );
    }

    for (id, service_url) in service_urls {
        resolved = resolved.replace(
            format!("{{service.{}.public_url}}", id).as_str(),
            service_url,
        );
    }

    resolved
}

fn validate_environment(environment: &QaEnvironmentSpec) -> Result<()> {
    if environment.port_range.start == 0 || environment.port_range.end == 0 {
        bail!("QA port range may not contain port zero");
    }

    if environment.port_range.start > environment.port_range.end {
        bail!("QA port range start must not exceed its end");
    }

    if environment.hostname_template.trim().is_empty() {
        bail!("QA hostname template is required");
    }

    for command in &environment.prepare.commands {
        if command.mode != TerminalCommandMode::Run {
            bail!("QA preparation command '{}' must use run mode", command.id);
        }
    }

    let mut service_ids = BTreeSet::new();

    for service in &environment.services {
        if service.id.trim().is_empty() {
            bail!("QA service id is required");
        }

        if !service_ids.insert(service.id.clone()) {
            bail!("duplicate QA service id '{}'", service.id);
        }

        if service.command.mode != TerminalCommandMode::Service {
            bail!("QA service '{}' command must use service mode", service.id);
        }

        if service.command.command.trim().is_empty() {
            bail!("QA service '{}' command is required", service.id);
        }

        if service.port.environment_variable.trim().is_empty() {
            bail!("QA service '{}' port environment variable is required", service.id);
        }
    }

    Ok(())
}

fn allocate_services(
    endpoints: &RuntimeEndpointManager,
    proxy: &QaReverseProxy,
    environment: &QaEnvironmentSpec,
    owner: &str,
    workflow_title: &str,
    proxy_port: u16,
) -> Result<Vec<AllocatedQaService>> {
    let mut allocated = Vec::<u16>::new();
    let mut services = Vec::new();

    for (index, service) in environment.services.iter().enumerate() {
        if let Some(port) = service.port.preferred {
            if port < environment.port_range.start || port > environment.port_range.end {
                bail!(
                    "preferred port {} for service '{}' is outside the QA port range",
                    port,
                    service.id
                );
            }
        }

        let endpoint = endpoints.allocate(
            NetworkExposure::Localhost,
            service.port.preferred,
            Some((environment.port_range.start, environment.port_range.end)),
            allocated.as_slice(),
        )?;
        let port = endpoint.port;
        allocated.push(port);

        let hostname = proxy.reserve(
            owner,
            &environment.hostname_template,
            workflow_title,
            &service.id,
            index == 0,
            port,
        );
        let public_url = Some(format!("http://{}:{}", hostname, proxy_port));

        services.push(AllocatedQaService {
            id: service.id.clone(),
            label: service.label.clone(),
            port,
            environment_variable: service.port.environment_variable.clone(),
            public: true,
            public_url,
            execution: None,
        });
    }

    Ok(services)
}

fn qa_session_id(ctx: &CapabilityContext<'_>) -> String {
    ctx.run_id
        .to_string()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(7)
        .collect::<String>()
        .to_ascii_lowercase()
}
