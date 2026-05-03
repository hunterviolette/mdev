use serde_json::{json, Value};

use crate::engine::capabilities::registry::CapabilityResult;

pub fn changeset_failure_files(result: &CapabilityResult) -> Vec<Value> {
    if result.capability != "gateway_model/changeset" {
        return Vec::new();
    }

    result
        .payload
        .get("failing_files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub fn changeset_touched_files(result: &CapabilityResult) -> Vec<String> {
    if result.capability != "gateway_model/changeset" {
        return Vec::new();
    }

    result
        .payload
        .get("touched_files")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub fn compile_failed(result: &CapabilityResult) -> bool {
    result.capability == "compile_commands" && !result.ok
}

pub fn compile_succeeded(result: &CapabilityResult) -> bool {
    result.capability == "compile_commands" && result.ok
}

pub fn default_guardrails() -> Value {
    json!({
        "changeset_context_inject_after_file_failures": 4,
        "changeset_pause_after_file_failures": 8,
        "compile_pause_after_failures": 5
    })
}
