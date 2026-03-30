mod app_state;
mod db;
mod engine;
mod executor;
mod inference;
mod migration;
mod models;
mod routes;

use std::{env, fs, net::SocketAddr, path::{Path, PathBuf}};

use anyhow::Context;
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
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "workflow_api=debug,tower_http=info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cwd = env::current_dir().context("failed to determine current directory")?;
    let repo_root = detect_repo_root(&cwd).context("failed to locate repo root containing web/ and api/")?;

    let data_dir = repo_root.join(".data");
    fs::create_dir_all(&data_dir).context("failed to create .data directory")?;

    let db_path = data_dir.join("workflow.db");
    let db_url = format!("sqlite://{}", db_path.to_string_lossy().replace('\\', "/"));

    let db = db::connect(&db_url).await?;
    db::migrate(&db).await?;

    let state = AppState::new(db);

    let web_dist = repo_root.join("web").join("dist");
    let app = build_router(state, &web_dist);

    let addr: SocketAddr = "127.0.0.1:8787"
        .parse()
        .context("invalid bind address")?;

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, web_dist = %web_dist.display(), "workflow api listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
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
        api.fallback_service(
            ServeDir::new(web_dist).fallback(ServeFile::new(index_html))
        )
    } else {
        tracing::warn!(path = %web_dist.display(), "web/dist not found; serving backend routes only");
        api
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
