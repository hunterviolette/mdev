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
    Ok(evaluate_inference_stage_bootstrap(run, step))
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

    let mut decisions = match result.capability.as_str() {
        "gateway_model/changeset" => evaluate_changeset_guardrails(run, step, result),
        "compile_commands" => evaluate_compile_guardrails(run, step, result),
        _ => Vec::new(),
    };

    if result.capability == "inference" {
        decisions.extend(evaluate_inference_stage_consumption(run, step));
    }

    Ok(decisions)
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

fn evaluate_inference_stage_bootstrap(
    run: &WorkflowRun,
    step: &WorkflowStepDefinition,
) -> Vec<GovernanceDecision> {
    let repo_context_enabled = stage_supports_repo_context(step)
        .then(|| shared_inference_primitive_default_enabled(run, step, "repo_context"));
    let changeset_schema_enabled = stage_supports_changeset_schema(step)
        .then(|| shared_inference_primitive_default_enabled(run, step, "changeset_schema"));

    let mut inference_patch = Map::new();

    if let Some(enabled) = repo_context_enabled {
        inference_patch.insert(
            "repo_context".to_string(),
            json!({
                "enabled": enabled
            }),
        );
    }

    if let Some(enabled) = changeset_schema_enabled {
        inference_patch.insert(
            "changeset_schema".to_string(),
            json!({
                "enabled": enabled
            }),
        );
    }

    if inference_patch.is_empty() {
        return Vec::new();
    }

    let mut decisions = vec![GovernanceDecision::MutateContext {
        mutation: ContextMutation {
            scope: GovernanceScope::Stage,
            patch: json!({
                "execution_logic": {
                    "connections": {
                        "inference": Value::Object(inference_patch)
                    }
                }
            }),
        },
    }];

    let mut stage_runtime_patch = Map::new();
    if let Some(enabled) = repo_context_enabled {
        stage_runtime_patch.insert(
            "repo_context".to_string(),
            json!({
                "enabled": enabled,
                "rehydrated_from_shared_runtime": true,
                "last_trigger": "before_stage"
            }),
        );
    }
    if let Some(enabled) = changeset_schema_enabled {
        stage_runtime_patch.insert(
            "changeset_schema".to_string(),
            json!({
                "enabled": enabled,
                "rehydrated_from_shared_runtime": true,
                "last_trigger": "before_stage"
            }),
        );
    }

    if !stage_runtime_patch.is_empty() {
        decisions.push(GovernanceDecision::MutateContext {
            mutation: ContextMutation {
                scope: GovernanceScope::Stage,
                patch: json!({
                    "inference_runtime": Value::Object(stage_runtime_patch)
                }),
            },
        });
    }

    decisions
}

fn evaluate_inference_stage_consumption(
    run: &WorkflowRun,
    step: &WorkflowStepDefinition,
) -> Vec<GovernanceDecision> {
    let current_stage_state = run
        .context
        .get("workflow_engine")
        .and_then(|v| v.get("stage_state"))
        .and_then(|v| v.get(step.id.as_str()))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let mut inference_patch = Map::new();

    if stage_supports_repo_context(step)
        && stage_inference_flag_enabled(&current_stage_state, "repo_context")
    {
        inference_patch.insert(
            "repo_context".to_string(),
            json!({
                "enabled": false
            }),
        );
    }

    if stage_supports_changeset_schema(step)
        && stage_inference_flag_enabled(&current_stage_state, "changeset_schema")
    {
        inference_patch.insert(
            "changeset_schema".to_string(),
            json!({
                "enabled": false
            }),
        );
    }

    if inference_patch.is_empty() {
        return Vec::new();
    }

    let mut decisions = vec![GovernanceDecision::MutateContext {
        mutation: ContextMutation {
            scope: GovernanceScope::Stage,
            patch: json!({
                "execution_logic": {
                    "connections": {
                        "inference": Value::Object(inference_patch.clone())
                    }
                }
            }),
        },
    }];

    let mut runtime_patch = Map::new();
    if inference_patch.contains_key("repo_context") {
        runtime_patch.insert(
            "shared_stage_family:design_code".to_string(),
            json!({
                "repo_context": {
                    "has_fired": true,
                    "active": true,
                    "last_trigger": "inference_consumed"
                }
            }),
        );
    }
    if inference_patch.contains_key("changeset_schema") {
        runtime_patch.insert(
            "changeset_schema".to_string(),
            json!({
                "has_fired": true,
                "active": true,
                "last_trigger": "inference_consumed"
            }),
        );
    }

    if !runtime_patch.is_empty() {
        decisions.push(GovernanceDecision::MutateContext {
            mutation: ContextMutation {
                scope: GovernanceScope::Global,
                patch: json!({
                    "capabilities": {
                        "inference": {
                            "connection_runtime": Value::Object(runtime_patch)
                        }
                    }
                }),
            },
        });
    }

    decisions
}

fn has_stage_inference_flag(stage_state: &Value, key: &str) -> bool {
    stage_state
        .get("execution_logic")
        .and_then(|v| v.get("connections"))
        .and_then(|v| v.get("inference"))
        .and_then(|v| v.get(key))
        .and_then(|v| v.get("enabled"))
        .is_some()
}

fn stage_inference_flag_enabled(stage_state: &Value, key: &str) -> bool {
    stage_state
        .get("execution_logic")
        .and_then(|v| v.get("connections"))
        .and_then(|v| v.get("inference"))
        .and_then(|v| v.get(key))
        .and_then(|v| v.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn stage_supports_repo_context(step: &WorkflowStepDefinition) -> bool {
    step.step_type == "design"
        || step.step_type == "code"
        || step.prompt.include_repo_context
        || step
            .execution_logic
            .get("connections")
            .and_then(|v| v.get("inference"))
            .and_then(|v| v.get("repo_context"))
            .is_some()
}

fn stage_supports_changeset_schema(step: &WorkflowStepDefinition) -> bool {
    step.step_type == "code"
        || step.prompt.include_changeset_schema
        || step
            .execution_logic
            .get("connections")
            .and_then(|v| v.get("inference"))
            .and_then(|v| v.get("changeset_schema"))
            .is_some()
}

fn shared_inference_primitive_default_enabled(
    run: &WorkflowRun,
    step: &WorkflowStepDefinition,
    primitive_key: &str,
) -> bool {
    let connection_runtime = run
        .context
        .get("workflow_engine")
        .and_then(|v| v.get("global_state"))
        .and_then(|v| v.get("capabilities"))
        .and_then(|v| v.get("inference"))
        .and_then(|v| v.get("connection_runtime"));

    let primitive_state = if primitive_key == "repo_context"
        && (step.step_type == "design" || step.step_type == "code")
    {
        connection_runtime
            .and_then(|v| v.get("shared_stage_family:design_code"))
            .and_then(|v| v.get("repo_context"))
    } else {
        connection_runtime.and_then(|v| v.get(primitive_key))
    };

    let has_fired = primitive_state
        .and_then(|v| v.get("has_fired"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let should_enable = !has_fired;
    if should_enable {
        let process_session_id = connection_runtime
            .and_then(|v| v.get("process_session_id"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let browser_session_id = run
            .context
            .get("workflow_engine")
            .and_then(|v| v.get("global_state"))
            .and_then(|v| v.get("capabilities"))
            .and_then(|v| v.get("inference"))
            .and_then(|v| v.get("browser"))
            .and_then(|v| v.get("session_id"))
            .and_then(Value::as_str)
            .unwrap_or("");

        if primitive_state.is_none() {
            tracing::warn!(
                run_id = %run.id,
                step_id = %step.id,
                step_type = %step.step_type,
                primitive_key = %primitive_key,
                process_session_id = %process_session_id,
                browser_session_id = %browser_session_id,
                "shared inference primitive defaulted to enabled because runtime marker was missing"
            );
        } else {
            tracing::warn!(
                run_id = %run.id,
                step_id = %step.id,
                step_type = %step.step_type,
                primitive_key = %primitive_key,
                process_session_id = %process_session_id,
                browser_session_id = %browser_session_id,
                "shared inference primitive defaulted to enabled because has_fired was false"
            );
        }
    }

    should_enable
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
