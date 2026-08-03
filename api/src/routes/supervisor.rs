use axum::{extract::{Path, State}, http::StatusCode, routing::{get, post}, Json, Router};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    supervisor,
    supervisor::models::{CreateSupervisorRunRequest, SupervisorActionRequest, SupervisorRun},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/supervisor-runs", post(create_supervisor_run))
        .route("/api/supervisor-runs/:supervisor_id", axum::routing::delete(delete_supervisor_run))
        .route("/api/supervisor-runs/:supervisor_id/queue", get(get_supervisor_queue).post(set_supervisor_queue))
        .route("/api/supervisor-runs/:supervisor_id/actions", post(supervisor_action))
}

async fn create_supervisor_run(
    State(state): State<AppState>,
    Json(req): Json<CreateSupervisorRunRequest>,
) -> Result<Json<SupervisorRun>, (axum::http::StatusCode, String)> {
    supervisor::create_supervisor_run(&state, req).await.map(Json).map_err(internal)
}

async fn delete_supervisor_run(
    State(state): State<AppState>,
    Path(supervisor_id): Path<Uuid>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    supervisor::delete_supervisor_run(&state, supervisor_id).await.map(|_| Json(json!({ "ok": true }))).map_err(internal)
}

async fn get_supervisor_queue(
    State(state): State<AppState>,
    Path(supervisor_id): Path<Uuid>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    supervisor::supervisor_queue_projection(&state, supervisor_id).await.map(Json).map_err(internal)
}

async fn set_supervisor_queue(
    State(state): State<AppState>,
    Path(supervisor_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    supervisor::select_supervisor_feature_pool(&state, supervisor_id, payload).await.map(Json).map_err(internal)
}

async fn supervisor_action(
    State(state): State<AppState>,
    Path(supervisor_id): Path<Uuid>,
    Json(req): Json<SupervisorActionRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let action = req.action_name();
    tracing::info!(supervisor_id = %supervisor_id, action = %action, "supervisor action requested");

    let response = match req {
        SupervisorActionRequest::CreateWorkUnit(request) => supervisor::create_supervisor_work_unit(&state, supervisor_id, request).await,
        SupervisorActionRequest::DeleteWorkUnit { work_unit_id } => supervisor::delete_supervisor_work_unit(&state, supervisor_id, work_unit_id).await,
        SupervisorActionRequest::RegenerateWorkUnit { work_unit_id } => supervisor::regenerate_supervisor_work_unit(&state, supervisor_id, work_unit_id).await,
        SupervisorActionRequest::StartWorkUnit { work_unit_id } => {
            let background_state = state.clone();
            let background_work_unit_id = work_unit_id.clone();

            tokio::spawn(async move {
                let _ = supervisor::start_supervisor_work_unit(
                    &background_state,
                    supervisor_id,
                    background_work_unit_id,
                )
                .await;
            });

            Ok(json!({
                "ok": true,
                "accepted": true,
                "background": true,
                "action": "start_work_unit",
                "supervisor_id": supervisor_id,
                "work_unit_id": work_unit_id
            }))
        },
        SupervisorActionRequest::PauseWorkUnit { work_unit_id } => supervisor::pause_supervisor_work_unit(&state, supervisor_id, work_unit_id).await,
        SupervisorActionRequest::StageWorkUnit { work_unit_id, staged } => supervisor::stage_supervisor_work_unit(&state, supervisor_id, work_unit_id, staged).await,
        SupervisorActionRequest::UpdateFlightDeckSettings { flight_deck_settings } => supervisor::update_supervisor_flight_deck_settings(&state, supervisor_id, json!({ "flight_deck_settings": flight_deck_settings })).await,
        SupervisorActionRequest::PauseFeaturePool => supervisor::pause_supervisor_feature_pool(&state, supervisor_id, json!({})).await,
        SupervisorActionRequest::ResumeFeaturePool => supervisor::resume_supervisor_feature_pool(&state, supervisor_id, json!({})).await,
        SupervisorActionRequest::SkipIntegrationInput { work_unit_id } => supervisor::set_supervisor_work_unit_integration_skipped(&state, supervisor_id, work_unit_id, true).await,
        SupervisorActionRequest::UnskipIntegrationInput { work_unit_id } => supervisor::set_supervisor_work_unit_integration_skipped(&state, supervisor_id, work_unit_id, false).await,
        SupervisorActionRequest::ApplyIntegration => supervisor::apply_supervisor_final_patch(&state, supervisor_id).await,
        SupervisorActionRequest::Cancel => supervisor::cancel_supervisor_run(&state, supervisor_id).await,
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

fn internal(err: impl std::fmt::Display) -> (StatusCode, String) {
    let message = err.to_string();
    if message.contains("no rows returned by a query that expected to return at least one row") {
        tracing::warn!(error = %message, "supervisor route referenced a missing or stale row");
        return (StatusCode::NOT_FOUND, "not found".to_string());
    }
    tracing::error!(error = %message, "supervisor route error");
    (StatusCode::INTERNAL_SERVER_ERROR, message)
}
