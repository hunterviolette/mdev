mod app_state;
mod browser_registry;
mod auth;
mod db;
mod environment_file;
mod engine;
mod models;
mod supervisor;

mod routes;

use std::{env, fs, io::ErrorKind, path::{Path, PathBuf}};

use anyhow::Context;
use sha2::{Digest, Sha256};
use axum::Router;
use tower_http::{
    cors::CorsLayer,
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::app_state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cwd = env::current_dir().context("failed to determine current directory")?;
    let app_root = resolve_app_root(&cwd)?;
    load_runtime_env(&app_root)?;

    if env::var("MDEV_BROWSER_USER_DATA_DIR")
        .ok()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
    {
        env::set_var(
            "MDEV_BROWSER_USER_DATA_DIR",
            default_browser_profile_dir(&app_root),
        );
    }

    let browser_profile_dir = PathBuf::from(env::var("MDEV_BROWSER_USER_DATA_DIR")?);
    fs::create_dir_all(&browser_profile_dir).with_context(|| {
        format!(
            "failed to create browser profile directory {}",
            browser_profile_dir.display()
        )
    })?;

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            "workflow_api=info,workflow_api::engine::capabilities::inference::api::oai=info,workflow_api::engine::capabilities::inference::browser::adapter=info,workflow_api::routes::runs=info,tower_http=info".into()
        }))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let layout = runtime_layout(app_root)?;

    fs::create_dir_all(&layout.data_dir)
        .with_context(|| format!("failed to create data directory {}", layout.data_dir.display()))?;

    let db_path = layout.data_dir.join("workflow.db");
    let db_url = format!("sqlite://{}", db_path.to_string_lossy().replace('\\', "/"));

    let db = db::connect(&db_url).await?;
    db::migrate(&db).await?;

    let auth = auth::initialize(&db).await?;
    let state = AppState::new(db);
    state
        .runtime_endpoints
        .refresh_local_lan_ipv4_in_background();

    match crate::engine::capabilities::inference::session::archive_non_persistent_sessions_on_startup(&state.db).await {
        Ok(archived_sessions) if archived_sessions > 0 => {
            tracing::info!(archived_sessions, "archived non-persistent inference sessions from the previous API process");
        }
        Ok(_) => {}
        Err(error) => {
            tracing::error!(
                error = %format!("{:#}", error),
                "failed to archive non-persistent inference sessions on startup"
            );
        }
    }

    match crate::engine::repair_process_interruption_errors_on_startup(&state).await {
        Ok(repaired_runs) if repaired_runs > 0 => {
            tracing::info!(
                repaired_runs,
                "repaired legacy workflow errors caused by API process interruption"
            );
        }
        Ok(_) => {}
        Err(error) => {
            tracing::error!(
                error = %format!("{:#}", error),
                "failed to repair legacy workflow process-interruption errors"
            );
        }
    }

    let addr = crate::environment_file::workflow_api_bind_addr()?;

    let shutdown_state = state.clone();
    let app = build_router(state, auth, &layout.web_dist);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) if err.kind() == ErrorKind::AddrInUse => {
            tracing::info!(%addr, "mdev api already running");
            return Ok(());
        }
        Err(err) => return Err(err).with_context(|| format!("failed to bind workflow api to {addr}")),
    };

    tracing::info!(%addr, web_dist = %layout.web_dist.display(), data_dir = %layout.data_dir.display(), "workflow api listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            await_shutdown_signal().await;

            match shutdown_state
                .workflow_coordinator
                .stop_active_executions(&shutdown_state)
                .await
            {
                Ok(short_circuited_run_ids) if !short_circuited_run_ids.is_empty() => {
                    tracing::warn!(
                        short_circuited_workflows = short_circuited_run_ids.len(),
                        "short-circuited active workflow executions during API shutdown"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(
                        error = %format!("{:#}", error),
                        "failed to short-circuit active workflow executions during API shutdown"
                    );
                }
            }

            let terminated = shutdown_state
                .process_registry
                .terminate_all(true)
                .await;

            for result in terminated {
                if let Err(error) = result {
                    tracing::warn!(
                        error = %format!("{:#}", error),
                        "failed to terminate a managed process during API shutdown"
                    );
                }
            }

            crate::engine::capabilities::inference::browser::adapter::shutdown_browser_bridge();
        })
        .await?;
    Ok(())
}

async fn await_shutdown_signal() {
    #[cfg(windows)]
    if env::var("MDEV_CONTROLLER_MANAGED").as_deref() == Ok("1") {
        use tokio::io::AsyncBufReadExt;

        let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let mut request = String::new();
        if let Ok(length) = stdin.read_line(&mut request).await {
            if length > 0 && request.trim() == "shutdown" {
                return;
            }
        }
    }

    let _ = tokio::signal::ctrl_c().await;
}

fn build_router(state: AppState, auth_state: auth::AuthState, web_dist: &Path) -> Router {
    let auth_enabled = auth_state.enabled();
    let auth_middleware_state = auth_state.clone();
    let mut api = Router::new()
        .merge(routes::router())
        .merge(auth::router())
        .route_layer(axum::middleware::from_fn_with_state(auth_middleware_state, auth::authorize))
        .with_state(state.clone())
        .layer(axum::Extension(auth_state))
        .layer(TraceLayer::new_for_http());

    if !auth_enabled {
        api = api.layer(CorsLayer::permissive());
    }

    let proxy_state = state.clone();
    let index_html = web_dist.join("index.html");
    if index_html.is_file() {
        tracing::info!(path = %web_dist.display(), "serving web frontend");
        api.fallback_service(ServeDir::new(web_dist).fallback(ServeFile::new(index_html)))
            .layer(axum::middleware::from_fn_with_state(proxy_state, engine::capabilities::qa_environment::proxy_middleware))
    } else {
        tracing::error!(path = %index_html.display(), "web frontend entrypoint not found; serving backend routes only");
        api.layer(axum::middleware::from_fn_with_state(proxy_state, engine::capabilities::qa_environment::proxy_middleware))
    }
}

struct RuntimeLayout {
    app_root: PathBuf,
    data_dir: PathBuf,
    web_dist: PathBuf,
}

pub(crate) fn resolve_app_root(cwd: &Path) -> anyhow::Result<PathBuf> {
    if let Some(repo_root) = detect_repo_root(cwd) {
        return Ok(repo_root);
    }

    let exe = env::current_exe().context("failed to determine executable path")?;
    let executable_dir = exe.parent().context("executable has no parent directory")?;
    if executable_dir.file_name().and_then(|name| name.to_str()) == Some("runtime") {
        return executable_dir.parent()
            .map(Path::to_path_buf)
            .context("runtime directory has no parent");
    }
    Ok(executable_dir.to_path_buf())
}

fn load_runtime_env(app_root: &Path) -> anyhow::Result<()> {
    environment_file::load(app_root)
}

fn resolve_runtime_path(app_root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        app_root.join(path)
    }
}

fn runtime_layout(app_root: PathBuf) -> anyhow::Result<RuntimeLayout> {
    if detect_repo_root(&app_root).is_some() {
        let data_dir = env::var("MDEV_DATA_DIR")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(|value| resolve_runtime_path(&app_root, &value))
            .unwrap_or_else(|| app_root.join(".data"));

        return Ok(RuntimeLayout {
            web_dist: app_root.join("web").join("dist"),
            data_dir,
            app_root,
        });
    }

    Ok(RuntimeLayout {
        web_dist: app_root.join("web").join("dist"),
        data_dir: mdev_data_dir(&app_root),
        app_root,
    })
}

fn default_browser_profile_dir(app_root: &Path) -> PathBuf {
    if detect_repo_root(app_root).is_some() {
        return app_root.join(".mdev").join("browser-profile");
    }

    let installation_path = fs::canonicalize(app_root)
        .unwrap_or_else(|_| app_root.to_path_buf());

    let installation_identity = installation_path
        .to_string_lossy()
        .replace('\\', "/");

    #[cfg(target_os = "windows")]
    let installation_identity = installation_identity.to_lowercase();

    let installation_hash = hex::encode(
        Sha256::digest(installation_identity.as_bytes())
    );

    mdev_data_dir(app_root)
        .join("installations")
        .join(installation_hash)
        .join("browser-profile")
}

pub(crate) fn mdev_data_dir(app_root: &Path) -> PathBuf {
    if let Ok(value) = env::var("MDEV_DATA_DIR") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return resolve_runtime_path(app_root, trimmed);
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(value) = env::var("LOCALAPPDATA") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join("mdev");
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(value) = env::var("HOME") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join("Library").join("Application Support").join("mdev");
            }
        }
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        if let Ok(value) = env::var("XDG_DATA_HOME") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join("mdev");
            }
        }
        if let Ok(value) = env::var("HOME") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join(".local").join("share").join("mdev");
            }
        }
    }

    app_root.join(".data")
}

fn detect_repo_root(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let has_web = dir.join("web").exists();
        let has_api = dir.join("api").exists();
        if has_web && has_api {
            return Some(dir.to_path_buf());
        }
    }
    None
}
