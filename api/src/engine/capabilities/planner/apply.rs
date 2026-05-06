use anyhow::Result;
use serde_json::json;

use crate::{
    engine::capabilities::registry::{
        CapabilityContext, CapabilityInvocationRequest, CapabilityResult,
    },
    supervisor,
};

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    prior_results: &[CapabilityResult],
    _config: serde_json::Value,
) -> Result<CapabilityResult> {
    let run = crate::engine::load_run(ctx.state, ctx.run_id).await?;
    let capability_results = prior_results
        .iter()
        .map(|result| {
            json!({
                "capability": result.capability,
                "key": result.capability,
                "ok": result.ok,
                "payload": result.payload,
                "result": result.payload
            })
        })
        .collect::<Vec<_>>();

    match supervisor::apply_refined_feature_output_from_workflow(ctx.state, &run, &capability_results).await {
        Ok(()) => Ok(CapabilityResult {
            ok: true,
            capability: "supervisor_planner_item".to_string(),
            payload: json!({
                "ok": true,
                "summary": "Refined planner item output applied successfully."
            }),
            follow_ups: CapabilityInvocationRequest::None,
        }),
        Err(err) => Ok(CapabilityResult {
            ok: false,
            capability: "supervisor_planner_item".to_string(),
            payload: json!({
                "ok": false,
                "summary": "Failed to apply refined planner item output.",
                "error": err.to_string()
            }),
            follow_ups: CapabilityInvocationRequest::None,
        }),
    }
}
