use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::engine::{
    capabilities::registry::{find_result, CapabilityContext, CapabilityResult},
    orchestration_inputs::{
        AttachmentRole,
        OrchestrationInputEnvelope,
        OrchestrationInputPayload,
    },
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelAttachment {
    pub path: PathBuf,
    pub filename: String,
    pub media_type: Option<String>,
    pub role: AttachmentRole,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelInputBlock {
    pub source: String,
    pub label: String,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelInput {
    pub text: String,
    pub attachments: Vec<ModelAttachment>,
    pub input_blocks: Vec<ModelInputBlock>,
    pub consumed_input_ids: Vec<Uuid>,
    pub primary_input_source: Option<String>,
}

#[derive(Default, Deserialize)]
struct AutomationPromptConfig {
    #[serde(default)]
    empty_user_input_default: String,
}

#[derive(Default, Deserialize)]
struct ExecutionLogicPromptConfig {
    #[serde(default)]
    automation: AutomationPromptConfig,
}

#[derive(Default, Deserialize)]
struct ContextExportResult {
    #[serde(default)]
    output_path: String,
}

fn configured_default_user_input(ctx: &CapabilityContext<'_>) -> Option<String> {
    let execution_logic = ctx
        .local_state
        .get("execution_logic")
        .cloned()
        .unwrap_or_else(|| ctx.step.execution_logic.clone());
    let config = serde_json::from_value::<ExecutionLogicPromptConfig>(execution_logic)
        .unwrap_or_default();

    let value = config.automation.empty_user_input_default.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn orchestration_blocks(
    inputs: &[OrchestrationInputEnvelope],
) -> (Option<(String, String)>, Vec<ModelInputBlock>, Vec<ModelAttachment>) {
    let mut primary = None;
    let mut blocks = Vec::new();
    let mut attachments = Vec::new();

    for item in inputs {
        match &item.payload {
            OrchestrationInputPayload::UserInstruction { text }
                if primary.is_none() && !text.trim().is_empty() =>
            {
                primary = Some(("user_instruction".to_string(), text.trim().to_string()));
            }
            OrchestrationInputPayload::PromptContribution {
                text,
                source,
                label,
            } if !text.trim().is_empty() => {
                let source = source
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("prompt_contribution")
                    .to_string();
                let label = label
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("Prompt contribution")
                    .to_string();

                blocks.push(ModelInputBlock {
                    source,
                    label,
                    content: text.trim().to_string(),
                });
            }
            OrchestrationInputPayload::Attachment {
                path,
                filename,
                media_type,
                role,
            } => {
                attachments.push(ModelAttachment {
                    path: PathBuf::from(path),
                    filename: filename.clone(),
                    media_type: media_type.clone(),
                    role: role.clone(),
                });
            }
            _ => {}
        }
    }

    (primary, blocks, attachments)
}

fn context_export_attachment(
    prior_results: &[CapabilityResult],
) -> Option<ModelAttachment> {
    let result = find_result(prior_results, "context_export")?;
    if !result.ok {
        return None;
    }

    let parsed = serde_json::from_value::<ContextExportResult>(result.payload.clone()).ok()?;
    let path = parsed.output_path.trim();
    if path.is_empty() {
        return None;
    }

    let path = PathBuf::from(path);
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("repo_context.txt")
        .to_string();

    Some(ModelAttachment {
        path,
        filename,
        media_type: Some("text/plain".to_string()),
        role: AttachmentRole::RepositoryContext,
    })
}

fn append_unique_section(target: &mut Vec<String>, value: impl Into<String>) {
    let value = value.into();
    let normalized = value.trim();
    if normalized.is_empty() {
        return;
    }

    if target.iter().any(|existing| existing.trim() == normalized) {
        return;
    }

    target.push(normalized.to_string());
}

pub fn build_model_input(
    ctx: &CapabilityContext<'_>,
    prior_results: &[CapabilityResult],
) -> Result<ModelInput> {
    let resolved = ctx
        .state
        .orchestration_inputs
        .resolve_for_step(ctx.run_id, ctx.step.id.as_str());

    let consumed_input_ids = resolved.iter().map(|item| item.id).collect::<Vec<_>>();
    let (primary, mut input_blocks, mut attachments) = orchestration_blocks(&resolved);

    let primary = primary.or_else(|| {
        configured_default_user_input(ctx)
            .map(|value| ("empty_user_input_default".to_string(), value))
    });

    let mut sections = Vec::new();

    if let Some((source, text)) = primary.as_ref() {
        append_unique_section(&mut sections, text.clone());
        input_blocks.insert(
            0,
            ModelInputBlock {
                source: source.clone(),
                label: if source == "user_instruction" {
                    "User instruction".to_string()
                } else {
                    "Default user instruction".to_string()
                },
                content: text.clone(),
            },
        );
    }

    for block in &input_blocks {
        if !matches!(
            block.source.as_str(),
            "user_instruction" | "empty_user_input_default"
        ) {
            append_unique_section(&mut sections, block.content.clone());
        }
    }

    let composed = super::resolve_inference_prompt(ctx.local_state);
    append_unique_section(&mut sections, composed);

    if let Some(attachment) = context_export_attachment(prior_results) {
        if !attachments
            .iter()
            .any(|existing| existing.path == attachment.path)
        {
            attachments.push(attachment);
        }
    }

    let text = sections.join("\n\n");

    tracing::info!(
        run_id = %ctx.run_id,
        step_id = %ctx.step.id,
        primary_input_source = ?primary.as_ref().map(|item| item.0.as_str()),
        text_length = text.chars().count(),
        attachment_count = attachments.len(),
        orchestration_input_count = resolved.len(),
        "central model input built"
    );

    Ok(ModelInput {
        text,
        attachments,
        input_blocks,
        consumed_input_ids,
        primary_input_source: primary.map(|item| item.0),
    })
}
