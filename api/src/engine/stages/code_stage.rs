use anyhow::Result;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    models::{StageExecutionNode, WorkflowStepDefinition},
};

use crate::engine::capabilities::{execute_capability_invocations, CapabilityContext, CapabilityInvocation};
use crate::engine::{load_run, persist_context};

use super::{compose_prompt_from_state, normalize_repo_context_payload, StageDisposition, StageOutcome};

pub async fn execute_code_stage(
    state: &AppState,
    run_id: Uuid,
    repo_ref: &str,
    step: &WorkflowStepDefinition,
    local_state: &Value,
    plan: &[StageExecutionNode],
) -> Result<StageOutcome> {
    let enabled = local_state
        .get("prompt_fragment_enabled")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let fragments = local_state
        .get("prompt_fragments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let include_repo_context = enabled
        .get("repo_context")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let repo_context = normalize_repo_context_payload(repo_ref, local_state.get("repo_context").cloned());

    let plan_invocations: Vec<CapabilityInvocation> = plan
        .iter()
        .filter(|node| node.enabled && node.kind == crate::models::StageExecutionNodeKind::Capability)
        .map(|node| CapabilityInvocation {
            capability: node.key.clone(),
            config: node.config.clone(),
        })
        .collect();

    let prompt = compose_prompt_from_state(&enabled, &fragments);
    let mut stage_local_state = local_state.clone();
    if let Some(obj) = stage_local_state.as_object_mut() {
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
        local_state: &stage_local_state,
    };

    let queue = if plan_invocations.is_empty() {
        vec![CapabilityInvocation {
            capability: "inference".to_string(),
            config: json!({}),
        }]
    } else {
        plan_invocations
    };

    let capability_chain = execute_capability_invocations(capability_ctx, queue).await?;
    tracing::info!(
        run_id = %run_id,
        step_id = %step.id,
        capability_count = capability_chain.len(),
        "code stage capability chain completed"
    );
    let capability_results: Vec<Value> = capability_chain
        .iter()
        .map(|item| {
            json!({
                "key": item.capability,
                "ok": item.ok,
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

    tracing::info!(
        run_id = %run_id,
        step_id = %step.id,
        payload_len = payload_text.len(),
        has_gateway_apply = capability_chain.iter().any(|item| item.capability == "gateway_model/changeset"),
        "code stage inference payload evaluated"
    );

    if payload_text.trim().is_empty() {
        return Ok(StageOutcome {
            ok: false,
            disposition: StageDisposition::ErrorCode("empty_changeset_payload".to_string()),
            message: "Inference returned an empty ChangeSet payload.".to_string(),
            capability_results,
            local_state: stage_local_state,
        });
    }

    if let Some(apply_json) = capability_chain
        .iter()
        .find(|item| item.capability == "gateway_model/changeset")
        .map(|item| item.payload.clone())
    {
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

            let apply_message = format!(
                "ChangeSet apply failed.\n\n{}\n\nPlease provide a NEW ChangeSet JSON (version 1) that fixes the apply errors.",
                lines
            );

            tracing::warn!(
                run_id = %run_id,
                step_id = %step.id,
                apply_error = %lines,
                "code stage changeset apply failed"
            );
            let apply_fragment = Value::String(apply_message);
            persist_stage_fragment(state, run_id, &step.id, "apply_error", apply_fragment).await?;

            return Ok(StageOutcome {
                ok: false,
                disposition: StageDisposition::RetryStage,
                message: "Code stage apply failed; retry the code stage with the apply error included in the prompt.".to_string(),
                capability_results,
                local_state: stage_local_state,
            });
        }
    }

    Ok(StageOutcome {
        ok: true,
        disposition: StageDisposition::Success,
        message: "Code stage completed successfully through backend workflow engine.".to_string(),
        capability_results,
        local_state: stage_local_state,
    })
}

async fn persist_stage_fragment(
    state: &AppState,
    run_id: Uuid,
    step_id: &str,
    fragment_key: &str,
    fragment_value: Value,
) -> Result<()> {
async fn clear_stage_fragment(
    state: &AppState,
    run_id: Uuid,
    step_id: &str,
    fragment_key: &str,
) -> Result<()> {
    let mut run = load_run(state, run_id).await?;
    let root = super::ensure_engine_root(&mut run.context);
    let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
    let stage_state_obj = stage_state.as_object_mut().expect("stage_state must be object");
    let existing = stage_state_obj.entry(step_id.to_string()).or_insert_with(|| json!({}));
    let obj = existing.as_object_mut().expect("stage state must be object");

    {
        let enabled = obj
            .entry("prompt_fragment_enabled".to_string())
            .or_insert_with(|| json!({}));
        if !enabled.is_object() {
            *enabled = json!({});
        }
        enabled
            .as_object_mut()
            .expect("prompt_fragment_enabled must be object")
            .insert(fragment_key.to_string(), Value::Bool(false));
    }

    {
        let fragments = obj
            .entry("prompt_fragments".to_string())
            .or_insert_with(|| json!({}));
        if !fragments.is_object() {
            *fragments = json!({});
        }
        fragments
            .as_object_mut()
            .expect("prompt_fragments must be object")
            .remove(fragment_key);
    }

    let prompt = compose_prompt_from_state(
        obj.get("prompt_fragment_enabled").unwrap_or(&Value::Null),
        obj.get("prompt_fragments").unwrap_or(&Value::Null),
    );
    obj.insert("composed_prompt".to_string(), Value::String(prompt));

    persist_context(state, run_id, &run.context).await?;
    Ok(())
}

    let mut run = load_run(state, run_id).await?;
    let root = super::ensure_engine_root(&mut run.context);
    let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
    let stage_state_obj = stage_state.as_object_mut().expect("stage_state must be object");
    let existing = stage_state_obj.entry(step_id.to_string()).or_insert_with(|| json!({}));
    let obj = existing.as_object_mut().expect("stage state must be object");

    {
        let enabled = obj
            .entry("prompt_fragment_enabled".to_string())
            .or_insert_with(|| json!({}));
        if !enabled.is_object() {
            *enabled = json!({});
        }
        enabled
            .as_object_mut()
            .expect("prompt_fragment_enabled must be object")
            .insert(fragment_key.to_string(), Value::Bool(true));
    }

    {
        let fragments = obj
            .entry("prompt_fragments".to_string())
            .or_insert_with(|| json!({}));
        if !fragments.is_object() {
            *fragments = json!({});
        }
        fragments
            .as_object_mut()
            .expect("prompt_fragments must be object")
            .insert(fragment_key.to_string(), fragment_value);
    }

    let prompt = compose_prompt_from_state(
        obj.get("prompt_fragment_enabled").unwrap_or(&Value::Null),
        obj.get("prompt_fragments").unwrap_or(&Value::Null),
    );
    obj.insert("composed_prompt".to_string(), Value::String(prompt));

    persist_context(state, run_id, &run.context).await?;
    Ok(())
}
