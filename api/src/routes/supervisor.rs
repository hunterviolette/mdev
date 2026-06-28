use axum::{extract::{Path, State}, routing::{get, post}, Json, Router};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    supervisor,
    supervisor::models::{CreateSupervisorRunRequest, EnsureSupervisorPlannerRequest, EnsureSupervisorPlannerResponse, SupervisorActionRequest, SupervisorRun},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/supervisor-runs", get(list_supervisor_runs).post(create_supervisor_run))
        .route("/api/supervisor-runs/ensure-planner", post(ensure_supervisor_planner_run))
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

async fn ensure_supervisor_planner_run(
    State(state): State<AppState>,
    Json(req): Json<EnsureSupervisorPlannerRequest>,
) -> Result<Json<EnsureSupervisorPlannerResponse>, (axum::http::StatusCode, String)> {
    supervisor::ensure_supervisor_planner_run(&state, req).await.map(Json).map_err(internal)
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
    let action = req.action.clone();
    tracing::info!(supervisor_id = %supervisor_id, action = %action, "supervisor action requested");

    let response = match action.as_str() {
        "start" => supervisor::start_supervisor_run(&state, supervisor_id).await,
        "tick" => supervisor::tick_supervisor_run(&state, supervisor_id).await,
        "start_integration" => supervisor::start_supervisor_integration_workflow(&state, supervisor_id).await,
        "apply" => supervisor::apply_supervisor_final_patch(&state, supervisor_id).await,
        "cancel" => supervisor::cancel_supervisor_run(&state, supervisor_id).await,
        "reopen_development" => supervisor::reopen_supervisor_development(&state, supervisor_id).await,
        "restart_integration" => supervisor::restart_supervisor_integration_workflow(&state, supervisor_id).await,
        "restart_sprint" => supervisor::restart_current_supervisor_sprint(&state, supervisor_id).await,
        "update_plan" => supervisor::update_supervisor_plan(&state, supervisor_id, req.payload).await,
        "preview_planner_import" => supervisor::preview_supervisor_planner_import(&state, supervisor_id, req.payload).await,
        "apply_planner_import" => supervisor::apply_supervisor_planner_import(&state, supervisor_id, req.payload).await,
        "refine_feature" => supervisor::refine_supervisor_feature(&state, supervisor_id, req.payload).await,
        "remove_child_workflow" => supervisor::remove_supervisor_child_workflow(&state, supervisor_id, req.payload).await,
        "new_sprint" => supervisor::start_next_supervisor_sprint(&state, supervisor_id).await,
        other => Err(anyhow::anyhow!("unsupported supervisor action {}", other)),
    };

    match response {
        Ok(value) => {
            tracing::info!(supervisor_id = %supervisor_id, action = %action, "supervisor action completed");
            Ok(Json(value))
        }
        Err(err) => {
            tracing::error!(supervisor_id = %supervisor_id, action = %action, error = %err, "supervisor action failed");
            Err(internal(err))
        }
    }
}

fn internal(err: impl std::fmt::Display) -> (axum::http::StatusCode, String) {
    tracing::error!(error = %err, "supervisor route error");
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
