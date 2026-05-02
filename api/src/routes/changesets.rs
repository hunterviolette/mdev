use axum::{extract::{Path, Query, State}, routing::{get, post}, Json, Router};
use serde::Deserialize;
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::capabilities::changeset::{self, ChangesetRequest},
};

use super::workflow_scope::{resolve_workflow_scope, WorkflowScope};

#[derive(Debug, Deserialize)]
struct ListChangesetsQuery {
    #[serde(default = "default_limit")]
    limit: i64,
}

#[derive(Debug, Deserialize)]
struct ApplyChangesetRequest {
    #[serde(default = "default_git_ref")]
    git_ref: String,
    payload_text: String,
}

fn default_git_ref() -> String {
    "WORKTREE".to_string()
}

fn default_limit() -> i64 {
    50
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/workflow-runs/:run_id/changesets", get(list_changesets))
        .route("/api/workflow-runs/:run_id/changesets/apply", post(apply_changeset))
        .route("/api/workflows/:workflow_key/changesets", get(list_workflow_changesets))
        .route("/api/workflows/:workflow_key/changesets/:attempt_id", get(get_workflow_changeset))
        .route("/api/workflows/:workflow_key/changesets/apply", post(apply_workflow_changeset))
}

async fn list_changesets(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Query(query): Query<ListChangesetsQuery>,
) -> Result<Json<Vec<Value>>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    let workflow_key = workflow_key_for_scope(&state, &scope).await?;
    let items = list_changesets_with_attempted_files(&state.db, &workflow_key, query.limit)
        .await
        .map_err(internal)?;
    Ok(Json(items))
}

async fn list_workflow_changesets(
    State(state): State<AppState>,
    Path(workflow_key): Path<String>,
    Query(query): Query<ListChangesetsQuery>,
) -> Result<Json<Vec<Value>>, (axum::http::StatusCode, String)> {
    let items = list_changesets_with_attempted_files(&state.db, &workflow_key, query.limit)
        .await
        .map_err(internal)?;
    Ok(Json(items))
}

async fn list_changesets_with_attempted_files(
    db: &SqlitePool,
    workflow_key: &str,
    limit: i64,
) -> Result<Vec<Value>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT
            id, run_id, step_id, repo_ref, git_ref, direction, reverses_attempt_id, source, status,
            total_ops, applied_ops, failed_ops, skipped_ops,
            total_actions, applied_actions, failed_actions,
            touched_file_count, success_rate,
            created_count, modified_count, deleted_count, moved_count,
            duration_ms, error_summary, display_summary, created_at,
            normalized_payload_json, result_json
        FROM changeset_attempts
        WHERE workflow_key = ?
        ORDER BY created_at DESC
        LIMIT ?
        "#,
    )
    .bind(workflow_key)
    .bind(limit)
    .fetch_all(db)
    .await?;

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let record = changeset::ChangesetAttemptRecord::from_row(row);
        let result_json = record.result_json_value();
        let mut file_action_summaries = changeset_file_action_summaries(db, &record.id).await?;
        if file_action_summaries.is_empty() {
            file_action_summaries = attempted_changeset_file_summaries(
                record.normalized_payload_json.as_deref().unwrap_or_default(),
                &result_json,
            );
        }
        items.push(record.summary_response(file_action_summaries));
    }

    Ok(items)
}

async fn changeset_file_action_summaries(db: &SqlitePool, attempt_id: &str) -> Result<Vec<Value>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT
            COALESCE(path_after, path_before, '') AS path,
            SUM(CASE WHEN status = 'applied' THEN 1 ELSE 0 END) AS applied,
            SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) AS failed,
            COUNT(*) AS total,
            MIN(op_index) AS first_op,
            MIN(action_index) AS first_action
        FROM changeset_file_effects
        WHERE attempt_id = ? AND COALESCE(path_after, path_before, '') <> ''
        GROUP BY COALESCE(path_after, path_before, '')
        ORDER BY first_op, first_action, path
        "#,
    )
    .bind(attempt_id)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "path": row.get::<String, _>("path"),
                "applied": row.get::<i64, _>("applied"),
                "failed": row.get::<i64, _>("failed"),
                "total": row.get::<i64, _>("total")
            })
        })
        .collect())
}

fn attempted_changeset_file_summaries(normalized_payload_json: &str, result_json: &Value) -> Vec<Value> {
    let mut files = Vec::<String>::new();
    for key in ["successful_files", "touched_files"] {
        if let Some(items) = result_json.get(key).and_then(Value::as_array) {
            for path in items.iter().filter_map(Value::as_str) {
                push_unique_file(&mut files, path);
            }
        }
    }
    if let Ok(payload) = serde_json::from_str::<Value>(normalized_payload_json) {
        if let Some(operations) = payload.get("operations").and_then(Value::as_array) {
            for op in operations {
                match op.get("op").and_then(Value::as_str).unwrap_or("") {
                    "edit" | "write" | "delete" => {
                        if let Some(path) = op.get("path").and_then(Value::as_str) {
                            push_unique_file(&mut files, path);
                        }
                    }
                    "move" => {
                        if let Some(path) = op.get("to").or_else(|| op.get("from")).and_then(Value::as_str) {
                            push_unique_file(&mut files, path);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    let total = result_json.get("total_actions").and_then(Value::as_i64).unwrap_or(0);
    let applied = result_json.get("applied_actions").and_then(Value::as_i64).unwrap_or(0);
    let failed = result_json.get("failed_actions").and_then(Value::as_i64).unwrap_or_else(|| (total - applied).max(0));
    files
        .into_iter()
        .enumerate()
        .map(|(idx, path)| {
            let count = files_count_denominator(idx, total, path.as_str());
            let applied_count = files_count_denominator(idx, applied, path.as_str()).min(count);
            serde_json::json!({
                "path": path,
                "applied": applied_count,
                "failed": files_count_denominator(idx, failed, "").min(count.saturating_sub(applied_count)),
                "total": count
            })
        })
        .collect()
}

fn files_count_denominator(index: usize, total: i64, path: &str) -> i64 {
    if total <= 0 {
        return 0;
    }
    let divisor = path.len().max(1) as i64;
    let base = (total / divisor).max(0);
    let remainder = total % divisor;
    base + if (index as i64) < remainder { 1 } else { 0 }
}

fn push_unique_file(files: &mut Vec<String>, path: &str) {
    let trimmed = path.trim();
    if !trimmed.is_empty() && !files.iter().any(|item| item == trimmed) {
        files.push(trimmed.to_string());
    }
}

async fn get_workflow_changeset(
    State(state): State<AppState>,
    Path((workflow_key, attempt_id)): Path<(String, String)>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let row = sqlx::query(
        r#"
        SELECT
            id, run_id, step_id, repo_ref, git_ref, direction, reverses_attempt_id, source, status,
            total_ops, applied_ops, failed_ops, skipped_ops,
            total_actions, applied_actions, failed_actions,
            touched_file_count, success_rate,
            created_count, modified_count, deleted_count, moved_count,
            duration_ms, error_summary, display_summary, created_at,
            payload_text, normalized_payload_json, result_json
        FROM changeset_attempts
        WHERE workflow_key = ? AND id = ?
        LIMIT 1
        "#,
    )
    .bind(workflow_key.as_str())
    .bind(attempt_id.as_str())
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?;

    let Some(row) = row else {
        return Err((axum::http::StatusCode::NOT_FOUND, format!("changeset attempt not found: {attempt_id}")));
    };

    let record = changeset::ChangesetAttemptRecord::from_row(row);
    let result_json = record.result_json_value();
    let mut file_action_summaries = changeset_file_action_summaries(&state.db, &record.id)
        .await
        .map_err(internal)?;
    if file_action_summaries.is_empty() {
        file_action_summaries = attempted_changeset_file_summaries(
            record.normalized_payload_json.as_deref().unwrap_or_default(),
            &result_json,
        );
    }

    Ok(Json(record.detail_response(file_action_summaries)))
}

async fn resolve_workflow_scope_by_key(
    state: &AppState,
    workflow_key: &str,
) -> Result<WorkflowScope, (axum::http::StatusCode, String)> {
    let row = sqlx::query(
        r#"
        SELECT id
        FROM workflow_runs
        WHERE workflow_key = ?
        ORDER BY updated_at DESC, created_at DESC
        LIMIT 1
        "#,
    )
    .bind(workflow_key)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?;

    let Some(row) = row else {
        return Err((axum::http::StatusCode::NOT_FOUND, format!("workflow not found: {workflow_key}")));
    };

    let run_id_text: String = row.get("id");
    let run_id = Uuid::parse_str(&run_id_text)
        .map_err(|err| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    resolve_workflow_scope(state, run_id).await
}

async fn workflow_key_for_scope(
    state: &AppState,
    scope: &WorkflowScope,
) -> Result<String, (axum::http::StatusCode, String)> {
    let row = sqlx::query("SELECT workflow_key FROM workflow_runs WHERE id = ?")
        .bind(scope.run_id.to_string())
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?;

    let key = row
        .map(|row| row.get::<String, _>("workflow_key"))
        .unwrap_or_default();

    if key.trim().is_empty() {
        Err((axum::http::StatusCode::INTERNAL_SERVER_ERROR, "workflow_key is missing for run".to_string()))
    } else {
        Ok(key)
    }
}

async fn apply_changeset(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Json(req): Json<ApplyChangesetRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    let workflow_key = workflow_key_for_scope(&state, &scope).await?;
    let git_ref = if req.git_ref.trim().is_empty() {
        scope.git_ref.clone()
    } else {
        req.git_ref.clone()
    };

    let result = changeset::apply_changeset(
        &state.db,
        ChangesetRequest {
            repo_ref: scope.repo_ref.clone(),
            git_ref,
            payload_text: req.payload_text,
            source: "manual".to_string(),
            workflow_key: Some(workflow_key),
            run_id: Some(scope.run_id.to_string()),
            step_id: Some(scope.step.id.clone()),
        },
    )
    .await
    .map_err(internal)?;

    Ok(Json(result))
}

async fn apply_workflow_changeset(
    State(state): State<AppState>,
    Path(workflow_key): Path<String>,
    Json(req): Json<ApplyChangesetRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope_by_key(&state, &workflow_key).await?;
    let git_ref = if req.git_ref.trim().is_empty() {
        scope.git_ref.clone()
    } else {
        req.git_ref.clone()
    };

    let result = changeset::apply_changeset(
        &state.db,
        ChangesetRequest {
            repo_ref: scope.repo_ref.clone(),
            git_ref,
            payload_text: req.payload_text,
            source: "manual".to_string(),
            workflow_key: Some(workflow_key),
            run_id: Some(scope.run_id.to_string()),
            step_id: Some(scope.step.id.clone()),
        },
    )
    .await
    .map_err(internal)?;

    Ok(Json(result))
}

fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
