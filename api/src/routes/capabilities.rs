use axum::{extract::{Path, State}, routing::{get, post}, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::capabilities::{
        changeset::schema::CHANGESET_SCHEMA_EXAMPLE,
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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/capabilities/changeset-schema", get(get_changeset_schema))
        .route("/api/workflow-runs/:run_id/capabilities", get(list_workflow_capabilities))
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

async fn execute_workflow_capability(
    State(state): State<AppState>,
    Path((run_id, capability_id)): Path<(Uuid, String)>,
    Json(req): Json<ExecuteWorkflowCapabilityRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    let config = req.config.or(req.input).unwrap_or_else(|| json!({}));
    let ctx = CapabilityContext {
        state: &state,
        run_id: scope.run_id,
        repo_ref: scope.repo_ref.as_str(),
        step: &scope.step,
        local_state: &scope.local_state,
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
