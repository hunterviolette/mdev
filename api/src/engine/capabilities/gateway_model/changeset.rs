use std::{fs, path::{Path, PathBuf}};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::engine::capabilities::registry::{
    find_result,
    CapabilityContext,
    CapabilityInvocationRequest,
    CapabilityResult,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApplyChangesetTarget {
    pub repo_ref: String,
    #[serde(default = "default_apply_git_ref")]
    pub git_ref: String,
}

fn default_apply_git_ref() -> String {
    "WORKTREE".to_string()
}

fn parse_apply_changeset_target(payload: Value) -> Result<ApplyChangesetTarget> {
    serde_json::from_value(payload).context("invalid apply_changeset target payload")
}

fn resolve_apply_changeset_target(ctx: &CapabilityContext<'_>, config: Value) -> Result<ApplyChangesetTarget> {
    let mut payload = if config.is_null() || config == json!({}) {
        ctx.local_state
            .get("repo_context")
            .cloned()
            .unwrap_or_else(|| json!({
                "repo_ref": ctx.repo_ref,
                "git_ref": "WORKTREE"
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
    }

    parse_apply_changeset_target(payload)
}

#[derive(Debug, Deserialize, Serialize)]
struct ChangeSetPayload {
    version: u32,
    #[serde(default)]
    description: String,
    operations: Vec<Operation>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Operation {
    Write { path: String, contents: String },
    Delete { path: String },
    Move { from: String, to: String },
    Edit { path: String, changes: Vec<EditAction> },
}

#[derive(Debug, Deserialize, Serialize)]
struct EditAction {
    action: String,
    #[serde(rename = "match")]
    match_spec: LiteralMatch,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    replacement: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LiteralMatch {
    #[serde(rename = "type")]
    match_type: String,
    mode: String,
    must_match: String,
    occurrence: usize,
    text: String,
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let inference = find_result(prior_results, "inference");
    let payload_text = inference
        .and_then(|item| item.payload.get("result"))
        .and_then(|v| v.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if payload_text.trim().is_empty() {
        return Ok(CapabilityResult {
            ok: false,
            capability: "gateway_model/changeset".to_string(),
            payload: json!({
                "ok": false,
                "summary": "Inference returned an empty ChangeSet payload.",
                "payload_text": payload_text,
            }),
            follow_ups: CapabilityInvocationRequest::None,
        });
    }

    let target = resolve_apply_changeset_target(ctx, config)?;

    let result = match execute_changeset_apply(
        PathBuf::from(&target.repo_ref).as_path(),
        &payload_text,
        &target.git_ref,
    ) {
        Ok(result) => result,
        Err(err) => json!({
            "ok": false,
            "mode": "changeset_apply",
            "summary": format!("ChangeSet apply failed: {:#}", err),
            "payload_text": payload_text,
            "lines": [format!("ChangeSet parse/apply error :: {:#}", err)],
            "target": {
                "repo_ref": target.repo_ref,
                "git_ref": target.git_ref,
            },
            "stats": {
                "successful_operations": 0,
                "failed_operations": 1,
                "total_operations": 0
            },
            "attempted": [],
            "failed": ["changeset_decode"],
            "operation_results": []
        }),
    };

    let mut result = result;
    if let Some(obj) = result.as_object_mut() {
        obj.insert("target".to_string(), json!({
            "repo_ref": target.repo_ref,
            "git_ref": target.git_ref,
        }));
    }

    Ok(CapabilityResult {
        ok: result.get("ok").and_then(Value::as_bool).unwrap_or(false),
        capability: "gateway_model/changeset".to_string(),
        payload: result,
        follow_ups: CapabilityInvocationRequest::None,
    })
}

fn execute_changeset_apply(repo: &Path, payload_text: &str, git_ref: &str) -> Result<Value> {
    let normalized = normalize_changeset_payload_text(payload_text)?;
    let payload: ChangeSetPayload = serde_json::from_str(&normalized)
        .context("failed to decode normalized changeset")?;

    let mut lines = Vec::new();
    let mut attempted = Vec::new();
    let mut failed = Vec::new();
    let mut successful_operations = 0usize;
    let mut operation_results = Vec::new();

    for (idx, op) in payload.operations.iter().enumerate() {
        match apply_operation(repo, op) {
            Ok(label) => {
                attempted.push(label.clone());
                lines.push(format!("[{}] ok {}", idx + 1, label));
                successful_operations += 1;
                operation_results.push(json!({
                    "index": idx + 1,
                    "status": "ok",
                    "label": label,
                    "error": Value::Null,
                }));
            }
            Err(err) => {
                let label = operation_label(op);
                attempted.push(label.clone());
                failed.push(label.clone());
                lines.push(format!("[{}] fail {} :: {:#}", idx + 1, label, err));
                operation_results.push(json!({
                    "index": idx + 1,
                    "status": "failed",
                    "label": label,
                    "error": format!("{:#}", err),
                }));
            }
        }
    }

    let total_operations = payload.operations.len();
    let summary = format!("Applied {}/{} operations successfully.", successful_operations, total_operations);

    Ok(json!({
        "ok": failed.is_empty(),
        "mode": "changeset_apply",
        "summary": summary,
        "target": {
            "repo_ref": repo.to_string_lossy(),
            "git_ref": git_ref,
        },
        "stats": {
            "successful_operations": successful_operations,
            "failed_operations": failed.len(),
            "total_operations": total_operations,
        },
        "attempted": attempted,
        "failed": failed,
        "operation_results": operation_results,
        "lines": lines,
        "normalized_payload": normalized,
    }))
}

fn extract_json_object_slice(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, &byte) in bytes.iter().enumerate() {
        let ch = byte as char;

        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => {
                if start.is_none() {
                    start = Some(idx);
                }
                depth += 1;
            }
            '}' => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0 {
                    if let Some(start_idx) = start {
                        return Some(&text[start_idx..=idx]);
                    }
                }
            }
            _ => {}
        }
    }

    None
}

fn normalize_changeset_payload_text(payload_text: &str) -> Result<String> {
    let mut text = payload_text.trim().to_string();

    if text.starts_with("```") {
        let mut lines = text.lines();
        let _ = lines.next();
        text = lines.collect::<Vec<_>>().join("\n");
        if let Some(idx) = text.rfind("```") {
            text.truncate(idx);
        }
        text = text.trim().to_string();
    }

    if text.is_empty() {
        bail!("changeset payload was empty");
    }

    let json_slice = extract_json_object_slice(&text)
        .context("no JSON object found in changeset payload")?;

    let payload: ChangeSetPayload = serde_json::from_str(json_slice)
        .context("failed to parse changeset payload JSON")?;

    if payload.version != 1 {
        bail!("unsupported changeset version {} (expected 1)", payload.version);
    }

    let normalized = serde_json::to_string_pretty(&payload)
        .context("failed to normalize changeset payload")?;

    Ok(normalized)
}

fn apply_operation(repo: &Path, op: &Operation) -> Result<String> {
    match op {
        Operation::Write { path, contents } => {
            let full = repo.join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&full, contents)?;
            Ok(format!("write {}", path))
        }
        Operation::Delete { path } => {
            let full = repo.join(path);
            if full.exists() {
                fs::remove_file(&full).or_else(|_| fs::remove_dir_all(&full))?;
            }
            Ok(format!("delete {}", path))
        }
        Operation::Move { from, to } => {
            let src = repo.join(from);
            let dst = repo.join(to);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&src, &dst)?;
            Ok(format!("move {} -> {}", from, to))
        }
        Operation::Edit { path, changes } => {
            let full = repo.join(path);
            let mut text = fs::read_to_string(&full)
                .with_context(|| format!("failed to read {}", path))?;
            for change in changes {
                text = apply_edit_change(&text, change)?;
            }
            fs::write(&full, text)?;
            Ok(format!("edit {}", path))
        }
    }
}

fn apply_edit_change(input: &str, change: &EditAction) -> Result<String> {
    let match_text = &change.match_spec.text;
    let occurrence = change.match_spec.occurrence.max(1);
    let start = nth_match_start(input, match_text, occurrence)
        .ok_or_else(|| anyhow!("literal match not found for occurrence {}", occurrence))?;
    let end = start + match_text.len();

    match change.action.as_str() {
        "replace_block" => {
            let replacement = change.replacement.clone().ok_or_else(|| anyhow!("replacement is required"))?;
            let mut out = String::new();
            out.push_str(&input[..start]);
            out.push_str(&replacement);
            out.push_str(&input[end..]);
            Ok(out)
        }
        "insert_before" => {
            let text = change.text.clone().ok_or_else(|| anyhow!("text is required"))?;
            let mut out = String::new();
            out.push_str(&input[..start]);
            out.push_str(&text);
            out.push_str(&input[start..]);
            Ok(out)
        }
        "insert_after" => {
            let text = change.text.clone().ok_or_else(|| anyhow!("text is required"))?;
            let mut out = String::new();
            out.push_str(&input[..end]);
            out.push_str(&text);
            out.push_str(&input[end..]);
            Ok(out)
        }
        "delete_block" => {
            let mut out = String::new();
            out.push_str(&input[..start]);
            out.push_str(&input[end..]);
            Ok(out)
        }
        other => bail!("unsupported edit action {}", other),
    }
}

fn nth_match_start(haystack: &str, needle: &str, occurrence: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let mut from = 0usize;
    let mut seen = 0usize;
    while let Some(idx) = haystack[from..].find(needle) {
        let absolute = from + idx;
        seen += 1;
        if seen == occurrence {
            return Some(absolute);
        }
        from = absolute + needle.len();
    }
    None
}

fn operation_label(op: &Operation) -> String {
    match op {
        Operation::Write { path, .. } => format!("write {}", path),
        Operation::Delete { path } => format!("delete {}", path),
        Operation::Move { from, to } => format!("move {} -> {}", from, to),
        Operation::Edit { path, .. } => format!("edit {}", path),
    }
}
