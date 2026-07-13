use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::{
    engine::stages::capability_contract::StageCapabilities,
    models::WorkflowStepDefinition,
};

pub fn capabilities() -> StageCapabilities {
    StageCapabilities::new(["shared_dependencies", "qa_environment"])
}

pub fn prepare_stage_state(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    mut local_state: Value,
) -> Result<Value> {
    let qa = step
        .execution
        .qa
        .as_ref()
        .ok_or_else(|| anyhow!("QA stage '{}' is missing execution.qa configuration", step.id))?;

    if qa.environment.services.is_empty() {
        return Err(anyhow!("QA stage '{}' must define at least one service", step.id));
    }

    let state = local_state
        .as_object_mut()
        .ok_or_else(|| anyhow!("QA stage local state must be an object"))?;

    state.insert("resources".to_string(), json!({
        "repo": {
            "repo_ref": repo_ref,
            "git_ref": "WORKTREE"
        }
    }));
    state.insert("global_state".to_string(), global_state.clone());
    state.insert("qa".to_string(), serde_json::to_value(qa)?);
    state.insert("execution".to_string(), json!({
        "qa": qa
    }));

    let execution_logic = state
        .entry("execution_logic".to_string())
        .or_insert_with(|| step.execution_logic.clone());

    if !execution_logic.is_object() {
        *execution_logic = json!({});
    }

    let execution_logic = execution_logic
        .as_object_mut()
        .ok_or_else(|| anyhow!("QA execution logic must be an object"))?;

    execution_logic
        .entry("on_success".to_string())
        .or_insert_with(|| json!({
            "disposition": "paused",
            "message": "QA environment is running and requires operator approval."
        }));

    execution_logic
        .entry("on_error".to_string())
        .or_insert_with(|| json!({
            "disposition": "stay",
            "message": "QA environment failed to start or become ready."
        }));

    Ok(local_state)
}
