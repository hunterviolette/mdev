use anyhow::Result;
use serde_json::Value;

use crate::models::WorkflowRun;

use super::super::decisions::AutomationDecision;

pub fn after_stage(run: &WorkflowRun) -> Result<Vec<AutomationDecision>> {
    let mut decisions = Vec::new();

    if let Some(reason) = automation_value(run, "changeset_file_failures")
        .and_then(|v| v.get("pause_reason"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        decisions.push(AutomationDecision::Pause {
            reason: reason.to_string(),
        });
        return Ok(decisions);
    }

    if let Some(reason) = automation_value(run, "compile_failures")
        .and_then(|v| v.get("pause_reason"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        decisions.push(AutomationDecision::Pause {
            reason: reason.to_string(),
        });
    }

    Ok(decisions)
}

fn automation_value<'a>(run: &'a WorkflowRun, policy_key: &str) -> Option<&'a Value> {
    run.context
        .get("workflow_engine")?
        .get("global_state")?
        .get("automation")?
        .get(policy_key)
}
