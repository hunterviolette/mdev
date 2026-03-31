use anyhow::Result;
use serde_json::{json, Value};

use crate::executor;

use super::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult};

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let mut repo_context = if config.is_null() || config == json!({}) {
        ctx.local_state
            .get("repo_context")
            .cloned()
            .unwrap_or_else(|| json!({ "repo_ref": ctx.repo_ref, "git_ref": "WORKTREE" }))
    } else {
        config
    };

    if let Some(obj) = repo_context.as_object_mut() {
        if !obj.contains_key("repo_ref") {
            obj.insert("repo_ref".to_string(), Value::String(ctx.repo_ref.to_string()));
        }
        if !obj.contains_key("git_ref") {
            obj.insert("git_ref".to_string(), Value::String("WORKTREE".to_string()));
        }
    }

    let result = executor::execute_context_export(
        ctx.state,
        ctx.run_id,
        Some(ctx.step.id.clone()),
        repo_context,
    )
    .await?;

    Ok(CapabilityResult {
        ok: result.get("ok").and_then(Value::as_bool).unwrap_or(true),
        capability: "context_export".to_string(),
        payload: result,
        follow_ups: CapabilityInvocationRequest::None,
    })
}
