use std::{fs, path::{Path, PathBuf}, process::Command};

use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContextExportPayload {
    pub repo_ref: String,
    #[serde(default = "default_git_ref")]
    pub git_ref: String,
    #[serde(default)]
    pub include_files: Option<Vec<String>>,
    #[serde(default)]
    pub include_directories: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_files: Vec<String>,
    #[serde(default)]
    pub exclude_directories: Vec<String>,
    #[serde(default)]
    pub include_override_regex: Vec<String>,
    #[serde(default)]
    pub include_staged_diff: bool,
    #[serde(default)]
    pub include_unstaged_diff: bool,
    #[serde(default)]
    pub skip_binary: bool,
    #[serde(default = "default_skip_gitignore")]
    pub skip_gitignore: bool,
    #[serde(default)]
    pub exclude_regex: Vec<String>,
    #[serde(default)]
    pub artifact_kind: String,
    #[serde(default)]
    pub inline_repo_context_in_prompt: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSyncFile {
    pub path: String,
    pub contents: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSyncSnapshot {
    pub files: Vec<ContextSyncFile>,
    pub include_files: Option<Vec<String>>,
    pub include_directories: Option<Vec<String>>,
    pub exclude_files: Vec<String>,
    pub exclude_directories: Vec<String>,
    pub include_override_regex: Vec<String>,
    pub skip_binary: bool,
    pub skip_gitignore: bool,
    pub exclude_regex: Vec<String>,
}

fn default_git_ref() -> String {
    "WORKTREE".to_string()
}

fn default_skip_gitignore() -> bool {
    true
}



pub async fn execute(
    ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let workflow_key = sqlx::query_scalar::<_, String>(
        "SELECT workflow_key FROM workflow_runs WHERE id = ? LIMIT 1",
    )
    .bind(ctx.run_id.to_string())
    .fetch_optional(&ctx.state.db)
    .await?
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| ctx.run_id.to_string());

    let result = execute_context_export(
        ctx.run_id,
        workflow_key.as_str(),
        resolve_context_export_payload(ctx, config)?,
    )?;

    Ok(CapabilityResult {
        ok: result.get("ok").and_then(Value::as_bool).unwrap_or(true),
        capability: "context_export".to_string(),
        payload: result,
        follow_ups: CapabilityInvocationRequest::None,
    })
}

fn resolve_context_export_payload(ctx: &CapabilityContext<'_>, config: Value) -> Result<Value> {
    let repo_resource = ctx
        .local_state
        .get("resources")
        .and_then(|v| v.get("repo"))
        .cloned();

    let capability_state = ctx
        .local_state
        .get("capabilities")
        .and_then(|v| v.get("context_export"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let payload = if config.is_null() || config == json!({}) {
        capability_state
    } else {
        config
    };

    Ok(normalize_context_export_payload(payload, repo_resource, ctx.repo_ref))
}

pub fn normalize_context_export_payload(payload: Value, repo_resource: Option<Value>, fallback_repo_ref: &str) -> Value {
    let repo_resource = repo_resource.unwrap_or_else(|| json!({
        "repo_ref": fallback_repo_ref,
        "git_ref": "WORKTREE"
    }));

    let mut normalized = match payload {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    };

    let obj = normalized.as_object_mut().expect("context export payload must be object");
    obj.insert(
        "repo_ref".to_string(),
        repo_resource
            .get("repo_ref")
            .cloned()
            .unwrap_or_else(|| Value::String(fallback_repo_ref.to_string())),
    );
    obj.insert(
        "git_ref".to_string(),
        repo_resource
            .get("git_ref")
            .cloned()
            .unwrap_or_else(|| Value::String("WORKTREE".to_string())),
    );
    obj.entry("exclude_regex".to_string()).or_insert_with(|| json!([]));
    obj.entry("include_files".to_string()).or_insert_with(|| json!([]));
    obj.entry("include_directories".to_string()).or_insert_with(|| json!([]));
    obj.entry("exclude_files".to_string()).or_insert_with(|| json!([]));
    obj.entry("exclude_directories".to_string()).or_insert_with(|| json!([]));
    obj.entry("include_override_regex".to_string()).or_insert_with(|| json!([]));
    obj.entry("include_staged_diff".to_string()).or_insert_with(|| Value::Bool(false));
    obj.entry("include_unstaged_diff".to_string()).or_insert_with(|| Value::Bool(false));
    obj.entry("skip_binary".to_string()).or_insert_with(|| Value::Bool(true));
    obj.entry("skip_gitignore".to_string()).or_insert_with(|| Value::Bool(true));
    obj.remove("save_path");
    obj.entry("artifact_kind".to_string()).or_insert_with(|| Value::String("broad".to_string()));
    obj.entry("inline_repo_context_in_prompt".to_string()).or_insert_with(|| Value::Bool(false));
    Value::Object(obj.clone())
}

pub fn parse_context_export_payload(payload: Value) -> Result<ContextExportPayload> {
    serde_json::from_value(payload).context("invalid context export payload")
}

pub fn build_context_sync_snapshot(payload: Value) -> Result<ContextSyncSnapshot> {
    build_context_sync_snapshot_inner(payload)
}

pub fn build_existing_context_sync_snapshot(payload: Value) -> Result<ContextSyncSnapshot> {
    build_context_sync_snapshot_inner(payload)
}

pub fn resolve_context_export_file_count(payload: Value) -> Result<usize> {
    Ok(build_context_sync_snapshot_inner(payload)?.files.len())
}

fn build_context_sync_snapshot_inner(
    payload: Value,
) -> Result<ContextSyncSnapshot> {
    let req = parse_context_export_payload(payload)?;
    let repo = PathBuf::from(&req.repo_ref);
    let compiled_excludes = compile_regex_patterns(&req.exclude_regex, "exclude")?;
    let compiled_overrides = compile_regex_patterns(
        &req.include_override_regex,
        "include override",
    )?;
    let excluded_files = req
        .exclude_files
        .iter()
        .map(|path| normalize_rel_path(path))
        .collect::<Vec<_>>();
    let excluded_directories = req
        .exclude_directories
        .iter()
        .map(|path| normalize_rel_path(path))
        .collect::<Vec<_>>();

    let mut candidates = collect_candidate_files(
        &repo,
        &req.git_ref,
        req.include_files.as_deref(),
        req.include_directories.as_deref(),
        req.skip_gitignore,
    )?;
    candidates.sort();
    candidates.dedup();

    let mut files = Vec::new();
    for rel in candidates {
        let rel = crate::engine::capabilities::filesystem::normalize_rel_path(&rel)?;
        if rel.is_empty()
            || rel == ".git"
            || rel.starts_with(".git/")
            || rel == ".mdev"
            || rel.starts_with(".mdev/")
        {
            continue;
        }

        let overridden = path_matches(&rel, &compiled_overrides);
        let normalized_rel = normalize_rel_path(&rel);
        let explicitly_excluded = excluded_files
            .iter()
            .any(|path| path == &normalized_rel);
        let excluded_by_directory = excluded_directories.iter().any(|directory| {
            normalized_rel == *directory
                || normalized_rel.starts_with(&format!("{directory}/"))
        });

        if (explicitly_excluded
            || excluded_by_directory
            || path_matches(&rel, &compiled_excludes))
            && !overridden
        {
            continue;
        }

        let full = repo.join(&rel);
        if effective_ref(&req.git_ref) == "WORKTREE" && !full.is_file() {
            continue;
        }


        let bytes = match read_file_bytes(&repo, effective_ref(&req.git_ref), &rel) {
            Ok(bytes) => bytes,
            Err(error)
                if effective_ref(&req.git_ref) == "WORKTREE"
                    && !full.exists() =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };

        if req.skip_binary && !overridden && is_probably_binary(&bytes) {
            continue;
        }

        files.push(ContextSyncFile {
            path: rel,
            contents: String::from_utf8_lossy(&bytes).into_owned(),
        });
    }

    Ok(ContextSyncSnapshot {
        files,
        include_files: req.include_files,
        include_directories: req.include_directories,
        exclude_files: req.exclude_files,
        exclude_directories: req.exclude_directories,
        include_override_regex: req.include_override_regex,
        skip_binary: req.skip_binary,
        skip_gitignore: req.skip_gitignore,
        exclude_regex: req.exclude_regex,
    })
}

pub fn render_context_export_text(payload: Value) -> Result<String> {
    let req = parse_context_export_payload(payload)?;
    let repo = PathBuf::from(&req.repo_ref);
    build_context_export_text(&repo, &req)
}

pub fn execute_context_export(
    run_id: uuid::Uuid,
    workflow_key: &str,
    payload: Value,
) -> Result<Value> {
    let req = parse_context_export_payload(payload)?;

    let repo = PathBuf::from(&req.repo_ref);
    let out_path = resolve_context_export_save_path(&repo, workflow_key, &req.artifact_kind);

    tracing::info!(%run_id, repo = %req.repo_ref, git_ref = %req.git_ref, output_path = %out_path.display(), "context export started");
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create parent dir {}", parent.display()))?;
    }

    let export_text = build_context_export_text(&repo, &req)?;
    fs::write(&out_path, export_text.as_bytes())
        .with_context(|| format!("failed to write {}", out_path.display()))?;

    let result = json!({
        "ok": true,
        "output_path": out_path.to_string_lossy().replace('\\', "/"),
        "bytes_written": export_text.len(),
    });

    tracing::info!(%run_id, output_path = %out_path.display(), bytes_written = export_text.len(), "context export completed");
    Ok(result)
}

fn normalize_context_workflow_key(value: &str) -> String {
    let normalized = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();

    let normalized = normalized.trim_matches('-');
    if normalized.is_empty() {
        "workflow".to_string()
    } else {
        normalized.to_string()
    }
}

fn resolve_context_export_save_path(
    repo: &Path,
    workflow_key: &str,
    artifact_kind: &str,
) -> PathBuf {
    let filename = if artifact_kind == "targeted" {
        "targeted_context_file.txt"
    } else {
        "broad_context_file.txt"
    };

    repo.join(".mdev")
        .join("context_files")
        .join(normalize_context_workflow_key(workflow_key))
        .join(filename)
}

fn build_context_export_text(repo: &Path, req: &ContextExportPayload) -> Result<String> {
    let compiled_excludes = compile_regex_patterns(&req.exclude_regex, "exclude")?;
    let compiled_overrides = compile_regex_patterns(&req.include_override_regex, "include override")?;
    let excluded_files = req.exclude_files.iter().map(|path| normalize_rel_path(path)).collect::<Vec<_>>();
    let excluded_directories = req.exclude_directories.iter().map(|path| normalize_rel_path(path)).collect::<Vec<_>>();
    let mut candidates = collect_candidate_files(
        repo,
        &req.git_ref,
        req.include_files.as_deref(),
        req.include_directories.as_deref(),
        req.skip_gitignore,
    )?;
    candidates.sort();
    candidates.dedup();

    let mut rendered = Vec::new();
    for rel in candidates {
        let overridden = path_matches(&rel, &compiled_overrides);
        let normalized_rel = normalize_rel_path(&rel);
        let explicitly_excluded = excluded_files.iter().any(|path| path == &normalized_rel);
        let excluded_by_directory = excluded_directories.iter().any(|directory| normalized_rel == *directory || normalized_rel.starts_with(&format!("{directory}/")));
        if (explicitly_excluded || excluded_by_directory || path_matches(&rel, &compiled_excludes)) && !overridden {
            continue;
        }
        let bytes = match read_file_bytes(repo, effective_ref(&req.git_ref), &rel) {
            Ok(bytes) => bytes,
            Err(error) if effective_ref(&req.git_ref) == "WORKTREE" && !repo.join(&rel).exists() => continue,
            Err(error) => return Err(error),
        };
        if req.skip_binary && !overridden && is_probably_binary(&bytes) {
            continue;
        }
        rendered.push((rel, bytes));
    }

    let mut out = String::new();
    out.push_str(&format!("## Repo Context Export\nrepo: {}\nref: {}\ninclude_staged_diff: {}\ninclude_unstaged_diff: {}\nfiles: {}\n\n", repo.display(), if req.git_ref.is_empty() { "WORKTREE" } else { &req.git_ref }, req.include_staged_diff, req.include_unstaged_diff, rendered.len()));

    for (rel, bytes) in rendered {
        let text = String::from_utf8_lossy(&bytes);
        out.push_str(&format!("==== {} ====\n{}\n\n", rel, text));
    }

    if effective_ref(&req.git_ref) == "WORKTREE" {
        if req.include_staged_diff {
            let diff = run_git_capture_string(repo, &["diff", "--cached"])?;
            if !diff.trim().is_empty() {
                out.push_str("==== STAGED DIFF ====\n");
                out.push_str(&diff);
                out.push_str("\n\n");
            }
        }
        if req.include_unstaged_diff {
            let diff = run_git_capture_string(repo, &["diff"])?;
            if !diff.trim().is_empty() {
                out.push_str("==== UNSTAGED DIFF ====\n");
                out.push_str(&diff);
                out.push_str("\n\n");
            }
        }
    }

    Ok(out)
}

fn collect_candidate_files(
    repo: &Path,
    git_ref: &str,
    include_files: Option<&[String]>,
    include_directories: Option<&[String]>,
    skip_gitignore: bool,
) -> Result<Vec<String>> {
    let files = normalize_selected_paths(include_files.unwrap_or_default())?;
    let directories = normalize_selected_paths(include_directories.unwrap_or_default())?;
    let select_all = files.is_empty() && directories.is_empty();

    if effective_ref(git_ref) == "WORKTREE" {
        if skip_gitignore {
            return collect_git_visible_worktree_files(repo, &files, &directories);
        }

        if select_all {
            let mut out = Vec::new();
            collect_worktree_files(repo, repo, &mut out)?;
            return Ok(out);
        }

        return collect_selected_worktree_files(repo, &files, &directories);
    }

    let available = run_git_capture_string(
        repo,
        &["ls-tree", "-r", "--name-only", effective_ref(git_ref)],
    )?
    .lines()
    .map(normalize_rel_path)
    .filter(|path| !path.is_empty())
    .collect::<Vec<_>>();

    if select_all {
        return Ok(available);
    }

    let explicit = files.into_iter().collect::<std::collections::HashSet<_>>();
    Ok(available
        .into_iter()
        .filter(|path| {
            explicit.contains(path)
                || directories
                    .iter()
                    .any(|directory| path_is_within_directory(path, directory))
        })
        .collect())
}

fn collect_git_visible_worktree_files(
    repo: &Path,
    files: &[String],
    directories: &[String],
) -> Result<Vec<String>> {
    let mut command = Command::new("git");
    command
        .arg("ls-files")
        .arg("--cached")
        .arg("--others")
        .arg("--exclude-standard")
        .arg("--")
        .current_dir(repo);

    for path in files {
        command.arg(path);
    }
    for directory in directories {
        command.arg(directory);
    }

    let output = command
        .output()
        .with_context(|| "failed to enumerate non-ignored Context Exporter files")?;

    if !output.status.success() {
        bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8(output.stdout)?
        .lines()
        .map(normalize_rel_path)
        .filter(|path| {
            !path.is_empty()
                && path != ".git"
                && !path.starts_with(".git/")
                && path != ".mdev"
                && !path.starts_with(".mdev/")
        })
        .collect())
}

fn collect_selected_worktree_files(
    repo: &Path,
    files: &[String],
    directories: &[String],
) -> Result<Vec<String>> {
    let mut out = Vec::new();

    for rel in files {
        let path = repo.join(rel);
        if path.is_file() {
            out.push(rel.clone());
        }
    }

    for rel in directories {
        let path = repo.join(rel);
        if !path.is_dir() {
            continue;
        }
        collect_worktree_files(repo, &path, &mut out)?;
    }

    out.sort();
    out.dedup();
    Ok(out)
}

fn normalize_selected_paths(paths: &[String]) -> Result<Vec<String>> {
    let mut normalized = Vec::new();
    for path in paths {
        let value = validate_relative_repo_path(path)?;
        if !value.is_empty() {
            normalized.push(value);
        }
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn validate_relative_repo_path(path: &str) -> Result<String> {
    let normalized = normalize_rel_path(path.trim()).trim_matches('/').to_string();
    if normalized.is_empty() {
        return Ok(normalized);
    }
    if Path::new(path).is_absolute() || normalized.split('/').any(|component| component == "..") {
        bail!("repository path must be a forward-slash relative path within the repository: {path}");
    }
    Ok(normalized)
}

fn path_is_within_directory(path: &str, directory: &str) -> bool {
    path == directory || path.strip_prefix(directory).is_some_and(|suffix| suffix.starts_with('/'))
}

fn collect_worktree_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if matches!(name.to_string_lossy().as_ref(), ".git" | ".mdev") {
            continue;
        }
        if path.is_dir() {
            collect_worktree_files(root, &path, out)?;
        } else if path.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            out.push(rel);
        }
    }
    Ok(())
}

fn path_matches(path: &str, patterns: &[Regex]) -> bool {
    patterns.iter().any(|regex| regex.is_match(path))
}

fn compile_regex_patterns(patterns: &[String], kind: &str) -> Result<Vec<Regex>> {
    let mut out = Vec::new();
    for pattern in patterns {
        out.push(Regex::new(pattern).with_context(|| format!("invalid {kind} regex '{pattern}'"))?);
    }
    Ok(out)
}

fn read_file_bytes(repo: &Path, git_ref: &str, rel: &str) -> Result<Vec<u8>> {
    if git_ref == "WORKTREE" {
        return fs::read(repo.join(rel)).with_context(|| format!("failed to read {}", rel));
    }
    run_git_capture(repo, &["show", &format!("{}:{}", git_ref, rel)])
}

fn normalize_rel_path(path: &str) -> String {
    path.trim().trim_matches('/').replace('\\', "/")
}

fn effective_ref(git_ref: &str) -> &str {
    if git_ref.trim().is_empty() { "WORKTREE" } else { git_ref }
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    bytes.iter().any(|b| *b == 0)
}

pub(crate) fn is_gitignored(repo: &Path, rel: &str) -> Result<bool> {
    let output = Command::new("git")
        .arg("check-ignore")
        .arg("--quiet")
        .arg("--")
        .arg(rel)
        .current_dir(repo)
        .output()
        .with_context(|| "failed to run git check-ignore")?;
    Ok(output.status.code() == Some(0))
}

fn run_git_capture(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .with_context(|| format!("failed to run git {:?}", args))?;
    if !output.status.success() {
        bail!("git {:?} failed: {}", args, String::from_utf8_lossy(&output.stderr));
    }
    Ok(output.stdout)
}

fn run_git_capture_string(repo: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8(run_git_capture(repo, args)?)?)
}
