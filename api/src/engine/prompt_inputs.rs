use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentRole {
    RepositoryContext,
    Image,
    Document,
    GeneratedArtifact,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromptBlockRole {
    User,
    Capability,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PromptBlock {
    pub index: usize,
    pub id: String,
    pub key: String,
    pub capability_key: String,
    pub label: String,
    pub title: String,
    pub role: PromptBlockRole,
    pub source: String,
    pub enabled: bool,
    pub default_collapsed: bool,
    pub content_format: String,
    pub char_count: usize,
    pub content: String,
}

pub fn prompt_blocks_from_state(state: &Value) -> Result<Vec<PromptBlock>> {
    let Some(value) = state.get("model_input_blocks") else {
        return Ok(Vec::new());
    };

    serde_json::from_value(value.clone()).context("invalid typed prompt block runtime state")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PromptInputPayload {
    UserInstruction {
        text: String,
    },
    PromptContribution {
        text: String,
        source: Option<String>,
        label: Option<String>,
    },
    Attachment {
        path: String,
        filename: String,
        media_type: Option<String>,
        role: AttachmentRole,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PromptInputScope {
    Run,
    Stage {
        step_id: String,
    },
    StageExecution {
        step_id: String,
        stage_execution_id: String,
    },
    CapabilityInvocation {
        step_id: String,
        capability_invocation_id: String,
    },
}

impl PromptInputScope {
    fn applies_to_step(&self, step_id: &str) -> bool {
        match self {
            Self::Run => true,
            Self::Stage { step_id: scoped_step }
            | Self::StageExecution {
                step_id: scoped_step,
                ..
            }
            | Self::CapabilityInvocation {
                step_id: scoped_step,
                ..
            } => scoped_step == step_id,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromptInputLifecycle {
    SingleUse,
    UntilStageCompletes,
    UntilRunCompletes,
    Persistent,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PromptInput {
    pub id: Uuid,
    pub run_id: Uuid,
    pub scope: PromptInputScope,
    pub lifecycle: PromptInputLifecycle,
    pub priority: i32,
    pub created_at: DateTime<Utc>,
    pub payload: PromptInputPayload,
}

#[derive(Clone, Default)]
pub struct PromptInputStore {
    inputs: Arc<DashMap<Uuid, Vec<PromptInput>>>,
}

impl PromptInputStore {
    pub fn publish(
        &self,
        run_id: Uuid,
        scope: PromptInputScope,
        lifecycle: PromptInputLifecycle,
        priority: i32,
        payload: PromptInputPayload,
    ) -> PromptInput {
        let envelope = PromptInput {
            id: Uuid::new_v4(),
            run_id,
            scope,
            lifecycle,
            priority,
            created_at: Utc::now(),
            payload,
        };

        self.inputs
            .entry(run_id)
            .or_default()
            .push(envelope.clone());

        envelope
    }

    pub fn set_user_instruction(&self, run_id: Uuid, step_id: &str, text: String) {
        let normalized = text.trim().to_string();
        let mut inputs = self.inputs.entry(run_id).or_default();

        inputs.retain(|item| {
            !matches!(
                (&item.scope, &item.payload),
                (
                    PromptInputScope::Stage { step_id: existing_step },
                    PromptInputPayload::UserInstruction { .. }
                ) if existing_step == step_id
            )
        });

        if normalized.is_empty() {
            return;
        }

        inputs.push(PromptInput {
            id: Uuid::new_v4(),
            run_id,
            scope: PromptInputScope::Stage {
                step_id: step_id.to_string(),
            },
            lifecycle: PromptInputLifecycle::SingleUse,
            priority: 1_000,
            created_at: Utc::now(),
            payload: PromptInputPayload::UserInstruction {
                text: normalized,
            },
        });
    }

    pub fn user_instruction(&self, run_id: Uuid, step_id: &str) -> String {
        self.resolve_for_step(run_id, step_id)
            .into_iter()
            .find_map(|item| match item.payload {
                PromptInputPayload::UserInstruction { text } => Some(text),
                _ => None,
            })
            .unwrap_or_default()
    }

    pub fn resolve_for_step(
        &self,
        run_id: Uuid,
        step_id: &str,
    ) -> Vec<PromptInput> {
        let mut resolved = self
            .inputs
            .get(&run_id)
            .map(|items| {
                items
                    .iter()
                    .filter(|item| item.scope.applies_to_step(step_id))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        resolved.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.created_at.cmp(&right.created_at))
        });

        resolved
    }

    pub fn acknowledge(&self, run_id: Uuid, input_ids: &[Uuid]) {
        if input_ids.is_empty() {
            return;
        }

        let Some(mut inputs) = self.inputs.get_mut(&run_id) else {
            return;
        };

        inputs.retain(|item| {
            !input_ids.contains(&item.id)
                || !matches!(item.lifecycle, PromptInputLifecycle::SingleUse)
        });
    }

    pub fn clear_run(&self, run_id: Uuid) {
        self.inputs.remove(&run_id);
    }
}
