use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    git::git::{generate_git_apply_patch, GitPatchScope},
    inference::model_output::clean_model_json_value,
    registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult},
};

#[derive(Debug, Deserialize)]
struct ReviewModelDecision {
    status: String,
    #[serde(default)]
    reason: String,
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    prior_results: &[CapabilityResult],
    _config: Value,
) -> Result<CapabilityResult> {
    let ai_review_required = ctx
        .local_state
        .get("review")
        .and_then(|v| v.get("ai_review_required"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let inference_result = prior_results
        .iter()
        .rev()
        .find(|item| item.capability == "inference" && item.ok)
        .and_then(|item| item.payload.get("result"))
        .and_then(|v| v.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    if inference_result.is_empty() {
        if ai_review_required {
            return Ok(CapabilityResult {
                ok: false,
                capability: "review_validation".to_string(),
                payload: json!({
                    "ok": false,
                    "mode": "supervisor_v2_review",
                    "status": "fail",
                    "summary": "Supervisor review did not produce an inference result.",
                    "reason": "The review stage expected an AI review because the planner fragment was armed, but no inference result was available.",
                    "disposition": "pause"
                }),
                follow_ups: CapabilityInvocationRequest::None,
            });
        }
        return fallback_manual_review(ctx).await;
    }

    let cleaned = clean_model_json_value(&inference_result)
        .context("failed to clean supervisor review model output")?;
    let decision: ReviewModelDecision = serde_json::from_value(cleaned.clone())
        .context("failed to decode supervisor review decision")?;

    let status = decision.status.trim().to_ascii_lowercase();
    if status != "pass" && status != "fail" {
        bail!("supervisor review status must be pass or fail");
    }

    let passed = status == "pass";
    let reason = decision.reason.trim().to_string();

    Ok(CapabilityResult {
        ok: passed,
        capability: "review_validation".to_string(),
        payload: json!({
            "ok": passed,
            "mode": "supervisor_v2_review",
            "status": status,
            "summary": if passed {
                "Supervisor review passed."
            } else {
                "Supervisor review failed."
            },
            "reason": reason,
            "model_output": cleaned,
            "disposition": if passed { "move_next" } else { "pause" }
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}

async fn fallback_manual_review(ctx: &CapabilityContext<'_>) -> Result<CapabilityResult> {
    let source_control = ctx
        .local_state
        .get("review")
        .and_then(|v| v.get("source_control"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let context_lines = source_control
        .get("context_lines")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .min(100) as u32;

    let patch = generate_git_apply_patch(
        std::path::Path::new(ctx.repo_ref),
        GitPatchScope::Unstaged,
        None,
        Some(context_lines),
    )?;

    Ok(CapabilityResult {
        ok: true,
        capability: "review_validation".to_string(),
        payload: json!({
            "ok": true,
            "mode": "manual_review",
            "status": "pass",
            "summary": "Manual review fallback completed.",
            "reason": "No supervisor v2 review inference was required for this stage.",
            "diff": patch,
            "disposition": "move_next"
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}
