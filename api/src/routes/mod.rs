mod capabilities;
mod events;
mod health;
mod repo_tree;
mod runs;
mod templates;

use axum::Router;

pub fn router() -> Router<crate::app_state::AppState> {
    Router::new()
        .merge(capabilities::router())
        .merge(health::router())
        .merge(repo_tree::router())
        .merge(templates::router())
        .merge(runs::router())
        .merge(events::router())
}
