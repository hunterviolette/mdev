use anyhow::Result;
use serde_json::{json, Value};

use super::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult};

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).filter(|item| !item.trim().is_empty())
}

fn normalize_checkpoint_option(value: &str) -> Option<&'static str> {
    match value {
        "continue_auto" | "auto" | "autonomous" => Some("continue_auto"),
        "select_stage" | "select" | "continue_manual" | "manual" => Some("select_stage"),
        "continue_auto" | "auto" | "autonomous" | "move_next" | "continue" => Some("continue_auto"),
        "pause_error" | "pause" | "paused" => Some("pause_error"),
        _ => None,
    }
}

fn normalize_options(value: Option<&Value>) -> Vec<String> {
    let options = value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter_map(normalize_checkpoint_option)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if options.is_empty() {
        vec![
            "continue_auto".to_string(),
            "select_stage".to_string(),
            "pause_error".to_string(),
        ]
    } else {
        options
    }
}

fn normalize_recommended(value: Option<&str>) -> String {
    value
        .and_then(normalize_checkpoint_option)
        .unwrap_or("continue_auto")
        .to_string()
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let latest_result = prior_results.last();
    let latest_payload = latest_result.map(|result| result.payload.clone()).unwrap_or_else(|| json!({}));

    if latest_result.map(|result| !result.ok).unwrap_or(false) {
        return Ok(CapabilityResult {
            ok: false,
            capability: "operator_checkpoint".to_string(),
            payload: json!({
                "ok": false,
                "mode": "operator_checkpoint",
                "status": "skipped",
                "needs_user_response": false,
                "summary": "Operator checkpoint skipped because the previous capability failed.",
                "message": "Operator checkpoint skipped because the previous capability failed.",
                "stage_id": ctx.step.id,
                "stage_type": ctx.step.step_type,
                "prior_result": latest_payload
            }),
            follow_ups: CapabilityInvocationRequest::None,
        });
    }

    let configured_options = config
        .get("available_dispositions")
        .or_else(|| config.get("options"));
    let options = normalize_options(configured_options);

    let recommended = normalize_recommended(
        string_field(&config, "recommended_disposition")
            .or_else(|| string_field(&config, "disposition"))
            .or_else(|| latest_payload.get("disposition").and_then(Value::as_str)),
    );

    let message = string_field(&config, "message")
        .or_else(|| latest_payload.get("summary").and_then(Value::as_str))
        .or_else(|| latest_payload.get("message").and_then(Value::as_str))
        .unwrap_or("Operator checkpoint is waiting for a disposition.")
        .to_string();

    Ok(CapabilityResult {
        ok: true,
        capability: "operator_checkpoint".to_string(),
        payload: json!({
            "ok": true,
            "mode": "operator_checkpoint",
            "status": "waiting",
            "needs_user_response": true,
            "summary": "Operator checkpoint is waiting for user input.",
            "message": message,
            "stage_id": ctx.step.id,
            "stage_type": ctx.step.step_type,
            "recommended_disposition": recommended,
            "available_dispositions": options,
            "prior_result": latest_payload,
            "response_options": {
                "continue_auto": {
                    "ok": true,
                    "resume_mode": "autonomous",
                    "label": "Continue automatically"
                },
                "select_stage": {
                    "ok": true,
                    "resume_mode": "manual",
                    "label": "Select stage"
                },
                "pause_error": {
                    "ok": false,
                    "resume_mode": "none",
                    "label": "Pause"
                }
            }
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}
