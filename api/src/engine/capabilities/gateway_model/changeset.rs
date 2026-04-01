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

#[derive(Debug, Clone, Serialize)]
struct EditActionFailure {
    index: usize,
    action: String,
    error: String,
}

#[derive(Debug, Clone, Serialize)]
struct EditActionResult {
    index: usize,
    action: String,
    status: String,
    descriptor: String,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
struct EditSequenceReport {
    lines: Vec<String>,
    successful_actions: usize,
    failed: Vec<EditActionFailure>,
    action_results: Vec<EditActionResult>,
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
                "total_operations": 0,
                "successful_actions": 0,
                "failed_actions": 1,
                "total_actions": 1
            },
            "attempted": [],
            "failed": ["changeset_decode"],
            "operation_results": [
                {
                    "index": 1,
                    "status": "failed",
                    "kind": "changeset_decode",
                    "label": "changeset_decode",
                    "error": format!("{:#}", err),
                    "action_results": []
                }
            ]
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

    let total_operations = payload.operations.len();
    let total_actions: usize = payload
        .operations
        .iter()
        .map(|op| match op {
            Operation::Edit { changes, .. } => changes.len(),
            _ => 1,
        })
        .sum();

    let mut lines = Vec::new();
    let mut attempted = Vec::new();
    let mut failed = Vec::new();
    let mut successful_operations = 0usize;
    let mut successful_actions = 0usize;
    let mut operation_results = Vec::new();
    let mut first_error = None::<String>;

    for (idx, op) in payload.operations.iter().enumerate() {
        let index = idx + 1;
        let label = operation_label_with_index(index, op);
        lines.push(label.clone());

        if let Some(path) = operation_primary_path(op) {
            attempted.push(path);
        }

        match apply_operation(repo, op) {
            Ok(report) => {
                successful_operations += 1;
                successful_actions += report.successful_actions;
                lines.extend(report.lines.clone());
                lines.push(format!("[{}] ok", index));
                operation_results.push(json!({
                    "index": index,
                    "status": "ok",
                    "kind": operation_kind(op),
                    "label": operation_label(op),
                    "error": Value::Null,
                    "action_results": report.action_results,
                }));
            }
            Err(err) => {
                let err_text = format!("{:#}", err);
                if first_error.is_none() {
                    first_error = Some(err_text.clone());
                }
                if let Some(path) = operation_primary_path(op) {
                    failed.push(path);
                } else {
                    failed.push(operation_label(op));
                }
                lines.push(format!("[{}] FAILED: {}", index, err_text));
                operation_results.push(json!({
                    "index": index,
                    "status": "failed",
                    "kind": operation_kind(op),
                    "label": operation_label(op),
                    "error": err_text,
                    "action_results": [],
                }));
            }
        }
    }

    let failed_operations = total_operations.saturating_sub(successful_operations);
    let failed_actions = total_actions.saturating_sub(successful_actions);
    let summary = format_apply_summary(
        successful_operations,
        total_operations,
        successful_actions,
        total_actions,
    );
    let status = if failed_operations == 0 {
        "ChangeSet applied successfully.".to_string()
    } else if failed_operations == total_operations {
        match first_error {
            Some(err) => format!("All {} operations failed. First error: {}", total_operations, err),
            None => format!("All {} operations failed.", total_operations),
        }
    } else {
        match first_error {
            Some(err) => format!(
                "Partially applied ChangeSet: {} succeeded, {} failed. First error: {}",
                successful_operations,
                failed_operations,
                err
            ),
            None => format!(
                "Partially applied ChangeSet: {} succeeded, {} failed.",
                successful_operations,
                failed_operations
            ),
        }
    };

    Ok(json!({
        "ok": failed_operations == 0,
        "mode": "changeset_apply",
        "summary": summary,
        "status": status,
        "target": {
            "repo_ref": repo.to_string_lossy(),
            "git_ref": git_ref,
        },
        "stats": {
            "successful_operations": successful_operations,
            "failed_operations": failed_operations,
            "total_operations": total_operations,
            "successful_actions": successful_actions,
            "failed_actions": failed_actions,
            "total_actions": total_actions
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

fn apply_operation(repo: &Path, op: &Operation) -> Result<EditSequenceReport> {
    match op {
        Operation::Write { path, contents } => {
            let full = repo.join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&full, contents)?;
            Ok(EditSequenceReport {
                lines: vec!["  - PASS write[1] write".to_string()],
                successful_actions: 1,
                failed: Vec::new(),
                action_results: vec![EditActionResult {
                    index: 1,
                    action: "write".to_string(),
                    status: "ok".to_string(),
                    descriptor: "write[1] write".to_string(),
                    error: None,
                }],
            })
        }
        Operation::Delete { path } => {
            let full = repo.join(path);
            if full.exists() {
                fs::remove_file(&full).or_else(|_| fs::remove_dir_all(&full))?;
            }
            Ok(EditSequenceReport {
                lines: vec!["  - PASS delete[1] delete".to_string()],
                successful_actions: 1,
                failed: Vec::new(),
                action_results: vec![EditActionResult {
                    index: 1,
                    action: "delete".to_string(),
                    status: "ok".to_string(),
                    descriptor: "delete[1] delete".to_string(),
                    error: None,
                }],
            })
        }
        Operation::Move { from, to } => {
            let src = repo.join(from);
            let dst = repo.join(to);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&src, &dst)?;
            Ok(EditSequenceReport {
                lines: vec!["  - PASS move[1] move".to_string()],
                successful_actions: 1,
                failed: Vec::new(),
                action_results: vec![EditActionResult {
                    index: 1,
                    action: "move".to_string(),
                    status: "ok".to_string(),
                    descriptor: "move[1] move".to_string(),
                    error: None,
                }],
            })
        }
        Operation::Edit { path, changes } => apply_edit_sequence(repo, path, changes),
    }
}

fn apply_edit_change(input: &str, change: &EditAction) -> Result<String> {
    if change.match_spec.match_type != "literal" {
        bail!("unsupported match.type {}", change.match_spec.match_type);
    }

    let needle = if change.match_spec.mode == "normalized_newlines" {
        change.match_spec.text.replace("\r\n", "\n")
    } else {
        change.match_spec.text.clone()
    };

    let haystack = if change.match_spec.mode == "normalized_newlines" {
        input.replace("\r\n", "\n")
    } else {
        input.to_string()
    };

    let mut matches = Vec::new();
    let mut start = 0usize;
    while let Some(idx) = haystack[start..].find(&needle) {
        let abs = start + idx;
        matches.push(abs);
        start = abs + needle.len();
    }

    match change.match_spec.must_match.as_str() {
        "exactly_one" if matches.len() != 1 => {
            bail!("Expected exactly one match, found {}", matches.len());
        }
        "at_least_one" if matches.is_empty() => {
            bail!("Expected at least one match, found none");
        }
        "exactly_one" | "at_least_one" => {}
        other => bail!("Unsupported must_match '{}'", other),
    }

    let occurrence = change.match_spec.occurrence.max(1);
    let target = matches
        .get(occurrence.saturating_sub(1))
        .copied()
        .context("Requested occurrence not found")?;
    let end = target + needle.len();

    match change.action.as_str() {
        "replace_block" => {
            let replacement = change.replacement.clone().ok_or_else(|| anyhow!("replacement is required"))?;
            Ok(format!("{}{}{}", &haystack[..target], replacement, &haystack[end..]))
        }
        "insert_before" => {
            let text = change.text.clone().ok_or_else(|| anyhow!("text is required"))?;
            Ok(format!("{}{}{}", &haystack[..target], text, &haystack[target..]))
        }
        "insert_after" => {
            let text = change.text.clone().ok_or_else(|| anyhow!("text is required"))?;
            Ok(format!("{}{}{}", &haystack[..end], text, &haystack[end..]))
        }
        "delete_block" => Ok(format!("{}{}", &haystack[..target], &haystack[end..])),
        other => bail!("unsupported edit action {}", other),
    }
}

fn operation_primary_path(op: &Operation) -> Option<String> {
    match op {
        Operation::Write { path, .. } => Some(path.clone()),
        Operation::Delete { path } => Some(path.clone()),
        Operation::Move { to, .. } => Some(to.clone()),
        Operation::Edit { path, .. } => Some(path.clone()),
    }
}

fn describe_edit_change(index: usize, change: &EditAction) -> String {
    format!(
        "edit[{}] {} (occurrence={}, must_match={}, mode={})",
        index,
        change.action,
        change.match_spec.occurrence.max(1),
        change.match_spec.must_match,
        change.match_spec.mode
    )
}

fn apply_edit_sequence(repo: &Path, path: &str, changes: &[EditAction]) -> Result<EditSequenceReport> {
    let full = repo.join(path);
    let mut report = EditSequenceReport::default();

    let mut text = fs::read_to_string(&full)
        .with_context(|| format!("Failed to read {path} for edit"))?;

    for (idx, change) in changes.iter().enumerate() {
        let descriptor = describe_edit_change(idx + 1, change);
        let pass_descriptor = descriptor.replacen("edit[", "PASS edit[", 1);

        match apply_edit_change(&text, change) {
            Ok(next_text) => {
                text = next_text;
                report.successful_actions += 1;
                report.lines.push(format!("  - {}", pass_descriptor));
                report.action_results.push(EditActionResult {
                    index: idx + 1,
                    action: change.action.clone(),
                    status: "ok".to_string(),
                    descriptor,
                    error: None,
                });
            }
            Err(e) => {
                let err_text = format!("{e:#}");
                let fail_descriptor = descriptor.replacen("edit[", "FAIL edit[", 1);
                report.lines.push(format!("  - {}", fail_descriptor));
                report.lines.push(format!("      {}", err_text));
                report.failed.push(EditActionFailure {
                    index: idx + 1,
                    action: change.action.clone(),
                    error: err_text.clone(),
                });
                report.action_results.push(EditActionResult {
                    index: idx + 1,
                    action: change.action.clone(),
                    status: "failed".to_string(),
                    descriptor,
                    error: Some(err_text),
                });
            }
        }
    }

    if report.successful_actions > 0 {
        fs::write(&full, text.as_bytes())
            .with_context(|| format!("Failed to write edited file {path}"))?;
    }

    if !report.failed.is_empty() {
        let first = report
            .failed
            .first()
            .map(|item| format!("edit[{}] {}: {}", item.index, item.action, item.error))
            .unwrap_or_else(|| "edit sequence failed".to_string());
        bail!(first);
    }

    Ok(report)
}

fn format_apply_summary(
    successful_operations: usize,
    total_operations: usize,
    successful_actions: usize,
    total_actions: usize,
) -> String {
    format!(
        "Applied {}/{} operations successfully. Applied {}/{} actions successfully.",
        successful_operations,
        total_operations,
        successful_actions,
        total_actions
    )
}

fn operation_kind(op: &Operation) -> &'static str {
    match op {
        Operation::Write { .. } => "write",
        Operation::Delete { .. } => "delete",
        Operation::Move { .. } => "move",
        Operation::Edit { .. } => "edit",
    }
}

fn operation_label(op: &Operation) -> String {
    match op {
        Operation::Write { path, .. } => format!("write {}", path),
        Operation::Delete { path } => format!("delete {}", path),
        Operation::Move { from, to } => format!("move {} -> {}", from, to),
        Operation::Edit { path, .. } => format!("edit {}", path),
    }
}

fn operation_label_with_index(index: usize, op: &Operation) -> String {
    match op {
        Operation::Write { path, .. } => format!("[{}] write {}", index, path),
        Operation::Delete { path } => format!("[{}] delete {}", index, path),
        Operation::Move { from, to } => format!("[{}] move {} -> {}", index, from, to),
        Operation::Edit { path, .. } => format!("[{}] edit {}", index, path),
    }
}
