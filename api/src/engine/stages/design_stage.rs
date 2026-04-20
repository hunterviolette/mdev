use anyhow::Result;
use serde_json::{json, Value};

use crate::{
    engine::capabilities::inference::stage_support::{prepare_inference_stage_state, InferenceStageSettings},
    models::WorkflowStepDefinition,
};

pub fn prepare_stage_state(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    local_state: Value,
) -> Result<Value> {
    let mut state = prepare_inference_stage_state(
        repo_ref,
        global_state,
        step,
        local_state,
        InferenceStageSettings {
            include_changeset_schema: false,
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
                "disposition": "stay",
                "message": "Design stage completed successfully through backend workflow engine.",
                "patch": {
                    "global_state": {
                        "capabilities": {
                            "inference": {
                                "connection_runtime": {
                                    "shared_stage_family:design_code": {
                                        "repo_context": {
                                            "has_fired": true,
                                            "active": true,
                                            "last_trigger": "design_success"
                                        }
                                    }
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
            json!({
                "disposition": "stay",
                "message": "Design stage failed during backend workflow execution."
            }),
        );
    }

    Ok(state)
}
