use anyhow::Result;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    models::{StageExecutionNode, WorkflowStepDefinition},
};

use crate::engine::capabilities::{execute_root_capability, CapabilityContext};
use crate::engine::{append_engine_event, load_run, persist_context};

use super::{compose_prompt_from_state, normalize_repo_context_payload, StageDisposition, StageOutcome};

pub async fn execute_code_stage(
    state: &AppState,
    run_id: Uuid,
    repo_ref: &str,
    step: &WorkflowStepDefinition,
    local_state: &Value,
    plan: &[StageExecutionNode],
) -> Result<StageOutcome> {
    let max_attempts = step
        .execution_logic
        .get("max_consecutive_apply_failures")
        .and_then(Value::as_u64)
        .unwrap_or(3) as usize;

    let mut enabled = local_state
        .get("prompt_fragment_enabled")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut fragments = local_state
        .get("prompt_fragments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let include_repo_context = enabled
        .get("repo_context")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let repo_context = normalize_repo_context_payload(repo_ref, local_state.get("repo_context").cloned());

    let _compile_commands: Vec<String> = plan
        .iter()
        .find(|n| n.key == "compile_checks")
        .and_then(|n| n.config.get("commands"))
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(ToString::to_string).collect())
        .unwrap_or_default();

    for attempt in 1..=max_attempts {
        let prompt = compose_prompt_from_state(&enabled, &fragments);
        let mut attempt_local_state = local_state.clone();
        if let Some(obj) = attempt_local_state.as_object_mut() {
            obj.insert("prompt_fragment_enabled".to_string(), enabled.clone());
            obj.insert("prompt_fragments".to_string(), fragments.clone());
            obj.insert("composed_prompt".to_string(), Value::String(prompt));
            if include_repo_context {
                obj.insert("repo_context".to_string(), repo_context.clone());
            }
        }

        let capability_ctx = CapabilityContext {
            state,
            run_id,
            repo_ref,
            step,
            local_state: &attempt_local_state,
        };

        let capability_chain = execute_root_capability(capability_ctx).await?;
        let capability_results: Vec<Value> = capability_chain
            .iter()
            .map(|item| {
                json!({
                    "key": item.capability,
                    "result": item.payload,
                })
            })
            .collect();

        let inference_json = capability_chain
            .iter()
            .find(|item| item.capability == "inference")
            .map(|item| item.payload.clone())
            .unwrap_or_else(|| json!({}));

        let payload_text = inference_json
            .get("result")
            .and_then(|v| v.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        if payload_text.trim().is_empty() {
            return Ok(StageOutcome {
                ok: false,
                disposition: StageDisposition::ErrorCode("empty_changeset_payload".to_string()),
                message: "Inference returned an empty ChangeSet payload.".to_string(),
                capability_results,
                local_state: attempt_local_state,
            });
        }

        let apply_json = capability_chain
            .iter()
            .find(|item| item.capability == "apply_changeset")
            .map(|item| item.payload.clone())
            .unwrap_or_else(|| json!({}));

        if !apply_json.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            let lines = apply_json
                .get("lines")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|v| v.as_str().unwrap_or(""))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_else(|| {
                    apply_json
                        .get("summary")
                        .and_then(Value::as_str)
                        .unwrap_or("ChangeSet apply failed.")
                        .to_string()
                });

            fragments["apply_error"] = Value::String(format!(
                "ChangeSet apply failed.\n\n{}\n\nPlease provide a NEW ChangeSet JSON (version 1) that fixes the apply errors.",
                lines
            ));
            enabled["apply_error"] = Value::Bool(true);
            persist_stage_retry_state(state, run_id, &step.id, &enabled, &fragments).await?;

            if attempt < max_attempts {
                append_engine_event(
                    state,
                    run_id,
                    Some(step.id.as_str()),
                    "warn",
                    "code_stage_retrying",
                    "ChangeSet apply failed; retrying current code stage attempt.",
                    json!({ "attempt": attempt, "max_attempts": max_attempts }),
                )
                .await?;
                continue;
            }

            return Ok(StageOutcome {
                ok: false,
                disposition: StageDisposition::ErrorCode("changeset_apply_failed".to_string()),
                message: "Code stage exhausted retry attempts on ChangeSet apply.".to_string(),
                capability_results,
                local_state: attempt_local_state,
            });
        }

        let terminal_json = capability_chain
            .iter()
            .find(|item| item.capability == "compile_commands")
            .map(|item| item.payload.clone());

        if let Some(terminal_json) = terminal_json {
            if !terminal_json.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                let outputs = terminal_json
                    .get("outputs")
                    .and_then(Value::as_array)
                    .map(|rows| {
                        rows.iter()
                            .map(|row| {
                                let obj = row.as_object().cloned().unwrap_or_default();
                                format!(
                                    "COMMAND: {}\n{}",
                                    obj.get("command").and_then(Value::as_str).unwrap_or(""),
                                    obj.get("output").and_then(Value::as_str).unwrap_or("")
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n\n")
                    })
                    .unwrap_or_else(|| "Compile checks failed.".to_string());

                fragments["compile_error"] = Value::String(format!(
                    "Postprocess command failed after applying the previous ChangeSet.\n\nPOSTPROCESS OUTPUT:\n{}\n\nPlease provide a NEW ChangeSet JSON (version 1) that fixes the errors.",
                    outputs
                ));
                enabled["compile_error"] = Value::Bool(true);
                persist_stage_retry_state(state, run_id, &step.id, &enabled, &fragments).await?;

                if attempt < max_attempts {
                    append_engine_event(
                        state,
                        run_id,
                        Some(step.id.as_str()),
                        "warn",
                        "code_stage_retrying",
                        "Compile checks failed; retrying current code stage attempt.",
                        json!({ "attempt": attempt, "max_attempts": max_attempts }),
                    )
                    .await?;
                    continue;
                }

                return Ok(StageOutcome {
                    ok: false,
                    disposition: StageDisposition::ErrorCode("compile_checks_failed".to_string()),
                    message: "Code stage exhausted retry attempts on compile checks.".to_string(),
                    capability_results,
                    local_state: attempt_local_state,
                });
            }
        }

        return Ok(StageOutcome {
            ok: true,
            disposition: StageDisposition::Success,
            message: "Code stage completed successfully through backend workflow engine.".to_string(),
            capability_results,
            local_state: attempt_local_state,
        });
    }

    Ok(StageOutcome {
        ok: false,
        disposition: StageDisposition::Error,
        message: "Code stage exhausted retry attempts.".to_string(),
        capability_results: vec![],
        local_state: local_state.clone(),
    })
}

async fn persist_stage_retry_state(
    state: &AppState,
    run_id: Uuid,
    step_id: &str,
    enabled: &Value,
    fragments: &Value,
) -> Result<()> {
    let mut run = load_run(state, run_id).await?;
    let root = super::ensure_engine_root(&mut run.context);
    let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
    let stage_state_obj = stage_state.as_object_mut().expect("stage_state must be object");
    let existing = stage_state_obj.entry(step_id.to_string()).or_insert_with(|| json!({}));
    if let Some(obj) = existing.as_object_mut() {
        obj.insert("prompt_fragment_enabled".to_string(), enabled.clone());
        obj.insert("prompt_fragments".to_string(), fragments.clone());
        let prompt = compose_prompt_from_state(enabled, fragments);
        obj.insert("composed_prompt".to_string(), Value::String(prompt));
    }
    persist_context(state, run_id, &run.context).await?;
    Ok(())
}
