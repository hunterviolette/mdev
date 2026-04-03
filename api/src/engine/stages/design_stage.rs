use anyhow::Result;
use serde_json::{json, Value};

use crate::{
    engine::{
        capabilities::inference::stage_support::{prepare_inference_stage_state, InferenceStageSettings},
        stages::pause_on_enter,
    },
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
                "disposition": if pause_on_enter(step) { "paused" } else { "success" },
                "message": if pause_on_enter(step) {
                    "Design stage completed and is paused."
                } else {
                    "Design stage completed successfully through backend workflow engine."
                }
            }),
        );
    }

    if !exec_obj.contains_key("on_error") {
        exec_obj.insert(
            "on_error".to_string(),
            json!({
                "disposition": "error",
                "message": "Design stage failed during backend workflow execution."
            }),
        );
    }

    Ok(state)
}
