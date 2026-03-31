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
    let disposition = if pause_on_enter(step) {
        StageDisposition::Paused
    } else {
        StageDisposition::Success
    };

    Ok(StageOutcome {
        ok: true,
        disposition,
        capability_results,
        local_state,
        message: "Design stage executed by backend workflow engine.".to_string(),
    })
}
