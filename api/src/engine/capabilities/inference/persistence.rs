use anyhow::Result;
use chrono::Utc;
use serde_json::Value;
use serde::{Serialize, Deserialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use super::prompting::{ModelInput, ModelInputOrigin};
use super::accounting::{count_tokens, CompositeInputSummary, InferenceAccounting, InputScope, ReportedUsage, TheoreticalUsage, TokenizerKind, InputTokenComponent};
use super::super::registry::CapabilityContext;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationInput {
    pub sequence_no: i64,
    pub input_type: String,
    pub input_origin: String,
    pub input_source: String,
    pub label: String,
    pub content: String,
}

pub async fn record_invocation(
    db: &SqlitePool,
    ctx: &CapabilityContext<'_>,
    session_id: &str,
    transport: &str,
    provider: &str,
    model: &str,
    input: &ModelInput,
    response: &Value,
    reported_usage: Option<&ReportedUsage>,
    output_text: &str,
) -> Result<String> {
    let invocation_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let status = if response.get("ok").and_then(Value::as_bool).unwrap_or(true) {
        "completed"
    } else {
        "failed"
    };

    let input_components: Vec<InputTokenComponent> = match response.get("theoretical_input_components") {
        Some(value) => serde_json::from_value(value.clone())?,
        None => super::prompting::estimate_input_components(input, None)?,
    };
    let composite_input = CompositeInputSummary::from_components(&input_components);
    let theoretical_usage = TheoreticalUsage {
        input_tokens: composite_input.theoretical_tokens,
        output_tokens: count_tokens(output_text)?,
        tokenizer: TokenizerKind::O200kBase,
        input_scope: InputScope::SubmittedContent,
    };
    let mut accounting = InferenceAccounting::new(
        reported_usage.cloned(),
        Some(theoretical_usage),
    );
    accounting.input_components = input_components;
    accounting.composite_input = composite_input;
    let accounting_json = serde_json::to_string(&accounting)?;

    let inputs = input.input_blocks.iter().enumerate().filter_map(|(index, block)| {
        if !is_instruction_source(&block.source) || block.content.trim().is_empty() { return None; }
        let origin = match block.origin {
            ModelInputOrigin::UserCustom => "user_custom",
            ModelInputOrigin::StageDefault => "stage_default",
            ModelInputOrigin::ConfiguredDefault => "configured_default",
            ModelInputOrigin::StageConfigured => "stage_configured",
            ModelInputOrigin::Inherited => "inherited",
            ModelInputOrigin::AutomationGenerated => "automation_generated",
        };
        Some(InvocationInput {
            sequence_no: index as i64,
            input_type: "instruction".to_string(),
            input_origin: origin.to_string(),
            input_source: block.source.clone(),
            label: block.label.clone(),
            content: block.content.clone(),
        })
    }).collect::<Vec<_>>();
    let output = serde_json::json!({
        "text": output_text,
        "payload": response,
        "provider_metadata": response.get("provider_metadata")
    });
    sqlx::query("INSERT INTO inference_invocations (id, inference_session_id, workflow_run_id, step_id, stage_type, capability_invocation_id, transport, provider, model, status, input_json, output_json, accounting_json, created_at, completed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
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
        .bind(serde_json::to_string(&inputs)?)
        .bind(serde_json::to_string(&output)?)
        .bind(&accounting_json)
        .bind(&now)
        .bind(&now)
        .execute(db)
        .await?;
    Ok(invocation_id)
}

fn is_instruction_source(source: &str) -> bool {
    matches!(source, "user_instruction" | "user_input" | "empty_user_input_default" | "workflow_step" | "prompt_contribution" | "automation" | "retry_instruction" | "apply_error" | "compile_error")
}

