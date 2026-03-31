use anyhow::Result;
use serde_json::{json, Value};

use crate::executor::CHANGESET_SCHEMA_EXAMPLE;

use super::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult};

pub async fn execute(
    _ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    _config: Value,
) -> Result<CapabilityResult> {
    Ok(CapabilityResult {
        ok: true,
        capability: "changeset_schema".to_string(),
        payload: json!({
            "ok": true,
            "message": "Changeset schema fragment enabled for inference prompt composition.",
            "schema": CHANGESET_SCHEMA_EXAMPLE,
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}
