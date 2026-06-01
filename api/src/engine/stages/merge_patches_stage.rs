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
    crate::engine::stages::capability_contract::StageCapabilities::empty()
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

    for patch in patches {
        let patch_path = patch.get("patch_path").and_then(Value::as_str).unwrap_or_default().to_string();
        if patch_path.trim().is_empty() {
            continue;
        }
        match apply_patch(repo_ref, &patch_path) {
            Ok(()) => applied.push(patch),
            Err(err) => {
                failed.push(json!({
                    "patch": patch,
                    "error": err.to_string()
                }));
                break;
            }
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

    Ok(StageOutcome {
        ok,
        disposition: if ok { StageDisposition::Success } else { StageDisposition::Error },
        message: if ok { "Patch merge completed".to_string() } else { "Patch merge failed".to_string() },
        capability_results: vec![json!({ "key": "merge_patches", "ok": ok, "result": report })],
        local_state,
    })
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

fn apply_patch(repo_ref: &str, patch_path: &str) -> Result<()> {
    let output = Command::new("git")
        .arg("apply")
        .arg("--3way")
        .arg(PathBuf::from(patch_path))
        .current_dir(repo_ref)
        .output()
        .with_context(|| format!("failed to apply patch {}", patch_path))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow!(String::from_utf8_lossy(&output.stderr).to_string()))
    }
}
