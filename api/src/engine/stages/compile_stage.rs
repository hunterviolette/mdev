use anyhow::Result;
use serde_json::{json, Value};

use crate::models::WorkflowStepDefinition;

pub fn prepare_stage_state(
    step: &WorkflowStepDefinition,
    local_state: Value,
) -> Result<Value> {
    let mut state = ensure_object(local_state);
    let is_automatic = state
        .get("execution")
        .and_then(|v| v.get("mode"))
        .and_then(Value::as_str)
        .map(|value| value.eq_ignore_ascii_case("automatic"))
        .unwrap_or(false);
    let has_compile_commands = compile_commands_present(&state, step.execution_logic.clone());

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
            if is_automatic && !has_compile_commands {
                json!({
                    "disposition": "paused",
                    "message": "Compile stage reached with no compile commands configured. Paused for manual intervention."
                })
            } else {
                json!({
                    "disposition": "move_next",
                    "message": "Compile stage completed successfully through backend workflow engine.",
                    "patch": {
                        "global_state": {
                            "capabilities": {
                                "inference": {
                                    "prompt_fragment_enabled": {
                                        "compile_error": false
                                    },
                                    "prompt_fragments": {
                                        "compile_error": null
                                    }
                                }
                            }
                        }
                    }
                })
            },
        );
    }

    if !exec_obj.contains_key("on_error") {
        exec_obj.insert(
            "on_error".to_string(),
            json!({
                "disposition": "move_back",
                "message": "Compile stage failed during backend workflow execution.",
                "patch_from_capability": {
                    "capability": "compile_commands",
                    "mode": "compile_error_to_code_prompt"
                }
            }),
        );
    }

    Ok(state)
}

fn compile_commands_present(local_state: &Value, execution_logic: Value) -> bool {
    resolved_compile_commands(local_state, execution_logic)
        .as_array()
        .map(|rows| {
            rows.iter().any(|item| match item {
                Value::String(command) => !command.trim().is_empty(),
                Value::Object(obj) => obj
                    .get("command")
                    .and_then(Value::as_str)
                    .map(|command| !command.trim().is_empty())
                    .unwrap_or(false),
                _ => false,
            })
        })
        .unwrap_or(false)
}

fn resolved_compile_commands(local_state: &Value, execution_logic: Value) -> Value {
    local_state
        .get("capabilities")
        .and_then(|v| v.get("compile_commands"))
        .and_then(|v| v.get("commands"))
        .cloned()
        .or_else(|| {
            local_state
                .get("execution")
                .and_then(|v| v.get("compile_checks"))
                .and_then(|v| v.get("commands"))
                .cloned()
        })
        .or_else(|| {
            local_state
                .get("execution_logic")
                .and_then(|v| v.get("compile_checks"))
                .and_then(|v| v.get("commands"))
                .cloned()
        })
        .or_else(|| {
            execution_logic
                .get("compile_checks")
                .and_then(|v| v.get("commands"))
                .cloned()
        })
        .unwrap_or_else(|| json!([]))
}

pub fn build_compile_error_patch(capability_results: &[Value]) -> Value {
    let outputs = capability_results
        .iter()
        .filter(|item| item.get("key").and_then(Value::as_str) == Some("compile_commands"))
        .filter_map(|item| item.get("result"))
        .filter_map(|result| result.get("results"))
        .filter_map(Value::as_array)
        .flat_map(|items| items.iter())
        .map(|row| {
            let label = row.get("label").and_then(Value::as_str).unwrap_or("command");
            let status = row.get("status").and_then(Value::as_i64).unwrap_or(-1);
            let stdout = row.get("stdout").and_then(Value::as_str).unwrap_or("");
            let stderr = row.get("stderr").and_then(Value::as_str).unwrap_or("");
            format!(
                "COMMAND: {}\nSTATUS: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
                label,
                status,
                stdout,
                stderr
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let compile_fragment = format!(
        "Postprocess command failed after applying the previous ChangeSet.\n\nPOSTPROCESS OUTPUT:\n{}\n\nPlease provide a NEW ChangeSet JSON (version 1) that fixes the errors.",
        outputs
    );

    json!({
        "global_state": {
            "capabilities": {
                "inference": {
                    "next_prompt_fragments": [
                        {
                            "text": compile_fragment
                        }
                    ]
                }
            }
        }
    })
}

fn ensure_object(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    }
}
