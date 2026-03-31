use anyhow::Result;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    models::{StageExecutionNode, WorkflowStepDefinition},
};

use super::{run_capability_plan, StageDisposition, StageOutcome};

pub async fn execute(
    state: &AppState,
    run_id: Uuid,
    repo_ref: &str,
    step: &WorkflowStepDefinition,
    local_state: Value,
    plan: &[StageExecutionNode],
) -> Result<StageOutcome> {
    let capability_results = run_capability_plan(state, run_id, repo_ref, step, &local_state, plan).await?;
    let require_manual_approval = step.execution_logic
        .get("require_manual_approval")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let disposition = if require_manual_approval {
        StageDisposition::Paused
    } else {
        StageDisposition::Success
    };

    Ok(StageOutcome {
        ok: true,
        disposition,
        capability_results,
        local_state,
        message: if require_manual_approval {
            "Review stage completed and is waiting for manual approval.".to_string()
        } else {
            "Review stage completed successfully.".to_string()
        },
    })
}
