use axum::http::StatusCode;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine,
    models::{WorkflowRun, WorkflowStepDefinition},
};

#[derive(Debug, Clone)]
pub struct WorkflowScope {
    pub run_id: Uuid,
    pub run: WorkflowRun,
    pub step: WorkflowStepDefinition,
    pub local_state: Value,
    pub repo_ref: String,
    pub git_ref: String,
}

pub async fn resolve_workflow_scope(
    state: &AppState,
    run_id: Uuid,
) -> Result<WorkflowScope, (StatusCode, String)> {
    let run = engine::load_run(state, run_id).await.map_err(internal)?;
    let root = run.context.get("workflow_engine").cloned().unwrap_or_else(|| json!({}));
    let global_state = root.get("global_state").cloned().unwrap_or_else(|| json!({}));
    let repo_resource = global_state.get("resources").and_then(|value| value.get("repo"));

    let repo_ref = repo_resource
        .and_then(|value| value.get("repo_ref"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(run.repo_ref.as_str())
        .trim()
        .to_string();

    if repo_ref.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "workflow has no repo resource".to_string()));
    }

    let git_ref = repo_resource
        .and_then(|value| value.get("git_ref"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("WORKTREE")
        .trim()
        .to_string();

    let step = run
        .current_step_id
        .as_deref()
        .and_then(|step_id| run.definition.steps.iter().find(|step| step.id == step_id))
        .or_else(|| run.definition.steps.first())
        .cloned()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "workflow has no current step".to_string()))?;

    let local_state = root
        .get("stage_overrides")
        .and_then(Value::as_object)
        .and_then(|obj| obj.get(step.id.as_str()))
        .cloned()
        .or_else(|| {
            root.get("stage_state")
                .and_then(Value::as_object)
                .and_then(|obj| obj.get(step.id.as_str()))
                .cloned()
        })
        .unwrap_or_else(|| json!({}));

    Ok(WorkflowScope {
        run_id,
        run,
        step,
        local_state,
        repo_ref,
        git_ref,
    })
}

fn internal<E: std::fmt::Display>(err: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
