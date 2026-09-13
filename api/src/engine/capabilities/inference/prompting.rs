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
    pub pasted_context_text: Option<String>,
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

fn configured_stage_input(
    ctx: &CapabilityContext<'_>,
) -> (Option<(String, String)>, Vec<ModelInputBlock>) {
    let Some(blocks) = ctx
        .local_state
        .get("model_input_blocks")
        .and_then(Value::as_array)
    else {
        return (None, Vec::new());
    };

    let mut primary = None;
    let mut input_blocks = Vec::new();

    for block in blocks {
        let Some(content) = block
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };

        let key = block
            .get("key")
            .or_else(|| block.get("id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("inference");
        let role = block
            .get("role")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();

        if primary.is_none() && (role == "user" || key == "user_input") {
            primary = Some(("user_input".to_string(), content.to_string()));
            continue;
        }

        let source = block
            .get("source")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(key)
            .to_string();
        let label = block
            .get("label")
            .or_else(|| block.get("title"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(key)
            .to_string();

        input_blocks.push(ModelInputBlock {
            source,
            label,
            content: content.to_string(),
        });
    }

    (primary, input_blocks)
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
    let (stage_primary, mut input_blocks) = configured_stage_input(ctx);
    let (orchestration_primary, orchestration_input_blocks, mut attachments) =
        orchestration_blocks(&resolved);
    input_blocks.extend(orchestration_input_blocks);

    let primary = orchestration_primary
        .or(stage_primary)
        .or_else(|| {
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
                label: match source.as_str() {
                    "user_instruction" | "user_input" => "User instruction".to_string(),
                    _ => "Default user instruction".to_string(),
                },
                content: text.clone(),
            },
        );
    }

    let mut pasted_context_sections = Vec::new();

    for block in &input_blocks {
        if matches!(
            block.source.as_str(),
            "user_instruction" | "user_input" | "empty_user_input_default"
        ) {
            continue;
        }

        if block.source == "compile_commands" {
            append_unique_section(&mut pasted_context_sections, block.content.clone());
        } else {
            append_unique_section(&mut sections, block.content.clone());
        }
    }


    if let Some(attachment) = context_export_attachment(prior_results) {
        if !attachments
            .iter()
            .any(|existing| existing.path == attachment.path)
        {
            attachments.push(attachment);
        }
    }

    let text = sections.join("\n\n");
    let pasted_context_text = if pasted_context_sections.is_empty() {
        None
    } else {
        Some(pasted_context_sections.join("\n\n"))
    };

    tracing::info!(
        run_id = %ctx.run_id,
        step_id = %ctx.step.id,
        primary_input_source = ?primary.as_ref().map(|item| item.0.as_str()),
        text_length = text.chars().count(),
        pasted_context_length = pasted_context_text
            .as_deref()
            .map(str::chars)
            .map(Iterator::count)
            .unwrap_or(0),
        attachment_count = attachments.len(),
        orchestration_input_count = resolved.len(),
        "central model input built"
    );

    Ok(ModelInput {
        text,
        pasted_context_text,
        attachments,
        input_blocks,
        consumed_input_ids,
        primary_input_source: primary.map(|item| item.0),
    })
}
