use axum::{extract::{Path, State}, http::StatusCode, routing::{get, post}, Json, Router};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    supervisor,
    supervisor::models::{CreateSupervisorRunRequest, SupervisorActionRequest, SupervisorRun},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/supervisor-runs", get(list_supervisor_runs).post(create_supervisor_run))
        .route("/api/supervisor-runs/:supervisor_id", axum::routing::delete(delete_supervisor_run))
        .route("/api/supervisor-runs/:supervisor_id/queue", get(get_supervisor_queue))
        .route("/api/supervisor-runs/:supervisor_id/actions", post(supervisor_action))
}

#[derive(Debug, Serialize)]
struct SupervisorListItem {
    id: String,
    title: String,
}

async fn list_supervisor_runs(
    State(state): State<AppState>,
) -> Result<Json<Vec<SupervisorListItem>>, (axum::http::StatusCode, String)> {
    let rows = sqlx::query(
        r#"
        SELECT id, title
        FROM supervisor_runs
        WHERE archived_at IS NULL
        ORDER BY updated_at DESC
        "#,
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(rows.into_iter().map(|row| SupervisorListItem {
        id: row.get("id"),
        title: row.get("title"),
    }).collect()))
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
    let _supervisor_guard = state.supervisor_coordinator.lock(supervisor_id).await;
    supervisor::delete_supervisor_run(&state, supervisor_id).await.map(|_| Json(json!({ "ok": true }))).map_err(internal)
}

async fn get_supervisor_queue(
    State(state): State<AppState>,
    Path(supervisor_id): Path<Uuid>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    supervisor::supervisor_queue_projection(&state, supervisor_id).await.map(Json).map_err(internal)
}

async fn supervisor_action(
    State(state): State<AppState>,
    Path(supervisor_id): Path<Uuid>,
    Json(req): Json<SupervisorActionRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let action = req.action_name();
    tracing::info!(supervisor_id = %supervisor_id, action = %action, "supervisor action requested");

    let _supervisor_guard = state.supervisor_coordinator.lock(supervisor_id).await;
    tracing::info!(supervisor_id = %supervisor_id, action = %action, "supervisor action acquired mutation lock");

    let response = match req {
        SupervisorActionRequest::CreateWorkUnit(request) => supervisor::create_supervisor_work_unit(&state, supervisor_id, request).await,
        SupervisorActionRequest::DeleteWorkUnit { work_unit_id } => supervisor::delete_supervisor_work_unit(&state, supervisor_id, work_unit_id).await,
        SupervisorActionRequest::RegenerateWorkUnit { work_unit_id } => supervisor::regenerate_supervisor_work_unit(&state, supervisor_id, work_unit_id).await,
        SupervisorActionRequest::StartWorkUnit { work_unit_id } => supervisor::start_supervisor_work_unit(&state, supervisor_id, work_unit_id).await,
        SupervisorActionRequest::PauseWorkUnit { work_unit_id } => supervisor::pause_supervisor_work_unit(&state, supervisor_id, work_unit_id).await,
        SupervisorActionRequest::StageWorkUnit { work_unit_id, staged } => supervisor::stage_supervisor_work_unit(&state, supervisor_id, work_unit_id, staged).await,
        SupervisorActionRequest::UpdateSupervisorConfig { config } => supervisor::update_supervisor_config(&state, supervisor_id, config).await,
        SupervisorActionRequest::SelectPlanner { planner_id } => supervisor::select_supervisor_planner(&state, supervisor_id, planner_id).await,
        SupervisorActionRequest::EnqueueFeature { planner_id, feature_id } => supervisor::enqueue_supervisor_feature(&state, supervisor_id, planner_id, feature_id).await,
        SupervisorActionRequest::DequeueFeature { planner_id, feature_id } => supervisor::dequeue_supervisor_feature(&state, supervisor_id, planner_id, feature_id).await,
        SupervisorActionRequest::ReorderFeaturePool { feature_ids } => supervisor::reorder_supervisor_feature_pool(&state, supervisor_id, feature_ids).await,
        SupervisorActionRequest::RefineFeature { feature_id, workflow_template_id } => supervisor::refine_supervisor_feature(
            &state,
            supervisor_id,
            feature_id,
            workflow_template_id,
        ).await,
        SupervisorActionRequest::PauseFeaturePool => supervisor::pause_supervisor_feature_pool(&state, supervisor_id, json!({})).await,
        SupervisorActionRequest::ResumeFeaturePool => supervisor::resume_supervisor_feature_pool(&state, supervisor_id, json!({})).await,
        SupervisorActionRequest::SkipIntegrationInput { work_unit_id } => supervisor::set_supervisor_work_unit_integration_skipped(&state, supervisor_id, work_unit_id, true).await,
        SupervisorActionRequest::UnskipIntegrationInput { work_unit_id } => supervisor::set_supervisor_work_unit_integration_skipped(&state, supervisor_id, work_unit_id, false).await,
        SupervisorActionRequest::ApplyIntegration { work_unit_id, archive_integrated_workflows } => supervisor::apply_supervisor_work_unit(
            &state,
            supervisor_id,
            work_unit_id,
            archive_integrated_workflows,
        ).await,
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
