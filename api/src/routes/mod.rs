mod capabilities;
mod event_chains;
mod health;
mod repo_tree;
mod runs;
mod templates;
mod workflow_builder;

use axum::Router;

pub fn router() -> Router<crate::app_state::AppState> {
    Router::new()
        .merge(health::router())
        .merge(repo_tree::router())
        .merge(templates::router())
        .merge(workflow_builder::router())
        .merge(runs::router())
        .merge(event_chains::router())
        .merge(capabilities::router())
}
