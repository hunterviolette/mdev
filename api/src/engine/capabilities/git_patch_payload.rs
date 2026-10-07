use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::engine::capabilities::{
    git::git::{
        apply_git_patch, apply_git_patch_reverse, generate_git_apply_patch, git_status, run_git,
        run_git_allow_fail, run_git_with_input, GitPatchScope,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GitPatchMergeFile {
    path: String,
    base: Option<String>,
    result: Option<String>,
    text_mergeable: bool,
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
    #[serde(default)]
    merge_files: Vec<GitPatchMergeFile>,
}

#[derive(Debug, Clone)]
pub struct GeneratedGitPatchPayload {
    pub payload_text: String,
    pub patch_text: String,
    pub patch_hash: String,
    pub base_head: String,
    pub patch_bytes: usize,
    pub scope: String,
    pub from_ref: String,
    pub to_ref: String,
}

fn generated_payload(
    repo_ref: &str,
    scope: &str,
    paths: &[String],
    context_lines: Option<u32>,
    patch: String,
) -> Result<GeneratedGitPatchPayload> {
    let repo = PathBuf::from(repo_ref);
    let (_, from_ref, to_ref) = parse_scope(scope)?;
    let head = String::from_utf8(run_git(&repo, &["rev-parse", "HEAD"])?).context("git HEAD was not valid UTF-8")?;
    let merge_files = build_merge_files(&repo, scope, paths)?;
    let envelope = GitPatchPayloadEnvelope {
        version: 2,
        kind: "git_apply_patch".to_string(),
        scope: scope.to_string(),
        from_ref: from_ref.to_string(),
        to_ref: to_ref.to_string(),
        base_head: head.trim().to_string(),
        paths: paths.to_vec(),
        context_lines,
        patch,
        merge_files,
    };
    let payload_text = serde_json::to_string_pretty(&envelope)?;
    let patch_hash = hex::encode(Sha256::digest(envelope.patch.as_bytes()));
    Ok(GeneratedGitPatchPayload {
        payload_text,
        patch_text: envelope.patch.clone(),
        patch_hash,
        base_head: envelope.base_head.clone(),
        patch_bytes: envelope.patch.len(),
        scope: envelope.scope.clone(),
        from_ref: envelope.from_ref.clone(),
        to_ref: envelope.to_ref.clone(),
    })
}

pub fn generate_payload(repo_ref: &str, scope: &str, paths: &[String], context_lines: Option<u32>) -> Result<GeneratedGitPatchPayload> {
    let repo = PathBuf::from(repo_ref);
    let (git_scope, _, _) = parse_scope(scope)?;
    let selected_paths = if paths.is_empty() { None } else { Some(paths) };
    let patch = generate_git_apply_patch(&repo, git_scope, selected_paths, context_lines)?;
    generated_payload(repo_ref, scope, paths, context_lines, patch)
}

pub fn payload_from_patch(repo_ref: &str, scope: &str, patch_text: &str) -> Result<GeneratedGitPatchPayload> {
    generated_payload(repo_ref, scope, &[], None, patch_text.to_string())
}

pub fn apply_payload(repo_ref: &str, payload_text: &str, reverse: bool) -> Result<()> {
    apply_payload_inner(repo_ref, payload_text, reverse, false)
}

pub fn apply_payload_for_integration(repo_ref: &str, payload_text: &str) -> Result<()> {
    apply_payload_inner(repo_ref, payload_text, false, true)
}

fn apply_payload_inner(
    repo_ref: &str,
    payload_text: &str,
    reverse: bool,
    prefer_incoming_conflicts: bool,
) -> Result<()> {
    let envelope: GitPatchPayloadEnvelope = serde_json::from_str(payload_text)
        .context("failed to parse git patch payload envelope")?;
    if !matches!(envelope.version, 1 | 2) || envelope.kind != "git_apply_patch" {
        bail!("unsupported git patch payload envelope");
    }
    let repo = PathBuf::from(repo_ref);
    if reverse {
        return apply_git_patch_reverse(&repo, &envelope.patch);
    }

    match apply_git_patch(&repo, &envelope.patch) {
        Ok(()) => Ok(()),
        Err(apply_error) => {
            if envelope.merge_files.is_empty() {
                return Err(apply_error);
            }
            apply_three_way_merge(
                &repo,
                &envelope.patch,
                &envelope.merge_files,
                prefer_incoming_conflicts,
            )
            .map_err(|merge_error| {
                anyhow::anyhow!(
                    "git apply failed and per-file merge fallback failed.\ngit_apply_error={:#}\nmerge_error={:#}",
                    apply_error,
                    merge_error
                )
            })
        }
    }
}

fn build_merge_files(repo: &Path, scope: &str, requested_paths: &[String]) -> Result<Vec<GitPatchMergeFile>> {
    let status = git_status(repo)?;
    let mut paths = status
        .files
        .into_iter()
        .filter(|file| match scope {
            "staged" => file.staged,
            "unstaged" => file.untracked || file.worktree_status != ".",
            "both" => file.staged || file.untracked || file.worktree_status != ".",
            _ => false,
        })
        .map(|file| file.path)
        .filter(|path| requested_paths.is_empty() || requested_paths.iter().any(|requested| requested == path))
        .collect::<Vec<_>>();

    paths.sort();
    paths.dedup();

    let (base_ref, result_ref) = match scope {
        "staged" => ("HEAD", "INDEX"),
        "unstaged" => ("INDEX", "WORKTREE"),
        "both" => ("HEAD", "WORKTREE"),
        other => bail!("unsupported git patch payload scope '{other}'"),
    };

    paths
        .into_iter()
        .map(|path| {
            let (base_exists, base) = read_version(repo, base_ref, &path)?;
            let (result_exists, result) = read_version(repo, result_ref, &path)?;
            let text_mergeable = (!base_exists || base.is_some()) && (!result_exists || result.is_some());
            Ok(GitPatchMergeFile {
                path,
                base,
                result,
                text_mergeable,
            })
        })
        .collect()
}

fn read_version(repo: &Path, source: &str, path: &str) -> Result<(bool, Option<String>)> {
    if source == "WORKTREE" {
        return match fs::read(repo.join(path)) {
            Ok(bytes) => Ok((true, String::from_utf8(bytes).ok())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((false, None)),
            Err(error) => Err(error).with_context(|| format!("failed to read worktree file {path}")),
        };
    }

    let spec = if source == "INDEX" {
        format!(":{path}")
    } else {
        format!("{source}:{path}")
    };
    let (code, stdout, _) = run_git_allow_fail(repo, &["show", &spec])?;
    if code != 0 {
        return Ok((false, None));
    }
    Ok((true, String::from_utf8(stdout).ok()))
}

fn apply_three_way_merge(
    repo: &Path,
    patch: &str,
    merge_files: &[GitPatchMergeFile],
    prefer_incoming_conflicts: bool,
) -> Result<()> {
    for merge_file in merge_files {
        if apply_patch_file_exact(repo, patch, &merge_file.path).is_ok() {
            continue;
        }

        if !merge_file.text_mergeable {
            bail!("three-way merge fallback does not support binary file {}", merge_file.path);
        }

        let target = safe_target_path(repo, &merge_file.path)?;
        let ours = match fs::read(&target) {
            Ok(bytes) => Some(String::from_utf8(bytes).with_context(|| {
                format!("three-way merge fallback does not support binary target file {}", merge_file.path)
            })?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error).with_context(|| format!("failed to read target file {}", merge_file.path)),
        };

        let merged = merge_text_versions(
            &merge_file.path,
            merge_file.base.as_deref(),
            ours.as_deref(),
            merge_file.result.as_deref(),
            prefer_incoming_conflicts,
        )?;

        match merged {
            Some(contents) => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&target, contents)
                    .with_context(|| format!("failed to write merged file {}", target.display()))?;
            }
            None => {
                if target.exists() {
                    fs::remove_file(&target)
                        .with_context(|| format!("failed to delete merged file {}", target.display()))?;
                }
            }
        }
    }

    Ok(())
}

fn apply_patch_file_exact(repo: &Path, patch: &str, path: &str) -> Result<()> {
    let include = format!("--include={path}");
    run_git_with_input(
        repo,
        &["apply", include.as_str(), "--whitespace=nowarn", "-"],
        patch.as_bytes(),
    )?;
    Ok(())
}

fn safe_target_path(repo: &Path, path: &str) -> Result<PathBuf> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative.components().any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
    {
        bail!("invalid patch path '{path}'");
    }
    Ok(repo.join(relative))
}

fn merge_text_versions(
    path: &str,
    base: Option<&str>,
    ours: Option<&str>,
    theirs: Option<&str>,
    prefer_incoming_conflicts: bool,
) -> Result<Option<String>> {
    match (base, ours, theirs) {
        (None, None, Some(theirs)) => Ok(Some(theirs.to_string())),
        (None, Some(ours), Some(theirs)) if ours == theirs => Ok(Some(ours.to_string())),
        (None, Some(_), Some(theirs)) if prefer_incoming_conflicts => Ok(Some(theirs.to_string())),
        (None, Some(_), Some(_)) => bail!("three-way add/add conflict in {path}"),
        (Some(base), Some(ours), None) if ours == base => Ok(None),
        (Some(_), Some(_), None) if prefer_incoming_conflicts => Ok(None),
        (Some(_), Some(_), None) => bail!("three-way modify/delete conflict in {path}"),
        (Some(_), None, None) => Ok(None),
        (Some(base), None, Some(theirs)) if theirs == base => Ok(None),
        (Some(_), None, Some(theirs)) if prefer_incoming_conflicts => Ok(Some(theirs.to_string())),
        (Some(_), None, Some(_)) => bail!("three-way delete/modify conflict in {path}"),
        (Some(base), Some(ours), Some(theirs)) if ours == theirs => Ok(Some(ours.to_string())),
        (Some(base), Some(ours), Some(theirs)) if ours == base => Ok(Some(theirs.to_string())),
        (Some(base), Some(ours), Some(theirs)) if theirs == base => Ok(Some(ours.to_string())),
        (Some(base), Some(ours), Some(theirs)) => merge_file_contents(
            path,
            base,
            ours,
            theirs,
            prefer_incoming_conflicts,
        )
        .map(Some),
        (None, _, None) => Ok(ours.map(str::to_string)),
    }
}

fn merge_file_contents(
    path: &str,
    base: &str,
    ours: &str,
    theirs: &str,
    prefer_incoming_conflicts: bool,
) -> Result<String> {
    let temp_dir = std::env::temp_dir().join(format!("mdev-three-way-{}", Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;
    let ours_path = temp_dir.join("ours");
    let base_path = temp_dir.join("base");
    let theirs_path = temp_dir.join("theirs");
    fs::write(&ours_path, ours)?;
    fs::write(&base_path, base)?;
    fs::write(&theirs_path, theirs)?;

    let output = Command::new("git")
        .arg("merge-file")
        .arg("-p")
        .arg(&ours_path)
        .arg(&base_path)
        .arg(&theirs_path)
        .output()
        .with_context(|| format!("failed to run three-way merge for {path}"));

    let result = match output {
        Ok(output) if output.status.success() => String::from_utf8(output.stdout)
            .with_context(|| format!("three-way merge output was not UTF-8 for {path}")),
        Ok(output) if prefer_incoming_conflicts => {
            let resolved = Command::new("git")
                .arg("merge-file")
                .arg("-p")
                .arg("--theirs")
                .arg(&ours_path)
                .arg(&base_path)
                .arg(&theirs_path)
                .output()
                .with_context(|| format!("failed to resolve integration merge for {path}"))?;
            if !resolved.status.success() {
                bail!(
                    "three-way content conflict in {path}: {}",
                    String::from_utf8_lossy(&resolved.stderr).trim()
                );
            }
            String::from_utf8(resolved.stdout)
                .with_context(|| format!("resolved three-way merge output was not UTF-8 for {path}"))
        }
        Ok(output) => bail!(
            "three-way content conflict in {path}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => Err(error),
    };

    let _ = fs::remove_dir_all(&temp_dir);
    result
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
        .and_then(|v| v.get("repo"))
        .and_then(|v| v.get("repo_ref"))
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

    match cfg.mode.as_str() {
        "generate" => {
            let generated = generate_payload(&repo_ref, &cfg.scope, &cfg.paths, cfg.context_lines)?;
            Ok(CapabilityResult {
                ok: true,
                capability: "git_patch_payload".to_string(),
                payload: json!({
                    "ok": true,
                    "mode": "generate",
                    "repo_ref": repo_ref,
                    "scope": generated.scope,
                    "from_ref": generated.from_ref,
                    "to_ref": generated.to_ref,
                    "base_head": generated.base_head,
                    "paths": cfg.paths,
                    "context_lines": cfg.context_lines,
                    "patch_bytes": generated.patch_bytes,
                    "patch_hash": generated.patch_hash,
                    "payload_text": generated.payload_text
                }),
                follow_ups: CapabilityInvocationRequest::None,
            })
        }
        "apply" => {
            let payload_text = cfg.payload_text.trim();
            if payload_text.is_empty() {
                bail!("git_patch_payload apply requires payload_text");
            }
            let envelope: GitPatchPayloadEnvelope = serde_json::from_str(payload_text)
                .context("failed to parse git patch payload envelope")?;
            if !matches!(envelope.version, 1 | 2) || envelope.kind != "git_apply_patch" {
                bail!("unsupported git patch payload envelope");
            }
            apply_payload(&repo_ref, payload_text, cfg.reverse)?;
            Ok(CapabilityResult {
                ok: true,
                capability: "git_patch_payload".to_string(),
                payload: json!({
                    "ok": true,
                    "mode": "apply",
                    "repo_ref": repo_ref,
                    "reverse": cfg.reverse,
                    "scope": envelope.scope,
                    "from_ref": envelope.from_ref,
                    "to_ref": envelope.to_ref,
                    "base_head": envelope.base_head,
                    "paths": envelope.paths,
                    "context_lines": envelope.context_lines,
                    "patch_bytes": envelope.patch.len()
                }),
                follow_ups: CapabilityInvocationRequest::None,
            })
        }
        other => bail!("unsupported git_patch_payload mode '{other}'"),
    }
}
