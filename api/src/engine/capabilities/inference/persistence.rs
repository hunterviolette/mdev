use anyhow::Result;
use chrono::Utc;
use serde_json::Value;
use sqlx::SqlitePool;
use uuid::Uuid;

use super::prompting::{ModelInput, ModelInputOrigin};
use super::super::registry::CapabilityContext;

pub async fn record_invocation(
    db: &SqlitePool,
    ctx: &CapabilityContext<'_>,
    session_id: &str,
    transport: &str,
    provider: &str,
    model: &str,
    input: &ModelInput,
    response: &Value,
) -> Result<String> {
    let invocation_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let status = if response.get("ok").and_then(Value::as_bool).unwrap_or(true) {
        "completed"
    } else {
        "failed"
    };

    let mut tx = db.begin().await?;

    sqlx::query(
        "INSERT INTO inference_invocations (id, inference_session_id, workflow_run_id, step_id, stage_type, capability_invocation_id, transport, provider, model, status, created_at, completed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&invocation_id)
    .bind(session_id)
    .bind(ctx.run_id.to_string())
    .bind(&ctx.step.id)
    .bind(&ctx.step.step_type)
    .bind(ctx.capability_invocation_id.as_deref())
    .bind(transport)
    .bind(provider)
    .bind(model)
    .bind(status)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    for (index, block) in input.input_blocks.iter().enumerate() {
        if !is_instruction_source(&block.source) || block.content.trim().is_empty() {
            continue;
        }

        let origin = match block.origin {
            ModelInputOrigin::UserCustom => "user_custom",
            ModelInputOrigin::StageDefault => "stage_default",
            ModelInputOrigin::ConfiguredDefault => "configured_default",
            ModelInputOrigin::StageConfigured => "stage_configured",
            ModelInputOrigin::Inherited => "inherited",
            ModelInputOrigin::AutomationGenerated => "automation_generated",
        };

        sqlx::query(
            "INSERT INTO inference_invocation_inputs (id, invocation_id, sequence_no, input_type, input_origin, input_source, label, content, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&invocation_id)
        .bind(index as i64)
        .bind("instruction")
        .bind(origin)
        .bind(&block.source)
        .bind(&block.label)
        .bind(&block.content)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }

    let metadata = response.get("provider_metadata").cloned().unwrap_or(Value::Null);
    let usage = metadata.get("usage").unwrap_or(&Value::Null);
    let input_tokens = usage.get("input_tokens").or_else(|| usage.get("prompt_tokens")).and_then(Value::as_i64);
    let output_tokens = usage.get("output_tokens").or_else(|| usage.get("completion_tokens")).and_then(Value::as_i64);
    let cached_input_tokens = usage.get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .or_else(|| usage.get("cache_read_input_tokens"))
        .and_then(Value::as_i64);

    sqlx::query(
        "INSERT INTO inference_invocation_outputs (id, invocation_id, response_text, response_payload_json, provider_metadata_json, input_tokens, output_tokens, cached_input_tokens, cost_amount, cost_currency, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&invocation_id)
    .bind(response.get("text").and_then(Value::as_str).unwrap_or(""))
    .bind(serde_json::to_string(response)?)
    .bind(serde_json::to_string(&metadata)?)
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(cached_input_tokens)
    .bind(metadata.get("cost").and_then(Value::as_f64))
    .bind(metadata.get("currency").and_then(Value::as_str))
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(invocation_id)
}

fn is_instruction_source(source: &str) -> bool {
    matches!(source, "user_instruction" | "user_input" | "empty_user_input_default" | "workflow_step" | "prompt_contribution" | "automation" | "retry_instruction" | "apply_error" | "compile_error")
}

