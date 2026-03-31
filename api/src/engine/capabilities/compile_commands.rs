use anyhow::Result;
use serde_json::{json, Value};

use crate::executor;

use super::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult};

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    _config: Value,
) -> Result<CapabilityResult> {
    let commands = ctx
        .step
        .execution_logic
        .get("compile_checks")
        .and_then(|v| v.get("commands"))
        .cloned()
        .unwrap_or_else(|| json!([]));

    let result = executor::execute_terminal_command(
        ctx.state,
        ctx.run_id,
        Some(ctx.step.id.clone()),
        json!({
            "repo_ref": ctx.repo_ref,
            "commands": commands,
        }),
    )
    .await?;

    Ok(CapabilityResult {
        ok: result.get("ok").and_then(Value::as_bool).unwrap_or(false),
        capability: "compile_commands".to_string(),
        payload: result,
        follow_ups: CapabilityInvocationRequest::None,
    })
}
