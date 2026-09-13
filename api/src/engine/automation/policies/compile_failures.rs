use anyhow::Result;
use serde_json::{json, Value};

use crate::{
    engine::{automation, capabilities::registry::CapabilityResult},
    models::{WorkflowRun, WorkflowStepDefinition},
};

use super::super::{
    decisions::{AutomationDecision, ContextMutation},
    scopes::AutomationScope,
};

pub fn after_capability(
    run: &WorkflowRun,
    _step: &WorkflowStepDefinition,
    result: &CapabilityResult,
    _prior_results: &[CapabilityResult],
) -> Result<Vec<AutomationDecision>> {
    if result.capability != "compile_commands" {
        return Ok(Vec::new());
    }

    let ok = result.ok;
    let previous_consecutive = automation_value(run, "compile_failures")
        .and_then(|v| v.get("consecutive"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let profile = automation::profile(run);
    let pause_after = profile.pause_after_compile_errors;
    let pause_enabled = profile.is_selected("pause_after_compile_errors");

    let consecutive = if ok { 0 } else { previous_consecutive.saturating_add(1) };
    let pause_triggered = pause_enabled && !ok && consecutive >= pause_after;
    let pause_reason = if pause_triggered {
        Some(format!(
            "Compile failure threshold exceeded after {} consecutive failures.",
            consecutive
        ))
    } else {
        None
    };
    let next_consecutive = if pause_triggered { 0 } else { consecutive };

    let patch = json!({
        "automation": {
            "compile_failures": {
                "pause_after_consecutive_failures": pause_after,
                "consecutive": next_consecutive,
                "pause_reason": pause_reason
            }
        }
    });

    Ok(vec![AutomationDecision::MutateContext {
        mutation: ContextMutation {
            scope: AutomationScope::Global,
            patch,
        },
    }])
}

fn automation_value<'a>(run: &'a WorkflowRun, policy_key: &str) -> Option<&'a Value> {
    run.context
        .get("workflow_engine")?
        .get("global_state")?
        .get("automation")?
        .get(policy_key)
}

