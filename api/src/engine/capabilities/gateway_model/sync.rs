use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::engine::capabilities::registry::{
    CapabilityContext,
    CapabilityInvocationRequest,
    CapabilityResult,
};

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    Entire,
    Tree,
    Diff,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SyncRequest {
    pub repo_ref: String,
    #[serde(default = "default_git_ref")]
    pub git_ref: String,
    #[serde(default = "default_sync_mode")]
    pub mode: SyncMode,
    #[serde(default)]
    pub include_files: Vec<String>,
    #[serde(default)]
    pub exclude_regex: Vec<String>,
    #[serde(default)]
    pub skip_binary: bool,
    #[serde(default)]
    pub skip_gitignore: bool,
}

fn default_git_ref() -> String {
    "WORKTREE".to_string()
}

fn default_sync_mode() -> SyncMode {
    SyncMode::Entire
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let mut payload = if config.is_null() || config == json!({}) {
        ctx.local_state
            .get("repo_context")
            .cloned()
            .unwrap_or_else(|| json!({
                "repo_ref": ctx.repo_ref,
                "git_ref": "WORKTREE",
                "mode": "entire"
            }))
    } else {
        config
    };

    if let Some(obj) = payload.as_object_mut() {
        if !obj.contains_key("repo_ref") {
            obj.insert("repo_ref".to_string(), Value::String(ctx.repo_ref.to_string()));
        }
        if !obj.contains_key("git_ref") {
            obj.insert("git_ref".to_string(), Value::String("WORKTREE".to_string()));
        }
        if !obj.contains_key("mode") {
            obj.insert("mode".to_string(), Value::String("entire".to_string()));
        }
    }

    let request: SyncRequest = serde_json::from_value(payload)?;

    Ok(CapabilityResult {
        ok: false,
        capability: "gateway_model_sync".to_string(),
        payload: json!({
            "ok": false,
            "summary": "gateway_model sync module scaffolded in API but not yet ported to backend-native execution",
            "request": request,
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}
