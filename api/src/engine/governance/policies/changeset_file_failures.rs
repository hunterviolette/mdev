use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::{json, Map, Value};

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
    if result.capability != "changeset" {
        return Ok(Vec::new());
    }

    let failing_files = failing_files_from_result(&result.payload);
    let mut next_files = existing_file_counts(run);

    if result.ok {
        for path in touched_files_from_result(&result.payload) {
            next_files.insert(path, Value::Number(0.into()));
        }
    } else {
        for path in &failing_files {
            let next = next_files
                .get(path)
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .saturating_add(1);
            next_files.insert(path.clone(), Value::Number(next.into()));
        }
    }

    let config = policy_config(run);
    let inject_after = config
        .get("inject_context_after_consecutive_failures")
        .and_then(Value::as_u64)
        .unwrap_or(2);
    let inject_broad_after = config
        .get("inject_broad_context_after_consecutive_failures")
        .and_then(Value::as_u64)
        .unwrap_or(4);
    let pause_after = config
        .get("pause_after_consecutive_failures")
        .and_then(Value::as_u64)
        .unwrap_or(6);

    let max_count = next_files
        .values()
        .filter_map(Value::as_u64)
        .max()
        .unwrap_or(0);

    let pause_reason = if max_count >= pause_after {
        Some(format!(
            "ChangeSet file failure threshold exceeded after {} consecutive failures.",
            max_count
        ))
    } else {
        None
    };

    let mut patch = json!({
        "governance": {
            "changeset_file_failures": {
                "inject_context_after_consecutive_failures": inject_after,
                "inject_broad_context_after_consecutive_failures": inject_broad_after,
                "pause_after_consecutive_failures": pause_after,
                "pause_reason": pause_reason,
                "state": {
                    "files": Value::Object(next_files.clone())
                }
            }
        }
    });

    if !result.ok && max_count >= inject_after {
        let include_files = if max_count >= inject_broad_after {
            Vec::<String>::new()
        } else {
            failing_files
        };

        merge_json_values(&mut patch, &json!({
            "capabilities": {
                "inference": {
                    "repo_context_armed": true
                },
                "context_export": {
                    "single_use_override": {
                        "include_files": include_files,
                        "include_staged_diff": false,
                        "include_unstaged_diff": false,
                        "git_ref": "WORKTREE",
                        "save_path": "/tmp/repo_context.txt"
                    }
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

fn policy_config(run: &WorkflowRun) -> Value {
    governance_value(run, "changeset_file_failures")
        .cloned()
        .unwrap_or_else(|| json!({}))
}

fn existing_file_counts(run: &WorkflowRun) -> Map<String, Value> {
    governance_value(run, "changeset_file_failures")
        .and_then(|v| v.get("state"))
        .and_then(|v| v.get("files"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn governance_value<'a>(run: &'a WorkflowRun, policy_key: &str) -> Option<&'a Value> {
    let root = run.context.get("workflow_engine")?;

    root.get("global_state")
        .and_then(|global| global.get("governance"))
        .and_then(|gov| gov.get(policy_key))
        .or_else(|| root.get("governance").and_then(|gov| gov.get(policy_key)))
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
