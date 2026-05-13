use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::models::WorkflowStepDefinition;

use super::models::{FeaturePlanItem, FeaturePlanItemStatus};

pub fn extract_inference_text(capability_results: &[Value]) -> Option<String> {
    capability_results
        .iter()
        .find(|item| item.get("key").and_then(Value::as_str) == Some("inference") || item.get("capability").and_then(Value::as_str) == Some("inference"))
        .and_then(|item| item.get("payload").or_else(|| item.get("result")).cloned())
        .and_then(|payload| {
            payload
                .get("result")
                .and_then(|result| result.get("text"))
                .and_then(Value::as_str)
                .or_else(|| payload.get("text").and_then(Value::as_str))
                .or_else(|| payload.get("output").and_then(Value::as_str))
                .map(ToString::to_string)
        })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictRefinedFeatureEnvelope {
    feature: StrictRefinedFeaturePayload,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictRefinedFeaturePayload {
    summary: String,
    requirements: Vec<String>,
    acceptance_criteria: Vec<String>,
    implementation_notes: Vec<String>,
    review_expectations: Vec<String>,
    target_files_or_areas: Vec<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

fn extract_json_object_slice(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, &byte) in bytes.iter().enumerate() {
        let ch = byte as char;

        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => {
                if start.is_none() {
                    start = Some(idx);
                }
                depth += 1;
            }
            '}' => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0 {
                    if let Some(start_idx) = start {
                        return Some(&text[start_idx..=idx]);
                    }
                }
            }
            _ => {}
        }
    }

    None
}

fn normalize_refined_feature_payload_text(payload_text: &str) -> Result<String> {
    let mut text = payload_text.trim().to_string();

    if text.starts_with("```") {
        let mut lines = text.lines();
        let _ = lines.next();
        text = lines.collect::<Vec<_>>().join("\n");
        if let Some(idx) = text.rfind("```") {
            text.truncate(idx);
        }
        text = text.trim().to_string();
    }

    if text.is_empty() {
        return Err(anyhow!("refined feature payload was empty"));
    }

    let json_slice = extract_json_object_slice(&text)
        .ok_or_else(|| anyhow!("no JSON object found in refined feature payload"))?;

    Ok(json_slice.to_string())
}

pub fn normalize_refined_feature_plan_item(
    feature_id: &str,
    title: &str,
    rough_summary: Option<String>,
    text: &str,
) -> Result<FeaturePlanItem> {
    let json_text = normalize_refined_feature_payload_text(text)?;

    let envelope: StrictRefinedFeatureEnvelope = serde_json::from_str(&json_text)
        .map_err(|err| anyhow!("failed to parse strict refined feature JSON: {}", err))?;

    let feature = envelope.feature;
    let summary = feature.summary.trim().to_string();
    let requirements = clean_string_array("requirements", feature.requirements)?;
    let acceptance_criteria = clean_string_array("acceptance_criteria", feature.acceptance_criteria)?;
    let implementation_notes = clean_string_array("implementation_notes", feature.implementation_notes)?;
    let review_expectations = clean_string_array("review_expectations", feature.review_expectations)?;
    let target_files_or_areas = clean_string_array("target_files_or_areas", feature.target_files_or_areas)?;
    let dependencies = clean_string_array("dependencies", feature.dependencies)?;

    let status = if refined_feature_is_complete(
        &summary,
        &requirements,
        &acceptance_criteria,
        &implementation_notes,
        &review_expectations,
        &target_files_or_areas,
    ) {
        FeaturePlanItemStatus::Fine
    } else {
        FeaturePlanItemStatus::Rough
    };

    Ok(FeaturePlanItem {
        id: feature_id.to_string(),
        title: title.to_string(),
        status,
        summary,
        rough_summary,
        refinement_workflow_run_id: None,
        requirements,
        acceptance_criteria,
        implementation_notes,
        review_expectations,
        target_files_or_areas,
        dependencies,
    })
}

fn clean_string_array(field: &str, values: Vec<String>) -> Result<Vec<String>> {
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let value = value.trim().to_string();
            if value.is_empty() {
                Err(anyhow!("{}.{} must not be empty", field, index))
            } else {
                Ok(value)
            }
        })
        .collect()
}

fn refined_feature_is_complete(
    summary: &str,
    requirements: &[String],
    acceptance_criteria: &[String],
    implementation_notes: &[String],
    review_expectations: &[String],
    target_files_or_areas: &[String],
) -> bool {
    !summary.trim().is_empty()
        && !requirements.is_empty()
        && !acceptance_criteria.is_empty()
        && !implementation_notes.is_empty()
        && !review_expectations.is_empty()
        && !target_files_or_areas.is_empty()
}

pub fn normalize_planner_features(step: &mut WorkflowStepDefinition, global_state: &Value, _repo_ref: &str) {
    if step.step_type != "design" && step.step_type != "code" {
        return;
    }

    let planner = global_state
        .get("capabilities")
        .and_then(|value| value.get("planner"));

    if !planner
        .and_then(|value| value.get("fragment_armed"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return;
    }

    if step.step_type == "design" {
        let _ = set_path(&mut step.config, "design_mode", Value::String("v2".to_string()));
        let _ = set_path(&mut step.execution_logic, "automation.disposition_review.enabled", Value::Bool(true));
        let _ = set_path(
            &mut step.execution_logic,
            "automation.disposition_review.available_dispositions",
            json!(["move_next", "pause"]),
        );
    }
}

fn set_path(root: &mut Value, path: &str, value: Value) -> std::result::Result<(), String> {
    let parts: Vec<&str> = path.split('.').filter(|part| !part.trim().is_empty()).collect();
    if parts.is_empty() {
        return Err("path cannot be empty".to_string());
    }

    let mut cursor = root;
    for part in &parts[..parts.len() - 1] {
        if !cursor.is_object() {
            *cursor = serde_json::json!({});
        }
        let obj = cursor.as_object_mut().ok_or_else(|| format!("{} is not an object", part))?;
        cursor = obj.entry((*part).to_string()).or_insert_with(|| serde_json::json!({}));
    }

    if !cursor.is_object() {
        *cursor = serde_json::json!({});
    }
    let obj = cursor.as_object_mut().ok_or_else(|| "target is not an object".to_string())?;
    obj.insert(parts[parts.len() - 1].to_string(), value);
    Ok(())
}
