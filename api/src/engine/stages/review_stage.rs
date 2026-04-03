use anyhow::Result;
use serde_json::{json, Value};

use crate::models::WorkflowStepDefinition;

pub fn prepare_stage_state(
    step: &WorkflowStepDefinition,
    local_state: Value,
) -> Result<Value> {
    let mut state = ensure_object(local_state);
    let require_manual_approval = step
        .execution_logic
        .get("require_manual_approval")
        .and_then(Value::as_bool)
        .unwrap_or(true);

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
                "disposition": if require_manual_approval { "paused" } else { "success" },
                "message": if require_manual_approval {
                    "Review stage completed and is waiting for manual approval."
                } else {
                    "Review stage completed successfully."
                }
            }),
        );
    }

    if !exec_obj.contains_key("on_error") {
        exec_obj.insert(
            "on_error".to_string(),
            json!({
                "disposition": "error",
                "message": "Review stage failed during backend workflow execution."
            }),
        );
    }

    Ok(state)
}

fn ensure_object(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    }
}
