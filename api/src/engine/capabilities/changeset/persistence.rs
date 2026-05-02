use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::{sqlite::SqliteRow, Row, SqlitePool};

use super::ChangesetAttemptSummary;
use uuid::Uuid;

pub const CHANGESET_ATTEMPTS_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS changeset_attempts (
    id TEXT PRIMARY KEY,
    run_id TEXT,
    step_id TEXT,
    repo_ref TEXT NOT NULL,
    workflow_key TEXT NOT NULL DEFAULT '',
    git_ref TEXT NOT NULL DEFAULT 'WORKTREE',
    direction TEXT NOT NULL DEFAULT 'forward',
    reverses_attempt_id TEXT,
    source TEXT NOT NULL,
    status TEXT NOT NULL,
    payload_text TEXT NOT NULL,
    normalized_payload_json TEXT NOT NULL,
    reverse_payload_json TEXT,
    result_json TEXT NOT NULL,
    total_ops INTEGER NOT NULL DEFAULT 0,
    applied_ops INTEGER NOT NULL DEFAULT 0,
    failed_ops INTEGER NOT NULL DEFAULT 0,
    skipped_ops INTEGER NOT NULL DEFAULT 0,
    total_actions INTEGER NOT NULL DEFAULT 0,
    applied_actions INTEGER NOT NULL DEFAULT 0,
    failed_actions INTEGER NOT NULL DEFAULT 0,
    touched_file_count INTEGER NOT NULL DEFAULT 0,
    success_rate REAL NOT NULL DEFAULT 0.0,
    created_count INTEGER NOT NULL DEFAULT 0,
    modified_count INTEGER NOT NULL DEFAULT 0,
    deleted_count INTEGER NOT NULL DEFAULT 0,
    moved_count INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER,
    error_summary TEXT,
    display_summary TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL
)
"#;

pub const CHANGESET_FILE_EFFECTS_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS changeset_file_effects (
    id TEXT PRIMARY KEY,
    attempt_id TEXT NOT NULL REFERENCES changeset_attempts(id) ON DELETE CASCADE,
    op_index INTEGER NOT NULL,
    action_index INTEGER NOT NULL DEFAULT 1,
    action TEXT NOT NULL,
    path_before TEXT,
    path_after TEXT,
    status TEXT NOT NULL,
    hash_before TEXT,
    hash_after TEXT,
    file_existed_before INTEGER NOT NULL DEFAULT 0,
    file_exists_after INTEGER NOT NULL DEFAULT 0,
    forward_op_json TEXT NOT NULL,
    reverse_op_json TEXT,
    error TEXT
)
"#;

pub struct ChangesetAttemptContext<'a> {
    pub run_id: Option<String>,
    pub step_id: Option<String>,
    pub workflow_key: Option<String>,
    pub repo_ref: &'a str,
    pub git_ref: &'a str,
    pub source: &'a str,
    pub payload_text: &'a str,
    pub duration_ms: i64,
}

pub struct ChangesetAttemptInsert {
    pub id: String,
    pub run_id: Option<String>,
    pub step_id: Option<String>,
    pub repo_ref: String,
    pub workflow_key: String,
    pub git_ref: String,
    pub source: String,
    pub status: String,
    pub payload_text: String,
    pub normalized_payload_json: String,
    pub result_json: String,
    pub total_ops: i64,
    pub applied_ops: i64,
    pub failed_ops: i64,
    pub skipped_ops: i64,
    pub total_actions: i64,
    pub applied_actions: i64,
    pub failed_actions: i64,
    pub touched_file_count: i64,
    pub success_rate: f64,
    pub created_count: i64,
    pub modified_count: i64,
    pub deleted_count: i64,
    pub moved_count: i64,
    pub duration_ms: i64,
    pub error_summary: Option<String>,
    pub display_summary: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ChangesetFileEffectLog {
    pub op_index: i64,
    pub action_index: i64,
    pub action: String,
    pub path_before: Option<String>,
    pub path_after: Option<String>,
    pub status: String,
    pub forward_op_json: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChangesetAttemptRecord {
    pub id: String,
    pub run_id: Option<String>,
    pub step_id: Option<String>,
    pub repo_ref: String,
    pub git_ref: String,
    pub direction: String,
    pub reverses_attempt_id: Option<String>,
    pub source: String,
    pub status: String,
    pub total_ops: i64,
    pub applied_ops: i64,
    pub failed_ops: i64,
    pub skipped_ops: i64,
    pub total_actions: i64,
    pub applied_actions: i64,
    pub failed_actions: i64,
    pub touched_file_count: i64,
    pub success_rate: f64,
    pub created_count: i64,
    pub modified_count: i64,
    pub deleted_count: i64,
    pub moved_count: i64,
    pub duration_ms: Option<i64>,
    pub error_summary: Option<String>,
    pub display_summary: String,
    pub created_at: String,
    pub payload_text: Option<String>,
    pub normalized_payload_json: Option<String>,
    pub result_json: String,
}

impl ChangesetAttemptRecord {
    pub fn from_row(row: SqliteRow) -> Self {
        Self {
            id: row.get("id"),
            run_id: row.get("run_id"),
            step_id: row.get("step_id"),
            repo_ref: row.get("repo_ref"),
            git_ref: row.get("git_ref"),
            direction: row.get("direction"),
            reverses_attempt_id: row.get("reverses_attempt_id"),
            source: row.get("source"),
            status: row.get("status"),
            total_ops: row.get("total_ops"),
            applied_ops: row.get("applied_ops"),
            failed_ops: row.get("failed_ops"),
            skipped_ops: row.get("skipped_ops"),
            total_actions: row.get("total_actions"),
            applied_actions: row.get("applied_actions"),
            failed_actions: row.get("failed_actions"),
            touched_file_count: row.get("touched_file_count"),
            success_rate: row.get("success_rate"),
            created_count: row.get("created_count"),
            modified_count: row.get("modified_count"),
            deleted_count: row.get("deleted_count"),
            moved_count: row.get("moved_count"),
            duration_ms: row.get("duration_ms"),
            error_summary: row.get("error_summary"),
            display_summary: row.get("display_summary"),
            created_at: row.get("created_at"),
            payload_text: row.try_get("payload_text").ok(),
            normalized_payload_json: row.try_get("normalized_payload_json").ok(),
            result_json: row.get("result_json"),
        }
    }

    pub fn result_json_value(&self) -> Value {
        serde_json::from_str::<Value>(&self.result_json).unwrap_or_else(|_| Value::String(self.result_json.clone()))
    }

    pub fn to_summary(&self) -> ChangesetAttemptSummary {
        ChangesetAttemptSummary {
            id: self.id.clone(),
            run_id: self.run_id.clone(),
            step_id: self.step_id.clone(),
            repo_ref: self.repo_ref.clone(),
            git_ref: self.git_ref.clone(),
            direction: self.direction.clone(),
            reverses_attempt_id: self.reverses_attempt_id.clone(),
            source: self.source.clone(),
            status: self.status.clone(),
            total_ops: self.total_ops,
            applied_ops: self.applied_ops,
            failed_ops: self.failed_ops,
            skipped_ops: self.skipped_ops,
            total_actions: self.total_actions,
            applied_actions: self.applied_actions,
            failed_actions: self.failed_actions,
            touched_file_count: self.touched_file_count,
            success_rate: self.success_rate,
            created_count: self.created_count,
            modified_count: self.modified_count,
            deleted_count: self.deleted_count,
            moved_count: self.moved_count,
            duration_ms: self.duration_ms,
            error_summary: self.error_summary.clone(),
            display_summary: self.display_summary.clone(),
            created_at: self.created_at.clone(),
            successful_files: successful_files_from_result_json(&self.result_json),
        }
    }

    pub fn summary_response(&self, file_action_summaries: Vec<Value>) -> Value {
        let successful_files = file_action_summaries
            .iter()
            .filter(|item| item.get("applied").and_then(Value::as_i64).unwrap_or(0) > 0)
            .filter_map(|item| item.get("path").and_then(Value::as_str).map(str::to_string))
            .collect::<Vec<_>>();
        let failed_files = file_action_summaries
            .iter()
            .filter(|item| item.get("failed").and_then(Value::as_i64).unwrap_or(0) > 0)
            .filter_map(|item| item.get("path").and_then(Value::as_str).map(str::to_string))
            .collect::<Vec<_>>();

        json!({
            "id": self.id,
            "run_id": self.run_id,
            "step_id": self.step_id,
            "repo_ref": self.repo_ref,
            "git_ref": self.git_ref,
            "direction": self.direction,
            "reverses_attempt_id": self.reverses_attempt_id,
            "source": self.source,
            "status": self.status,
            "total_ops": self.total_ops,
            "applied_ops": self.applied_ops,
            "failed_ops": self.failed_ops,
            "skipped_ops": self.skipped_ops,
            "total_actions": self.total_actions,
            "applied_actions": self.applied_actions,
            "failed_actions": self.failed_actions,
            "touched_file_count": self.touched_file_count,
            "success_rate": self.success_rate,
            "created_count": self.created_count,
            "modified_count": self.modified_count,
            "deleted_count": self.deleted_count,
            "moved_count": self.moved_count,
            "duration_ms": self.duration_ms,
            "error_summary": self.error_summary,
            "display_summary": self.display_summary,
            "created_at": self.created_at,
            "successful_files": successful_files,
            "failed_files": failed_files,
            "file_action_summaries": file_action_summaries
        })
    }

    pub fn detail_response(&self, file_action_summaries: Vec<Value>) -> Value {
        let mut response = self.summary_response(file_action_summaries);
        if let Some(obj) = response.as_object_mut() {
            obj.insert("payload_text".to_string(), Value::String(self.payload_text.clone().unwrap_or_default()));
            obj.insert("normalized_payload_json".to_string(), Value::String(self.normalized_payload_json.clone().unwrap_or_default()));
            obj.insert("result_json".to_string(), self.result_json_value());
        }
        response
    }
}

pub fn row_to_summary(row: SqliteRow) -> Result<ChangesetAttemptSummary> {
    Ok(ChangesetAttemptRecord::from_row(row).to_summary())
}

fn successful_files_from_result_json(result_json: &str) -> Vec<String> {
    let Ok(result) = serde_json::from_str::<Value>(result_json) else {
        return Vec::new();
    };

    let failing_paths = result
        .get("failing_files")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("path").and_then(Value::as_str))
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();

    let mut files = result
        .get("touched_files")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|path| !failing_paths.contains(*path))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    files.sort();
    files.dedup();
    files
}

#[derive(Default)]
struct OperationEffectCounts {
    created: i64,
    modified: i64,
    deleted: i64,
    moved: i64,
}

pub async fn insert_changeset_attempt_from_result(
    db: &SqlitePool,
    ctx: ChangesetAttemptContext<'_>,
    result: &Value,
) -> Result<String> {
    insert_changeset_log_from_result(db, ctx, result, Vec::new()).await
}

pub async fn insert_changeset_log_from_result(
    db: &SqlitePool,
    ctx: ChangesetAttemptContext<'_>,
    result: &Value,
    file_effects: Vec<ChangesetFileEffectLog>,
) -> Result<String> {
    let attempt = build_changeset_attempt_from_result(db, ctx, result).await?;
    let id = attempt.id.clone();
    insert_changeset_attempt(db, &attempt).await?;
    insert_changeset_file_effects(db, id.as_str(), &file_effects).await?;
    Ok(id)
}

pub async fn build_changeset_attempt_from_result(
    db: &SqlitePool,
    ctx: ChangesetAttemptContext<'_>,
    result: &Value,
) -> Result<ChangesetAttemptInsert> {
    let stats = result.get("stats").unwrap_or(&Value::Null);
    let total_ops = stat_i64(stats, "total_operations");
    let applied_ops = stat_i64(stats, "successful_operations");
    let failed_ops = stat_i64(stats, "failed_operations");
    let skipped_ops = 0_i64;
    let total_actions = stat_i64(stats, "total_actions");
    let applied_actions = stat_i64(stats, "successful_actions");
    let failed_actions = stat_i64(stats, "failed_actions");
    let success_rate = if total_actions > 0 {
        applied_actions as f64 / total_actions as f64
    } else if total_ops > 0 {
        applied_ops as f64 / total_ops as f64
    } else {
        0.0
    };
    let status = changeset_attempt_status(applied_actions, failed_actions, applied_ops, failed_ops);
    let normalized_payload_json = result
        .get("normalized_payload")
        .and_then(Value::as_str)
        .unwrap_or(ctx.payload_text)
        .to_string();
    let parsed_payload = serde_json::from_str::<Value>(&normalized_payload_json).ok();
    let effect_counts = parsed_payload
        .as_ref()
        .map(count_operation_effects)
        .unwrap_or_default();
    let touched_file_count = touched_file_count(result, parsed_payload.as_ref()) as i64;
    let display_summary = result
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let error_summary = if status == "applied" {
        None
    } else {
        result
            .get("status")
            .and_then(Value::as_str)
            .or_else(|| result.get("summary").and_then(Value::as_str))
            .map(str::to_string)
    };
    let workflow_key = match ctx.workflow_key {
        Some(key) if !key.trim().is_empty() => key,
        _ => match ctx.run_id.as_deref() {
            Some(run_id) => workflow_key_for_run(db, run_id).await?,
            None => String::new(),
        },
    };

    Ok(ChangesetAttemptInsert {
        id: Uuid::new_v4().to_string(),
        run_id: ctx.run_id,
        step_id: ctx.step_id,
        repo_ref: ctx.repo_ref.to_string(),
        workflow_key,
        git_ref: ctx.git_ref.to_string(),
        source: ctx.source.to_string(),
        status,
        payload_text: ctx.payload_text.to_string(),
        normalized_payload_json,
        result_json: serde_json::to_string(result).context("failed to encode changeset result log JSON")?,
        total_ops,
        applied_ops,
        failed_ops,
        skipped_ops,
        total_actions,
        applied_actions,
        failed_actions,
        touched_file_count,
        success_rate,
        created_count: effect_counts.created,
        modified_count: effect_counts.modified,
        deleted_count: effect_counts.deleted,
        moved_count: effect_counts.moved,
        duration_ms: ctx.duration_ms,
        error_summary,
        display_summary,
        created_at: Utc::now().to_rfc3339(),
    })
}

pub async fn insert_changeset_attempt(db: &SqlitePool, attempt: &ChangesetAttemptInsert) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO changeset_attempts (
            id, run_id, step_id, repo_ref, workflow_key, git_ref, direction, reverses_attempt_id, source, status,
            payload_text, normalized_payload_json, reverse_payload_json, result_json,
            total_ops, applied_ops, failed_ops, skipped_ops,
            total_actions, applied_actions, failed_actions,
            touched_file_count, success_rate,
            created_count, modified_count, deleted_count, moved_count,
            duration_ms, error_summary, display_summary, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, 'forward', NULL, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(attempt.id.as_str())
    .bind(attempt.run_id.as_deref())
    .bind(attempt.step_id.as_deref())
    .bind(attempt.repo_ref.as_str())
    .bind(attempt.workflow_key.as_str())
    .bind(attempt.git_ref.as_str())
    .bind(attempt.source.as_str())
    .bind(attempt.status.as_str())
    .bind(attempt.payload_text.as_str())
    .bind(attempt.normalized_payload_json.as_str())
    .bind(attempt.result_json.as_str())
    .bind(attempt.total_ops)
    .bind(attempt.applied_ops)
    .bind(attempt.failed_ops)
    .bind(attempt.skipped_ops)
    .bind(attempt.total_actions)
    .bind(attempt.applied_actions)
    .bind(attempt.failed_actions)
    .bind(attempt.touched_file_count)
    .bind(attempt.success_rate)
    .bind(attempt.created_count)
    .bind(attempt.modified_count)
    .bind(attempt.deleted_count)
    .bind(attempt.moved_count)
    .bind(attempt.duration_ms)
    .bind(attempt.error_summary.as_deref())
    .bind(attempt.display_summary.as_str())
    .bind(attempt.created_at.as_str())
    .execute(db)
    .await?;

    Ok(())
}

pub async fn insert_changeset_file_effects(
    db: &SqlitePool,
    attempt_id: &str,
    effects: &[ChangesetFileEffectLog],
) -> Result<()> {
    for effect in effects {
        sqlx::query(
            r#"
            INSERT INTO changeset_file_effects (
                id, attempt_id, op_index, action_index, action, path_before, path_after, status,
                hash_before, hash_after, file_existed_before, file_exists_after,
                forward_op_json, reverse_op_json, error
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, 0, 0, ?, NULL, ?)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(attempt_id)
        .bind(effect.op_index)
        .bind(effect.action_index)
        .bind(effect.action.as_str())
        .bind(effect.path_before.as_deref())
        .bind(effect.path_after.as_deref())
        .bind(effect.status.as_str())
        .bind(effect.forward_op_json.as_str())
        .bind(effect.error.as_deref())
        .execute(db)
        .await?;
    }

    Ok(())
}

async fn workflow_key_for_run(db: &SqlitePool, run_id: &str) -> Result<String> {
    let row = sqlx::query("SELECT workflow_key FROM workflow_runs WHERE id = ?")
        .bind(run_id)
        .fetch_optional(db)
        .await?;

    Ok(row
        .map(|row| row.get::<String, _>("workflow_key"))
        .unwrap_or_default())
}

fn stat_i64(stats: &Value, key: &str) -> i64 {
    stats.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn changeset_attempt_status(
    applied_actions: i64,
    failed_actions: i64,
    applied_ops: i64,
    failed_ops: i64,
) -> String {
    if failed_actions == 0 && failed_ops == 0 {
        "applied".to_string()
    } else if applied_actions == 0 && applied_ops == 0 {
        "failed".to_string()
    } else {
        "partial".to_string()
    }
}

fn count_operation_effects(payload: &Value) -> OperationEffectCounts {
    let mut counts = OperationEffectCounts::default();
    for op in payload.get("operations").and_then(Value::as_array).into_iter().flatten() {
        match op.get("op").and_then(Value::as_str).unwrap_or("") {
            "write" => counts.created += 1,
            "delete" => counts.deleted += 1,
            "move" => counts.moved += 1,
            "edit" => counts.modified += 1,
            _ => {}
        }
    }
    counts
}

fn touched_file_count(result: &Value, payload: Option<&Value>) -> usize {
    let mut paths = HashSet::new();
    if let Some(files) = result.get("touched_files").and_then(Value::as_array) {
        for file in files.iter().filter_map(Value::as_str) {
            paths.insert(file.to_string());
        }
    }
    if paths.is_empty() {
        if let Some(payload) = payload {
            for op in payload.get("operations").and_then(Value::as_array).into_iter().flatten() {
                if let Some(path) = operation_primary_path(op) {
                    paths.insert(path);
                }
            }
        }
    }
    paths.len()
}

fn operation_primary_path(op: &Value) -> Option<String> {
    match op.get("op").and_then(Value::as_str).unwrap_or("") {
        "write" | "delete" | "edit" => op.get("path").and_then(Value::as_str).map(str::to_string),
        "move" => op.get("to").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}
