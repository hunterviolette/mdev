use axum::{extract::{Path, Query, State}, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use crate::app_state::AppState;

#[derive(Deserialize)]
pub struct SupervisorImpactQuery {
    from: Option<String>,
    to: Option<String>,
    status: Option<String>,
    visibility: Option<String>,
}

#[derive(Serialize, Default, Clone)]
pub struct ImpactMetrics {
    input_tokens: i64,
    output_tokens: i64,
    total_tokens: i64,
    input_tokens_estimated: bool,
    output_tokens_estimated: bool,
    input_tokens_incomplete: bool,
    output_tokens_incomplete: bool,
    runtime_seconds: f64,
    stage_executions: i64,
    completed_stages: i64,
    failed_stages: i64,
    stage_error_rate: f64,
    changeset_attempts: i64,
    changeset_applied: i64,
    files_changed: i64,
    lines_added: Option<i64>,
    lines_removed: Option<i64>,
}

#[derive(Serialize)]
pub struct WorkflowImpactRow {
    run_id: String,
    title: String,
    status: String,
    pool_type: String,
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
    metrics: ImpactMetrics,
}

#[derive(Serialize)]
pub struct SupervisorImpactResponse {
    supervisor_id: String,
    from: Option<String>,
    to: Option<String>,
    status: String,
    visibility: String,
    workflow_count: usize,
    totals: ImpactMetrics,
    workflows: Vec<WorkflowImpactRow>,
    unattributed: ImpactMetrics,
}

type ApiError = (StatusCode, String);

fn internal(error: sqlx::Error) -> ApiError {
    tracing::error!(%error, "supervisor impact query failed");
    (StatusCode::INTERNAL_SERVER_ERROR, "Unable to load supervisor impact".to_owned())
}

pub async fn get_supervisor_impact(
    State(state): State<AppState>,
    Path(supervisor_id): Path<String>,
    Query(query): Query<SupervisorImpactQuery>,
) -> Result<Json<SupervisorImpactResponse>, ApiError> {
    let root_repo_path: Option<String> = sqlx::query_scalar("SELECT root_repo_path FROM supervisor_runs WHERE id = ?")
        .bind(&supervisor_id).fetch_optional(&state.db).await.map_err(internal)?;
    let Some(root_repo_path) = root_repo_path else {
        return Err((StatusCode::NOT_FOUND, "Supervisor not found".into()));
    };
    let status = query.status.as_deref().unwrap_or("all");
    let visibility = query.visibility.as_deref().unwrap_or("all");
    if !["all", "running", "completed", "failed", "queued", "paused", "cancelled", "waiting"].contains(&status) || !["all", "current", "archived"].contains(&visibility) {
        return Err((StatusCode::BAD_REQUEST, "Invalid impact filter".into()));
    }
    let from = query.from.as_deref().filter(|value| !value.is_empty());
    let to = query.to.as_deref().filter(|value| !value.is_empty());
    if from.is_some_and(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err()) || to.is_some_and(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err()) || from.zip(to).is_some_and(|(start, end)| start > end) {
        return Err((StatusCode::BAD_REQUEST, "Invalid date range".into()));
    }
    let lower = from.map(|value| format!("{}T00:00:00", value));
    let upper = to.map(|value| format!("{}T23:59:59.999999999", value));
    let rows = sqlx::query("SELECT DISTINCT w.id, w.title, w.status, w.archived_at, w.created_at, w.updated_at, (SELECT u2.kind FROM supervisor_work_units u2 WHERE u2.workflow_run_id = w.id AND u2.supervisor_run_id = ? ORDER BY u2.created_at DESC LIMIT 1) AS pool_type FROM workflow_runs w JOIN supervisor_work_units u ON u.workflow_run_id = w.id WHERE u.supervisor_run_id = ? AND (? = 'all' OR w.status = ?) AND (? = 'all' OR (? = 'current' AND w.archived_at IS NULL) OR (? = 'archived' AND w.archived_at IS NOT NULL)) ORDER BY w.created_at DESC, w.id DESC")
        .bind(&supervisor_id).bind(&supervisor_id).bind(status).bind(status).bind(visibility).bind(visibility).bind(visibility)
        .fetch_all(&state.db).await.map_err(internal)?;
    let mut workflows = Vec::new();
    let mut totals = ImpactMetrics::default();

    for row in rows {
        let run_id: String = row.get("id");
        let events = sqlx::query("SELECT stage_execution_id, MIN(CASE WHEN kind LIKE '%stage%start%' THEN created_at END) AS started_at, MAX(CASE WHEN kind LIKE '%stage%complet%' OR kind LIKE '%stage%fail%' THEN created_at END) AS finished_at, MAX(CASE WHEN kind LIKE '%stage%start%' THEN 1 ELSE 0 END) AS started, MAX(CASE WHEN kind LIKE '%stage%complet%' THEN 1 ELSE 0 END) AS completed, MAX(CASE WHEN kind LIKE '%stage%fail%' THEN 1 ELSE 0 END) AS failed FROM workflow_events WHERE run_id = ? AND stage_execution_id IS NOT NULL AND (? IS NULL OR created_at >= ?) AND (? IS NULL OR created_at <= ?) GROUP BY stage_execution_id")
            .bind(&run_id).bind(&lower).bind(&lower).bind(&upper).bind(&upper)
            .fetch_all(&state.db).await.map_err(internal)?;
        let mut metrics = ImpactMetrics::default();
        for event in events {
            let started: i64 = event.get("started");
            let completed: i64 = event.get("completed");
            let failed: i64 = event.get("failed");
            metrics.stage_executions += started;
            metrics.completed_stages += completed;
            metrics.failed_stages += failed;
            let start: Option<String> = event.get("started_at");
            let finish: Option<String> = event.get("finished_at");
            if let (Some(start), Some(finish)) = (start, finish) {
                if let (Ok(start), Ok(finish)) = (chrono::DateTime::parse_from_rfc3339(&start), chrono::DateTime::parse_from_rfc3339(&finish)) {
                    metrics.runtime_seconds += (finish - start).num_milliseconds().max(0) as f64 / 1000.0;
                }
            }
        }
        let inference = sqlx::query(r#"
            SELECT
                COALESCE(SUM(COALESCE(
                    CAST(json_extract(accounting_json, '$.reported_usage.input_tokens') AS INTEGER),
                    CAST(json_extract(accounting_json, '$.theoretical_usage.input_tokens') AS INTEGER)
                )), 0) AS input_tokens,
                COALESCE(SUM(COALESCE(
                    CAST(json_extract(accounting_json, '$.reported_usage.output_tokens') AS INTEGER),
                    CAST(json_extract(accounting_json, '$.theoretical_usage.output_tokens') AS INTEGER)
                )), 0) AS output_tokens,
                COALESCE(MAX(CASE WHEN
                    json_type(accounting_json, '$.reported_usage.input_tokens') IS NULL
                    AND json_type(accounting_json, '$.theoretical_usage.input_tokens') IS NOT NULL
                THEN 1 ELSE 0 END), 0) AS input_estimated,
                COALESCE(MAX(CASE WHEN
                    json_type(accounting_json, '$.reported_usage.output_tokens') IS NULL
                    AND json_type(accounting_json, '$.theoretical_usage.output_tokens') IS NOT NULL
                THEN 1 ELSE 0 END), 0) AS output_estimated,
                COALESCE(MAX(CASE WHEN
                    json_type(accounting_json, '$.reported_usage.input_tokens') IS NULL
                    AND json_type(accounting_json, '$.theoretical_usage.input_tokens') IS NULL
                THEN 1 ELSE 0 END), 0) AS input_incomplete,
                COALESCE(MAX(CASE WHEN
                    json_type(accounting_json, '$.reported_usage.output_tokens') IS NULL
                    AND json_type(accounting_json, '$.theoretical_usage.output_tokens') IS NULL
                THEN 1 ELSE 0 END), 0) AS output_incomplete
            FROM inference_invocations
            WHERE workflow_run_id = ?
              AND (? IS NULL OR created_at >= ?)
              AND (? IS NULL OR created_at <= ?)
        "#)
            .bind(&run_id).bind(&lower).bind(&lower).bind(&upper).bind(&upper)
            .fetch_one(&state.db).await.map_err(internal)?;
        metrics.input_tokens = inference.get("input_tokens");
        metrics.output_tokens = inference.get("output_tokens");
        metrics.total_tokens = metrics.input_tokens + metrics.output_tokens;
        metrics.input_tokens_estimated = inference.get::<i64, _>("input_estimated") != 0;
        metrics.output_tokens_estimated = inference.get::<i64, _>("output_estimated") != 0;
        metrics.input_tokens_incomplete = inference.get::<i64, _>("input_incomplete") != 0;
        metrics.output_tokens_incomplete = inference.get::<i64, _>("output_incomplete") != 0;
        let changes = sqlx::query("SELECT COUNT(*) AS attempts, COALESCE(SUM(CASE WHEN status IN ('applied', 'success') THEN 1 ELSE 0 END), 0) AS applied FROM changeset_attempts WHERE run_id = ? AND direction = 'forward' AND (? IS NULL OR created_at >= ?) AND (? IS NULL OR created_at <= ?)")
            .bind(&run_id).bind(&lower).bind(&lower).bind(&upper).bind(&upper)
            .fetch_one(&state.db).await.map_err(internal)?;
        metrics.changeset_attempts = changes.get("attempts");
        metrics.changeset_applied = changes.get("applied");
        if metrics.stage_executions > 0 {
            metrics.stage_error_rate = metrics.failed_stages as f64 / metrics.stage_executions as f64;
        }
        let has_activity = metrics.stage_executions > 0 || metrics.total_tokens > 0 || metrics.changeset_attempts > 0 || metrics.input_tokens_incomplete || metrics.output_tokens_incomplete;
        if (lower.is_some() || upper.is_some()) && !has_activity {
            continue;
        }
        totals.input_tokens += metrics.input_tokens;
        totals.output_tokens += metrics.output_tokens;
        totals.input_tokens_estimated |= metrics.input_tokens_estimated;
        totals.output_tokens_estimated |= metrics.output_tokens_estimated;
        totals.input_tokens_incomplete |= metrics.input_tokens_incomplete;
        totals.output_tokens_incomplete |= metrics.output_tokens_incomplete;
        totals.runtime_seconds += metrics.runtime_seconds;
        totals.stage_executions += metrics.stage_executions;
        totals.completed_stages += metrics.completed_stages;
        totals.failed_stages += metrics.failed_stages;
        totals.changeset_attempts += metrics.changeset_attempts;
        totals.changeset_applied += metrics.changeset_applied;

        workflows.push(WorkflowImpactRow {
            run_id,
            title: row.get("title"),
            status: row.get("status"),
            pool_type: row.get::<Option<String>, _>("pool_type").unwrap_or_else(|| "unknown".to_string()),
            archived_at: row.get("archived_at"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
            metrics,
        });
    }
    let unattributed = ImpactMetrics::default();

    totals.total_tokens = totals.input_tokens + totals.output_tokens;

    if totals.stage_executions > 0 {
        totals.stage_error_rate = totals.failed_stages as f64 / totals.stage_executions as f64;
    }
    Ok(Json(SupervisorImpactResponse {
        supervisor_id,
        from: query.from,
        to: query.to,
        status: status.to_string(),
        visibility: visibility.to_string(),
        workflow_count: workflows.len(),
        totals,
        workflows,
        unattributed,
    }))
}
