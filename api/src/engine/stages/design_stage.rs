use anyhow::Result;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    models::{StageExecutionNode, WorkflowStepDefinition},
};

use super::{run_capability_plan, pause_on_enter, StageDisposition, StageOutcome};

pub async fn execute(
    state: &AppState,
    run_id: Uuid,
    repo_ref: &str,
    step: &WorkflowStepDefinition,
    local_state: Value,
    plan: &[StageExecutionNode],
) -> Result<StageOutcome> {
    let capability_results = run_capability_plan(state, run_id, repo_ref, step, &local_state, plan).await?;
    let capability_failed = capability_results
        .iter()
        .any(|item| item.get("ok").and_then(Value::as_bool) == Some(false));

    let disposition = if capability_failed {
        StageDisposition::Error
    } else if pause_on_enter(step) {
        StageDisposition::Paused
    } else {
        StageDisposition::Success
    };

    Ok(StageOutcome {
        ok: !capability_failed,
        disposition,
        capability_results,
        local_state,
        message: if capability_failed {
            "Design stage failed during backend workflow execution.".to_string()
        } else {
            "Design stage executed by backend workflow engine.".to_string()
        },
    })
}
