use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::engine::capabilities::{
    git::git::{
        apply_git_patch, apply_git_patch_reverse, generate_git_apply_patch, run_git, GitPatchScope,
    },
    registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult},
};

#[derive(Debug, Deserialize)]
struct GitPatchPayloadConfig {
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default)]
    repo_ref: String,
    #[serde(default = "default_scope")]
    scope: String,
    #[serde(default)]
    paths: Vec<String>,
    context_lines: Option<u32>,
    #[serde(default)]
    payload_text: String,
    #[serde(default)]
    reverse: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct GitPatchPayloadEnvelope {
    version: u32,
    kind: String,
    scope: String,
    from_ref: String,
    to_ref: String,
    base_head: String,
    #[serde(default)]
    paths: Vec<String>,
    context_lines: Option<u32>,
    patch: String,
}

fn default_mode() -> String {
    "generate".to_string()
}

fn default_scope() -> String {
    "both".to_string()
}

fn parse_scope(scope: &str) -> Result<(GitPatchScope, &'static str, &'static str)> {
    match scope {
        "staged" => Ok((GitPatchScope::Staged, "HEAD", "INDEX")),
        "unstaged" => Ok((GitPatchScope::Unstaged, "INDEX", "WORKTREE")),
        "both" => Ok((GitPatchScope::Both, "HEAD", "WORKTREE")),
        other => bail!("unsupported git patch payload scope '{other}'"),
    }
}

fn resolve_repo_ref(ctx: &CapabilityContext<'_>, repo_ref: &str) -> String {
    if !repo_ref.trim().is_empty() {
        return repo_ref.trim().to_string();
    }

    ctx.local_state
        .get("resources")
        .and_then(|value| value.get("repo"))
        .and_then(|value| value.get("repo_ref"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(ctx.repo_ref)
        .to_string()
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let cfg: GitPatchPayloadConfig =
        serde_json::from_value(config).context("invalid git_patch_payload config")?;
    let repo_ref = resolve_repo_ref(ctx, &cfg.repo_ref);
    let repo = PathBuf::from(&repo_ref);

    match cfg.mode.as_str() {
        "generate" => {
            let (scope, from_ref, to_ref) = parse_scope(&cfg.scope)?;
            let paths = if cfg.paths.is_empty() {
                None
            } else {
                Some(cfg.paths.as_slice())
            };
            let context_lines = cfg.context_lines.map(|value| value.min(1000));
            let patch = generate_git_apply_patch(&repo, scope, paths, context_lines)?;
            let base_head = String::from_utf8(run_git(&repo, &["rev-parse", "HEAD"])?)
                .context("HEAD was not valid UTF-8")?
                .trim()
                .to_string();

            let envelope = GitPatchPayloadEnvelope {
                version: 1,
                kind: "git_apply_patch".to_string(),
                scope: cfg.scope,
                from_ref: from_ref.to_string(),
                to_ref: to_ref.to_string(),
                base_head,
                paths: cfg.paths,
                context_lines,
                patch,
            };
            let payload_text = serde_json::to_string_pretty(&envelope)?;

            Ok(CapabilityResult {
                ok: true,
                capability: "git_patch_payload".to_string(),
                payload: json!({
                    "ok": true,
                    "mode": "generate",
                    "repo_ref": repo_ref,
                    "payload_text": payload_text,
                    "envelope": envelope,
                }),
                follow_ups: CapabilityInvocationRequest::None,
            })
        }
        "apply" => {
            if cfg.payload_text.trim().is_empty() {
                bail!("payload_text is required for git_patch_payload apply");
            }

            let envelope: GitPatchPayloadEnvelope =
                serde_json::from_str(&cfg.payload_text).context("invalid git patch payload envelope")?;

            if envelope.version != 1 || envelope.kind != "git_apply_patch" {
                bail!("unsupported git patch payload envelope");
            }

            if cfg.reverse {
                apply_git_patch_reverse(&repo, &envelope.patch)?;
            } else {
                apply_git_patch(&repo, &envelope.patch)?;
            }

            Ok(CapabilityResult {
                ok: true,
                capability: "git_patch_payload".to_string(),
                payload: json!({
                    "ok": true,
                    "mode": "apply",
                    "repo_ref": repo_ref,
                    "reverse": cfg.reverse,
                    "source_base_head": envelope.base_head,
                    "scope": envelope.scope,
                    "paths": envelope.paths,
                }),
                follow_ups: CapabilityInvocationRequest::None,
            })
        }
        other => bail!("unsupported git_patch_payload mode '{other}'"),
    }
}
