use crate::app::actions::ComponentId;
use crate::app::state::{AppState, WORKTREE_REF};


use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize, serde::Serialize)]
struct ChangeSetPayload {
    version: u32,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    operations: Vec<Operation>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(tag = "op")]
enum Operation {
    #[serde(rename = "write")]
    Write { path: String, contents: String },

    #[serde(rename = "delete")]
    Delete { path: String },

    #[serde(rename = "move")]
    Move { from: String, to: String },

    #[serde(rename = "git_apply")]
    GitApply { patch: String },

    #[serde(rename = "edit")]
    Edit { path: String, changes: Vec<EditChange> },
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct EditChange {
    action: EditAction,
    #[serde(default, rename = "match")]
    match_: Option<EditMatch>,
    #[serde(default)]
    replacement: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum EditAction {
    ReplaceBlock,
    InsertBefore,
    InsertAfter,
    DeleteBlock,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct EditMatch {
    #[serde(rename = "type")]
    match_type: String,
    text: String,
    #[serde(default)]
    occurrence: Option<usize>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    must_match: Option<String>,
}


pub const CHANGESET_SCHEMA_EXAMPLE: &str = r#"{
  \"version\": 1,
  \"description\": \"Schema example. Do not waste tokens/operations inserting or adjusting comments unless required.\",
  \"operations\": [
    {
      \"op\": \"edit\",
      \"path\": \"src/app/ui/changeset_applier.rs\",
      \"changes\": [
        {
          \"action\": \"insert_before\",
          \"match\": {
            \"type\": \"literal\",
            \"mode\": \"normalized_newlines\",
            \"must_match\": \"exactly_one\",
            \"occurrence\": 1,
            \"text\": \"ui.label(\\\"Payload\\\");\"
          },
          \"text\": \"    // inserted comment (example)\\n\"
        },
        {
          \"action\": \"replace_block\",
          \"match\": {
            \"type\": \"literal\",
            \"mode\": \"normalized_newlines\",
            \"must_match\": \"exactly_one\",
            \"occurrence\": 1,
            \"text\": \"ui.label(\\\"Payload\\\");\"
          },
          \"replacement\": \"ui.label(\\\"Payload (example)\\\");\"
        },
        {
          \"action\": \"insert_after\",
          \"match\": {
            \"type\": \"literal\",
            \"mode\": \"normalized_newlines\",
            \"must_match\": \"exactly_one\",
            \"occurrence\": 1,
            \"text\": \"egui::ScrollArea::vertical().id_source(\\\"example_scroll_id\\\")\"
          },
          \"text\": \"\\n                .id_source(\\\"example_scroll_id\\\")\"
        }
      ]
    },
    {
      \"op\": \"write\",
      \"path\": \"tmp/changeset_example.txt\",
      \"contents\": \"hello from write\\n\"
    },
    {
      \"op\": \"move\",
      \"from\": \"tmp/changeset_example.txt\",
      \"to\": \"tmp/changeset_example_moved.txt\"
    },
    {
      \"op\": \"delete\",
      \"path\": \"tmp/changeset_example_moved.txt\"
    }
  ]
}"#;

fn extract_json_object_slice(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end < start {
        return None;
    }
    Some(&text[start..=end])
}

pub fn normalize_and_validate_payload_text(raw: &str) -> Result<String> {
    let json = extract_json_object_slice(raw)
        .context("No JSON object found in payload. Paste the full payload.")?;
    let payload: ChangeSetPayload = serde_json::from_str(json)
        .context("Failed to parse changeset payload JSON")?;

    if payload.version != 1 {
        bail!("Unsupported changeset version {} (expected 1)", payload.version);
    }

    let normalized = serde_json::to_string_pretty(&payload)
        .context("Failed to normalize changeset payload")?;
    Ok(normalized)
}

pub fn apply(state: &mut AppState, applier_id: ComponentId) {
    let Some(repo) = state.inputs.repo.clone() else {
        set_applier_response(state, applier_id, "No repo selected.".to_string());
        return;
    };

    let raw = match state.changeset_appliers.get(&applier_id) {
        Some(st) => st.payload.clone(),
        None => return,
    };

    if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
        st.last_changeset_payload = raw.clone();
        st.result_payload.clear();
        st.changeset_show_result = false;
    }

    let normalized = match normalize_and_validate_payload_text(&raw) {
        Ok(v) => v,
        Err(e) => {
            set_applier_response(state, applier_id, format!("Invalid changeset payload: {e:#}"));
            return;
        }
    };

    let payload: ChangeSetPayload = match serde_json::from_str(&normalized) {
        Ok(v) => v,
        Err(e) => {
            set_applier_response(state, applier_id, format!("Failed to decode normalized changeset: {e:#}"));
            return;
        }
    };

    if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
        st.payload = normalized.clone();
        st.last_changeset_payload = normalized;
        st.last_attempted_paths.clear();
        st.last_failed_paths.clear();
    }

    let total_operations = payload.operations.len();
    let total_actions: usize = payload
        .operations
        .iter()
        .map(|op| match op {
            Operation::Edit { changes, .. } => changes.len(),
            _ => 1,
        })
        .sum();

    let mut attempted = Vec::new();
    let mut failed = Vec::new();
    let mut lines = Vec::new();
    let mut operation_failures = 0usize;
    let mut successful_operations = 0usize;
    let mut successful_actions = 0usize;
    let mut first_error = None::<String>;

    for (idx, op) in payload.operations.into_iter().enumerate() {
        let index = idx + 1;
        let label = operation_label(index, &op);
        lines.push(label);

        let target_path = operation_primary_path(&op);
        if let Some(path) = target_path.as_ref() {
            attempted.push(path.clone());
        }

        match op {
            Operation::Write { path, contents } => {
                let result = std::fs::write(repo.join(&path), contents.as_bytes())
                    .with_context(|| format!("Failed to write {path}"));
                match result {
                    Ok(()) => {
                        successful_operations += 1;
                        successful_actions += 1;
                        lines.push(format!("[{}] ok", index));
                    }
                    Err(e) => {
                        operation_failures += 1;
                        if let Some(path) = target_path {
                            failed.push(path);
                        }
                        let err_text = format!("{e:#}");
                        if first_error.is_none() {
                            first_error = Some(err_text.clone());
                        }
                        lines.push(format!("[{}] FAILED: {}", index, err_text));
                    }
                }
            }
            Operation::Delete { path } => {
                let result = std::fs::remove_file(repo.join(&path))
                    .with_context(|| format!("Failed to delete {path}"));
                match result {
                    Ok(()) => {
                        successful_operations += 1;
                        successful_actions += 1;
                        lines.push(format!("[{}] ok", index));
                    }
                    Err(e) => {
                        operation_failures += 1;
                        if let Some(path) = target_path {
                            failed.push(path);
                        }
                        let err_text = format!("{e:#}");
                        if first_error.is_none() {
                            first_error = Some(err_text.clone());
                        }
                        lines.push(format!("[{}] FAILED: {}", index, err_text));
                    }
                }
            }
            Operation::Move { from, to } => {
                let result = std::fs::rename(repo.join(&from), repo.join(&to))
                    .with_context(|| format!("Failed to move {from} -> {to}"));
                match result {
                    Ok(()) => {
                        successful_operations += 1;
                        successful_actions += 1;
                        lines.push(format!("[{}] ok", index));
                    }
                    Err(e) => {
                        operation_failures += 1;
                        if let Some(path) = target_path {
                            failed.push(path);
                        }
                        let err_text = format!("{e:#}");
                        if first_error.is_none() {
                            first_error = Some(err_text.clone());
                        }
                        lines.push(format!("[{}] FAILED: {}", index, err_text));
                    }
                }
            }
            Operation::GitApply { patch } => {
                let result = crate::git::apply_git_patch(&repo, &patch);
                match result {
                    Ok(()) => {
                        successful_operations += 1;
                        successful_actions += 1;
                        lines.push(format!("[{}] ok", index));
                    }
                    Err(e) => {
                        operation_failures += 1;
                        if let Some(path) = target_path {
                            failed.push(path);
                        }
                        let err_text = format!("{e:#}");
                        if first_error.is_none() {
                            first_error = Some(err_text.clone());
                        }
                        lines.push(format!("[{}] FAILED: {}", index, err_text));
                    }
                }
            }
            Operation::Edit { path, changes } => {
                match apply_edit_sequence(&repo, &path, &changes) {
                    Ok(report) => {
                        successful_actions += report.successful_actions;
                        lines.extend(report.lines);

                        if report.failed.is_empty() {
                            successful_operations += 1;
                            lines.push(format!("[{}] ok", index));
                        } else {
                            operation_failures += 1;
                            if let Some(path) = target_path {
                                failed.push(path);
                            }

                            if first_error.is_none() {
                                first_error = report
                                    .failed
                                    .first()
                                    .map(|f| format!("edit[{}] {}: {}", f.index, f.action, f.error));
                            }

                            if report.successful_actions > 0 {
                                lines.push(format!(
                                    "[{}] PARTIAL: {} passed, {} failed",
                                    index,
                                    report.successful_actions,
                                    report.failed.len()
                                ));
                            } else {
                                lines.push(format!("[{}] FAILED: no edit actions applied successfully", index));
                            }
                        }
                    }
                    Err(e) => {
                        operation_failures += 1;
                        if let Some(path) = target_path {
                            failed.push(path);
                        }
                        let err_text = format!("{e:#}");
                        if first_error.is_none() {
                            first_error = Some(err_text.clone());
                        }
                        lines.push(format!("[{}] FAILED: {}", index, err_text));
                    }
                }
            }
        }
    }

    let status = if operation_failures == 0 {
        "ChangeSet applied successfully.".to_string()
    } else if operation_failures == total_operations {
        match first_error {
            Some(err) => format!("All {} operations failed. First error: {}", total_operations, err),
            None => format!("All {} operations failed.", total_operations),
        }
    } else {
        match first_error {
            Some(err) => format!(
                "Partially applied ChangeSet: {} succeeded, {} failed. First error: {}",
                total_operations.saturating_sub(operation_failures),
                operation_failures,
                err
            ),
            None => format!(
                "Partially applied ChangeSet: {} succeeded, {} failed.",
                total_operations.saturating_sub(operation_failures),
                operation_failures
            ),
        }
    };

    set_applier_result(
        state,
        applier_id,
        attempted,
        failed,
        status,
        successful_operations,
        total_operations,
        successful_actions,
        total_actions,
        lines,
    );
}


fn apply_edit_change(input: &str, change: &EditChange) -> Result<String> {
    let m = change.match_.as_ref().context("Edit change missing match block")?;
    if m.match_type != "literal" {
        bail!("Unsupported match.type '{}'", m.match_type);
    }

    let needle = if m.mode.as_deref() == Some("normalized_newlines") {
        m.text.replace("\r\n", "\n")
    } else {
        m.text.clone()
    };

    let haystack = if m.mode.as_deref() == Some("normalized_newlines") {
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

    let must_match = m.must_match.as_deref().unwrap_or("exactly_one");
    match must_match {
        "exactly_one" if matches.len() != 1 => {
            bail!("Expected exactly one match, found {}", matches.len());
        }
        "at_least_one" if matches.is_empty() => {
            bail!("Expected at least one match, found none");
        }
        "exactly_one" | "at_least_one" => {}
        other => bail!("Unsupported must_match '{}'", other),
    }

    let occurrence = m.occurrence.unwrap_or(1);
    let target = matches
        .get(occurrence.saturating_sub(1))
        .copied()
        .context("Requested occurrence not found")?;
    let end = target + needle.len();

    let result = match change.action {
        EditAction::ReplaceBlock => {
            let replacement = change.replacement.as_deref().context("replace_block requires replacement")?;
            format!("{}{}{}", &haystack[..target], replacement, &haystack[end..])
        }
        EditAction::InsertBefore => {
            let text = change.text.as_deref().context("insert_before requires text")?;
            format!("{}{}{}", &haystack[..target], text, &haystack[target..])
        }
        EditAction::InsertAfter => {
            let text = change.text.as_deref().context("insert_after requires text")?;
            format!("{}{}{}", &haystack[..end], text, &haystack[end..])
        }
        EditAction::DeleteBlock => format!("{}{}", &haystack[..target], &haystack[end..]),
    };

    Ok(result)
}

fn operation_primary_path(op: &Operation) -> Option<String> {
    match op {
        Operation::Write { path, .. } => Some(path.clone()),
        Operation::Delete { path } => Some(path.clone()),
        Operation::Move { to, .. } => Some(to.clone()),
        Operation::GitApply { .. } => None,
        Operation::Edit { path, .. } => Some(path.clone()),
    }
}

fn describe_edit_change(index: usize, change: &EditChange) -> String {
    let action = match change.action {
        EditAction::ReplaceBlock => "replace_block",
        EditAction::InsertBefore => "insert_before",
        EditAction::InsertAfter => "insert_after",
        EditAction::DeleteBlock => "delete_block",
    };

    match change.match_.as_ref() {
        Some(m) => {
            let occurrence = m.occurrence.unwrap_or(1);
            let must_match = m.must_match.as_deref().unwrap_or("exactly_one");
            let mode = m.mode.as_deref().unwrap_or("literal");
            format!(
                "edit[{}] {} (occurrence={}, must_match={}, mode={})",
                index, action, occurrence, must_match, mode
            )
        }
        None => format!("edit[{}] {}", index, action),
    }
}

#[derive(Debug, Clone)]
struct EditActionFailure {
    index: usize,
    action: String,
    error: String,
}

#[derive(Debug, Clone, Default)]
struct EditSequenceReport {
    lines: Vec<String>,
    successful_actions: usize,
    failed: Vec<EditActionFailure>,
}

fn apply_edit_sequence(repo: &std::path::Path, path: &str, changes: &[EditChange]) -> Result<EditSequenceReport> {
    let full = repo.join(path);
    let mut report = EditSequenceReport::default();

    let mut text = std::fs::read_to_string(&full)
        .with_context(|| format!("Failed to read {path} for edit"))?;

    for (idx, change) in changes.iter().enumerate() {
        let descriptor = describe_edit_change(idx + 1, change);
        let descriptor = descriptor.replacen("edit[", "PASS edit[", 1);
        let action_name = match change.action {
            EditAction::ReplaceBlock => "replace_block",
            EditAction::InsertBefore => "insert_before",
            EditAction::InsertAfter => "insert_after",
            EditAction::DeleteBlock => "delete_block",
        }
        .to_string();

        match apply_edit_change(&text, change) {
            Ok(next_text) => {
                text = next_text;
                report.successful_actions += 1;
                report.lines.push(format!("  - {}", descriptor));
            }
            Err(e) => {
                let err_text = format!("{e:#}");
                let fail_descriptor = descriptor.replacen("PASS edit[", "FAIL edit[", 1);
                report.lines.push(format!("  - {}", fail_descriptor));
                report.lines.push(format!("      {}", err_text));
                report.failed.push(EditActionFailure {
                    index: idx + 1,
                    action: action_name,
                    error: err_text,
                });
            }
        }
    }

    if report.successful_actions > 0 {
        std::fs::write(&full, text.as_bytes())
            .with_context(|| format!("Failed to write edited file {path}"))?;
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

fn format_apply_result(
    successful_operations: usize,
    total_operations: usize,
    successful_actions: usize,
    total_actions: usize,
    lines: &[String],
) -> String {
    let summary = format_apply_summary(
        successful_operations,
        total_operations,
        successful_actions,
        total_actions,
    );

    if lines.is_empty() {
        summary
    } else {
        format!("{}\n\n{}", summary, lines.join("\n"))
    }
}

fn operation_label(index: usize, op: &Operation) -> String {
    match op {
        Operation::Write { path, .. } => format!("[{}] write {}", index, path),
        Operation::Delete { path } => format!("[{}] delete {}", index, path),
        Operation::Move { from, to } => format!("[{}] move {} -> {}", index, from, to),
        Operation::GitApply { .. } => format!("[{}] git_apply", index),
        Operation::Edit { path, .. } => format!("[{}] edit {}", index, path),
    }
}

fn set_applier_status(state: &mut AppState, applier_id: ComponentId, status: String) {
    if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
        st.status = Some(status);
    }
}

fn set_applier_response(state: &mut AppState, applier_id: ComponentId, response: String) {
    if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
        st.status = Some(response.clone());
        st.result_payload = response;
        st.changeset_show_result = true;
    }
}

fn set_applier_result(
    state: &mut AppState,
    applier_id: ComponentId,
    attempted: Vec<String>,
    failed: Vec<String>,
    status: String,
    successful_operations: usize,
    total_operations: usize,
    successful_actions: usize,
    total_actions: usize,
    lines: Vec<String>,
) {
    let response = format_apply_result(
        successful_operations,
        total_operations,
        successful_actions,
        total_actions,
        &lines,
    );

    if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
        st.last_attempted_paths = attempted;
        st.last_failed_paths = failed;
        st.status = Some(status);
        st.result_payload = response;
        st.changeset_show_result = true;
    }

    if state.inputs.git_ref == WORKTREE_REF {
        state.refresh_tree_git_status();
    }
}
