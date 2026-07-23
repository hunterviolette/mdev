use anyhow::Result;
use serde_json::{json, Value};

use crate::{
    engine::stages::capability_contract::StageCapabilities,
    models::WorkflowStepDefinition,
};

pub fn capabilities() -> StageCapabilities {
    StageCapabilities::new(["shared_dependencies", "compile_commands"])
}

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

    let compile_commands = resolved_compile_commands(
        &state,
        step.execution.compile_checks.clone(),
        step.execution_logic.clone(),
    );
    let has_compile_commands = compile_commands_present(&compile_commands);

    let obj = state.as_object_mut().expect("stage state must be object");

    let execution_state = obj
        .entry("execution".to_string())
        .or_insert_with(|| json!({}));
    if !execution_state.is_object() {
        *execution_state = json!({});
    }
    let execution_state_obj = execution_state.as_object_mut().expect("execution must be object");

    let compile_checks = execution_state_obj
        .entry("compile_checks".to_string())
        .or_insert_with(|| json!({}));
    if !compile_checks.is_object() {
        *compile_checks = json!({});
    }
    let compile_checks_obj = compile_checks.as_object_mut().expect("compile_checks must be object");

    if has_compile_commands {
        compile_checks_obj.insert("commands".to_string(), compile_commands.clone());
    }

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
                    "message": "Compile stage completed successfully through backend workflow engine."
                })
            },
        );
    }

    if !exec_obj.contains_key("on_error") {
        exec_obj.insert(
            "on_error".to_string(),
            json!({
                "disposition": "move_back",
                "message": "Compile stage failed during backend workflow execution."
            }),
        );
    }

    Ok(state)
}

fn compile_commands_present(commands: &Value) -> bool {
    commands
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

fn resolved_compile_commands(local_state: &Value, step_compile_checks: Value, execution_logic: Value) -> Value {
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
        .or_else(|| commands_text_to_rows(
            local_state
                .get("execution")
                .and_then(|v| v.get("compile_checks"))
                .and_then(|v| v.get("commands_text")),
        ))
        .or_else(|| step_compile_checks.get("commands").cloned())
        .or_else(|| commands_text_to_rows(step_compile_checks.get("commands_text")))
        .or_else(|| {
            local_state
                .get("execution_logic")
                .and_then(|v| v.get("compile_checks"))
                .and_then(|v| v.get("commands"))
                .cloned()
        })
        .or_else(|| commands_text_to_rows(
            local_state
                .get("execution_logic")
                .and_then(|v| v.get("compile_checks"))
                .and_then(|v| v.get("commands_text")),
        ))
        .or_else(|| {
            execution_logic
                .get("compile_checks")
                .and_then(|v| v.get("commands"))
                .cloned()
        })
        .or_else(|| commands_text_to_rows(execution_logic.get("compile_checks").and_then(|v| v.get("commands_text"))))
        .unwrap_or_else(|| json!([]))
}

fn commands_text_to_rows(value: Option<&Value>) -> Option<Value> {
    let text = value.and_then(Value::as_str)?.trim();
    if text.is_empty() {
        return None;
    }

    let rows = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|command| Value::String(command.to_string()))
        .collect::<Vec<_>>();

    if rows.is_empty() {
        None
    } else {
        Some(Value::Array(rows))
    }
}


fn ensure_object(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    }
}
