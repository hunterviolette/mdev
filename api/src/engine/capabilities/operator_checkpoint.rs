use std::sync::Arc;

use anyhow::Result;
use dashmap::DashMap;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use uuid::Uuid;

use super::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult};

#[derive(Debug, Clone)]
pub struct OperatorInputResponse {
    pub disposition: String,
    pub selected_step_id: Option<String>,
}

#[derive(Clone, Default)]
pub struct OperatorInputRegistry {
    waiters: Arc<DashMap<Uuid, oneshot::Sender<OperatorInputResponse>>>,
}

impl OperatorInputRegistry {
    pub fn register(&self, run_id: Uuid) -> Option<oneshot::Receiver<OperatorInputResponse>> {
        if self.waiters.contains_key(&run_id) {
            return None;
        }

        let (sender, receiver) = oneshot::channel();
        self.waiters.insert(run_id, sender);
        Some(receiver)
    }

    pub fn resolve(&self, run_id: Uuid, response: OperatorInputResponse) -> bool {
        self.waiters
            .remove(&run_id)
            .is_some_and(|(_, sender)| sender.send(response).is_ok())
    }

    pub fn clear(&self, run_id: Uuid) {
        self.waiters.remove(&run_id);
    }
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).filter(|item| !item.trim().is_empty())
}

fn normalize_checkpoint_option(value: &str) -> Option<&'static str> {
    match value {
        "continue_auto" | "auto" | "autonomous" | "move_next" | "continue" => Some("continue_auto"),
        "select_stage" | "select" | "continue_manual" | "manual" => Some("select_stage"),
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

    let previous_failed = latest_result.map(|result| !result.ok).unwrap_or(false);

    let configured_options = config
        .get("available_dispositions")
        .or_else(|| config.get("options"));
    let options = normalize_options(configured_options);

    let recommended = if previous_failed {
        "pause_error".to_string()
    } else {
        normalize_recommended(
            string_field(&config, "recommended_disposition")
                .or_else(|| string_field(&config, "disposition"))
                .or_else(|| latest_payload.get("disposition").and_then(Value::as_str)),
        )
    };

    let message = if previous_failed {
        latest_payload
            .get("summary")
            .and_then(Value::as_str)
            .or_else(|| latest_payload.get("message").and_then(Value::as_str))
            .map(|value| format!("The previous capability failed: {}", value))
            .unwrap_or_else(|| "The previous capability failed. Select how the workflow should proceed.".to_string())
    } else {
        string_field(&config, "message")
            .or_else(|| latest_payload.get("summary").and_then(Value::as_str))
            .or_else(|| latest_payload.get("message").and_then(Value::as_str))
            .unwrap_or("Operator checkpoint is waiting for a disposition.")
            .to_string()
    };

    let response = ctx
        .request_user_input(message.clone(), recommended.clone(), options.clone())
        .await?;

    Ok(CapabilityResult {
        ok: true,
        capability: "operator_checkpoint".to_string(),
        payload: json!({
            "ok": true,
            "mode": "operator_checkpoint",
            "status": if response.disposition == "pause_error" { "paused" } else { "success" },
            "needs_user_response": false,
            "summary": if response.disposition == "pause_error" {
                "Operator paused the workflow."
            } else {
                "Operator checkpoint resolved."
            },
            "message": message,
            "previous_capability_failed": previous_failed,
            "stage_id": ctx.step.id,
            "stage_type": ctx.step.step_type,
            "recommended_disposition": recommended,
            "available_dispositions": options,
            "disposition": response.disposition,
            "selected_step_id": response.selected_step_id,
            "prior_result": latest_payload
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}
