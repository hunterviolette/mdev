use std::{path::PathBuf, process::Command};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::{append_engine_event, persist_context},
    models::{WorkflowRun, WorkflowStepDefinition},
};

use super::{StageDisposition, StageOutcome};

pub fn capabilities() -> crate::engine::stages::capability_contract::StageCapabilities {
    crate::engine::stages::capability_contract::StageCapabilities::new(["git_patch_payload"])
}

pub fn prepare_stage_state(step: &WorkflowStepDefinition, mut local_state: Value) -> Result<Value> {
    if !local_state.is_object() {
        local_state = json!({});
    }
    if local_state.get("patches").is_none() {
        if let Some(patches) = step.config.get("patches") {
            local_state.as_object_mut().unwrap().insert("patches".to_string(), patches.clone());
        }
    }
    Ok(local_state)
}

pub async fn execute_stage(
    state: &AppState,
    run_id: Uuid,
    run: &mut WorkflowRun,
    step: &WorkflowStepDefinition,
    repo_ref: &str,
    mut local_state: Value,
) -> Result<StageOutcome> {
    let patches = resolve_patches(run, step, &local_state);
    let mut applied = Vec::new();
    let mut failed = Vec::new();
    let mut capability_results = Vec::new();

    for patch in patches {
        let patch_path = patch.get("patch_path").and_then(Value::as_str).unwrap_or_default().to_string();
        if patch_path.trim().is_empty() {
            continue;
        }

        let payload_text = match build_patch_payload_text(repo_ref, &patch_path) {
            Ok(value) => value,
            Err(err) => {
                failed.push(json!({
                    "patch": patch,
                    "error": err.to_string()
                }));
                break;
            }
        };

        let ctx = crate::engine::capabilities::CapabilityContext {
            state,
            run_id,
            repo_ref,
            step,
            local_state: &local_state,
        };
        let results = crate::engine::capabilities::execute_capability_invocations(
            ctx,
            vec![crate::engine::capabilities::CapabilityInvocation {
                capability: "git_patch_payload".to_string(),
                config: json!({
                    "mode": "apply",
                    "repo_ref": repo_ref,
                    "payload_text": payload_text,
                    "reverse": false
                }),
            }],
        ).await?;

        let result = results.into_iter().next().ok_or_else(|| anyhow!("git_patch_payload returned no result"))?;
        let result_ok = result.ok;
        let result_payload = result.payload.clone();
        capability_results.push(json!({
            "key": "git_patch_payload",
            "ok": result_ok,
            "result": result_payload
        }));

        if result_ok {
            applied.push(patch);
        } else {
            failed.push(json!({
                "patch": patch,
                "error": result_payload.get("error").and_then(Value::as_str).unwrap_or("git_patch_payload failed")
            }));
            break;
        }
    }

    let ok = failed.is_empty();
    let report = json!({
        "ok": ok,
        "applied": applied,
        "failed": failed
    });

    local_state.as_object_mut().unwrap().insert("merge_report".to_string(), report.clone());
    persist_context(state, run_id, &run.context).await?;
    append_engine_event(
        state,
        run_id,
        Some(step.id.as_str()),
        if ok { "info" } else { "error" },
        "merge_patches_completed",
        if ok { "Patch merge completed" } else { "Patch merge failed" },
        report.clone(),
    ).await?;

    if capability_results.is_empty() {
        capability_results.push(json!({ "key": "merge_patches", "ok": ok, "result": report }));
    }

    Ok(StageOutcome {
        ok,
        disposition: if ok { StageDisposition::MoveNext } else { StageDisposition::Error },
        message: if ok { "Patch merge completed".to_string() } else { "Patch merge failed".to_string() },
        capability_results,
        local_state,
    })
}

fn build_patch_payload_text(repo_ref: &str, patch_path: &str) -> Result<String> {
    let patch = std::fs::read_to_string(PathBuf::from(patch_path))
        .with_context(|| format!("failed to read patch {}", patch_path))?;
    let base_head_output = Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(repo_ref)
        .output()
        .with_context(|| format!("failed to resolve HEAD for {}", repo_ref))?;
    if !base_head_output.status.success() {
        return Err(anyhow!(String::from_utf8_lossy(&base_head_output.stderr).to_string()));
    }
    let base_head = String::from_utf8_lossy(&base_head_output.stdout).trim().to_string();
    Ok(json!({
        "version": 1,
        "kind": "git_apply_patch",
        "scope": "unstaged",
        "from_ref": "HEAD",
        "to_ref": "WORKTREE",
        "base_head": base_head,
        "paths": [],
        "context_lines": null,
        "patch": patch
    }).to_string())
}

fn resolve_patches(run: &WorkflowRun, step: &WorkflowStepDefinition, local_state: &Value) -> Vec<Value> {
    local_state
        .get("patches")
        .or_else(|| step.config.get("patches"))
        .or_else(|| run.context.get("workflow_engine").and_then(|v| v.get("global_state")).and_then(|v| v.get("supervisor")).and_then(|v| v.get("patches")))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}


