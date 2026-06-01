use anyhow::Result;
use serde_json::Value;

use crate::models::WorkflowRun;

use super::super::decisions::GovernanceDecision;

pub fn after_stage(run: &WorkflowRun) -> Result<Vec<GovernanceDecision>> {
    let mut decisions = Vec::new();

    if let Some(reason) = governance_value(run, "changeset_file_failures")
        .and_then(|v| v.get("pause_reason"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        decisions.push(GovernanceDecision::Pause {
            reason: reason.to_string(),
        });
        return Ok(decisions);
    }

    if let Some(reason) = governance_value(run, "compile_failures")
        .and_then(|v| v.get("pause_reason"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        decisions.push(GovernanceDecision::Pause {
            reason: reason.to_string(),
        });
    }

    Ok(decisions)
}

fn governance_value<'a>(run: &'a WorkflowRun, policy_key: &str) -> Option<&'a Value> {
    let root = run.context.get("workflow_engine")?;

    root.get("global_state")
        .and_then(|global| global.get("governance"))
        .and_then(|gov| gov.get(policy_key))
        .or_else(|| root.get("governance").and_then(|gov| gov.get(policy_key)))
}
