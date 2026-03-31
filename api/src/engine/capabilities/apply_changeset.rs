use anyhow::Result;
use serde_json::{json, Value};

use crate::executor;

use super::registry::{
    find_result,
    CapabilityContext,
    CapabilityInvocation,
    CapabilityInvocationRequest,
    CapabilityResult,
};

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    prior_results: &[CapabilityResult],
    _config: Value,
) -> Result<CapabilityResult> {
    let inference = find_result(prior_results, "inference");
    let payload_text = inference
        .and_then(|item| item.payload.get("result"))
        .and_then(|v| v.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if payload_text.trim().is_empty() {
        return Ok(CapabilityResult {
            ok: false,
            capability: "apply_changeset".to_string(),
            payload: json!({
                "ok": false,
                "summary": "Inference returned an empty ChangeSet payload.",
                "payload_text": payload_text,
            }),
            follow_ups: CapabilityInvocationRequest::None,
        });
    }

    let repo_context = ctx
        .local_state
        .get("repo_context")
        .cloned()
        .unwrap_or_else(|| json!({ "git_ref": "WORKTREE" }));

    let result = executor::execute_payload_gateway(
        ctx.state,
        ctx.run_id,
        Some(ctx.step.id.clone()),
        json!({
            "repo_ref": ctx.repo_ref,
            "git_ref": repo_context.get("git_ref").cloned().unwrap_or_else(|| Value::String("WORKTREE".to_string())),
            "mode": "changeset_apply",
            "payload_text": payload_text,
        }),
    )
    .await?;

    let should_compile = result.get("ok").and_then(Value::as_bool).unwrap_or(false)
        && ctx
            .step
            .execution_logic
            .get("compile_checks")
            .and_then(|v| v.get("commands"))
            .and_then(Value::as_array)
            .map(|rows| !rows.is_empty())
            .unwrap_or(false);

    Ok(CapabilityResult {
        ok: result.get("ok").and_then(Value::as_bool).unwrap_or(false),
        capability: "apply_changeset".to_string(),
        payload: result,
        follow_ups: if should_compile {
            CapabilityInvocationRequest::One(CapabilityInvocation {
                capability: "compile_commands".to_string(),
                config: json!({}),
            })
        } else {
            CapabilityInvocationRequest::None
        },
    })
}
