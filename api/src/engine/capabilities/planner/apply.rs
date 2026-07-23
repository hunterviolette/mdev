use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::Row;

use crate::engine::capabilities::{
    planner::{normalize_refined_feature_plan_item, FeaturePlanItem, PlannerCapabilityBinding},
    registry::{find_result, CapabilityContext, CapabilityInvocationRequest, CapabilityResult},
};

pub fn build_apply_error_feedback(capability_results: &[Value]) -> String {
    let apply_result = capability_results
        .iter()
        .find(|item| item.get("key").and_then(Value::as_str) == Some("planner_apply"))
        .and_then(|item| item.get("result"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let summary = apply_result
        .get("summary")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Planner apply failed.");

    let error = apply_result
        .get("error")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());

    let detail = match error {
        Some(error) if error != summary => format!("{}\n\n{}", summary, error),
        _ => summary.to_string(),
    };

    format!(
        "The previous planner apply attempt failed.\n\n{}\n\nRevise the planner output to resolve this apply error. Do not repeat the failed output unchanged.",
        detail
    )
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let binding: PlannerCapabilityBinding =
        serde_json::from_value(config).context("invalid planner_apply binding")?;

    let output_text = find_result(prior_results, "inference")
        .and_then(|result| result.payload.get("result"))
        .and_then(|result| result.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("planner_apply requires inference result text"))?;

    let row = sqlx::query(
        "SELECT title, payload_json FROM planner_features WHERE planner_id = ? AND id = ? AND COALESCE(status, '') != 'deleted' LIMIT 1",
    )
    .bind(&binding.planner_id)
    .bind(&binding.feature_id)
    .fetch_optional(&ctx.state.db)
    .await?
    .ok_or_else(|| {
        anyhow!(
            "planner feature '{}' was not found in planner '{}'",
            binding.feature_id,
            binding.planner_id
        )
    })?;

    let title = row.get::<String, _>("title");
    let payload_json = row.get::<String, _>("payload_json");
    let existing: FeaturePlanItem =
        serde_json::from_str(&payload_json).context("invalid persisted planner feature payload")?;

    let mut refined = normalize_refined_feature_plan_item(
        &binding.feature_id,
        &title,
        existing
            .rough_summary
            .clone()
            .or_else(|| Some(existing.summary.clone())),
        output_text,
    )?;

    refined.refinement_workflow_run_id = Some(ctx.run_id);
    refined.applied_sprint_id = existing.applied_sprint_id;
    refined.applied_sprint_title = existing.applied_sprint_title;
    refined.applied_at = existing.applied_at;

    let now = Utc::now().to_rfc3339();
    let status = serde_json::to_value(&refined.status)?
        .as_str()
        .unwrap_or("rough")
        .to_string();

    sqlx::query(
        "UPDATE planner_features SET title = ?, status = ?, payload_json = ?, refined_at = ?, updated_at = ? WHERE planner_id = ? AND id = ?",
    )
    .bind(&refined.title)
    .bind(&status)
    .bind(serde_json::to_string(&refined)?)
    .bind(&now)
    .bind(&now)
    .bind(&binding.planner_id)
    .bind(&binding.feature_id)
    .execute(&ctx.state.db)
    .await?;

    Ok(CapabilityResult {
        ok: true,
        capability: "planner_apply".to_string(),
        payload: json!({
            "ok": true,
            "planner_id": binding.planner_id,
            "feature_id": binding.feature_id,
            "feature": refined
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}
