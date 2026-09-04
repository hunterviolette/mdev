use axum::{extract::{Path, State}, routing::{get, post}, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::capabilities::{
        changeset::schema::CHANGESET_SCHEMA_EXAMPLE,
        context_export,
        registry::{stage_capability_policy, CapabilityContext, CapabilityInvocation, execute_capability_invocations},
    },
};

use super::workflow_scope::resolve_workflow_scope;

#[derive(Debug, Serialize)]
struct WorkflowCapabilityItem {
    capability: String,
    entrypoint: bool,
}

#[derive(Debug, Serialize)]
struct WorkflowCapabilityListResponse {
    ok: bool,
    run_id: String,
    step_id: String,
    repo_ref: String,
    capabilities: Vec<WorkflowCapabilityItem>,
}

#[derive(Debug, Deserialize)]
struct ExecuteWorkflowCapabilityRequest {
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    config: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct WorkflowContextExportSummaryRequest {
    #[serde(default)]
    config: Option<Value>,
}

#[derive(Debug, Serialize)]
struct WorkflowContextExportSummaryResponse {
    ok: bool,
    run_id: String,
    included_file_count: usize,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/capabilities/changeset-schema", get(get_changeset_schema))
        .route("/api/workflow-runs/:run_id/capabilities", get(list_workflow_capabilities))
        .route("/api/workflow-runs/:run_id/context-export/summary", post(get_workflow_context_export_summary))
        .route("/api/workflow-runs/:run_id/capabilities/:capability_id/execute", post(execute_workflow_capability))
}

async fn get_changeset_schema() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "schema": CHANGESET_SCHEMA_EXAMPLE,
    }))
}

async fn list_workflow_capabilities(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<WorkflowCapabilityListResponse>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    let policy = stage_capability_policy(&scope.step).map_err(internal)?;
    let capabilities = policy
        .allowed_invocations
        .iter()
        .map(|capability| WorkflowCapabilityItem {
            capability: capability.clone(),
            entrypoint: capability == &policy.entrypoint,
        })
        .collect::<Vec<_>>();

    Ok(Json(WorkflowCapabilityListResponse {
        ok: true,
        run_id: scope.run_id.to_string(),
        step_id: scope.step.id,
        repo_ref: scope.repo_ref,
        capabilities,
    }))
}

async fn get_workflow_context_export_summary(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Json(req): Json<WorkflowContextExportSummaryRequest>,
) -> Result<Json<WorkflowContextExportSummaryResponse>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    let repo_resource = scope
        .local_state
        .get("resources")
        .and_then(|value| value.get("repo"))
        .cloned();
    let persisted_config = scope
        .local_state
        .get("capabilities")
        .and_then(|value| value.get("context_export"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut config = req.config.unwrap_or(persisted_config);

    if let Some(object) = config.as_object_mut() {
        object.remove("repo_ref");
    }

    let mut payload = context_export::normalize_context_export_payload(
        config,
        repo_resource,
        scope.repo_ref.as_str(),
    );

    if let Some(object) = payload.as_object_mut() {
        object.insert("repo_ref".to_string(), Value::String(scope.repo_ref.clone()));
    }

    let included_file_count = tokio::task::spawn_blocking(move || {
        context_export::resolve_context_export_file_count(payload)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;

    Ok(Json(WorkflowContextExportSummaryResponse {
        ok: true,
        run_id: scope.run_id.to_string(),
        included_file_count,
    }))
}

async fn execute_workflow_capability(
    State(state): State<AppState>,
    Path((run_id, capability_id)): Path<(Uuid, String)>,
    Json(req): Json<ExecuteWorkflowCapabilityRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    let config = req.config.or(req.input).unwrap_or_else(|| json!({}));
    let cancellation = state
        .workflow_coordinator
        .execution_token(scope.run_id)
        .await;

    if cancellation.is_cancelled() {
        return Err((
            axum::http::StatusCode::CONFLICT,
            "workflow execution was cancelled".to_string(),
        ));
    }

    let ctx = CapabilityContext {
        state: &state,
        run_id: scope.run_id,
        repo_ref: scope.repo_ref.as_str(),
        step: &scope.step,
        local_state: &scope.local_state,
        cancellation,
    };
    let results = execute_capability_invocations(ctx, vec![CapabilityInvocation { capability: capability_id, config }])
        .await
        .map_err(internal)?;

    Ok(Json(json!({
        "ok": results.iter().all(|item| item.ok),
        "run_id": scope.run_id,
        "step_id": scope.step.id,
        "repo_ref": scope.repo_ref,
        "results": results.into_iter().map(|item| json!({
            "ok": item.ok,
            "capability": item.capability,
            "payload": item.payload
        })).collect::<Vec<_>>()
    })))
}

fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
