use anyhow::Result;
use serde_json::{json, Value};

use crate::{
    engine::capabilities::inference::stage_support::{
        auto_apply_enabled,
        prepare_inference_stage_state,
        InferenceStageSettings,
    },
    models::WorkflowStepDefinition,
};

pub fn prepare_stage_state(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    local_state: Value,
) -> Result<Value> {
    let auto_apply = auto_apply_enabled(step, &local_state);
    let mut state = prepare_inference_stage_state(
        repo_ref,
        global_state,
        step,
        local_state,
        InferenceStageSettings {
            include_changeset_schema: step.prompt.include_changeset_schema,
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
                "disposition": "success",
                "message": "Code stage completed successfully through backend workflow engine.",
                "patch": {
                    "global_state": {
                        "capabilities": {
                            "inference": {
                                "prompt_fragment_enabled": {
                                    "apply_error": false
                                },
                                "prompt_fragments": {
                                    "apply_error": null
                                }
                            }
                        }
                    }
                }
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
                    "disposition": "error",
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

    let lines = apply_result
        .get("lines")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            apply_result
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("ChangeSet apply failed.")
                .to_string()
        });

    let apply_fragment = format!(
        "ChangeSet apply failed.\n\n{}\n\nPlease provide a NEW ChangeSet JSON (version 1) that fixes the apply errors.",
        lines
    );

    json!({
        "global_state": {
            "capabilities": {
                "inference": {
                    "prompt_fragment_enabled": {
                        "apply_error": true
                    },
                    "prompt_fragments": {
                        "apply_error": apply_fragment
                    }
                }
            }
        }
    })
}
