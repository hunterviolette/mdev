use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::engine::{
    capabilities::registry::{find_result, CapabilityContext, CapabilityResult},
    prompt_inputs::{AttachmentRole, PromptBlockRole, PromptInput, PromptInputPayload},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelAttachmentKind {
    Text,
    Image,
    Document,
    Binary,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelAttachment {
    pub path: PathBuf,
    pub filename: String,
    pub media_type: String,
    pub kind: ModelAttachmentKind,
    pub role: AttachmentRole,
}

fn model_attachment(
    path: PathBuf,
    filename: String,
    media_type: Option<String>,
    role: AttachmentRole,
) -> ModelAttachment {
    let media_type = media_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            mime_guess::from_path(&path)
                .first_raw()
                .map(str::to_string)
        })
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let kind = model_attachment_kind(&media_type);

    ModelAttachment {
        path,
        filename,
        media_type,
        kind,
        role,
    }
}

fn model_attachment_kind(media_type: &str) -> ModelAttachmentKind {
    let media_type = media_type.trim().to_ascii_lowercase();

    if media_type.starts_with("image/") {
        return ModelAttachmentKind::Image;
    }

    if media_type == "application/pdf" {
        return ModelAttachmentKind::Document;
    }

    if media_type.starts_with("text/")
        || media_type == "application/json"
        || media_type == "application/xml"
        || media_type == "application/yaml"
        || media_type.ends_with("+json")
        || media_type.ends_with("+xml")
    {
        return ModelAttachmentKind::Text;
    }

    ModelAttachmentKind::Binary
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
    pub primary_input_text: Option<String>,
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
    let mut primary = None;
    let mut input_blocks = Vec::new();

    for block in &ctx.prompt_blocks {
        if !block.enabled {
            continue;
        }

        let content = block.content.trim();
        if content.is_empty() {
            continue;
        }

        if primary.is_none() && block.role == PromptBlockRole::User {
            primary = Some(("user_input".to_string(), content.to_string()));
            continue;
        }

        let source = block.source.trim();
        let source = if source.is_empty() {
            block.key.as_str()
        } else {
            source
        };
        let label = block.label.trim();
        let label = if label.is_empty() {
            block.key.as_str()
        } else {
            label
        };

        input_blocks.push(ModelInputBlock {
            source: source.to_string(),
            label: label.to_string(),
            content: content.to_string(),
        });
    }

    (primary, input_blocks)
}

fn prompt_input_blocks(
    inputs: &[PromptInput],
) -> (Option<(String, String)>, Vec<ModelInputBlock>, Vec<ModelAttachment>) {
    let mut primary = None;
    let mut blocks = Vec::new();
    let mut attachments = Vec::new();

    for item in inputs {
        match &item.payload {
            PromptInputPayload::UserInstruction { text }
                if primary.is_none() && !text.trim().is_empty() =>
            {
                primary = Some(("user_instruction".to_string(), text.trim().to_string()));
            }
            PromptInputPayload::PromptContribution {
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
            PromptInputPayload::Attachment {
                path,
                filename,
                media_type,
                role,
            } => {
                attachments.push(model_attachment(
                    PathBuf::from(path),
                    filename.clone(),
                    media_type.clone(),
                    role.clone(),
                ));
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

    Some(model_attachment(
        path,
        filename,
        Some("text/plain".to_string()),
        AttachmentRole::RepositoryContext,
    ))
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
        .prompt_inputs
        .resolve_for_step(ctx.run_id, ctx.step.id.as_str());

    let consumed_input_ids = resolved.iter().map(|item| item.id).collect::<Vec<_>>();
    let (stage_primary, mut input_blocks) = configured_stage_input(ctx);
    let (prompt_primary, contributed_prompt_blocks, mut attachments) =
        prompt_input_blocks(&resolved);
    input_blocks.extend(contributed_prompt_blocks);

    let primary = prompt_primary
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
        prompt_input_count = resolved.len(),
        "central model input built"
    );

    Ok(ModelInput {
        text,
        pasted_context_text,
        attachments,
        input_blocks,
        consumed_input_ids,
        primary_input_source: primary.as_ref().map(|item| item.0.clone()),
        primary_input_text: primary.map(|item| item.1),
    })
}
