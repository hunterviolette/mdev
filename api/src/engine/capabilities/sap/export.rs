use anyhow::Result;
use serde_json::{json, Value};

use crate::engine::capabilities::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult};

pub async fn execute(
    _ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    _config: Value,
) -> Result<CapabilityResult> {
    Ok(CapabilityResult {
        ok: false,
        capability: "sap/export".to_string(),
        payload: json!({
            "ok": false,
            "message": "sap/export capability is not wired yet."
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}
