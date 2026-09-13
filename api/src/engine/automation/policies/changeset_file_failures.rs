use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::{json, Map, Value};

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
    if result.capability != "changeset" {
        return Ok(Vec::new());
    }

    let profile = automation::profile(run);
    let inject_after = profile.inject_file_context_after_changeset_failures;
    let inject_broad_after = profile.inject_broad_context_after_changeset_failures;
    let pause_after = profile.pause_after_changeset_failures;
    let inject_schema_after = profile.inject_changeset_schema_after_errors;
    let inject_file_enabled = profile.is_selected("inject_file_context_after_changeset_failures");
    let inject_broad_enabled = profile.is_selected("inject_broad_context_after_changeset_failures");
    let pause_enabled = profile.is_selected("pause_after_changeset_failures");
    let inject_schema_enabled = profile.is_selected("inject_changeset_schema_after_errors");
    let payload_error = result
        .payload
        .get("error_kind")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind == "payload");
    let previous_payload_errors = automation_value(run, "changeset_payload_errors")
        .and_then(|value| value.get("consecutive"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let consecutive_payload_errors = if payload_error {
        previous_payload_errors.saturating_add(1)
    } else {
        0
    };

    let failing_files = failing_files_from_result(&result.payload);
    let mut next_files = existing_file_counts(run);

    if result.ok {
        for path in touched_files_from_result(&result.payload) {
            next_files.insert(path, Value::Number(0.into()));
        }
    } else if !payload_error {
        for path in &failing_files {
            let next = next_files
                .get(path)
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .saturating_add(1);
            next_files.insert(path.clone(), Value::Number(next.into()));
        }
    }

    let max_count = failing_files
        .iter()
        .filter_map(|path| next_files.get(path).and_then(Value::as_u64))
        .max()
        .unwrap_or(0);

    let pause_triggered = pause_enabled && !payload_error && max_count >= pause_after;
    let pause_reason = if pause_triggered {
        Some(format!(
            "Changeset file failure threshold exceeded after {} consecutive failures.",
            max_count
        ))
    } else {
        None
    };

    if pause_triggered {
        for count in next_files.values_mut() {
            *count = Value::Number(0.into());
        }
    }

    let mut patch = json!({
        "automation": {
            "changeset_file_failures": {
                "inject_context_after_consecutive_failures": inject_after,
                "inject_broad_context_after_consecutive_failures": inject_broad_after,
                "pause_after_consecutive_failures": pause_after,
                "pause_reason": pause_reason,
                "state": {
                    "files": Value::Object(next_files.clone())
                }
            },
            "changeset_payload_errors": {
                "inject_changeset_schema_after_consecutive_errors": inject_schema_after,
                "consecutive": consecutive_payload_errors
            }
        }
    });

    if inject_schema_enabled && payload_error && consecutive_payload_errors >= inject_schema_after {
        merge_json_values(&mut patch, &json!({
            "capabilities": {
                "inference": {
                    "changeset_schema_armed": true
                }
            }
        }));
    } else if inject_broad_enabled && !result.ok && max_count >= inject_broad_after {
        merge_json_values(&mut patch, &json!({
            "capabilities": {
                "inference": {
                    "repo_context_armed": true
                },
                "context_export": {
                    "single_use_override": Value::Null
                }
            }
        }));
    } else if inject_file_enabled && !result.ok && max_count >= inject_after && !failing_files.is_empty() {
        merge_json_values(&mut patch, &json!({
            "capabilities": {
                "inference": {
                    "repo_context_armed": true
                },
                "context_export": {
                    "single_use_override": {
                        "include_files": failing_files,
                        "include_directories": [],
                        "include_staged_diff": false,
                        "include_unstaged_diff": false,
                        "git_ref": "WORKTREE",
                        "artifact_kind": "targeted"
                    }
                }
            }
        }));
    }

    Ok(vec![AutomationDecision::MutateContext {
        mutation: ContextMutation {
            scope: AutomationScope::Global,
            patch,
        },
    }])
}

fn existing_file_counts(run: &WorkflowRun) -> Map<String, Value> {
    automation_value(run, "changeset_file_failures")
        .and_then(|v| v.get("state"))
        .and_then(|v| v.get("files"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn automation_value<'a>(run: &'a WorkflowRun, policy_key: &str) -> Option<&'a Value> {
    run.context
        .get("workflow_engine")?
        .get("global_state")?
        .get("automation")?
        .get(policy_key)
}

fn failing_files_from_result(result: &Value) -> Vec<String> {
    result
        .get("failing_files")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("path").and_then(Value::as_str).map(ToString::to_string))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .unwrap_or_default()
}

fn touched_files_from_result(result: &Value) -> Vec<String> {
    result
        .get("touched_files")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .unwrap_or_default()
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
