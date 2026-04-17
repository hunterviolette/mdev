use anyhow::Result;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::capabilities::registry::{CapabilityInvocation, CapabilityResult},
    models::{WorkflowRun, WorkflowStepDefinition},
};

use super::{
    ensure_governance_slots, signals, CapabilityInjection, ContextMutation, GovernanceContext,
    GovernanceDecision, GovernanceHook, GovernanceScope,
};

pub async fn before_stage(
    _state: &AppState,
    run_id: Uuid,
    run: &mut WorkflowRun,
    step: &WorkflowStepDefinition,
) -> Result<Vec<GovernanceDecision>> {
    ensure_governance_slots(run);
    let _ctx = GovernanceContext::new(run_id, run, Some(step), None, None, None, &[], &[]);
    Ok(Vec::new())
}

pub async fn after_stage(
    state: &AppState,
    run_id: Uuid,
    run: &mut WorkflowRun,
    step: &WorkflowStepDefinition,
    stage_execution_id: &str,
    capability_results: &[Value],
) -> Result<Vec<GovernanceDecision>> {
    ensure_governance_slots(run);
    let latest_run = crate::engine::load_run(state, run_id).await.unwrap_or_else(|_| run.clone());
    let _ctx = GovernanceContext::new(
        run_id,
        &latest_run,
        Some(step),
        Some(stage_execution_id),
        None,
        None,
        &[],
        capability_results,
    );

    let stage_pause = latest_run
        .context
        .get("workflow_engine")
        .and_then(|v| v.get("stage_state"))
        .and_then(|v| v.get(step.id.as_str()))
        .and_then(|v| v.get("changeset_failures"))
        .and_then(|v| v.get("pause_reason"))
        .and_then(Value::as_str)
        .map(ToString::to_string);

    if let Some(reason) = stage_pause {
        return Ok(vec![GovernanceDecision::Pause { reason }]);
    }

    let run_pause = latest_run
        .context
        .get("workflow_engine")
        .and_then(|v| v.get("run_state"))
        .and_then(|v| v.get("compile_failures"))
        .and_then(|v| v.get("pause_reason"))
        .and_then(Value::as_str)
        .map(ToString::to_string);

    if let Some(reason) = run_pause {
        return Ok(vec![GovernanceDecision::Pause { reason }]);
    }

    Ok(Vec::new())
}

pub async fn before_capability(
    _state: &AppState,
    run_id: Uuid,
    run: &WorkflowRun,
    step: &WorkflowStepDefinition,
    stage_execution_id: Option<&str>,
    invocation: &CapabilityInvocation,
    prior_results: &[CapabilityResult],
) -> Result<Vec<GovernanceDecision>> {
    let _ctx = GovernanceContext::new(
        run_id,
        run,
        Some(step),
        stage_execution_id,
        Some(invocation),
        None,
        prior_results,
        &[],
    );
    Ok(Vec::new())
}

pub async fn after_capability(
    _state: &AppState,
    run_id: Uuid,
    run: &WorkflowRun,
    step: &WorkflowStepDefinition,
    stage_execution_id: Option<&str>,
    result: &CapabilityResult,
    prior_results: &[CapabilityResult],
) -> Result<Vec<GovernanceDecision>> {
    let invocation = CapabilityInvocation {
        capability: result.capability.clone(),
        config: Value::Null,
    };
    let _ctx = GovernanceContext::new(
        run_id,
        run,
        Some(step),
        stage_execution_id,
        Some(&invocation),
        Some(result),
        prior_results,
        &[],
    );

    Ok(match result.capability.as_str() {
        "gateway_model/changeset" => evaluate_changeset_guardrails(run, step, result),
        "compile_commands" => evaluate_compile_guardrails(run, step, result),
        _ => Vec::new(),
    })
}

pub fn pause_message(decisions: &[GovernanceDecision]) -> Option<String> {
    for decision in decisions {
        match decision {
            GovernanceDecision::Pause { reason } => return Some(reason.clone()),
            GovernanceDecision::RequireApproval { reason } => return Some(reason.clone()),
            _ => {}
        }
    }
    None
}

pub fn injected_capabilities(decisions: &[GovernanceDecision]) -> Vec<CapabilityInvocation> {
    decisions
        .iter()
        .filter_map(|decision| match decision {
            GovernanceDecision::InjectCapability {
                capability:
                    CapabilityInjection {
                        capability,
                        config,
                    },
            } => Some(CapabilityInvocation {
                capability: capability.clone(),
                config: config.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn evaluate_changeset_guardrails(
    run: &WorkflowRun,
    step: &WorkflowStepDefinition,
    result: &CapabilityResult,
) -> Vec<GovernanceDecision> {
    let thresholds = changeset_guardrails(run, step);
    let inject_after = thresholds
        .get("inject_context_after_consecutive_failures")
        .and_then(Value::as_u64)
        .unwrap_or(4);
    let pause_after = thresholds
        .get("pause_after_consecutive_failures")
        .and_then(Value::as_u64)
        .unwrap_or(8);

    let mut decisions = Vec::new();
    let current_stage_state = run
        .context
        .get("workflow_engine")
        .and_then(|v| v.get("stage_state"))
        .and_then(|v| v.get(step.id.as_str()))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let current_files = current_stage_state
        .get("changeset_failures")
        .and_then(|v| v.get("files"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let touched_files = signals::changeset_touched_files(result);
    let failed_files = signals::changeset_failure_files(result);
    let failed_paths = failed_files
        .iter()
        .filter_map(|file| file.get("path").and_then(Value::as_str))
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .collect::<std::collections::HashSet<_>>();

    let mut next_files = Map::new();
    let mut pause_reason: Option<String> = None;

    for path in touched_files.iter() {
        if failed_paths.contains(path) {
            continue;
        }
        next_files.insert(path.clone(), json!({
            "consecutive_failures": 0,
            "last_failure": Value::Null,
            "context_injected": false
        }));
    }

    for file in failed_files {
        let path = file.get("path").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if path.is_empty() {
            continue;
        }

        let previous = current_files
            .get(path.as_str())
            .and_then(|v| v.get("consecutive_failures"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let next = previous + 1;
        let context_injected = next >= inject_after;

        next_files.insert(path.clone(), json!({
            "consecutive_failures": next,
            "last_failure": file.clone(),
            "context_injected": context_injected
        }));

        if next == inject_after {
            decisions.push(GovernanceDecision::InjectCapability {
                capability: CapabilityInjection {
                    capability: "context_export".to_string(),
                    config: json!({
                        "include_files": [path.clone()],
                        "git_ref": "WORKTREE"
                    }),
                },
            });
        }

        if next >= pause_after && pause_reason.is_none() {
            pause_reason = Some(format!(
                "Paused after '{}' failed to update across {} consecutive changeset attempts.",
                path,
                next
            ));
        }
    }

    decisions.push(GovernanceDecision::MutateContext {
        mutation: ContextMutation {
            scope: GovernanceScope::Stage,
            patch: json!({
                "changeset_failures": {
                    "files": Value::Object(next_files),
                    "latest_failure": if result.ok { Value::Null } else { result.payload.clone() },
                    "pause_reason": pause_reason.clone()
                }
            }),
        },
    });

    if let Some(reason) = pause_reason {
        decisions.push(GovernanceDecision::Pause { reason });
    }

    decisions
}

fn evaluate_compile_guardrails(
    run: &WorkflowRun,
    step: &WorkflowStepDefinition,
    result: &CapabilityResult,
) -> Vec<GovernanceDecision> {
    let thresholds = compile_guardrails(run, step);
    let pause_after = thresholds
        .get("pause_after_consecutive_failures")
        .and_then(Value::as_u64)
        .unwrap_or(5);

    let current = run
        .context
        .get("workflow_engine")
        .and_then(|v| v.get("run_state"))
        .and_then(|v| v.get("compile_failures"))
        .and_then(|v| v.get("consecutive"))
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let next = if signals::compile_failed(result) {
        current + 1
    } else if signals::compile_succeeded(result) {
        0
    } else {
        current
    };

    let pause_reason = if signals::compile_failed(result) && next >= pause_after {
        Some(format!("Paused after {} consecutive compile failures.", next))
    } else {
        None
    };

    let mut decisions = vec![GovernanceDecision::MutateContext {
        mutation: ContextMutation {
            scope: GovernanceScope::Run,
            patch: json!({
                "compile_failures": {
                    "consecutive": next,
                    "latest_result": result.payload.clone(),
                    "pause_reason": pause_reason.clone()
                }
            }),
        },
    }];

    if let Some(reason) = pause_reason {
        decisions.push(GovernanceDecision::Pause { reason });
    }

    decisions
}

fn stage_policy_config(step: &WorkflowStepDefinition, policy_key: &str) -> Option<Value> {
    step.governance
        .policies
        .iter()
        .find(|policy| policy.enabled && policy.key == policy_key)
        .map(|policy| policy.config.clone())
}

fn changeset_guardrails(run: &WorkflowRun, step: &WorkflowStepDefinition) -> Value {
    stage_policy_config(step, "changeset_file_failures")
        .or_else(|| {
            run.context
                .get("workflow_engine")
                .and_then(|root| root.get("governance"))
                .and_then(|gov| gov.get("guardrails"))
                .cloned()
        })
        .unwrap_or_else(|| json!({
            "inject_context_after_consecutive_failures": 4,
            "pause_after_consecutive_failures": 8
        }))
}

fn compile_guardrails(run: &WorkflowRun, step: &WorkflowStepDefinition) -> Value {
    stage_policy_config(step, "compile_failures")
        .or_else(|| {
            run.context
                .get("workflow_engine")
                .and_then(|root| root.get("governance"))
                .and_then(|gov| gov.get("guardrails"))
                .cloned()
        })
        .unwrap_or_else(|| json!({
            "pause_after_consecutive_failures": 5
        }))
}
