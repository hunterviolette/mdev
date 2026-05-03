mod capabilities;
mod changesets;
mod event_chains;
mod filesystem;
mod health;
mod repo_tree;
mod runs;
mod sap;
mod review;
mod settings;
mod supervisor;
mod templates;
mod workflow_builder;
mod workflow_scope;

use axum::Router;

pub fn router() -> Router<crate::app_state::AppState> {
    Router::new()
        .merge(health::router())
        .merge(settings::router())
        .merge(supervisor::router())
        .merge(repo_tree::router())
        .merge(templates::router())
        .merge(review::router())
        .merge(workflow_builder::router())
        .merge(runs::router())
        .merge(sap::router())
        .merge(filesystem::router())
        .merge(event_chains::router())
        .merge(capabilities::router())
        .merge(changesets::router())
}
