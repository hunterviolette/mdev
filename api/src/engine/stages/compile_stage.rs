use anyhow::Result;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::{load_run, persist_context},
    models::{StageExecutionNode, WorkflowStepDefinition},
};

use super::{run_capability_plan, StageDisposition, StageOutcome};

pub async fn execute(
    state: &AppState,
    run_id: Uuid,
    repo_ref: &str,
    step: &WorkflowStepDefinition,
    local_state: Value,
    plan: &[StageExecutionNode],
) -> Result<StageOutcome> {
    let capability_results = run_capability_plan(state, run_id, repo_ref, step, &local_state, plan).await?;
    let capability_failed = capability_results
        .iter()
        .any(|item| item.get("ok").and_then(Value::as_bool) == Some(false));

    if capability_failed {
        persist_compile_error_for_code_stage(state, run_id, &capability_results).await?;
    }

    Ok(StageOutcome {
        ok: !capability_failed,
        disposition: if capability_failed {
            StageDisposition::ErrorCode("compile_checks_failed".to_string())
        } else {
            StageDisposition::Success
        },
        capability_results,
        local_state,
        message: if capability_failed {
            "Compile stage failed during backend workflow execution.".to_string()
        } else {
            "Compile stage completed successfully through backend workflow engine.".to_string()
        },
    })
}

async fn persist_compile_error_for_code_stage(
    state: &AppState,
    run_id: Uuid,
    capability_results: &[Value],
) -> Result<()> {
    let compile_result = capability_results
        .iter()
        .find(|item| item.get("key").and_then(Value::as_str) == Some("compile_commands"))
        .and_then(|item| item.get("result"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let outputs = compile_result
        .get("results")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let obj = row.as_object().cloned().unwrap_or_default();
                    format!(
                        "COMMAND: {}\nSTATUS: {}\nSTDOUT:\n{}\n\nSTDERR:\n{}",
                        obj.get("command").and_then(Value::as_str).unwrap_or(""),
                        obj.get("status").and_then(Value::as_i64).unwrap_or(-1),
                        obj.get("stdout").and_then(Value::as_str).unwrap_or(""),
                        obj.get("stderr").and_then(Value::as_str).unwrap_or(""),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_else(|| {
            compile_result
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("Compile checks failed.")
                .to_string()
        });

    let compile_fragment = format!(
        "Postprocess command failed after applying the previous ChangeSet.\n\nPOSTPROCESS OUTPUT:\n{}\n\nPlease provide a NEW ChangeSet JSON (version 1) that fixes the errors.",
        outputs
    );

    let mut run = load_run(state, run_id).await?;
    let root = super::ensure_engine_root(&mut run.context);
    let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
    let stage_state_obj = stage_state.as_object_mut().expect("stage_state must be object");
    let code_state = stage_state_obj.entry("code".to_string()).or_insert_with(|| json!({}));
    let code_state_obj = code_state.as_object_mut().expect("code stage state must be object");

    {
        let enabled = code_state_obj
            .entry("prompt_fragment_enabled".to_string())
            .or_insert_with(|| json!({}));
        if !enabled.is_object() {
            *enabled = json!({});
        }
        enabled
            .as_object_mut()
            .expect("prompt_fragment_enabled must be object")
            .insert("compile_error".to_string(), Value::Bool(true));
    }

    {
        let fragments = code_state_obj
            .entry("prompt_fragments".to_string())
            .or_insert_with(|| json!({}));
        if !fragments.is_object() {
            *fragments = json!({});
        }
        fragments
            .as_object_mut()
            .expect("prompt_fragments must be object")
            .insert("compile_error".to_string(), Value::String(compile_fragment));
    }

    let prompt = super::compose_prompt_from_state(
        code_state_obj.get("prompt_fragment_enabled").unwrap_or(&Value::Null),
        code_state_obj.get("prompt_fragments").unwrap_or(&Value::Null),
    );
    code_state_obj.insert("composed_prompt".to_string(), Value::String(prompt));

    persist_context(state, run_id, &run.context).await?;
    Ok(())
}
