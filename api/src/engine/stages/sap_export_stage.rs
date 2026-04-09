use anyhow::Result;
use serde_json::{json, Value};

use crate::models::WorkflowStepDefinition;

pub fn prepare_stage_state(
    step: &WorkflowStepDefinition,
    local_state: Value,
) -> Result<Value> {
    let mut state = ensure_object(local_state);
    let obj = state.as_object_mut().expect("stage state must be object");

    obj.entry("sap_execution".to_string()).or_insert_with(|| {
        json!({
            "stage": "sap_export",
            "mode": "export",
            "summary": "",
            "status_rows": [],
            "changed_paths": []
        })
    });

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
                "message": "SAP export stage completed successfully.",
                "patch_from_capability": {
                    "capability": "sap/export",
                    "mode": "sap_execution_state"
                }
            }),
        );
    }

    if !exec_obj.contains_key("on_error") {
        exec_obj.insert(
            "on_error".to_string(),
            json!({
                "disposition": "error",
                "message": "SAP export stage failed.",
                "patch_from_capability": {
                    "capability": "sap/export",
                    "mode": "sap_execution_state"
                }
            }),
        );
    }

    Ok(state)
}

pub fn build_sap_execution_patch(capability_results: &[Value]) -> Value {
    let export_result = capability_results
        .iter()
        .find(|item| item.get("key").and_then(Value::as_str) == Some("sap/export"))
        .and_then(|item| item.get("result"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let mode = export_result
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("export")
        .to_string();
    let summary = export_result
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let changed_paths = export_result
        .get("changed_paths")
        .cloned()
        .unwrap_or_else(|| json!([]));

    let status_rows = export_result
        .get("objects")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|object| {
            let object_name = object
                .get("object_name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let object_type = object
                .get("object_type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let manifest_path = object
                .get("manifest_path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();

            object
                .get("resources")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |resource| {
                    let path = resource
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let update = resource.get("update").cloned().unwrap_or_else(|| json!({}));
                    let syntax = resource.get("syntax").cloned().unwrap_or_else(|| json!({}));
                    let activation = object.get("activation").cloned().unwrap_or_else(|| json!({}));

                    let update_ok = update.get("ok").and_then(Value::as_bool).unwrap_or(false);
                    let syntax_ok = syntax.get("ok").and_then(Value::as_bool).unwrap_or(false);
                    let activation_attempted = activation.get("attempted").and_then(Value::as_bool).unwrap_or(false);
                    let activation_ok = activation.get("ok").and_then(Value::as_bool);

                    let mut error_messages = Vec::new();
                    for bucket in [update.get("problems"), syntax.get("problems"), activation.get("problems")] {
                        if let Some(items) = bucket.and_then(Value::as_array) {
                            for item in items {
                                if let Some(text) = item.as_str() {
                                    let trimmed = text.trim();
                                    if !trimmed.is_empty() {
                                        error_messages.push(trimmed.to_string());
                                    }
                                }
                            }
                        }
                    }

                    if error_messages.is_empty() {
                        for bucket in [update.get("body"), syntax.get("body"), activation.get("body")] {
                            if let Some(text) = bucket.and_then(Value::as_str) {
                                let trimmed = text.trim();
                                if !trimmed.is_empty() {
                                    error_messages.push(trimmed.to_string());
                                }
                            }
                        }
                    }

                    let status = if !update_ok {
                        "update_failed"
                    } else if !syntax_ok {
                        "syntax_failed"
                    } else if activation_attempted && activation_ok == Some(false) {
                        "activation_failed"
                    } else if activation_attempted && activation_ok == Some(true) {
                        "activated"
                    } else if syntax_ok {
                        "syntax_passed"
                    } else {
                        "pending"
                    };

                    json!({
                        "object_name": object_name,
                        "object_type": object_type,
                        "manifest_path": manifest_path,
                        "path": path,
                        "status": status,
                        "update_ok": update_ok,
                        "syntax_ok": syntax_ok,
                        "activation_attempted": activation_attempted,
                        "activation_ok": activation_ok,
                        "error_message": error_messages.join("\n")
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    json!({
        "local_state": {
            "sap_execution": {
                "stage": if mode == "syntax" { "sap_syntax" } else { "sap_export" },
                "mode": mode,
                "summary": summary,
                "changed_paths": changed_paths,
                "status_rows": status_rows
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
