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
    pub include_staged_diff: bool,
    #[serde(default)]
    pub include_unstaged_diff: bool,
    #[serde(default)]
    pub skip_binary: bool,
    #[serde(default)]
    pub skip_gitignore: bool,
    #[serde(default)]
    pub exclude_regex: Vec<String>,
    #[serde(default)]
    pub save_path: String,
}

fn default_git_ref() -> String {
    "WORKTREE".to_string()
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let result = execute_context_export(ctx.run_id, resolve_context_export_payload(ctx, config)?)?;

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
    obj.entry("repo_ref".to_string()).or_insert_with(|| {
        repo_resource
            .get("repo_ref")
            .cloned()
            .unwrap_or_else(|| Value::String(fallback_repo_ref.to_string()))
    });
    obj.entry("git_ref".to_string()).or_insert_with(|| {
        repo_resource
            .get("git_ref")
            .cloned()
            .unwrap_or_else(|| Value::String("WORKTREE".to_string()))
    });
    obj.entry("exclude_regex".to_string()).or_insert_with(|| json!([]));
    obj.entry("include_files".to_string()).or_insert_with(|| json!([]));
    obj.entry("include_staged_diff".to_string()).or_insert_with(|| Value::Bool(false));
    obj.entry("include_unstaged_diff".to_string()).or_insert_with(|| Value::Bool(false));
    obj.entry("skip_binary".to_string()).or_insert_with(|| Value::Bool(true));
    obj.entry("skip_gitignore".to_string()).or_insert_with(|| Value::Bool(true));
    obj.entry("save_path".to_string()).or_insert_with(|| Value::String("/tmp/repo_context.txt".to_string()));
    Value::Object(obj.clone())
}

pub fn parse_context_export_payload(payload: Value) -> Result<ContextExportPayload> {
    serde_json::from_value(payload).context("invalid context export payload")
}

pub fn render_context_export_text(payload: Value) -> Result<String> {
    let req = parse_context_export_payload(payload)?;
    let repo = PathBuf::from(&req.repo_ref);
    build_context_export_text(&repo, &req)
}

pub fn execute_context_export(run_id: uuid::Uuid, payload: Value) -> Result<Value> {
    let req = parse_context_export_payload(payload)?;

    tracing::info!(%run_id, repo = %req.repo_ref, git_ref = %req.git_ref, save_path = %req.save_path, "context export started");

    let repo = PathBuf::from(&req.repo_ref);
    let out_path = resolve_context_export_save_path(&req);
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

fn default_context_export_save_path() -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut p = std::env::temp_dir();
    p.push(format!("repo_context_{ts}.txt"));
    p
}

fn resolve_context_export_save_path(req: &ContextExportPayload) -> PathBuf {
    if req.save_path.trim().is_empty() {
        default_context_export_save_path()
    } else {
        PathBuf::from(&req.save_path)
    }
}

fn build_context_export_text(repo: &Path, req: &ContextExportPayload) -> Result<String> {
    let compiled = compile_excludes(&req.exclude_regex)?;
    let mut files = collect_candidate_files(repo, &req.git_ref, req.include_files.as_ref())?;
    files.sort();
    files.dedup();

    let mut out = String::new();
    out.push_str(&format!("## Repo Context Export\nrepo: {}\nref: {}\ninclude_staged_diff: {}\ninclude_unstaged_diff: {}\nfiles: {}\n\n", repo.display(), if req.git_ref.is_empty() { "WORKTREE" } else { &req.git_ref }, req.include_staged_diff, req.include_unstaged_diff, files.len()));

    for rel in files {
        if path_is_excluded(&rel, &compiled) {
            continue;
        }
        if req.skip_gitignore && is_gitignored(repo, &rel)? {
            continue;
        }
        let bytes = read_file_bytes(repo, effective_ref(&req.git_ref), &rel)?;
        if req.skip_binary && is_probably_binary(&bytes) {
            continue;
        }
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

fn collect_candidate_files(repo: &Path, git_ref: &str, include_files: Option<&Vec<String>>) -> Result<Vec<String>> {
    if let Some(include_files) = include_files {
        return Ok(include_files.iter().map(|p| normalize_rel_path(p)).filter(|p| !p.is_empty()).collect());
    }

    if effective_ref(git_ref) == "WORKTREE" {
        let mut out = Vec::new();
        collect_worktree_files(repo, repo, &mut out)?;
        return Ok(out);
    }

    let stdout = run_git_capture_string(repo, &["ls-tree", "-r", "--name-only", effective_ref(git_ref)])?;
    Ok(stdout
        .lines()
        .map(normalize_rel_path)
        .filter(|p| !p.is_empty())
        .collect())
}

fn collect_worktree_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy() == ".git" {
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

fn path_is_excluded(path: &str, patterns: &[Regex]) -> bool {
    patterns.iter().any(|re| re.is_match(path))
}

fn compile_excludes(patterns: &[String]) -> Result<Vec<Regex>> {
    let mut out = Vec::new();
    for p in patterns {
        out.push(Regex::new(p).with_context(|| format!("Bad exclude regex '{p}'"))?);
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

fn is_gitignored(repo: &Path, rel: &str) -> Result<bool> {
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
