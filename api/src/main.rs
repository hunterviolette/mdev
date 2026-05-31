mod app_state;
mod db;
mod engine;
mod models;
mod supervisor;
mod runtime_env;
mod routes;

use std::{env, fs, io::ErrorKind, path::{Path, PathBuf}, process::Command};

use anyhow::Context;
use dotenvy::dotenv;
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
    let _ = dotenv();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            "workflow_api=info,workflow_api::engine::capabilities::inference::api::oai=info,workflow_api::engine::capabilities::inference::browser::adapter=info,workflow_api::routes::runs=info,tower_http=info".into()
        }))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let layout = runtime_layout()?;
    load_runtime_env(&layout.app_root);

    fs::create_dir_all(&layout.data_dir)
        .with_context(|| format!("failed to create data directory {}", layout.data_dir.display()))?;

    let db_path = layout.data_dir.join("workflow.db");
    let db_url = format!("sqlite://{}", db_path.to_string_lossy().replace('\\', "/"));

    let db = db::connect(&db_url).await?;
    db::migrate(&db).await?;

    let state = AppState::new(db);

    let app = build_router(state, &layout.web_dist);

    let addr = crate::runtime_env::workflow_api_bind_addr()?;

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) if err.kind() == ErrorKind::AddrInUse => {
            tracing::info!(%addr, "mdev api already running; opening existing web ui");
            open_mdev_web_ui(addr);
            return Ok(());
        }
        Err(err) => return Err(err).with_context(|| format!("failed to bind workflow api to {addr}")),
    };

    tracing::info!(%addr, web_dist = %layout.web_dist.display(), data_dir = %layout.data_dir.display(), "workflow api listening");
    open_mdev_web_ui(addr);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            crate::engine::capabilities::inference::browser::adapter::shutdown_browser_bridge();
        })
        .await?;
    Ok(())
}

fn build_router(state: AppState, web_dist: &Path) -> Router {
    let api = Router::new()
        .merge(routes::router())
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    if web_dist.exists() {
        let index_html = web_dist.join("index.html");
        api.fallback_service(ServeDir::new(web_dist).fallback(ServeFile::new(index_html)))
    } else {
        tracing::warn!(path = %web_dist.display(), "web/dist not found; serving backend routes only");
        api
    }
}

struct RuntimeLayout {
    app_root: PathBuf,
    data_dir: PathBuf,
    web_dist: PathBuf,
}

fn load_runtime_env(app_root: &Path) {
    let env_path = app_root.join(".env");
    if env_path.exists() {
        let _ = dotenvy::from_path(env_path);
        return;
    }

    let example_path = app_root.join(".env.example");
    if example_path.exists() {
        let _ = dotenvy::from_path(example_path);
    }
}

fn runtime_layout() -> anyhow::Result<RuntimeLayout> {
    let cwd = env::current_dir().context("failed to determine current directory")?;
    if let Some(repo_root) = detect_repo_root(&cwd) {
        return Ok(RuntimeLayout {
            app_root: repo_root.clone(),
            data_dir: repo_root.join(".data"),
            web_dist: repo_root.join("web").join("dist"),
        });
    }

    let exe = env::current_exe().context("failed to determine executable path")?;
    let app_root = exe
        .parent()
        .context("executable has no parent directory")?
        .to_path_buf();

    Ok(RuntimeLayout {
        web_dist: app_root.join("web").join("dist"),
        data_dir: mdev_data_dir(&app_root),
        app_root,
    })
}

fn mdev_data_dir(app_root: &Path) -> PathBuf {
    if let Ok(value) = env::var("MDEV_DATA_DIR") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
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

fn open_mdev_web_ui(addr: std::net::SocketAddr) {
    if env::var("MDEV_NO_BROWSER_OPEN").ok().as_deref() == Some("1") {
        return;
    }

    let url = format!("http://{}", addr);
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", &url]);
        command
    };

    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut command = Command::new("open");
        command.arg(&url);
        command
    };

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let mut cmd = {
        let mut command = Command::new("xdg-open");
        command.arg(&url);
        command
    };

    if let Err(err) = cmd.spawn() {
        tracing::warn!(url = %url, error = %format!("{:#}", err), "failed to open mdev web ui");
    }
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
