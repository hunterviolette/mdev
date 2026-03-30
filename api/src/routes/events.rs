use axum::{extract::{Path, State}, routing::get, Json, Router};
use sqlx::Row;
use uuid::Uuid;

use crate::{app_state::AppState, models::WorkflowEvent};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/workflow-runs/:run_id/events", get(list_run_events))
}

async fn list_run_events(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<Vec<WorkflowEvent>>, (axum::http::StatusCode, String)> {
    let rows = sqlx::query(
        "SELECT id, run_id, step_id, level, kind, message, payload_json, created_at FROM workflow_events WHERE run_id = ? ORDER BY created_at ASC"
    )
    .bind(run_id.to_string())
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    let events = rows
        .into_iter()
        .map(row_to_event)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Json(events))
}

fn row_to_event(
    row: sqlx::sqlite::SqliteRow,
) -> Result<WorkflowEvent, (axum::http::StatusCode, String)> {
    Ok(WorkflowEvent {
        id: Uuid::parse_str(row.get::<String, _>("id").as_str()).map_err(internal)?,
        run_id: Uuid::parse_str(row.get::<String, _>("run_id").as_str()).map_err(internal)?,
        step_id: row.get("step_id"),
        level: row.get("level"),
        kind: row.get("kind"),
        message: row.get("message"),
        payload: serde_json::from_str(row.get::<String, _>("payload_json").as_str()).map_err(internal)?,
        created_at: chrono::DateTime::parse_from_rfc3339(row.get::<String, _>("created_at").as_str()).map_err(internal)?.with_timezone(&chrono::Utc),
    })
}

fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
