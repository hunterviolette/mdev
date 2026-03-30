use axum::{routing::get, Json, Router};
use serde_json::json;

pub fn router() -> Router<crate::app_state::AppState> {
    Router::new().route("/health", get(health))
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}
