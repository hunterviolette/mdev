use anyhow::Result;
use serde_json::{json, Value};

use super::super::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult};

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let payload = resolve_payload(ctx, config);

    Ok(CapabilityResult {
        ok: true,
        capability: "sap/import".to_string(),
        payload: json!({
            "ok": true,
            "summary": "SAP import capability scaffolded in API and ready to delegate to the existing ADT bridge adapter.",
            "request": payload,
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}

fn resolve_payload(ctx: &CapabilityContext<'_>, config: Value) -> Value {
    let capability_state = ctx
        .local_state
        .get("capabilities")
        .and_then(|v| v.get("sap/import"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let mut payload = if config.is_null() || config == json!({}) {
        capability_state
    } else {
        config
    };

    if !payload.is_object() {
        payload = json!({});
    }

    let obj = payload.as_object_mut().expect("sap import payload must be object");
    obj.entry("repo_ref".to_string())
        .or_insert_with(|| Value::String(ctx.repo_ref.to_string()));
    obj.entry("git_ref".to_string())
        .or_insert_with(|| Value::String("WORKTREE".to_string()));
    obj.entry("mode".to_string())
        .or_insert_with(|| Value::String("import".to_string()));

    payload
}
