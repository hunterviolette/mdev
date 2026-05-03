use std::{path::PathBuf, time::Instant};

use anyhow::{bail, Result};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::SqlitePool;

pub mod apply;
pub mod persistence;
pub mod schema;

pub use persistence::ChangesetAttemptRecord;
use persistence::{insert_changeset_attempt_from_result, row_to_summary, ChangesetAttemptContext};

#[derive(Debug, Clone)]
pub struct ChangesetRequest {
    pub repo_ref: String,
    pub git_ref: String,
    pub payload_text: String,
    pub source: String,
    pub workflow_key: Option<String>,
    pub run_id: Option<String>,
    pub step_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ChangesetAttemptSummary {
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
    pub successful_files: Vec<String>,
}

pub async fn apply_changeset(db: &SqlitePool, request: ChangesetRequest) -> Result<Value> {
    if request.payload_text.trim().is_empty() {
        bail!("payload_text is required");
    }

    let started = Instant::now();
    let result = match apply::execute_changeset_apply(
        PathBuf::from(&request.repo_ref).as_path(),
        request.payload_text.as_str(),
        request.git_ref.as_str(),
    ) {
        Ok(result) => result,
        Err(err) => json!({
            "ok": false,
            "mode": "changeset_apply",
            "summary": format!("ChangeSet apply failed: {:#}", err),
            "status": format!("ChangeSet parse/apply error: {:#}", err),
            "payload_text": request.payload_text,
            "lines": [format!("ChangeSet parse/apply error :: {:#}", err)],
            "target": {
                "repo_ref": request.repo_ref.as_str(),
                "git_ref": request.git_ref.as_str(),
                "workflow_key": request.workflow_key.as_deref(),
                "run_id": request.run_id.as_deref(),
                "step_id": request.step_id.as_deref()
            },
            "stats": {
                "successful_operations": 0,
                "failed_operations": 1,
                "total_operations": 1,
                "successful_actions": 0,
                "failed_actions": 1,
                "total_actions": 1,
                "failed_files": 0
            },
            "touched_files": []
        }),
    };

    let mut result = result;
    if let Some(obj) = result.as_object_mut() {
        obj.insert("target".to_string(), json!({
            "repo_ref": request.repo_ref.as_str(),
            "git_ref": request.git_ref.as_str(),
            "workflow_key": request.workflow_key.as_deref(),
            "run_id": request.run_id.as_deref(),
            "step_id": request.step_id.as_deref()
        }));
    }

    let attempt_id = insert_changeset_attempt_from_result(
        db,
        ChangesetAttemptContext {
            run_id: request.run_id.clone(),
            step_id: request.step_id.clone(),
            workflow_key: request.workflow_key.clone(),
            repo_ref: request.repo_ref.as_str(),
            git_ref: request.git_ref.as_str(),
            source: request.source.as_str(),
            payload_text: request.payload_text.as_str(),
            duration_ms: started.elapsed().as_millis().min(i64::MAX as u128) as i64,
        },
        &result,
    )
    .await?;

    if let Some(obj) = result.as_object_mut() {
        obj.insert("changeset_attempt_id".to_string(), Value::String(attempt_id));
    }

    Ok(result)
}

pub async fn list_changesets(
    db: &SqlitePool,
    workflow_key: &str,
    requested_limit: i64,
) -> Result<Vec<ChangesetAttemptSummary>> {
    let limit = requested_limit.clamp(1, 200);
    let rows = sqlx::query(
        r#"
        SELECT ca.id, ca.run_id, ca.step_id, ca.repo_ref, ca.git_ref, ca.direction, ca.reverses_attempt_id, ca.source, ca.status,
               ca.total_ops, ca.applied_ops, ca.failed_ops, ca.skipped_ops,
               ca.total_actions, ca.applied_actions, ca.failed_actions,
               ca.touched_file_count, ca.success_rate,
               ca.created_count, ca.modified_count, ca.deleted_count, ca.moved_count,
               ca.duration_ms, ca.error_summary, ca.display_summary, ca.created_at, ca.result_json
        FROM changeset_attempts ca
        LEFT JOIN workflow_runs wr ON wr.id = ca.run_id
        WHERE ca.workflow_key = ?
           OR wr.workflow_key = ?
        ORDER BY ca.created_at DESC
        LIMIT ?
        "#,
    )
    .bind(workflow_key)
    .bind(workflow_key)
    .bind(limit)
    .fetch_all(db)
    .await?;

    rows.into_iter().map(row_to_summary).collect()
}


