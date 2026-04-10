use anyhow::Result;
use serde_json::{json, Value};

use crate::models::WorkflowStepDefinition;

pub fn prepare_stage_state(
    step: &WorkflowStepDefinition,
    local_state: Value,
) -> Result<Value> {
    let mut state = ensure_object(local_state);
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
                "message": "SAP syntax stage completed successfully.",
                "patch_from_capability": {
                    "capability": "sap/export",
                    "mode": "sap_syntax_success_state"
                }
            }),
        );
    }

    if !exec_obj.contains_key("on_error") {
        exec_obj.insert(
            "on_error".to_string(),
            json!({
                "disposition": "move_back",
                "message": "SAP syntax stage failed. Return to code and fix the SAP syntax errors.",
                "patch_from_capability": {
                    "capability": "sap/export",
                    "mode": "sap_syntax_error_to_code_prompt"
                }
            }),
        );
    }

    Ok(state)
}

pub fn build_sap_syntax_success_patch(capability_results: &[Value]) -> Value {
    let export_result = find_export_result(capability_results);
    json!({
        "local_state": {
            "sap_execution": build_sap_execution_state(&export_result, "sap_syntax")
        },
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
    })
}

pub fn build_sap_syntax_error_patch(capability_results: &[Value]) -> Value {
    let export_result = find_export_result(capability_results);
    let sap_execution = build_sap_execution_state(&export_result, "sap_syntax");
    let fragment = build_sap_syntax_error_fragment(&export_result);

    json!({
        "local_state": {
            "sap_execution": sap_execution
        },
        "global_state": {
            "capabilities": {
                "sap/export": {
                    "latest_sap_syntax_error": {
                        "summary": export_result.get("summary").and_then(Value::as_str).unwrap_or("SAP syntax validation failed."),
                        "objects": export_result.get("objects").cloned().unwrap_or_else(|| json!([]))
                    }
                },
                "inference": {
                    "prompt_fragment_enabled": {
                        "compile_error": true
                    },
                    "prompt_fragments": {
                        "compile_error": fragment
                    }
                }
            }
        }
    })
}

fn find_export_result(capability_results: &[Value]) -> Value {
    capability_results
        .iter()
        .find(|item| item.get("key").and_then(Value::as_str) == Some("sap/export"))
        .and_then(|item| item.get("result"))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

fn build_sap_execution_state(export_result: &Value, stage: &str) -> Value {
    let mode = export_result
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("syntax")
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

                    let update_ok = update.get("ok").and_then(Value::as_bool).unwrap_or(false);
                    let syntax_ok = syntax.get("ok").and_then(Value::as_bool).unwrap_or(false);

                    let error_message = collect_error_lines(&[update.clone(), syntax.clone()]).join("\n");
                    let status = if !update_ok {
                        "update_failed"
                    } else if !syntax_ok {
                        "syntax_failed"
                    } else {
                        "syntax_passed"
                    };

                    json!({
                        "object_name": object_name,
                        "object_type": object_type,
                        "manifest_path": manifest_path,
                        "path": path,
                        "status": status,
                        "update_ok": update_ok,
                        "syntax_ok": syntax_ok,
                        "error_message": error_message
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    json!({
        "stage": stage,
        "mode": mode,
        "summary": summary,
        "changed_paths": changed_paths,
        "status_rows": status_rows
    })
}

fn build_sap_syntax_error_fragment(export_result: &Value) -> String {
    let rows = export_result
        .get("objects")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|object| {
            let object_name = object
                .get("object_name")
                .and_then(Value::as_str)
                .unwrap_or("object")
                .to_string();
            let object_type = object
                .get("object_type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();

            object
                .get("resources")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(move |resource| {
                    let path = resource
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let update = resource.get("update").cloned().unwrap_or_else(|| json!({}));
                    let syntax = resource.get("syntax").cloned().unwrap_or_else(|| json!({}));
                    let update_ok = update.get("ok").and_then(Value::as_bool).unwrap_or(false);
                    let syntax_ok = syntax.get("ok").and_then(Value::as_bool).unwrap_or(false);
                    if update_ok && syntax_ok {
                        return None;
                    }

                    let errors = collect_error_lines(&[update.clone(), syntax.clone()]);
                    let header = format!("FILE: {} | OBJECT: {} {} | UPDATE_OK: {} | SYNTAX_OK: {}", path, object_type, object_name, update_ok, syntax_ok);
                    let mut lines = vec![header];
                    if errors.is_empty() {
                        lines.push("- Unknown SAP syntax/update error".to_string());
                    } else {
                        lines.extend(errors.into_iter().map(|error| format!("- {}", error)));
                    }
                    Some(lines.join("\n"))
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    format!(
        "{}\n\nPlease provide a NEW ChangeSet JSON (version 1) that fixes the SAP syntax errors.",
        rows.join("\n\n")
    )
}

fn collect_error_lines(entries: &[Value]) -> Vec<String> {
    let mut out = Vec::new();
    for entry in entries {
        if let Some(items) = entry.get("problems").and_then(Value::as_array) {
            for item in items {
                if let Some(text) = item.as_str() {
                    for line in sanitize_error_text(text) {
                        if !line.is_empty() && !out.contains(&line) {
                            out.push(line);
                        }
                    }
                }
            }
        }
    }
    if out.is_empty() {
        for entry in entries {
            if let Some(text) = entry.get("body").and_then(Value::as_str) {
                for line in sanitize_error_text(text) {
                    if !line.is_empty() && !out.contains(&line) {
                        out.push(line);
                    }
                }
            }
        }
    }
    out
}

fn sanitize_error_text(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    if trimmed.starts_with("<?xml") || trimmed.contains("<chkrun:checkMessage") {
        return extract_checkrun_error_lines(trimmed);
    }

    trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&"))
        .collect()
}

fn extract_checkrun_error_lines(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let re = regex::Regex::new(r#"<chkrun:checkMessage\b[^>]*chkrun:uri=\"([^\"]*)\"[^>]*chkrun:(?:shortText|text)=\"([^\"]*)\"[^>]*/?>"#).ok();
    if let Some(re) = re {
        for caps in re.captures_iter(xml) {
            let uri = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let text = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let rendered = decode_xml_entities(text);
            let location = parse_checkrun_location(uri);
            let line = if location.is_empty() {
                rendered
            } else {
                format!("{}: {}", location, rendered)
            };
            if !line.trim().is_empty() && !out.contains(&line) {
                out.push(line);
            }
        }
    }
    if out.is_empty() {
        let fallback = decode_xml_entities(xml);
        let compact = fallback.split_whitespace().collect::<Vec<_>>().join(" ");
        if !compact.is_empty() {
            out.push(compact);
        }
    }
    out
}

fn parse_checkrun_location(uri: &str) -> String {
    let re = regex::Regex::new(r#"#start=(\d+),(\d+)"#).ok();
    if let Some(re) = re {
        if let Some(caps) = re.captures(uri) {
            let line = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let column = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if !line.is_empty() && !column.is_empty() {
                return format!("line {}, col {}", line, column);
            }
        }
    }
    String::new()
}

fn decode_xml_entities(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}


fn ensure_object(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    }
}
