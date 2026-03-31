use axum::{
    extract::{Path, Query, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use tokio::time::{self, Duration};
use futures_util::StreamExt;
use tokio_stream::wrappers::IntervalStream;
use uuid::Uuid;

use crate::{app_state::AppState, engine::{load_run, load_template_definition}};

#[derive(Debug, Serialize)]
struct StageChainEvent {
    id: String,
    run_id: String,
    step_id: Option<String>,
    stage_execution_id: Option<String>,
    capability_invocation_id: Option<String>,
    parent_invocation_id: Option<String>,
    sequence_no: i64,
    level: String,
    kind: String,
    message: String,
    payload: Value,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct StageExecutionChain {
    run_id: String,
    step_id: String,
    stage_execution_id: String,
    items: Vec<StageChainEvent>,
}

#[derive(Debug, Serialize)]
struct CapabilityChainSummary {
    key: String,
    capability_id: String,
    name: String,
    status_color: String,
    status_label: String,
    message: String,
    started_at: Option<String>,
    duration_ms: Option<i64>,
    latest_created_at: String,
    is_active: bool,
    event_count: usize,
}

#[derive(Debug, Serialize)]
struct StageChainSummary {
    key: String,
    step_id: String,
    label: String,
    stage_execution_id: String,
    latest_kind: String,
    latest_message: String,
    latest_level: String,
    latest_created_at: String,
    is_current: bool,
    is_active: bool,
    event_count: usize,
    duration_ms: Option<i64>,
    capabilities: Vec<CapabilityChainSummary>,
}

#[derive(Debug, Serialize)]
struct EventChainSummaryResponse {
    run_id: String,
    stages: Vec<StageChainSummary>,
}

#[derive(Debug, Deserialize)]
struct StreamQuery {
    #[serde(default)]
    after_sequence: Option<i64>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/workflow-runs/:run_id/event-chain", get(get_event_chain_summary))
        .route("/api/workflow-runs/:run_id/stages/:step_id/executions/:stage_execution_id", get(get_stage_execution_chain))
        .route("/api/workflow-runs/:run_id/events/stream", get(stream_events))
}

async fn get_event_chain_summary(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<EventChainSummaryResponse>, (axum::http::StatusCode, String)> {
    Ok(Json(build_event_chain_summary(&state, run_id).await?))
}

async fn build_event_chain_summary(
    state: &AppState,
    run_id: Uuid,
) -> Result<EventChainSummaryResponse, (axum::http::StatusCode, String)> {
    let run = load_run(state, run_id).await.map_err(internal)?;
    let definition = load_template_definition(state, &run).await.map_err(internal)?;
    let order = definition
        .map(|d| d.steps.into_iter().map(|s| (s.id, s.name)).collect::<Vec<_>>())
        .unwrap_or_default();

    let rows = sqlx::query(
        "SELECT id, run_id, step_id, stage_execution_id, capability_invocation_id, parent_invocation_id, sequence_no, level, kind, message, payload_json, created_at FROM workflow_events WHERE run_id = ? AND step_id IS NOT NULL AND stage_execution_id IS NOT NULL ORDER BY sequence_no ASC"
    )
    .bind(run_id.to_string())
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    use std::collections::BTreeMap;

    let mut grouped: BTreeMap<(String, String), Vec<StageChainEvent>> = BTreeMap::new();
    for row in rows {
        let item = row_to_stage_chain_event(row)?;
        let step_id = item.step_id.clone().unwrap_or_default();
        let stage_execution_id = item.stage_execution_id.clone().unwrap_or_default();
        grouped.entry((step_id, stage_execution_id)).or_default().push(item);
    }

    let mut stages = grouped
        .into_iter()
        .filter_map(|((step_id, stage_execution_id), stage_rows)| {
            let latest = stage_rows.last()?;
            let label = order
                .iter()
                .find(|(id, _)| id == &step_id)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| step_id.clone());

            let completed_stage_event = stage_rows
                .iter()
                .rev()
                .find(|event| event.kind == "stage_execution_completed");

            let duration_ms = completed_stage_event.and_then(|event| {
                event.payload
                    .get("duration_ms")
                    .and_then(Value::as_i64)
            });

            let mut capability_groups: BTreeMap<String, Vec<&StageChainEvent>> = BTreeMap::new();
            for event in &stage_rows {
                let Some(capability_id) = event.capability_invocation_id.clone() else {
                    continue;
                };
                capability_groups.entry(capability_id).or_default().push(event);
            }

            let mut capabilities = capability_groups
                .into_iter()
                .filter_map(|(capability_id, capability_rows)| {
                    let first = capability_rows.first()?;
                    let latest_capability = capability_rows.last()?;
                    let started = capability_rows
                        .iter()
                        .find(|event| event.kind.ends_with("_started"))
                        .copied()
                        .unwrap_or(first);
                    let completed = capability_rows
                        .iter()
                        .rev()
                        .find(|event| event.kind.ends_with("_completed") || event.kind.ends_with("_failed"))
                        .copied();
                    let status_event = completed.unwrap_or(latest_capability);
                    let capability_name = first
                        .kind
                        .trim_end_matches("_started")
                        .trim_end_matches("_completed")
                        .trim_end_matches("_failed")
                        .split('_')
                        .filter(|part| !part.is_empty())
                        .map(|part| {
                            let mut chars = part.chars();
                            match chars.next() {
                                Some(ch) => format!("{}{}", ch.to_uppercase(), chars.as_str()),
                                None => String::new(),
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    let status_color = if status_event.level == "error" {
                        "red"
                    } else if completed.is_some() {
                        "green"
                    } else {
                        "blue"
                    }
                    .to_string();
                    let status_label = if status_event.level == "error" {
                        "ERROR"
                    } else if completed.is_some() {
                        "SUCCESS"
                    } else {
                        "RUNNING"
                    }
                    .to_string();
                    let capability_duration_ms = completed.and_then(|event| {
                        event.payload
                            .get("duration_ms")
                            .and_then(Value::as_i64)
                    });

                    Some(CapabilityChainSummary {
                        key: format!("{}-{}", stage_execution_id, capability_id),
                        capability_id,
                        name: capability_name,
                        status_color,
                        status_label,
                        message: status_event.message.clone(),
                        started_at: Some(started.created_at.clone()),
                        duration_ms: capability_duration_ms,
                        latest_created_at: latest_capability.created_at.clone(),
                        is_active: completed.is_none(),
                        event_count: capability_rows.len(),
                    })
                })
                .collect::<Vec<_>>();

            capabilities.sort_by(|a, b| a.latest_created_at.cmp(&b.latest_created_at));

            Some(StageChainSummary {
                key: format!("{}-{}", step_id, stage_execution_id),
                step_id: step_id.clone(),
                label,
                stage_execution_id,
                latest_kind: latest.kind.clone(),
                latest_message: latest.message.clone(),
                latest_level: latest.level.clone(),
                latest_created_at: latest.created_at.clone(),
                is_current: run.current_step_id.as_deref() == Some(step_id.as_str()) && completed_stage_event.is_none(),
                is_active: completed_stage_event.is_none(),
                event_count: stage_rows.len(),
                duration_ms,
                capabilities,
            })
        })
        .collect::<Vec<_>>();

    stages.sort_by(|a, b| {
        let a_rank = if a.is_current && a.is_active { 0 } else if a.is_active { 1 } else { 2 };
        let b_rank = if b.is_current && b.is_active { 0 } else if b.is_active { 1 } else { 2 };
        a_rank.cmp(&b_rank).then_with(|| b.latest_created_at.cmp(&a.latest_created_at))
    });

    stages.truncate(6);

    Ok(EventChainSummaryResponse {
        run_id: run_id.to_string(),
        stages,
    })
}

async fn get_stage_execution_chain(
    State(state): State<AppState>,
    Path((run_id, step_id, stage_execution_id)): Path<(Uuid, String, String)>,
) -> Result<Json<StageExecutionChain>, (axum::http::StatusCode, String)> {
    let rows = sqlx::query(
        "SELECT id, run_id, step_id, stage_execution_id, capability_invocation_id, parent_invocation_id, sequence_no, level, kind, message, payload_json, created_at FROM workflow_events WHERE run_id = ? AND step_id = ? AND stage_execution_id = ? ORDER BY sequence_no ASC"
    )
    .bind(run_id.to_string())
    .bind(&step_id)
    .bind(&stage_execution_id)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    let items = rows.into_iter().map(row_to_stage_chain_event).collect::<Result<Vec<_>, _>>()?;

    Ok(Json(StageExecutionChain {
        run_id: run_id.to_string(),
        step_id,
        stage_execution_id,
        items,
    }))
}

async fn stream_events(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Query(query): Query<StreamQuery>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut last_sequence = query.after_sequence.unwrap_or(0);
    let mut sent_initial_snapshot = false;
    let stream = futures_util::StreamExt::then(IntervalStream::new(time::interval(Duration::from_millis(1000))), move |_| {
        let state = state.clone();
        let run_id = run_id;
        async move {
            let rows = sqlx::query(
                "SELECT id, run_id, step_id, stage_execution_id, capability_invocation_id, parent_invocation_id, sequence_no, level, kind, message, payload_json, created_at FROM workflow_events WHERE run_id = ? AND sequence_no > ? ORDER BY sequence_no ASC"
            )
            .bind(run_id.to_string())
            .bind(last_sequence)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

            let mut events = Vec::new();
            let mut saw_new_rows = false;
            for row in rows {
                let sequence_no: i64 = row.get("sequence_no");
                if sequence_no > last_sequence {
                    last_sequence = sequence_no;
                }
                if let Ok(item) = row_to_stage_chain_event(row) {
                    saw_new_rows = true;
                    if let Ok(text) = serde_json::to_string(&item) {
                        events.push(Ok(Event::default().event("workflow_event").data(text)));
                    }
                }
            }

            if !sent_initial_snapshot || saw_new_rows {
                if let Ok(summary) = build_event_chain_summary(&state, run_id).await {
                    if let Ok(text) = serde_json::to_string(&summary) {
                        events.push(Ok(Event::default().event("monitor_snapshot").data(text)));
                    }
                }
                sent_initial_snapshot = true;
            }

            events
        }
    }).flat_map(tokio_stream::iter);

    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn row_to_stage_chain_event(row: sqlx::sqlite::SqliteRow) -> Result<StageChainEvent, (axum::http::StatusCode, String)> {
    Ok(StageChainEvent {
        id: row.get("id"),
        run_id: row.get("run_id"),
        step_id: row.get("step_id"),
        stage_execution_id: row.get("stage_execution_id"),
        capability_invocation_id: row.get("capability_invocation_id"),
        parent_invocation_id: row.get("parent_invocation_id"),
        sequence_no: row.get("sequence_no"),
        level: row.get("level"),
        kind: row.get("kind"),
        message: row.get("message"),
        payload: serde_json::from_str(row.get::<String, _>("payload_json").as_str()).map_err(internal)?,
        created_at: row.get("created_at"),
    })
}

fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
