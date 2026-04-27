use anyhow::Result;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::{
    engine::capabilities::binding_specs,
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
        .and_then(|v| v.get("governance"))
        .and_then(|v| v.get("changeset_file_failures"))
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

        if result.ok {
            decisions.push(GovernanceDecision::MutateContext {
                mutation: ContextMutation {
                    scope: GovernanceScope::Stage,
                    patch: json!({
                        "prompt": {
                            "user_input": Value::Null
                        }
                    }),
                },
            });
        }
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
    _run: &WorkflowRun,
    _step: &WorkflowStepDefinition,
) -> Vec<GovernanceDecision> {
    Vec::new()
}

fn evaluate_inference_stage_consumption(
    run: &WorkflowRun,
    step: &WorkflowStepDefinition,
) -> Vec<GovernanceDecision> {
    let workflow_engine = run
        .context
        .get("workflow_engine")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let global_state = workflow_engine
        .get("global_state")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let mut inference_patch = Map::new();
    let mut context_export_patch = Map::new();

    if shared_single_use_capability_should_clear(&global_state, step, "repo_context") {
        inference_patch.insert("repo_context_armed".to_string(), Value::Bool(false));
        context_export_patch.insert("single_use_override".to_string(), Value::Null);
    }

    if shared_single_use_capability_should_clear(&global_state, step, "changeset_schema") {
        inference_patch.insert("changeset_schema_armed".to_string(), Value::Bool(false));
    }

    if inference_patch.is_empty() && context_export_patch.is_empty() {
        return Vec::new();
    }

    let mut capabilities_patch = Map::new();
    if !inference_patch.is_empty() {
        capabilities_patch.insert("inference".to_string(), Value::Object(inference_patch));
    }
    if !context_export_patch.is_empty() {
        capabilities_patch.insert("context_export".to_string(), Value::Object(context_export_patch));
    }

    vec![GovernanceDecision::MutateContext {
        mutation: ContextMutation {
            scope: GovernanceScope::Global,
            patch: json!({
                "capabilities": Value::Object(capabilities_patch)
            }),
        },
    }]
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

fn shared_single_use_capability_should_clear(
    global_state: &Value,
    step: &WorkflowStepDefinition,
    primitive_key: &str,
) -> bool {
    binding_specs::stage_supports_shared_capability(step, primitive_key)
        && binding_specs::shared_capability_enabled(global_state, primitive_key, false)
        && matches!(
            binding_specs::shared_capability_lifecycle(primitive_key),
            binding_specs::SharedCapabilityLifecycle::SingleUseGlobal
        )
}

fn stage_supports_repo_context(step: &WorkflowStepDefinition) -> bool {
    binding_specs::stage_supports_shared_capability(step, "repo_context")
}

fn stage_supports_changeset_schema(step: &WorkflowStepDefinition) -> bool {
    binding_specs::stage_supports_shared_capability(step, "changeset_schema")
}

fn evaluate_changeset_guardrails(
    run: &WorkflowRun,
    step: &WorkflowStepDefinition,
    result: &CapabilityResult,
) -> Vec<GovernanceDecision> {
    let Some(thresholds) = governance_policy_config(run, "changeset_file_failures") else {
        return Vec::new();
    };
    let inject_after = thresholds
        .get("inject_context_after_consecutive_failures")
        .and_then(Value::as_u64)
        .unwrap_or(4);
    let inject_broad_after = thresholds
        .get("inject_broad_context_after_consecutive_failures")
        .and_then(Value::as_u64)
        .unwrap_or(inject_after.saturating_add(3));
    let pause_after = thresholds
        .get("pause_after_consecutive_failures")
        .and_then(Value::as_u64)
        .unwrap_or(8);

    let mut decisions = Vec::new();
    let current_files = run
        .context
        .get("workflow_engine")
        .and_then(|v| v.get("governance"))
        .and_then(|v| v.get("changeset_file_failures"))
        .and_then(|v| v.get("state"))
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
        next_files.insert(path.clone(), json!(0));
    }

    for file in failed_files {
        let path = file.get("path").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if path.is_empty() {
            continue;
        }

        let previous = current_files
            .get(path.as_str())
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let next = previous + 1;
        let should_export_broad = next >= inject_broad_after;
        let should_export_targeted = next >= inject_after && !should_export_broad;

        if should_export_targeted {
            decisions.push(GovernanceDecision::MutateContext {
                mutation: ContextMutation {
                    scope: GovernanceScope::Global,
                    patch: json!({
                        "capabilities": {
                            "context_export": {
                                "single_use_override": {
                                    "include_files": [path.clone()],
                                    "include_staged_diff": false,
                                    "include_unstaged_diff": false,
                                    "git_ref": "WORKTREE",
                                    "save_path": targeted_context_save_path(run.id, path.as_str())
                                }
                            },
                            "inference": {
                                "repo_context_armed": true
                            }
                        }
                    }),
                },
            });
        }

        if should_export_broad {
            decisions.push(GovernanceDecision::MutateContext {
                mutation: ContextMutation {
                    scope: GovernanceScope::Global,
                    patch: json!({
                        "capabilities": {
                            "context_export": {
                                "single_use_override": Value::Null
                            },
                            "inference": {
                                "repo_context_armed": true
                            }
                        }
                    }),
                },
            });
        }

        let stored_next = if should_export_targeted || should_export_broad { 0 } else { next };
        next_files.insert(path.clone(), json!(stored_next));

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
            scope: GovernanceScope::Governance,
            patch: json!({
                "changeset_file_failures": {
                    "state": {
                        "files": Value::Object(next_files)
                    },
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
    _step: &WorkflowStepDefinition,
    result: &CapabilityResult,
) -> Vec<GovernanceDecision> {
    let Some(thresholds) = governance_policy_config(run, "compile_failures") else {
        return Vec::new();
    };
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
                    "latest_result": compile_runtime_signal(result),
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

fn targeted_context_save_path(run_id: Uuid, path: &str) -> String {
    format!(
        "/tmp/mdev_targeted_context_{}_{}.txt",
        run_id,
        sanitize_context_path_component(path)
    )
}

fn sanitize_context_path_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string();

    if sanitized.is_empty() {
        "context".to_string()
    } else {
        sanitized
    }
}

fn compact_changeset_file_failure(file: &Value) -> Value {
    json!({
        "path": file.get("path").cloned().unwrap_or(Value::Null),
        "operation_index": file.get("operation_index").cloned().unwrap_or(Value::Null),
        "operation_kind": file.get("operation_kind").cloned().unwrap_or(Value::Null),
        "failed_actions": file.get("failed_actions").cloned().unwrap_or_else(|| json!([]))
    })
}

fn compact_changeset_result(result: &CapabilityResult) -> Value {
    json!({
        "ok": result.ok,
        "summary": result.payload.get("summary").cloned().unwrap_or(Value::Null),
        "stats": result.payload.get("stats").cloned().unwrap_or(Value::Null),
        "failing_files": result.payload
            .get("failing_files")
            .and_then(Value::as_array)
            .map(|items| items.iter().map(compact_changeset_file_failure).collect::<Vec<_>>())
            .unwrap_or_default()
    })
}

fn compile_runtime_signal(result: &CapabilityResult) -> Value {
    let results = result
        .payload
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let commands = results
        .iter()
        .filter_map(|row| row.get("command").and_then(Value::as_str))
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    let failed_command_count = results
        .iter()
        .filter(|row| row.get("status").and_then(Value::as_i64).unwrap_or(0) != 0)
        .count();

    json!({
        "ok": result.ok,
        "had_error": signals::compile_failed(result),
        "has_compile_log": !results.is_empty(),
        "failed_command_count": failed_command_count,
        "commands": commands,
        "no_commands_configured": result.payload.get("no_commands_configured").and_then(Value::as_bool).unwrap_or(false),
        "skipped": result.payload.get("skipped").and_then(Value::as_bool).unwrap_or(false)
    })
}

fn governance_policy_config(run: &WorkflowRun, policy_key: &str) -> Option<Value> {
    run.context
        .get("workflow_engine")
        .and_then(|root| root.get("governance"))
        .and_then(|gov| gov.get(policy_key))
        .cloned()
}
