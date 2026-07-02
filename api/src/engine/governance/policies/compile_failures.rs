use anyhow::Result;
use serde_json::{json, Value};

use crate::{
    engine::capabilities::registry::CapabilityResult,
    models::{WorkflowRun, WorkflowStepDefinition},
};

use super::super::{
    decisions::{ContextMutation, GovernanceDecision},
    scopes::GovernanceScope,
};

pub fn after_capability(
    run: &WorkflowRun,
    _step: &WorkflowStepDefinition,
    result: &CapabilityResult,
    _prior_results: &[CapabilityResult],
) -> Result<Vec<GovernanceDecision>> {
    if result.capability != "compile_commands" {
        return Ok(Vec::new());
    }

    let ok = result.ok;
    let previous_consecutive = governance_value(run, "compile_failures")
        .and_then(|v| v.get("consecutive"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let pause_after = governance_value(run, "compile_failures")
        .and_then(|v| v.get("pause_after_consecutive_failures"))
        .and_then(Value::as_u64)
        .unwrap_or(4);

    let consecutive = if ok { 0 } else { previous_consecutive.saturating_add(1) };
    let pause_reason = if !ok && consecutive >= pause_after {
        Some(format!(
            "Compile failure threshold exceeded after {} consecutive failures.",
            consecutive
        ))
    } else {
        None
    };

    let mut patch = json!({
        "governance": {
            "compile_failures": {
                "pause_after_consecutive_failures": pause_after,
                "consecutive": consecutive,
                "latest_result": result.payload,
                "pause_reason": pause_reason
            }
        }
    });

    if !ok {
        merge_json_values(&mut patch, &json!({
            "capabilities": {
                "inference": {
                    "repo_context_armed": true
                }
            }
        }));
    }

    Ok(vec![GovernanceDecision::MutateContext {
        mutation: ContextMutation {
            scope: GovernanceScope::Global,
            patch,
        },
    }])
}

fn governance_value<'a>(run: &'a WorkflowRun, policy_key: &str) -> Option<&'a Value> {
    let root = run.context.get("workflow_engine")?;

    root.get("global_state")
        .and_then(|global| global.get("governance"))
        .and_then(|gov| gov.get(policy_key))
        .or_else(|| root.get("governance").and_then(|gov| gov.get(policy_key)))
}

fn merge_json_values(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                merge_json_values(target.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (target, patch) => {
            *target = patch.clone();
        }
    }
}
