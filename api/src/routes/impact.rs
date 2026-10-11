use axum::{extract::{Path, Query, State}, http::StatusCode, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use crate::app_state::AppState;

#[derive(Serialize)]
struct WorkflowImpact {
    run_id: String, user_inputs: i64, inference_outputs: i64, input_tokens: i64,
    output_tokens: i64, stage_executions: i64, completed_stages: i64, failed_stages: i64,
    capability_activations: i64, changeset_attempts: i64, changeset_applied: i64,
    files_changed: i64, files_created: i64, files_modified: i64, files_deleted: i64,
    files_moved: i64, file_paths: Vec<String>, lines_added: Option<i64>, lines_removed: Option<i64>,
}

#[derive(Deserialize)]
struct EventQuery {
    after: Option<i64>, before: Option<i64>, from: Option<String>, to: Option<String>,
    step_id: Option<String>, stage_execution_id: Option<String>,
    capability_invocation_id: Option<String>, kind: Option<String>, level: Option<String>,
    search: Option<String>, limit: Option<i64>,
}

#[derive(Serialize)]
struct EventRecord {
    id: String, sequence_no: i64, step_id: Option<String>, stage_execution_id: Option<String>,
    capability_invocation_id: Option<String>, parent_invocation_id: Option<String>,
    is_header_event: bool, level: String, kind: String, message: String,
    payload: serde_json::Value, created_at: String,
}

#[derive(Serialize)]
struct EventsResult { events: Vec<EventRecord>, has_more: bool, next_cursor: Option<i64> }

#[derive(Deserialize)]
struct InferenceQuery {
    invocation_id: Option<String>, step_id: Option<String>, capability_invocation_id: Option<String>,
    from: Option<String>, to: Option<String>, before: Option<String>, limit: Option<i64>,
}

#[derive(Serialize)]
struct InvocationInput {
    sequence_no: i64, input_type: String, input_origin: String,
    input_source: String, label: String, content: String,
}

#[derive(Serialize)]
struct InvocationRecord {
    id: String, step_id: String, stage_type: String, capability_invocation_id: Option<String>,
    provider: String, model: String, transport: String, status: String,
    error_message: Option<String>, created_at: String, completed_at: Option<String>,
    inputs: Vec<InvocationInput>, response_text: Option<String>,
    response_payload: Option<serde_json::Value>, provider_metadata: Option<serde_json::Value>,
    input_tokens: Option<i64>, output_tokens: Option<i64>, cached_input_tokens: Option<i64>,
    cost_amount: Option<f64>, cost_currency: Option<String>,
}

#[derive(Serialize)]
struct InferenceResult { invocations: Vec<InvocationRecord>, has_more: bool }

type ApiError = (StatusCode, String);
fn db_error(error: sqlx::Error) -> ApiError {
    tracing::error!(%error, "impact query failed");
    (StatusCode::INTERNAL_SERVER_ERROR, "Unable to load impact data".to_owned())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/workflow-runs/:run_id/impact", get(get_impact))
        .route("/api/workflow-runs/:run_id/impact/events", get(get_events))
        .route("/api/workflow-runs/:run_id/impact/inference", get(get_inference))
        .route("/api/workflow-runs/:run_id/impact/detail", get(get_detail))
        .route("/api/workflow-runs/:run_id/inference/inputs", get(get_inference_inputs))
}

async fn check_run(state: &AppState, run_id: &str) -> Result<(), ApiError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_runs WHERE id = ?")
        .bind(run_id).fetch_one(&state.db).await.map_err(db_error)?;
    if count == 0 { return Err((StatusCode::NOT_FOUND, "Workflow not found".to_owned())); }
    Ok(())
}

#[derive(Deserialize)]
struct InferenceInputsQuery {
    search: Option<String>,
    stage_type: Option<String>,
    hide_defaults: Option<bool>,
    offset: Option<i64>,
    limit: Option<i64>,
}

#[derive(Serialize)]
struct InferenceInputRow {
    id: String,
    invocation_id: String,
    workflow_run_id: String,
    step_id: String,
    stage_type: String,
    session_id: String,
    provider: String,
    model: String,
    sequence_no: i64,
    input_type: String,
    input_origin: String,
    input_source: String,
    label: String,
    content: String,
    created_at: String,
}

#[derive(Serialize)]
struct InferenceInputsResult {
    inputs: Vec<InferenceInputRow>,
    total: i64,
    has_more: bool,
    next_offset: Option<i64>,
}

async fn get_inference_inputs(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Query(query): Query<InferenceInputsQuery>,
) -> Result<Json<InferenceInputsResult>, ApiError> {
    check_run(&state, &run_id).await?;

    let limit = query.limit.unwrap_or(100).clamp(1, 250);
    let offset = query.offset.unwrap_or(0).max(0);
    let search = query.search.as_deref().map(str::trim).filter(|value| !value.is_empty());
    let stage_type = query.stage_type.as_deref().map(str::trim).filter(|value| !value.is_empty());
    let hide_defaults = query.hide_defaults.unwrap_or(true);

    let scope = "(i.workflow_run_id = ? OR EXISTS (SELECT 1 FROM supervisor_work_units target JOIN supervisor_work_units sibling ON sibling.feature_id = target.feature_id WHERE target.workflow_run_id = ? AND target.feature_id IS NOT NULL AND sibling.workflow_run_id = i.workflow_run_id))";
    let filters = format!("{} AND (? = 0 OR x.input_origin = 'user_custom') AND (? IS NULL OR i.stage_type = ?) AND (? IS NULL OR x.content LIKE '%' || ? || '%' OR x.label LIKE '%' || ? || '%' OR x.input_source LIKE '%' || ? || '%')", scope);

    let count_sql = format!("SELECT COUNT(*) FROM inference_invocation_inputs x JOIN inference_invocations i ON i.id = x.invocation_id WHERE {}", filters);
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(count_sql))
        .bind(&run_id)
        .bind(&run_id)
        .bind(hide_defaults)
        .bind(stage_type)
        .bind(stage_type)
        .bind(search)
        .bind(search)
        .bind(search)
        .bind(search)
        .fetch_one(&state.db)
        .await
        .map_err(db_error)?;

    let rows_sql = format!("SELECT x.id, x.invocation_id, i.workflow_run_id, i.step_id, i.stage_type, i.inference_session_id, i.provider, i.model, x.sequence_no, x.input_type, x.input_origin, x.input_source, x.label, x.content, x.created_at FROM inference_invocation_inputs x JOIN inference_invocations i ON i.id = x.invocation_id WHERE {} ORDER BY x.created_at ASC, i.created_at ASC, x.sequence_no ASC, x.id ASC LIMIT ? OFFSET ?", filters);

    let rows = sqlx::query(sqlx::AssertSqlSafe(rows_sql))
        .bind(&run_id)
        .bind(&run_id)
        .bind(hide_defaults)
        .bind(stage_type)
        .bind(stage_type)
        .bind(search)
        .bind(search)
        .bind(search)
        .bind(search)
        .bind(limit + 1)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(db_error)?;

    let has_more = rows.len() as i64 > limit;
    let inputs = rows.into_iter().take(limit as usize).map(|row| InferenceInputRow {
        id: row.get("id"),
        invocation_id: row.get("invocation_id"),
        workflow_run_id: row.get("workflow_run_id"),
        step_id: row.get("step_id"),
        stage_type: row.get("stage_type"),
        session_id: row.get("inference_session_id"),
        provider: row.get("provider"),
        model: row.get("model"),
        sequence_no: row.get("sequence_no"),
        input_type: row.get("input_type"),
        input_origin: row.get("input_origin"),
        input_source: row.get("input_source"),
        label: row.get("label"),
        content: row.get("content"),
        created_at: row.get("created_at"),
    }).collect::<Vec<_>>();

    let next_offset = if has_more { Some(offset + limit) } else { None };

    Ok(Json(InferenceInputsResult {
        inputs,
        total,
        has_more,
        next_offset,
    }))
}

async fn get_impact(State(state): State<AppState>, Path(run_id): Path<String>) -> Result<Json<WorkflowImpact>, ApiError> {
    check_run(&state, &run_id).await?;
    let events = sqlx::query("SELECT COUNT(DISTINCT CASE WHEN kind LIKE '%stage%start%' THEN stage_execution_id END) AS stages, COUNT(DISTINCT CASE WHEN kind LIKE '%stage%complet%' THEN stage_execution_id END) AS completed, COUNT(DISTINCT CASE WHEN kind LIKE '%stage%fail%' THEN stage_execution_id END) AS failed, COUNT(DISTINCT capability_invocation_id) AS capabilities FROM workflow_events WHERE run_id = ?")
        .bind(&run_id).fetch_one(&state.db).await.map_err(db_error)?;
    let inference = sqlx::query("SELECT (SELECT COUNT(*) FROM inference_invocation_inputs x JOIN inference_invocations i ON i.id = x.invocation_id WHERE i.workflow_run_id = ? AND x.input_origin = 'user_custom') AS inputs, (SELECT COUNT(*) FROM inference_invocation_outputs o JOIN inference_invocations i ON i.id = o.invocation_id WHERE i.workflow_run_id = ?) AS outputs, (SELECT COALESCE(SUM(o.input_tokens), 0) FROM inference_invocation_outputs o JOIN inference_invocations i ON i.id = o.invocation_id WHERE i.workflow_run_id = ?) AS in_tokens, (SELECT COALESCE(SUM(o.output_tokens), 0) FROM inference_invocation_outputs o JOIN inference_invocations i ON i.id = o.invocation_id WHERE i.workflow_run_id = ?) AS out_tokens")
        .bind(&run_id).bind(&run_id).bind(&run_id).bind(&run_id).fetch_one(&state.db).await.map_err(db_error)?;
    let changes = sqlx::query("SELECT COUNT(*) AS attempts, COALESCE(SUM(CASE WHEN status = 'applied' OR status = 'success' THEN 1 ELSE 0 END), 0) AS applied, COALESCE(SUM(created_count), 0) AS created, COALESCE(SUM(modified_count), 0) AS modified, COALESCE(SUM(deleted_count), 0) AS deleted, COALESCE(SUM(moved_count), 0) AS moved FROM changeset_attempts WHERE run_id = ? AND direction = 'forward'")
        .bind(&run_id).fetch_one(&state.db).await.map_err(db_error)?;
    let paths = sqlx::query("SELECT DISTINCT COALESCE(NULLIF(path_after, ''), path_before) AS path FROM changeset_file_effects e JOIN changeset_attempts a ON a.id = e.attempt_id WHERE a.run_id = ? AND a.direction = 'forward' AND e.status = 'applied' AND COALESCE(NULLIF(path_after, ''), path_before) IS NOT NULL ORDER BY path")
        .bind(&run_id).fetch_all(&state.db).await.map_err(db_error)?;
    let file_paths: Vec<String> = paths.into_iter().map(|row| row.get("path")).collect();
    let line_stats = sqlx::query("SELECT COUNT(*) AS attempts, SUM(CASE WHEN json_type(result_json, '$.stats.lines_added') = 'integer' AND json_type(result_json, '$.stats.lines_removed') = 'integer' THEN 1 ELSE 0 END) AS measured, COALESCE(SUM(json_extract(result_json, '$.stats.lines_added')), 0) AS added, COALESCE(SUM(json_extract(result_json, '$.stats.lines_removed')), 0) AS removed FROM changeset_attempts WHERE run_id = ? AND direction = 'forward'")
        .bind(&run_id).fetch_one(&state.db).await.map_err(db_error)?;
    let measured: i64 = line_stats.get("measured");
    let attempts: i64 = line_stats.get("attempts");
    let complete = measured == attempts;
    Ok(Json(WorkflowImpact {
        run_id, user_inputs: inference.get("inputs"), inference_outputs: inference.get("outputs"),
        input_tokens: inference.get("in_tokens"), output_tokens: inference.get("out_tokens"),
        stage_executions: events.get("stages"), completed_stages: events.get("completed"),
        failed_stages: events.get("failed"), capability_activations: events.get("capabilities"),
        changeset_attempts: changes.get("attempts"), changeset_applied: changes.get("applied"),
        files_changed: file_paths.len() as i64, files_created: changes.get("created"),
        files_modified: changes.get("modified"), files_deleted: changes.get("deleted"),
        files_moved: changes.get("moved"), file_paths,
        lines_added: if complete { Some(line_stats.get("added")) } else { None },
        lines_removed: if complete { Some(line_stats.get("removed")) } else { None },
    }))
}

#[derive(Serialize)]
struct ImpactDetail {
    effects: Vec<serde_json::Value>,
    inference: Vec<serde_json::Value>,
    attempts: Vec<serde_json::Value>,
}

async fn get_detail(State(state): State<AppState>, Path(run_id): Path<String>) -> Result<Json<ImpactDetail>, ApiError> {
    check_run(&state, &run_id).await?;
    let effects = sqlx::query("SELECT a.id AS attempt_id, a.step_id, a.status AS attempt_status, a.created_at, e.action, e.status, e.path_before, e.path_after, e.forward_op_json, e.error FROM changeset_file_effects e JOIN changeset_attempts a ON a.id = e.attempt_id WHERE a.run_id = ? AND a.direction = 'forward' ORDER BY a.created_at DESC, e.op_index, e.action_index")
        .bind(&run_id).fetch_all(&state.db).await.map_err(db_error)?
        .into_iter().map(|r| serde_json::json!({
            "attempt_id": r.get::<String,_>("attempt_id"), "step_id": r.get::<Option<String>,_>("step_id"),
            "attempt_status": r.get::<String,_>("attempt_status"), "created_at": r.get::<String,_>("created_at"),
            "action": r.get::<String,_>("action"), "status": r.get::<String,_>("status"),
            "path_before": r.get::<Option<String>,_>("path_before"), "path_after": r.get::<Option<String>,_>("path_after"),
            "operation": serde_json::from_str::<serde_json::Value>(&r.get::<String,_>("forward_op_json")).unwrap_or(serde_json::Value::Null),
            "error": r.get::<Option<String>,_>("error")
        })).collect();
    let inference = sqlx::query("SELECT i.id, i.inference_session_id, i.step_id, i.stage_type, i.provider, i.model, i.transport, i.status, i.error_message, i.created_at, i.completed_at, o.input_tokens, o.output_tokens, o.cached_input_tokens, o.cost_amount, o.cost_currency, o.provider_metadata_json FROM inference_invocations i LEFT JOIN inference_invocation_outputs o ON o.invocation_id = i.id WHERE i.workflow_run_id = ? ORDER BY i.created_at DESC")
        .bind(&run_id).fetch_all(&state.db).await.map_err(db_error)?
        .into_iter().map(|r| {
            let metadata: Option<String> = r.get("provider_metadata_json");
            serde_json::json!({
                "id": r.get::<String,_>("id"), "session_id": r.get::<String,_>("inference_session_id"),
                "step_id": r.get::<String,_>("step_id"), "stage_type": r.get::<String,_>("stage_type"),
                "provider": r.get::<String,_>("provider"), "model": r.get::<String,_>("model"),
                "transport": r.get::<String,_>("transport"), "status": r.get::<String,_>("status"),
                "error": r.get::<Option<String>,_>("error_message"), "created_at": r.get::<String,_>("created_at"),
                "completed_at": r.get::<Option<String>,_>("completed_at"), "input_tokens": r.get::<Option<i64>,_>("input_tokens"),
                "output_tokens": r.get::<Option<i64>,_>("output_tokens"), "cached_tokens": r.get::<Option<i64>,_>("cached_input_tokens"),
                "cost": r.get::<Option<f64>,_>("cost_amount"), "currency": r.get::<Option<String>,_>("cost_currency"),
                "metadata": metadata.and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
            })
        }).collect();
    let attempts = sqlx::query("SELECT id, status, total_ops, applied_ops, failed_ops, skipped_ops, total_actions, applied_actions, failed_actions, created_at FROM changeset_attempts WHERE run_id = ? AND direction = 'forward' ORDER BY created_at DESC")
        .bind(&run_id).fetch_all(&state.db).await.map_err(db_error)?
        .into_iter().map(|r| serde_json::json!({
            "id": r.get::<String,_>("id"), "status": r.get::<String,_>("status"),
            "total_ops": r.get::<i64,_>("total_ops"), "applied_ops": r.get::<i64,_>("applied_ops"),
            "failed_ops": r.get::<i64,_>("failed_ops"), "skipped_ops": r.get::<i64,_>("skipped_ops"),
            "total_actions": r.get::<i64,_>("total_actions"), "applied_actions": r.get::<i64,_>("applied_actions"),
            "failed_actions": r.get::<i64,_>("failed_actions"), "created_at": r.get::<String,_>("created_at")
        })).collect();
    Ok(Json(ImpactDetail { effects, inference, attempts }))
}

async fn get_events(State(state): State<AppState>, Path(run_id): Path<String>, Query(q): Query<EventQuery>) -> Result<Json<EventsResult>, ApiError> {
    check_run(&state, &run_id).await?;
    let limit = q.limit.unwrap_or(100).clamp(1, 250);
    let rows = sqlx::query(r#"SELECT id, sequence_no, step_id, stage_execution_id, capability_invocation_id, parent_invocation_id, is_header_event, level, kind, message, payload_json, created_at FROM workflow_events WHERE run_id = ? AND (? IS NULL OR sequence_no > ?) AND (? IS NULL OR sequence_no < ?) AND (? IS NULL OR created_at >= ?) AND (? IS NULL OR created_at <= ?) AND (? IS NULL OR step_id = ?) AND (? IS NULL OR stage_execution_id = ?) AND (? IS NULL OR capability_invocation_id = ?) AND (? IS NULL OR kind = ?) AND (? IS NULL OR level = ?) AND (? IS NULL OR message LIKE '%' || ? || '%' OR payload_json LIKE '%' || ? || '%') ORDER BY sequence_no ASC LIMIT ?"#)
        .bind(&run_id).bind(q.after).bind(q.after).bind(q.before).bind(q.before)
        .bind(&q.from).bind(&q.from).bind(&q.to).bind(&q.to)
        .bind(&q.step_id).bind(&q.step_id)
        .bind(&q.stage_execution_id).bind(&q.stage_execution_id)
        .bind(&q.capability_invocation_id).bind(&q.capability_invocation_id)
        .bind(&q.kind).bind(&q.kind).bind(&q.level).bind(&q.level)
        .bind(&q.search).bind(&q.search).bind(&q.search).bind(limit + 1)
        .fetch_all(&state.db).await.map_err(db_error)?;
    let has_more = rows.len() as i64 > limit;
    let events: Vec<EventRecord> = rows.into_iter().take(limit as usize).map(|row| {
        let payload_json: String = row.get("payload_json");
        EventRecord {
            id: row.get("id"), sequence_no: row.get("sequence_no"), step_id: row.get("step_id"),
            stage_execution_id: row.get("stage_execution_id"),
            capability_invocation_id: row.get("capability_invocation_id"),
            parent_invocation_id: row.get("parent_invocation_id"),
            is_header_event: row.get::<i64, _>("is_header_event") != 0,
            level: row.get("level"), kind: row.get("kind"), message: row.get("message"),
            payload: serde_json::from_str(&payload_json).unwrap_or(serde_json::Value::Null),
            created_at: row.get("created_at"),
        }
    }).collect();
    let next_cursor = events.last().map(|event| event.sequence_no);
    Ok(Json(EventsResult { events, has_more, next_cursor }))
}

async fn get_inference(State(state): State<AppState>, Path(run_id): Path<String>, Query(q): Query<InferenceQuery>) -> Result<Json<InferenceResult>, ApiError> {
    check_run(&state, &run_id).await?;
    let limit = q.limit.unwrap_or(30).clamp(1, 100);
    let rows = sqlx::query(r#"SELECT i.id, i.step_id, i.stage_type, i.capability_invocation_id, i.provider, i.model, i.transport, i.status, i.error_message, i.created_at, i.completed_at, o.response_text, o.response_payload_json, o.provider_metadata_json, o.input_tokens, o.output_tokens, o.cached_input_tokens, o.cost_amount, o.cost_currency FROM inference_invocations i LEFT JOIN inference_invocation_outputs o ON o.invocation_id = i.id WHERE i.workflow_run_id = ? AND (? IS NULL OR i.id = ?) AND (? IS NULL OR i.step_id = ?) AND (? IS NULL OR i.capability_invocation_id = ?) AND (? IS NULL OR i.created_at >= ?) AND (? IS NULL OR i.created_at <= ?) AND (? IS NULL OR i.created_at < ?) ORDER BY i.created_at DESC, i.id DESC LIMIT ?"#)
        .bind(&run_id).bind(&q.invocation_id).bind(&q.invocation_id)
        .bind(&q.step_id).bind(&q.step_id)
        .bind(&q.capability_invocation_id).bind(&q.capability_invocation_id)
        .bind(&q.from).bind(&q.from).bind(&q.to).bind(&q.to)
        .bind(&q.before).bind(&q.before).bind(limit + 1)
        .fetch_all(&state.db).await.map_err(db_error)?;
    let has_more = rows.len() as i64 > limit;
    let mut invocations = Vec::new();
    for row in rows.into_iter().take(limit as usize) {
        let id: String = row.get("id");
        let inputs = sqlx::query("SELECT sequence_no, input_type, input_origin, input_source, label, content FROM inference_invocation_inputs WHERE invocation_id = ? ORDER BY sequence_no")
            .bind(&id).fetch_all(&state.db).await.map_err(db_error)?
            .into_iter().map(|input| InvocationInput {
                sequence_no: input.get("sequence_no"), input_type: input.get("input_type"),
                input_origin: input.get("input_origin"), input_source: input.get("input_source"),
                label: input.get("label"), content: input.get("content"),
            }).collect();
        let payload: Option<String> = row.get("response_payload_json");
        let metadata: Option<String> = row.get("provider_metadata_json");
        invocations.push(InvocationRecord {
            id, step_id: row.get("step_id"), stage_type: row.get("stage_type"),
            capability_invocation_id: row.get("capability_invocation_id"),
            provider: row.get("provider"), model: row.get("model"), transport: row.get("transport"),
            status: row.get("status"), error_message: row.get("error_message"),
            created_at: row.get("created_at"), completed_at: row.get("completed_at"),
            inputs, response_text: row.get("response_text"),
            response_payload: payload.and_then(|value| serde_json::from_str(&value).ok()),
            provider_metadata: metadata.and_then(|value| serde_json::from_str(&value).ok()),
            input_tokens: row.get("input_tokens"), output_tokens: row.get("output_tokens"),
            cached_input_tokens: row.get("cached_input_tokens"), cost_amount: row.get("cost_amount"),
            cost_currency: row.get("cost_currency"),
        });
    }
    Ok(Json(InferenceResult { invocations, has_more }))
}
