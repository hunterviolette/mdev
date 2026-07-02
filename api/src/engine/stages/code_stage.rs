use anyhow::Result;
use serde_json::{json, Value};

use crate::{
    engine::{
        capabilities::inference::stage_support::{
            auto_apply_enabled,
            prepare_inference_stage_state_with_hooks,
            InferenceStageHooks,
            InferenceStageSettings,
        },
        stages::capability_contract::StageCapabilities,
    },
    models::WorkflowStepDefinition,
};

pub fn capabilities() -> StageCapabilities {
    StageCapabilities::new(["inference", "changeset"])
}

pub fn prepare_stage_state(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    local_state: Value,
) -> Result<Value> {
    let auto_apply = auto_apply_enabled(step, &local_state);
    let mut state = prepare_inference_stage_state_with_hooks(
        repo_ref,
        global_state,
        step,
        local_state,
        InferenceStageSettings {
            include_changeset_schema: step.prompt.include_changeset_schema,
        },
        InferenceStageHooks {
            empty_user_input_default: Some(
                "please provide changeset in a codeblock with no comments to align coding".to_string(),
            ),
        },
    )?;

    let obj = state.as_object_mut().expect("stage state must be object");
    let execution_logic = obj
        .entry("execution_logic".to_string())
        .or_insert_with(|| step.execution_logic.clone());
    if !execution_logic.is_object() {
        *execution_logic = json!({});
    }
    let exec_obj = execution_logic.as_object_mut().expect("execution_logic must be object");

    if !exec_obj.contains_key("on_success") {
        exec_obj.insert(
            "on_success".to_string(),
            json!({
                "disposition": "move_next",
                "message": "Code stage completed successfully through backend workflow engine."
            }),
        );
    }

    if !exec_obj.contains_key("on_error") {
        exec_obj.insert(
            "on_error".to_string(),
            if auto_apply {
                json!({
                    "disposition": "retry_stage",
                    "message": "Code stage apply failed; retry the code stage with the apply error included in the prompt.",
                    "patch_from_capability": {
                        "capability": "gateway_model/changeset",
                        "mode": "apply_error_to_code_prompt"
                    }
                })
            } else {
                json!({
                    "disposition": "retry_stage",
                    "message": "Code stage failed during backend workflow execution."
                })
            },
        );
    }

    Ok(state)
}

pub fn build_apply_error_patch(capability_results: &[Value]) -> Value {
    let apply_result = capability_results
        .iter()
        .find(|item| item.get("key").and_then(Value::as_str) == Some("gateway_model/changeset"))
        .and_then(|item| item.get("result"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let summary = apply_result
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("ChangeSet apply failed.")
        .to_string();

    let lines = apply_result
        .get("lines")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let detail = if summary.contains("no JSON object found in changeset payload") {
        summary.clone()
    } else if lines.is_empty() {
        summary.clone()
    } else {
        format!("{}\n\n{}", summary, lines.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))
    };

    let fragment = format!(
        "{}\n\nPlease provide a NEW ChangeSet JSON (version 1) that fixes the apply errors.",
        detail
    );

    json!({
        "global_state": {
            "capabilities": {
                "inference": {
                    "next_prompt_fragments": [
                        {
                            "text": fragment
                        }
                    ]
                }
            }
        }
    })
}
