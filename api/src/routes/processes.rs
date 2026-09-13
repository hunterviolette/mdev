use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app_state::AppState;

#[derive(Debug, Deserialize)]
struct TerminateRequest {
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Deserialize)]
struct DeploymentRequest {
    run_id: String,
    step_id: String,
    #[serde(default)]
    force: bool,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/processes", get(list_processes))
        .route("/api/processes/completed", delete(clear_completed))
        .route("/api/processes/:execution_id", get(get_process))
        .route("/api/processes/:execution_id/terminate", post(terminate_process))
        .route("/api/processes/deployment/terminate", post(terminate_deployment))
        .route("/api/processes/run/:run_id/terminate", post(terminate_run))
}

async fn list_processes(State(state): State<AppState>) -> Json<Value> {
    let processes = state.process_registry.list().await;
    Json(json!({
        "ok": true,
        "processes": processes
    }))
}

async fn get_process(
    State(state): State<AppState>,
    Path(execution_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let process = state
        .process_registry
        .get(execution_id.as_str())
        .await
        .ok_or_else(|| (StatusCode::NOT_FOUND, "process not found".to_string()))?;
    Ok(Json(json!({ "ok": true, "process": process })))
}

async fn terminate_process(
    State(state): State<AppState>,
    Path(execution_id): Path<String>,
    Json(request): Json<TerminateRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let process = state
        .process_registry
        .terminate(execution_id.as_str(), request.force)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "ok": true, "process": process })))
}

async fn terminate_deployment(
    State(state): State<AppState>,
    Json(request): Json<DeploymentRequest>,
) -> Json<Value> {
    let results = state
        .process_registry
        .terminate_deployment(
            request.run_id.as_str(),
            request.step_id.as_str(),
            request.force,
        )
        .await;
    let terminated = results
        .into_iter()
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    Json(json!({ "ok": true, "processes": terminated }))
}

async fn terminate_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(request): Json<TerminateRequest>,
) -> Json<Value> {
    let results = state
        .process_registry
        .terminate_run(run_id.as_str(), request.force)
        .await;
    let terminated = results
        .into_iter()
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    Json(json!({ "ok": true, "processes": terminated }))
}

async fn clear_completed(State(state): State<AppState>) -> Json<Value> {
    let removed = state.process_registry.remove_completed().await;
    Json(json!({ "ok": true, "removed": removed }))
}

fn internal(error: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}
