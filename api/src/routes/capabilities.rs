use axum::{routing::get, Json, Router};
use serde_json::json;

use crate::{app_state::AppState, engine::capabilities::changeset_schema::CHANGESET_SCHEMA_EXAMPLE};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/capabilities/changeset-schema", get(get_changeset_schema))
}

async fn get_changeset_schema() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "schema": CHANGESET_SCHEMA_EXAMPLE,
    }))
}
