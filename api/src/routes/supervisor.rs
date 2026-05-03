use axum::{extract::{Path, State}, routing::{get, post}, Json, Router};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    supervisor,
    supervisor::models::{CreateSupervisorRunRequest, SupervisorActionRequest, SupervisorRun},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/supervisor-runs", get(list_supervisor_runs).post(create_supervisor_run))
        .route("/api/supervisor-runs/:supervisor_id", get(get_supervisor_run).delete(delete_supervisor_run))
        .route("/api/supervisor-runs/:supervisor_id/actions", post(supervisor_action))
}

async fn list_supervisor_runs(State(state): State<AppState>) -> Result<Json<Vec<SupervisorRun>>, (axum::http::StatusCode, String)> {
    supervisor::list_supervisor_runs(&state).await.map(Json).map_err(internal)
}

async fn create_supervisor_run(
    State(state): State<AppState>,
    Json(req): Json<CreateSupervisorRunRequest>,
) -> Result<Json<SupervisorRun>, (axum::http::StatusCode, String)> {
    supervisor::create_supervisor_run(&state, req).await.map(Json).map_err(internal)
}

async fn get_supervisor_run(
    State(state): State<AppState>,
    Path(supervisor_id): Path<Uuid>,
) -> Result<Json<SupervisorRun>, (axum::http::StatusCode, String)> {
    supervisor::load_supervisor_run(&state, supervisor_id).await.map(Json).map_err(internal)
}

async fn delete_supervisor_run(
    State(state): State<AppState>,
    Path(supervisor_id): Path<Uuid>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    supervisor::delete_supervisor_run(&state, supervisor_id).await.map(|_| Json(json!({ "ok": true }))).map_err(internal)
}

async fn supervisor_action(
    State(state): State<AppState>,
    Path(supervisor_id): Path<Uuid>,
    Json(req): Json<SupervisorActionRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let response = match req.action.as_str() {
        "start" => supervisor::start_supervisor_run(&state, supervisor_id).await,
        "tick" => supervisor::tick_supervisor_run(&state, supervisor_id).await,
        "apply" => supervisor::apply_supervisor_final_patch(&state, supervisor_id).await,
        "cancel" => supervisor::cancel_supervisor_run(&state, supervisor_id).await,
        "update_plan" => supervisor::update_supervisor_plan(&state, supervisor_id, req.payload).await,
        "refine_feature" => supervisor::refine_supervisor_feature(&state, supervisor_id, req.payload).await,
        other => Err(anyhow::anyhow!("unsupported supervisor action {}", other)),
    };

    response.map(Json).map_err(internal)
}

fn internal(err: impl std::fmt::Display) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
