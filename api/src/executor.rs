use std::{fs, path::{Path, PathBuf}, process::Command};

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{app_state::AppState, models::RunStatus};
use crate::engine::capabilities::inference::{api::oai::OpenAIInferenceClient, browser::adapter as browser_adapter, InferenceConfig, InferenceResult, InferenceTransport};

#[derive(Debug, serde::Serialize, Deserialize)]
pub struct ContextExportPayload {
    pub repo_ref: String,
    #[serde(default)]
    pub git_ref: String,
    #[serde(default)]
    pub exclude_regex: Vec<String>,
    #[serde(default = "default_true")]
    pub skip_binary: bool,
    #[serde(default = "default_true")]
    pub skip_gitignore: bool,
    #[serde(default)]
    pub include_staged_diff: bool,
    #[serde(default)]
    pub include_unstaged_diff: bool,
    #[serde(default)]
    pub include_files: Option<Vec<String>>,
    #[serde(default)]
    pub save_path: String,
}

#[derive(Debug, Deserialize)]
struct PayloadGatewayPayload {
    repo_ref: String,
    #[serde(default)]
    git_ref: String,
    #[serde(default)]
    exclude_regex: Vec<String>,
    mode: String,
    #[serde(default)]
    sync_mode: String,
    #[serde(default)]
    payload_text: String,
    #[serde(default)]
    tree_selection: Vec<String>,
    #[serde(default = "default_true")]
    sync_skip_binary: bool,
    #[serde(default = "default_true")]
    sync_skip_gitignore: bool,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct ChangeSetPayload {
    version: u32,
    #[serde(default)]
    description: String,
    operations: Vec<Operation>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Operation {
    Write { path: String, contents: String },
    Delete { path: String },
    Move { from: String, to: String },
    Edit { path: String, changes: Vec<EditAction> },
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct EditAction {
    action: String,
    #[serde(rename = "match")]
    match_spec: LiteralMatch,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    replacement: Option<String>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct LiteralMatch {
    #[serde(rename = "type")]
    match_type: String,
    mode: String,
    must_match: String,
    occurrence: usize,
    text: String,
}

fn default_true() -> bool { true }

#[derive(Debug, Clone, Deserialize)]
pub struct ModelInferenceExecutionRequest {
    pub prompt: String,
    #[serde(default)]
    pub include_repo_context: bool,
    #[serde(default)]
    pub repo_context: Option<Value>,
}

pub async fn execute_model_inference_send_prompt(
    state: &AppState,
    run_id: Uuid,
    step_id: Option<String>,
    req: ModelInferenceExecutionRequest,
) -> Result<Value> {
    let row = sqlx::query("SELECT context_json FROM workflow_runs WHERE id = ?")
        .bind(run_id.to_string())
        .fetch_one(&state.db)
        .await?;

    let mut context: Value = serde_json::from_str(row.get::<String, _>("context_json").as_str())?;
    let mut inference_cfg: InferenceConfig = serde_json::from_value(
        context.get("model_inference").cloned().unwrap_or_else(|| json!({}))
    ).unwrap_or_default();

    let mut repo_context_mode = Value::Null;
    let mut repo_context_output_path = Value::Null;
    let mut final_prompt = req.prompt.clone();

    let result: InferenceResult = match inference_cfg.transport {
        InferenceTransport::Api => {
            if req.include_repo_context {
                let repo_context_payload = req.repo_context
                    .clone()
                    .ok_or_else(|| anyhow!("repo_context is required when include_repo_context is true"))?;
                let repo_context_text = render_context_export_text(repo_context_payload)?;
                final_prompt = format!("{}\n\n[REPO CONTEXT]\n{}", req.prompt, repo_context_text);
                repo_context_mode = Value::String("inline_text".to_string());
            }

            let client = OpenAIInferenceClient::from_env();
            let (text, conversation_id) = client
                .chat_in_conversation(
                    &inference_cfg.model,
                    inference_cfg.conversation_id.clone(),
                    Vec::new(),
                    vec![("user".to_string(), final_prompt.clone())],
                )
                .await?;
            inference_cfg.conversation_id = Some(conversation_id.clone());

            InferenceResult {
                transport: InferenceTransport::Api,
                text,
                conversation_id: Some(conversation_id),
                browser_session_id: None,
            }
        }
        InferenceTransport::Browser => {
            if req.include_repo_context {
                let repo_context_payload = req.repo_context
                    .clone()
                    .ok_or_else(|| anyhow!("repo_context is required when include_repo_context is true"))?;
                let mut export_req = parse_context_export_payload(repo_context_payload)?;
                if export_req.save_path.trim().is_empty() {
                    let export_path = std::env::temp_dir().join(format!("repo_context_{}.txt", run_id));
                    export_req.save_path = export_path.to_string_lossy().replace('\\', "/");
                }

                let export_result = execute_context_export(
                    state,
                    run_id,
                    step_id.clone(),
                    serde_json::to_value(&export_req)?,
                )
                .await?;

                let output_path = export_result
                    .get("output_path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("context export did not return output_path"))?;

                browser_adapter::upload_file(&mut inference_cfg.browser, std::path::Path::new(output_path))?;
                repo_context_mode = Value::String("uploaded_file".to_string());
                repo_context_output_path = Value::String(output_path.to_string());
            }

            browser_adapter::send_chat_and_wait(&mut inference_cfg.browser, &req.prompt)?
        }
    };

    context["model_inference"] = serde_json::to_value(&inference_cfg)?;
    context["last_inference"] = serde_json::to_value(&result)?;
    if req.include_repo_context {
        context["last_repo_context"] = json!({
            "mode": repo_context_mode,
            "output_path": repo_context_output_path,
            "git_ref": req.repo_context.as_ref().and_then(|v| v.get("git_ref")).cloned().unwrap_or(Value::Null),
            "include_files": req.repo_context.as_ref().and_then(|v| v.get("include_files")).cloned().unwrap_or(Value::Null),
        });
    }
    update_run_context(&state.db, run_id, &context).await?;

    Ok(json!({ "ok": true, "result": result }))
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

pub fn parse_context_export_payload(payload: Value) -> Result<ContextExportPayload> {
    serde_json::from_value(payload).context("invalid context export payload")
}

pub fn render_context_export_text(payload: Value) -> Result<String> {
    let req = parse_context_export_payload(payload)?;
    let repo = PathBuf::from(&req.repo_ref);
    build_context_export_text(&repo, &req)
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

pub async fn append_event(
    db: &SqlitePool,
    run_id: Uuid,
    step_id: Option<&str>,
    level: &str,
    kind: &str,
    message: &str,
    payload: Value,
) -> anyhow::Result<()> {
    let stage_execution_id = payload.get("event_meta")
        .and_then(|v| v.get("stage_execution_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let capability_invocation_id = payload.get("event_meta")
        .and_then(|v| v.get("capability_invocation_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let parent_invocation_id = payload.get("event_meta")
        .and_then(|v| v.get("parent_invocation_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let is_header_event = payload.get("event_meta")
        .and_then(|v| v.get("is_header_event"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let sequence_no: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence_no), 0) + 1 FROM workflow_events WHERE run_id = ?")
        .bind(run_id.to_string())
        .fetch_one(db)
        .await?;
    let now = Utc::now().to_rfc3339();

    sqlx::query(
        r#"
        INSERT INTO workflow_events (
            id,
            run_id,
            step_id,
            stage_execution_id,
            capability_invocation_id,
            parent_invocation_id,
            sequence_no,
            is_header_event,
            level,
            kind,
            message,
            payload_json,
            created_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(run_id.to_string())
    .bind(step_id)
    .bind(stage_execution_id)
    .bind(capability_invocation_id)
    .bind(parent_invocation_id)
    .bind(sequence_no)
    .bind(if is_header_event { 1 } else { 0 })
    .bind(level)
    .bind(kind)
    .bind(message)
    .bind(payload.to_string())
    .bind(&now)
    .execute(db)
    .await?;

    sqlx::query("UPDATE workflow_runs SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(run_id.to_string())
        .execute(db)
        .await?;

    Ok(())
}

pub async fn update_run_status(
    db: &SqlitePool,
    run_id: Uuid,
    status: RunStatus,
    current_step_id: Option<&str>,
) -> anyhow::Result<()> {
    let status_str = serde_json::to_string(&status)?;
    let status_str = status_str.trim_matches('"').to_string();

    sqlx::query(
        "UPDATE workflow_runs SET status = ?, current_step_id = ?, updated_at = ? WHERE id = ?",
    )
    .bind(status_str)
    .bind(current_step_id)
    .bind(Utc::now().to_rfc3339())
    .bind(run_id.to_string())
    .execute(db)
    .await?;

    Ok(())
}

pub async fn update_run_context(
    db: &SqlitePool,
    run_id: Uuid,
    context: &Value,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE workflow_runs SET context_json = ?, updated_at = ? WHERE id = ?",
    )
    .bind(serde_json::to_string_pretty(context)?)
    .bind(Utc::now().to_rfc3339())
    .bind(run_id.to_string())
    .execute(db)
    .await?;

    Ok(())
}

pub async fn execute_context_export(
    _state: &AppState,
    run_id: Uuid,
    _step_id: Option<String>,
    payload: Value,
) -> anyhow::Result<Value> {
    let req = parse_context_export_payload(payload.clone())?;

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

pub async fn execute_payload_gateway(
    _state: &AppState,
    run_id: Uuid,
    _step_id: Option<String>,
    payload: Value,
) -> anyhow::Result<Value> {
    let req: PayloadGatewayPayload = serde_json::from_value(payload.clone())
        .context("invalid payload gateway payload")?;

    let repo = PathBuf::from(&req.repo_ref);
    let result = match req.mode.as_str() {
        "sync_generate" => execute_sync_generate(&repo, &req)?,
        "changeset_apply" => match execute_changeset_apply(&repo, &req) {
            Ok(result) => result,
            Err(err) => json!({
                "ok": false,
                "mode": "changeset_apply",
                "summary": format!("ChangeSet payload was invalid and could not be applied: {}", err),
                "stats": {
                    "successful_operations": 0,
                    "failed_operations": 1,
                    "total_operations": 1,
                },
                "attempted": [],
                "failed": ["changeset_validation"],
                "operation_results": [
                    {
                        "index": 1,
                        "status": "failed",
                        "label": "changeset_validation",
                        "error": format!("{}", err)
                    }
                ],
                "lines": [
                    format!("Invalid ChangeSet payload: {}", err),
                    "Return only a valid ChangeSet JSON object, version 1, with at least one operation.".to_string()
                ],
                "normalized_payload": Value::Null,
                "validation_error": true,
            }),
        },
        other => bail!("unsupported payload gateway mode {other}"),
    };

    let ok = result.get("ok").and_then(Value::as_bool).unwrap_or(false);
    tracing::info!(%run_id, mode = %req.mode, ok, "payload gateway completed");
    Ok(result)
}

#[derive(Debug, Deserialize)]
struct TerminalCommandPayload {
    repo_ref: String,
    #[serde(default)]
    commands: Vec<String>,
}

pub async fn execute_terminal_command(
    _state: &AppState,
    _run_id: Uuid,
    _step_id: Option<String>,
    payload: Value,
) -> anyhow::Result<Value> {
    let req: TerminalCommandPayload = serde_json::from_value(payload.clone())
        .context("invalid terminal capability payload")?;

    let repo = PathBuf::from(&req.repo_ref);
    let mut outputs = Vec::new();
    let mut ok = true;

    for command in req.commands.iter().filter(|cmd| !cmd.trim().is_empty()) {
        let (success, output) = run_shell_command(&repo, command)
            .with_context(|| format!("failed to run terminal command {command}"))?;
        outputs.push(json!({
            "command": command,
            "ok": success,
            "output": output,
        }));
        if !success {
            ok = false;
            break;
        }
    }

    let result = json!({
        "ok": ok,
        "outputs": outputs,
    });

    Ok(result)
}

fn run_shell_command(repo: &Path, command: &str) -> Result<(bool, String)> {
    #[cfg(target_os = "windows")]
    let output = Command::new("cmd")
        .args(["/C", command])
        .current_dir(repo)
        .output()
        .with_context(|| format!("failed to run shell command {command}"))?;

    #[cfg(not(target_os = "windows"))]
    let output = Command::new("sh")
        .args(["-lc", command])
        .current_dir(repo)
        .output()
        .with_context(|| format!("failed to run shell command {command}"))?;

    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    if !output.stderr.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }

    Ok((output.status.success(), text))
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

fn execute_sync_generate(repo: &Path, req: &PayloadGatewayPayload) -> Result<Value> {
    let compiled = compile_excludes(&req.exclude_regex)?;
    let include = match req.sync_mode.as_str() {
        "entire" => None,
        "tree" | "diff" | "" => Some(req.tree_selection.clone()),
        _ => Some(req.tree_selection.clone()),
    };

    let mut files = collect_candidate_files(repo, &req.git_ref, include.as_ref())?;
    files.sort();
    files.dedup();

    let mut operations = Vec::new();
    for rel in files {
        if path_is_excluded(&rel, &compiled) {
            continue;
        }
        if req.sync_skip_gitignore && is_gitignored(repo, &rel)? {
            continue;
        }
        let bytes = read_file_bytes(repo, effective_ref(&req.git_ref), &rel)?;
        if req.sync_skip_binary && is_probably_binary(&bytes) {
            continue;
        }
        let contents = String::from_utf8(bytes)
            .with_context(|| format!("sync payload requires utf-8 text for {}", rel))?;
        operations.push(json!({
            "op": "write",
            "path": rel,
            "contents": contents,
        }));
    }

    let payload = json!({
        "version": 1,
        "description": "Generated by workflow-api payload gateway",
        "operations": operations,
    });
    let payload_text = serde_json::to_string_pretty(&payload)?;

    Ok(json!({
        "ok": true,
        "mode": "sync_generate",
        "payload_text": payload_text,
        "operation_count": payload["operations"].as_array().map(|a| a.len()).unwrap_or(0),
    }))
}

fn execute_changeset_apply(repo: &Path, req: &PayloadGatewayPayload) -> Result<Value> {
    let normalized = normalize_changeset_payload_text(&req.payload_text)?;
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

fn normalize_changeset_payload_text(raw: &str) -> Result<String> {
    let json_slice = extract_json_object_slice(raw)
        .ok_or_else(|| anyhow!("No JSON object found in payload. Paste the full payload."))?;
    let payload: ChangeSetPayload = serde_json::from_str(json_slice)
        .context("Failed to parse changeset payload JSON")?;
    if payload.version != 1 {
        bail!("Unsupported changeset version {} (expected 1)", payload.version);
    }
    if payload.operations.is_empty() {
        bail!("ChangeSet payload contains no operations.");
    }
    Ok(serde_json::to_string_pretty(&payload).context("Failed to normalize changeset payload")?)
}

fn extract_json_object_slice(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end < start { return None; }
    Some(&text[start..=end])
}

fn apply_operation(repo: &Path, op: &Operation) -> Result<String> {
    match op {
        Operation::Write { path, contents } => {
            let abs = repo.join(path);
            if let Some(parent) = abs.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&abs, contents.as_bytes())?;
            Ok(format!("write {}", path))
        }
        Operation::Delete { path } => {
            let abs = repo.join(path);
            if abs.is_dir() {
                fs::remove_dir_all(&abs)?;
            } else if abs.exists() {
                fs::remove_file(&abs)?;
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
            let abs = repo.join(path);
            let mut text = fs::read_to_string(&abs)
                .with_context(|| format!("failed to read {}", abs.display()))?;
            text = normalize_newlines(&text);
            for change in changes {
                apply_edit_change(&mut text, change)?;
            }
            fs::write(&abs, text.as_bytes())?;
            Ok(format!("edit {}", path))
        }
    }
}

fn apply_edit_change(text: &mut String, change: &EditAction) -> Result<()> {
    let m = &change.match_spec;
    if m.match_type != "literal" {
        bail!("unsupported match type {}", m.match_type);
    }
    if m.mode != "normalized_newlines" {
        bail!("unsupported match mode {}", m.mode);
    }
    if m.must_match != "exactly_one" {
        bail!("unsupported must_match {}", m.must_match);
    }

    let needle = normalize_newlines(&m.text);
    let positions = find_all_occurrences(text, &needle);
    if positions.len() != 1 {
        bail!("expected exactly one match, found {}", positions.len());
    }
    let (start, end) = positions[0];

    match change.action.as_str() {
        "insert_before" => {
            let insert = normalize_newlines(change.text.as_deref().unwrap_or_default());
            text.insert_str(start, &insert);
        }
        "insert_after" => {
            let insert = normalize_newlines(change.text.as_deref().unwrap_or_default());
            text.insert_str(end, &insert);
        }
        "replace_block" => {
            let replacement = normalize_newlines(change.replacement.as_deref().unwrap_or_default());
            text.replace_range(start..end, &replacement);
        }
        other => bail!("unsupported edit action {}", other),
    }

    Ok(())
}

fn operation_label(op: &Operation) -> String {
    match op {
        Operation::Write { path, .. } => format!("write {}", path),
        Operation::Delete { path } => format!("delete {}", path),
        Operation::Move { from, to } => format!("move {} -> {}", from, to),
        Operation::Edit { path, .. } => format!("edit {}", path),
    }
}

fn find_all_occurrences(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = haystack[offset..].find(needle) {
        let start = offset + pos;
        let end = start + needle.len();
        out.push((start, end));
        offset = end;
    }
    out
}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

fn collect_candidate_files(repo: &Path, git_ref: &str, include_files: Option<&Vec<String>>) -> Result<Vec<String>> {
    if let Some(files) = include_files {
        if !files.is_empty() {
            return Ok(files.iter().map(|s| s.replace('\\', "/")).collect());
        }
    }

    if effective_ref(git_ref) == "WORKTREE" {
        let mut out = Vec::new();
        collect_worktree_files(repo, repo, &mut out)?;
        return Ok(out);
    }

    let stdout = run_git_capture_string(repo, &["ls-tree", "-r", "--name-only", effective_ref(git_ref)])?;
    Ok(stdout.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
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
