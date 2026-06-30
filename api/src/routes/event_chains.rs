use std::convert::Infallible;

use axum::{
    extract::{Path, Query, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::get,
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use tokio::sync::broadcast::error::RecvError;
use tokio_stream::wrappers::UnboundedReceiverStream;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::{load_run, load_template_definition},
    models::{SprintEventStreamItem, WorkflowEventStreamItem},
};

type StageChainEvent = WorkflowEventStreamItem;

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
    completed_at: Option<String>,
    duration_ms: Option<i64>,
    latest_created_at: String,
    latest_kind: String,
    latest_level: String,
    is_active: bool,
    event_count: usize,
    start_event_id: Option<String>,
    end_event_id: Option<String>,
    start_payload: Value,
    end_payload: Value,
    latest_payload: Value,
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

#[derive(Debug, Serialize)]
struct RuntimeProjectionResponse {
    runs: Vec<EventChainSummaryResponse>,
}

#[derive(Debug, Deserialize)]
struct StreamQuery {
    #[serde(default)]
    after_sequence: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RuntimeEventQuery {
    #[serde(default)]
    run_id: Option<Uuid>,
    #[serde(default)]
    supervisor_run_id: Option<Uuid>,
    #[serde(default)]
    workflow_key: Option<String>,
    #[serde(default)]
    repo_ref: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    after_sequence: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeNode {
    key: String,
    node_type: String,
    id: String,
    status: String,
    title: String,
    repo_ref: String,
    workflow_key: Option<String>,
    current_step_id: Option<String>,
    updated_at: String,
    payload: Value,
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeEdge {
    key: String,
    parent_key: String,
    child_key: String,
    edge_type: String,
    label: String,
    sort_order: i64,
    payload: Value,
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeSnapshotResponse {
    nodes: Vec<RuntimeNode>,
    edges: Vec<RuntimeEdge>,
    latest_sequence_no: i64,
    server_time: String,
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeEventEnvelope {
    scope: String,
    node_key: String,
    run_id: Option<String>,
    supervisor_run_id: Option<String>,
    workflow_key: Option<String>,
    repo_ref: Option<String>,
    event: StageChainEvent,
}

#[derive(Debug, Clone, Serialize)]
struct SprintEventEnvelope {
    scope: String,
    node_key: String,
    run_id: Option<String>,
    supervisor_run_id: Option<String>,
    workflow_key: Option<String>,
    repo_ref: Option<String>,
    event: SprintEventStreamItem,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/events/snapshot", get(get_runtime_snapshot))
        .route("/api/events/projection", get(get_runtime_projection))
        .route("/api/events/stream", get(stream_runtime_events))
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

async fn get_runtime_projection(
    State(state): State<AppState>,
    Query(query): Query<RuntimeEventQuery>,
) -> Result<Json<RuntimeProjectionResponse>, (axum::http::StatusCode, String)> {
    let mut run_ids = runtime_filter_run_ids(&state, &query).await?;
    if run_ids.is_empty() && runtime_filter_allows_empty_run_set(&query) {
        run_ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM workflow_runs ORDER BY updated_at DESC LIMIT 100",
        )
        .fetch_all(&state.db)
        .await
        .map_err(internal)?;
    }

    let mut runs = Vec::new();
    for run_id in run_ids {
        let Ok(uuid) = Uuid::parse_str(&run_id) else {
            continue;
        };
        if let Ok(projection) = build_event_chain_summary(&state, uuid).await {
            runs.push(projection);
        }
    }

    Ok(Json(RuntimeProjectionResponse { runs }))
}

fn strip_runtime_context_from_capability_payload(payload: &Value) -> Value {
    let Some(object) = payload.as_object() else {
        return payload.clone();
    };

    let runtime_keys = [
        "run_context",
        "final_context",
        "prepared_context",
        "workflow_engine",
        "global_state",
        "local_state",
        "stage_state",
        "capability_results",
        "available_transitions",
        "blocked_on",
        "next_step_id",
        "current_step_id",
    ];

    let mut out = serde_json::Map::new();
    for (key, value) in object {
        if runtime_keys.iter().any(|runtime_key| runtime_key == key) {
            continue;
        }
        out.insert(key.clone(), value.clone());
    }

    Value::Object(out)
}

fn capability_result_payload(result: &Value) -> Value {
    result
        .get("result")
        .map(strip_runtime_context_from_capability_payload)
        .unwrap_or_else(|| strip_runtime_context_from_capability_payload(result))
}

fn payload_indicates_user_wait(payload: &Value) -> bool {
    payload
        .get("waiting_for_user")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || payload
            .get("needs_user_response")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || payload
            .get("result")
            .map(payload_indicates_user_wait)
            .unwrap_or(false)
}

fn event_indicates_operator_checkpoint_wait(event: &StageChainEvent) -> bool {
    event.kind == "operator_checkpoint_waiting"
        || event.kind == "stage_execution_waiting_for_operator_checkpoint"
        || event.kind == "workflow_waiting_for_operator_checkpoint"
        || event
            .payload
            .get("capability")
            .and_then(Value::as_str)
            .map(|capability| capability == "operator_checkpoint")
            .unwrap_or(false)
            && payload_indicates_user_wait(&event.payload)
}

fn capability_result_key(result: &Value) -> Option<String> {
    result
        .get("key")
        .or_else(|| result.get("capability"))
        .or_else(|| result.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn normalized_capability_key(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn capability_name_from_event(event: &StageChainEvent) -> String {
    let payload_name = event
        .payload
        .get("capability")
        .or_else(|| event.payload.get("capability_key"))
        .or_else(|| event.payload.get("key"))
        .or_else(|| event.payload.get("name"))
        .and_then(Value::as_str);

    let raw = payload_name.unwrap_or_else(|| {
        event.kind
            .trim_end_matches("_started")
            .trim_end_matches("_completed")
            .trim_end_matches("_failed")
    });

    raw
        .split(|ch: char| ch == '_' || ch == '-' || ch == '/')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(ch) => format!("{}{}", ch.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("/")
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

            let terminal_stage_event = stage_rows
                .iter()
                .rev()
                .find(|event| {
                    event.capability_invocation_id.is_none()
                        && (event.kind == "stage_execution_completed"
                            || event.kind == "stage_executed"
                            || event.kind.ends_with("_completed")
                            || event.kind.ends_with("_failed"))
                });

            let stage_terminal_duration_ms = terminal_stage_event.and_then(|event| {
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
                    let capability_name = capability_name_from_event(first);
                    let waiting_for_user = event_indicates_operator_checkpoint_wait(status_event);
                    let status_color = if waiting_for_user {
                        "yellow"
                    } else if status_event.level == "error" {
                        "red"
                    } else if completed.is_some() {
                        "green"
                    } else {
                        "blue"
                    }
                    .to_string();
                    let status_label = if waiting_for_user {
                        "USER INPUT"
                    } else if status_event.level == "error" {
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
                        completed_at: completed.map(|event| event.created_at.clone()),
                        duration_ms: capability_duration_ms,
                        latest_created_at: latest_capability.created_at.clone(),
                        latest_kind: status_event.kind.clone(),
                        latest_level: status_event.level.clone(),
                        is_active: completed.is_none(),
                        event_count: capability_rows.len(),
                        start_event_id: Some(started.id.clone()),
                        end_event_id: completed.map(|event| event.id.clone()),
                        start_payload: strip_runtime_context_from_capability_payload(&started.payload),
                        end_payload: completed
                            .map(|event| strip_runtime_context_from_capability_payload(&event.payload))
                            .unwrap_or(Value::Null),
                        latest_payload: strip_runtime_context_from_capability_payload(&status_event.payload),
                    })
                })
                .collect::<Vec<_>>();

            let result_stage_event = stage_rows
                .iter()
                .rev()
                .find(|event| {
                    event.capability_invocation_id.is_none()
                        && (event.kind == "stage_execution_completed"
                            || event.kind == "stage_execution_waiting_for_operator_checkpoint"
                            || event.kind == "stage_execution_waiting_for_disposition_review"
                            || event.kind == "stage_executed"
                            || event.payload
                                .get("capability_results")
                                .and_then(Value::as_array)
                                .map(|items| !items.is_empty())
                                .unwrap_or(false))
                });

            let capability_results = result_stage_event
                .and_then(|event| event.payload.get("capability_results"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            for (idx, result) in capability_results.iter().enumerate() {
                let Some(result_key) = capability_result_key(result) else {
                    continue;
                };
                let normalized_result_key = normalized_capability_key(&result_key);
                let result_payload = capability_result_payload(result);
                let ok = result.get("ok").and_then(Value::as_bool).unwrap_or(true);
                let result_message = result
                    .get("message")
                    .or_else(|| result.get("summary"))
                    .or_else(|| result.get("status"))
                    .and_then(Value::as_str)
                    .unwrap_or(&result_key)
                    .to_string();

                let result_waiting_for_user = result_key == "operator_checkpoint" && payload_indicates_user_wait(&result_payload);

                if let Some(existing) = capabilities.iter_mut().find(|capability| {
                    normalized_capability_key(&capability.name) == normalized_result_key
                        || normalized_capability_key(&capability.capability_id) == normalized_result_key
                }) {
                    existing.status_color = if result_waiting_for_user { "yellow" } else if ok { "green" } else { "red" }.to_string();
                    existing.status_label = if result_waiting_for_user { "USER INPUT" } else if ok { "SUCCESS" } else { "ERROR" }.to_string();
                    existing.is_active = result_waiting_for_user;
                    existing.latest_level = if result_waiting_for_user { "warn" } else if ok { "info" } else { "error" }.to_string();
                    existing.message = result_message;
                    if result_waiting_for_user {
                        existing.latest_payload = result_payload.clone();
                    } else {
                        if existing.end_payload.is_null() {
                            existing.end_payload = result_payload.clone();
                        }
                        if existing.latest_payload.is_null() {
                            existing.latest_payload = result_payload.clone();
                        }
                        if existing.completed_at.is_none() {
                            if let Some(stage_event) = result_stage_event {
                                existing.completed_at = Some(stage_event.created_at.clone());
                                existing.latest_created_at = stage_event.created_at.clone();
                                existing.latest_kind = stage_event.kind.clone();
                                existing.end_event_id = Some(stage_event.id.clone());
                                existing.duration_ms = stage_event
                                    .payload
                                    .get("duration_ms")
                                    .and_then(Value::as_i64)
                                    .or(existing.duration_ms);
                            }
                        }
                    }
                    continue;
                }

                let result_name = result_key
                    .split(|ch: char| ch == '_' || ch == '-' || ch == '/')
                    .filter(|part| !part.is_empty())
                    .map(|part| {
                        let mut chars = part.chars();
                        match chars.next() {
                            Some(ch) => format!("{}{}", ch.to_uppercase(), chars.as_str()),
                            None => String::new(),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                let stage_event = result_stage_event;
                capabilities.push(CapabilityChainSummary {
                    key: format!("{}-result-{}", stage_execution_id, result_key),
                    capability_id: format!("{}:result:{}", stage_execution_id, result_key),
                    name: result_name,
                    status_color: if result_waiting_for_user { "yellow" } else if ok { "green" } else { "red" }.to_string(),
                    status_label: if result_waiting_for_user { "USER INPUT" } else if ok { "SUCCESS" } else { "ERROR" }.to_string(),
                    message: result_message,
                    started_at: stage_event.map(|event| event.created_at.clone()),
                    completed_at: if result_waiting_for_user { None } else { stage_event.map(|event| event.created_at.clone()) },
                    duration_ms: None,
                    latest_created_at: stage_event.map(|event| event.created_at.clone()).unwrap_or_else(|| latest.created_at.clone()),
                    latest_kind: stage_event.map(|event| event.kind.clone()).unwrap_or_else(|| "capability_result".to_string()),
                    latest_level: if result_waiting_for_user { "warn" } else if ok { "info" } else { "error" }.to_string(),
                    is_active: result_waiting_for_user,
                    event_count: 1 + idx,
                    start_event_id: stage_event.map(|event| event.id.clone()),
                    end_event_id: if result_waiting_for_user { None } else { stage_event.map(|event| event.id.clone()) },
                    start_payload: Value::Null,
                    end_payload: if result_waiting_for_user { Value::Null } else { result_payload.clone() },
                    latest_payload: result_payload,
                });
            }

            if let Some(stage_event) = result_stage_event {
                if !event_indicates_operator_checkpoint_wait(stage_event) {
                    for capability in &mut capabilities {
                        if !capability.is_active {
                            continue;
                        }
                        capability.is_active = false;
                        capability.status_color = if capability.status_color == "red" { "red" } else { "green" }.to_string();
                        capability.status_label = if capability.status_label == "ERROR" || capability.status_label == "FAILED" {
                            capability.status_label.clone()
                        } else {
                            "SUCCESS".to_string()
                        };
                        capability.completed_at = Some(stage_event.created_at.clone());
                        capability.latest_created_at = stage_event.created_at.clone();
                        capability.latest_kind = stage_event.kind.clone();
                        capability.latest_level = if capability.status_color == "red" { "error" } else { "info" }.to_string();
                    }
                }
            }

            capabilities.sort_by(|a, b| {
                if a.is_active != b.is_active {
                    return b.is_active.cmp(&a.is_active);
                }
                b.latest_created_at.cmp(&a.latest_created_at)
            });

            let capability_terminal = !capabilities.is_empty() && capabilities.iter().all(|capability| !capability.is_active);
            let capability_failed = capabilities.iter().any(|capability| capability.status_label == "ERROR");
            let stage_is_active = terminal_stage_event.is_none() && !capability_terminal;
            let inferred_latest_kind = if terminal_stage_event.is_none() && capability_terminal {
                if capability_failed { "capability_stage_failed".to_string() } else { "capability_stage_completed".to_string() }
            } else {
                latest.kind.clone()
            };
            let inferred_latest_message = if terminal_stage_event.is_none() && capability_terminal {
                capabilities
                    .last()
                    .map(|capability| capability.message.clone())
                    .unwrap_or_else(|| latest.message.clone())
            } else {
                latest.message.clone()
            };
            let inferred_latest_level = if terminal_stage_event.is_none() && capability_terminal {
                if capability_failed { "error".to_string() } else { "info".to_string() }
            } else {
                latest.level.clone()
            };
            let duration_ms = stage_terminal_duration_ms.or_else(|| {
                if capability_terminal {
                    capabilities.iter().filter_map(|capability| capability.duration_ms).max()
                } else {
                    None
                }
            });

            Some(StageChainSummary {
                key: format!("{}-{}", step_id, stage_execution_id),
                step_id: step_id.clone(),
                label,
                stage_execution_id,
                latest_kind: inferred_latest_kind,
                latest_message: inferred_latest_message,
                latest_level: inferred_latest_level,
                latest_created_at: latest.created_at.clone(),
                is_current: run.current_step_id.as_deref() == Some(step_id.as_str()) && stage_is_active,
                is_active: stage_is_active,
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

async fn get_runtime_snapshot(
    State(state): State<AppState>,
    Query(query): Query<RuntimeEventQuery>,
) -> Result<Json<RuntimeSnapshotResponse>, (axum::http::StatusCode, String)> {
    Ok(Json(build_runtime_snapshot(&state, &query).await?))
}

async fn stream_events(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Query(query): Query<StreamQuery>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Event, Infallible>>();
    let state_for_task = state.clone();
    let run_id_str = run_id.to_string();
    let mut live_rx = state.subscribe_workflow_events();
    let mut last_sequence = query.after_sequence.unwrap_or(0);

    tokio::spawn(async move {
        let rows = sqlx::query(
            "SELECT id, run_id, step_id, stage_execution_id, capability_invocation_id, parent_invocation_id, sequence_no, level, kind, message, payload_json, created_at FROM workflow_events WHERE run_id = ? AND sequence_no > ? ORDER BY sequence_no ASC"
        )
        .bind(&run_id_str)
        .bind(last_sequence)
        .fetch_all(&state_for_task.db)
        .await
        .unwrap_or_default();

        let mut sent_snapshot = false;
        for row in rows {
            if let Ok(item) = row_to_stage_chain_event(row) {
                last_sequence = last_sequence.max(item.sequence_no);
                if let Some(event) = workflow_event_sse(&item) {
                    if tx.send(Ok(event)).is_err() {
                        return;
                    }
                }
            }
        }

        if let Ok(summary) = build_event_chain_summary(&state_for_task, run_id).await {
            if let Some(event) = monitor_snapshot_sse(&summary) {
                if tx.send(Ok(event)).is_err() {
                    return;
                }
                sent_snapshot = true;
            }
        }

        loop {
            match live_rx.recv().await {
                Ok(item) => {
                    if item.run_id != run_id_str || item.sequence_no <= last_sequence {
                        continue;
                    }
                    last_sequence = item.sequence_no;
                    if let Some(event) = workflow_event_sse(&item) {
                        if tx.send(Ok(event)).is_err() {
                            return;
                        }
                    }
                    if let Ok(summary) = build_event_chain_summary(&state_for_task, run_id).await {
                        if let Some(event) = monitor_snapshot_sse(&summary) {
                            if tx.send(Ok(event)).is_err() {
                                return;
                            }
                            sent_snapshot = true;
                        }
                    }
                }
                Err(RecvError::Lagged(_)) => {
                    let rows = sqlx::query(
                        "SELECT id, run_id, step_id, stage_execution_id, capability_invocation_id, parent_invocation_id, sequence_no, level, kind, message, payload_json, created_at FROM workflow_events WHERE run_id = ? AND sequence_no > ? ORDER BY sequence_no ASC"
                    )
                    .bind(&run_id_str)
                    .bind(last_sequence)
                    .fetch_all(&state_for_task.db)
                    .await
                    .unwrap_or_default();

                    let mut saw_rows = false;
                    for row in rows {
                        if let Ok(item) = row_to_stage_chain_event(row) {
                            saw_rows = true;
                            last_sequence = last_sequence.max(item.sequence_no);
                            if let Some(event) = workflow_event_sse(&item) {
                                if tx.send(Ok(event)).is_err() {
                                    return;
                                }
                            }
                        }
                    }

                    if saw_rows || !sent_snapshot {
                        if let Ok(summary) = build_event_chain_summary(&state_for_task, run_id).await {
                            if let Some(event) = monitor_snapshot_sse(&summary) {
                                if tx.send(Ok(event)).is_err() {
                                    return;
                                }
                                sent_snapshot = true;
                            }
                        }
                    }
                }
                Err(RecvError::Closed) => return,
            }
        }
    });

    Sse::new(UnboundedReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

fn row_to_sprint_event(row: sqlx::sqlite::SqliteRow) -> Result<SprintEventStreamItem, (axum::http::StatusCode, String)> {
    let payload_json: String = row.get("payload_json");
    Ok(SprintEventStreamItem {
        id: row.get("id"),
        sprint_id: row.get("sprint_id"),
        sequence_no: row.get("sequence_no"),
        event_type: row.get("event_type"),
        event_time: row.get("event_time"),
        feature_id: row.get("feature_id"),
        actor: row.get("actor"),
        message: row.get("message"),
        payload: serde_json::from_str(&payload_json).unwrap_or_else(|_| json!({})),
        created_at: row.get("created_at"),
    })
}

async fn runtime_sprint_event_rows(
    state: &AppState,
    query: &RuntimeEventQuery,
    after_sequence: i64,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, (axum::http::StatusCode, String)> {
    if matches!(query.scope.as_deref(), Some("workflow") | Some("workflow_run")) || query.run_id.is_some() {
        return Ok(Vec::new());
    }

    if let Some(supervisor_run_id) = query.supervisor_run_id {
        let rows = sqlx::query(
            "SELECT se.id, se.sprint_id, se.sequence_no, se.event_type, se.event_time, se.feature_id, se.actor, se.message, se.payload_json, se.created_at
             FROM sprint_events se
             JOIN sprints s ON s.id = se.sprint_id
             WHERE s.supervisor_run_id = ? AND se.sequence_no > ?
             ORDER BY se.sequence_no ASC",
        )
        .bind(supervisor_run_id.to_string())
        .bind(after_sequence)
        .fetch_all(&state.db)
        .await
        .map_err(internal)?;
        return Ok(rows);
    }

    let rows = sqlx::query(
        "SELECT id, sprint_id, sequence_no, event_type, event_time, feature_id, actor, message, payload_json, created_at
         FROM sprint_events
         WHERE sequence_no > ?
         ORDER BY created_at ASC, sequence_no ASC
         LIMIT 500",
    )
    .bind(after_sequence)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    Ok(rows)
}

async fn sprint_event_envelope(
    state: &AppState,
    query: &RuntimeEventQuery,
    event: SprintEventStreamItem,
) -> Result<Option<SprintEventEnvelope>, (axum::http::StatusCode, String)> {
    let row = sqlx::query("SELECT supervisor_run_id FROM sprints WHERE id = ?")
        .bind(&event.sprint_id)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?;
    let supervisor_run_id: Option<String> = row.and_then(|row| row.get("supervisor_run_id"));

    if let Some(expected) = query.supervisor_run_id {
        if supervisor_run_id.as_deref() != Some(expected.to_string().as_str()) {
            return Ok(None);
        }
    }

    Ok(Some(SprintEventEnvelope {
        scope: "sprint".to_string(),
        node_key: supervisor_run_id.as_deref().map(supervisor_node_key).unwrap_or_else(|| format!("sprint:{}", event.sprint_id)),
        run_id: None,
        supervisor_run_id,
        workflow_key: None,
        repo_ref: query.repo_ref.clone(),
        event,
    }))
}

fn sprint_event_sse(envelope: &SprintEventEnvelope) -> Option<Event> {
    let name = if envelope.event.event_type == "supervisor_snapshot" {
        "supervisor_snapshot"
    } else {
        "sprint_event"
    };
    serde_json::to_string(envelope).ok().map(|payload| {
        Event::default()
            .event(name)
            .retry(std::time::Duration::from_secs(5))
            .id(format!("sprint:{}:{}", envelope.event.sprint_id, envelope.event.sequence_no))
            .data(payload)
    })
}

async fn send_supervisor_snapshot_sse(
    state: &AppState,
    query: &RuntimeEventQuery,
    tx: &tokio::sync::mpsc::UnboundedSender<Result<Event, Infallible>>,
) -> bool {
    let Some(supervisor_id) = query.supervisor_run_id else {
        return true;
    };
    let Ok(run) = crate::supervisor::load_supervisor_run(state, supervisor_id).await else {
        return true;
    };
    let snapshot = json!({
        "scope": "sprint",
        "node_key": supervisor_node_key(&supervisor_id.to_string()),
        "run_id": null,
        "supervisor_run_id": supervisor_id.to_string(),
        "workflow_key": null,
        "repo_ref": run.root_repo_path,
        "event": {
            "id": format!("snapshot-{}", Utc::now().timestamp_millis()),
            "sprint_id": run.context.get("current_sprint_id").and_then(Value::as_str).unwrap_or(""),
            "sequence_no": 0,
            "event_type": "supervisor_snapshot",
            "event_time": Utc::now().to_rfc3339(),
            "feature_id": null,
            "actor": "system",
            "message": "current supervisor snapshot",
            "payload": {
                "supervisor_run_id": supervisor_id,
                "supervisor_run": run,
                "snapshot": true,
                "synthetic": true
            },
            "created_at": Utc::now().to_rfc3339()
        }
    });
    match Event::default()
        .event("supervisor_snapshot")
        .retry(std::time::Duration::from_secs(5))
        .data(snapshot.to_string())
    {
        event => tx.send(Ok(event)).is_ok(),
    }
}

async fn stream_runtime_events(
    State(state): State<AppState>,
    Query(query): Query<RuntimeEventQuery>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Event, Infallible>>();
    let state_for_task = state.clone();
    let mut workflow_live_rx = state.subscribe_workflow_events();
    let mut sprint_live_rx = state.subscribe_sprint_events();
    let mut last_sequence = query.after_sequence.unwrap_or(0);
    let mut last_sprint_sequence = query.after_sequence.unwrap_or(0);

    tokio::spawn(async move {
        if let Ok(snapshot) = build_runtime_snapshot(&state_for_task, &query).await {
            last_sequence = last_sequence.max(snapshot.latest_sequence_no);
            if let Some(event) = runtime_snapshot_sse(&snapshot) {
                if tx.send(Ok(event)).is_err() {
                    return;
                }
            }
        }

        let workflow_rows = runtime_event_rows(&state_for_task, &query, query.after_sequence.unwrap_or(0))
            .await
            .unwrap_or_default();
        for row in workflow_rows {
            if let Ok(item) = row_to_stage_chain_event(row) {
                last_sequence = last_sequence.max(item.sequence_no);
                if let Ok(Some(envelope)) = runtime_event_envelope(&state_for_task, item).await {
                    if let Some(event) = runtime_event_sse(&envelope) {
                        if tx.send(Ok(event)).is_err() {
                            return;
                        }
                    }
                    if let Some(run_id) = envelope.run_id.as_deref().and_then(|id| Uuid::parse_str(id).ok()) {
                        if let Ok(projection) = build_event_chain_summary(&state_for_task, run_id).await {
                            if let Some(event) = runtime_projection_sse(&projection) {
                                if tx.send(Ok(event)).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        }

        let sprint_rows = runtime_sprint_event_rows(&state_for_task, &query, query.after_sequence.unwrap_or(0))
            .await
            .unwrap_or_default();
        for row in sprint_rows {
            if let Ok(item) = row_to_sprint_event(row) {
                last_sprint_sequence = last_sprint_sequence.max(item.sequence_no);
                if let Ok(Some(envelope)) = sprint_event_envelope(&state_for_task, &query, item).await {
                    if let Some(event) = sprint_event_sse(&envelope) {
                        if tx.send(Ok(event)).is_err() {
                            return;
                        }
                    }
                }
            }
        }

        if !send_supervisor_snapshot_sse(&state_for_task, &query, &tx).await {
            return;
        }

        loop {
            tokio::select! {
                workflow_message = workflow_live_rx.recv() => match workflow_message {
                    Ok(item) => {
                        if item.sequence_no <= last_sequence {
                            continue;
                        }
                        match runtime_event_allowed(&state_for_task, &query, &item.run_id).await {
                            Ok(true) => {}
                            Ok(false) => continue,
                            Err(_) => continue,
                        }
                        last_sequence = item.sequence_no;
                        if let Ok(Some(envelope)) = runtime_event_envelope(&state_for_task, item).await {
                            if let Some(event) = runtime_event_sse(&envelope) {
                                if tx.send(Ok(event)).is_err() {
                                    return;
                                }
                            }
                            if let Some(run_id) = envelope.run_id.as_deref().and_then(|id| Uuid::parse_str(id).ok()) {
                                if let Ok(projection) = build_event_chain_summary(&state_for_task, run_id).await {
                                    if let Some(event) = runtime_projection_sse(&projection) {
                                        if tx.send(Ok(event)).is_err() {
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                        if let Ok(snapshot) = build_runtime_snapshot(&state_for_task, &query).await {
                            if let Some(event) = runtime_snapshot_sse(&snapshot) {
                                if tx.send(Ok(event)).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        let rows = runtime_event_rows(&state_for_task, &query, last_sequence)
                            .await
                            .unwrap_or_default();
                        let mut sent_any = false;
                        for row in rows {
                            if let Ok(item) = row_to_stage_chain_event(row) {
                                last_sequence = last_sequence.max(item.sequence_no);
                                if let Ok(Some(envelope)) = runtime_event_envelope(&state_for_task, item).await {
                                    if let Some(event) = runtime_event_sse(&envelope) {
                                        if tx.send(Ok(event)).is_err() {
                                            return;
                                        }
                                        sent_any = true;
                                    }
                                    if let Some(run_id) = envelope.run_id.as_deref().and_then(|id| Uuid::parse_str(id).ok()) {
                                        if let Ok(projection) = build_event_chain_summary(&state_for_task, run_id).await {
                                            if let Some(event) = runtime_projection_sse(&projection) {
                                                if tx.send(Ok(event)).is_err() {
                                                    return;
                                                }
                                                sent_any = true;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if sent_any {
                            if let Ok(snapshot) = build_runtime_snapshot(&state_for_task, &query).await {
                                if let Some(event) = runtime_snapshot_sse(&snapshot) {
                                    if tx.send(Ok(event)).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    Err(RecvError::Closed) => {
                        let _ = tx.send(Ok(stream_error_sse("workflow event broadcast channel closed")));
                        return;
                    },
                },
                sprint_message = sprint_live_rx.recv() => match sprint_message {
                    Ok(item) => {
                        if item.sequence_no <= last_sprint_sequence {
                            continue;
                        }
                        last_sprint_sequence = item.sequence_no;
                        if let Ok(Some(envelope)) = sprint_event_envelope(&state_for_task, &query, item).await {
                            if let Some(event) = sprint_event_sse(&envelope) {
                                if tx.send(Ok(event)).is_err() {
                                    return;
                                }
                            }
                            if !send_supervisor_snapshot_sse(&state_for_task, &query, &tx).await {
                                return;
                            }
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        let rows = runtime_sprint_event_rows(&state_for_task, &query, last_sprint_sequence)
                            .await
                            .unwrap_or_default();
                        for row in rows {
                            if let Ok(item) = row_to_sprint_event(row) {
                                last_sprint_sequence = last_sprint_sequence.max(item.sequence_no);
                                if let Ok(Some(envelope)) = sprint_event_envelope(&state_for_task, &query, item).await {
                                    if let Some(event) = sprint_event_sse(&envelope) {
                                        if tx.send(Ok(event)).is_err() {
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                        if !send_supervisor_snapshot_sse(&state_for_task, &query, &tx).await {
                            return;
                        }
                    }
                    Err(RecvError::Closed) => {
                        let _ = tx.send(Ok(stream_error_sse("sprint event broadcast channel closed")));
                        return;
                    },
                },
            }
        }
    });

    Sse::new(UnboundedReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

async fn build_runtime_snapshot(
    state: &AppState,
    query: &RuntimeEventQuery,
) -> Result<RuntimeSnapshotResponse, (axum::http::StatusCode, String)> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let run_ids = runtime_filter_run_ids(state, query).await?;

    let workflow_rows = if run_ids.is_empty() && !runtime_filter_allows_empty_run_set(query) {
        Vec::new()
    } else if run_ids.is_empty() {
        sqlx::query(
            "SELECT id, status, current_step_id, title, repo_ref, workflow_key, context_json, updated_at FROM workflow_runs ORDER BY updated_at DESC LIMIT 500",
        )
        .fetch_all(&state.db)
        .await
        .map_err(internal)?
    } else {
        let mut rows = Vec::new();
        for run_id in &run_ids {
            if let Ok(row) = sqlx::query(
                "SELECT id, status, current_step_id, title, repo_ref, workflow_key, context_json, updated_at FROM workflow_runs WHERE id = ?",
            )
            .bind(run_id)
            .fetch_one(&state.db)
            .await
            {
                rows.push(row);
            }
        }
        rows
    };

    for row in workflow_rows {
        let id: String = row.get("id");
        let context_json: String = row.get("context_json");
        let context = serde_json::from_str::<Value>(&context_json).unwrap_or_else(|_| json!({}));
        nodes.push(RuntimeNode {
            key: workflow_node_key(&id),
            node_type: "workflow_run".to_string(),
            id,
            status: row.get("status"),
            title: row.get("title"),
            repo_ref: row.get("repo_ref"),
            workflow_key: row.get("workflow_key"),
            current_step_id: row.get("current_step_id"),
            updated_at: row.get("updated_at"),
            payload: json!({ "context": context }),
        });
    }

    let supervisor_rows = if let Some(supervisor_run_id) = query.supervisor_run_id {
        sqlx::query(
            "SELECT id, mode, status, title, root_repo_path, child_runs_json, integration_run_id, context_json, updated_at FROM supervisor_runs WHERE id = ?",
        )
        .bind(supervisor_run_id.to_string())
        .fetch_all(&state.db)
        .await
        .map_err(internal)?
    } else if matches!(query.scope.as_deref(), Some("workflow") | Some("workflow_run")) || query.run_id.is_some() {
        Vec::new()
    } else {
        sqlx::query(
            "SELECT id, mode, status, title, root_repo_path, child_runs_json, integration_run_id, context_json, updated_at FROM supervisor_runs ORDER BY updated_at DESC LIMIT 500",
        )
        .fetch_all(&state.db)
        .await
        .map_err(internal)?
    };

    for row in supervisor_rows {
        let id: String = row.get("id");
        let supervisor_key = supervisor_node_key(&id);
        let child_runs_json: String = row.get("child_runs_json");
        let child_runs = serde_json::from_str::<Value>(&child_runs_json).unwrap_or_else(|_| json!([]));
        let context_json: String = row.get("context_json");
        let context = serde_json::from_str::<Value>(&context_json).unwrap_or_else(|_| json!({}));
        nodes.push(RuntimeNode {
            key: supervisor_key.clone(),
            node_type: "supervisor_run".to_string(),
            id: id.clone(),
            status: row.get("status"),
            title: row.get("title"),
            repo_ref: row.get("root_repo_path"),
            workflow_key: None,
            current_step_id: None,
            updated_at: row.get("updated_at"),
            payload: json!({
                "mode": row.get::<String, _>("mode"),
                "context": context
            }),
        });

        if let Some(children) = child_runs.as_array() {
            for (idx, child) in children.iter().enumerate() {
                let Some(child_run_id) = child.get("workflow_run_id").and_then(Value::as_str) else {
                    continue;
                };
                let child_key = workflow_node_key(child_run_id);
                edges.push(RuntimeEdge {
                    key: format!("{}->{}", supervisor_key, child_key),
                    parent_key: supervisor_key.clone(),
                    child_key,
                    edge_type: "supervisor_child_workflow".to_string(),
                    label: child
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Feature workflow")
                        .to_string(),
                    sort_order: idx as i64,
                    payload: child.clone(),
                });
            }
        }

        let integration_run_id: Option<String> = row.get("integration_run_id");
        if let Some(integration_run_id) = integration_run_id {
            let child_key = workflow_node_key(&integration_run_id);
            edges.push(RuntimeEdge {
                key: format!("{}->{}", supervisor_key, child_key),
                parent_key: supervisor_key.clone(),
                child_key,
                edge_type: "supervisor_integration_workflow".to_string(),
                label: "Integration workflow".to_string(),
                sort_order: 10_000,
                payload: json!({}),
            });
        }
    }

    let latest_sequence_no = latest_runtime_sequence(state, &run_ids).await.unwrap_or(0);

    Ok(RuntimeSnapshotResponse {
        nodes,
        edges,
        latest_sequence_no,
        server_time: chrono::Utc::now().to_rfc3339(),
    })
}

async fn runtime_event_rows(
    state: &AppState,
    query: &RuntimeEventQuery,
    after_sequence: i64,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, (axum::http::StatusCode, String)> {
    let run_ids = runtime_filter_run_ids(state, query).await?;
    if run_ids.is_empty() && !runtime_filter_allows_empty_run_set(query) {
        return Ok(Vec::new());
    }

    if run_ids.is_empty() {
        return sqlx::query(
            "SELECT id, run_id, step_id, stage_execution_id, capability_invocation_id, parent_invocation_id, sequence_no, level, kind, message, payload_json, created_at FROM workflow_events WHERE sequence_no > ? ORDER BY created_at ASC, sequence_no ASC LIMIT 1000",
        )
        .bind(after_sequence)
        .fetch_all(&state.db)
        .await
        .map_err(internal);
    }

    let mut rows = Vec::new();
    for run_id in run_ids {
        let mut run_rows = sqlx::query(
            "SELECT id, run_id, step_id, stage_execution_id, capability_invocation_id, parent_invocation_id, sequence_no, level, kind, message, payload_json, created_at FROM workflow_events WHERE run_id = ? AND sequence_no > ? ORDER BY sequence_no ASC LIMIT 1000",
        )
        .bind(run_id)
        .bind(after_sequence)
        .fetch_all(&state.db)
        .await
        .map_err(internal)?;
        rows.append(&mut run_rows);
    }

    rows.sort_by(|a, b| {
        let a_time: String = a.get("created_at");
        let b_time: String = b.get("created_at");
        a_time.cmp(&b_time).then_with(|| {
            let a_seq: i64 = a.get("sequence_no");
            let b_seq: i64 = b.get("sequence_no");
            a_seq.cmp(&b_seq)
        })
    });
    Ok(rows)
}

async fn runtime_filter_run_ids(
    state: &AppState,
    query: &RuntimeEventQuery,
) -> Result<Vec<String>, (axum::http::StatusCode, String)> {
    if let Some(run_id) = query.run_id {
        return Ok(vec![run_id.to_string()]);
    }

    if let Some(supervisor_run_id) = query.supervisor_run_id {
        return supervisor_child_workflow_run_ids(state, supervisor_run_id).await;
    }

    if let Some(workflow_key) = query.workflow_key.as_deref().filter(|value| !value.trim().is_empty()) {
        return sqlx::query_scalar::<_, String>(
            "SELECT id FROM workflow_runs WHERE workflow_key = ? ORDER BY updated_at DESC LIMIT 500",
        )
        .bind(workflow_key)
        .fetch_all(&state.db)
        .await
        .map_err(internal);
    }

    if let Some(repo_ref) = query.repo_ref.as_deref().filter(|value| !value.trim().is_empty()) {
        return sqlx::query_scalar::<_, String>(
            "SELECT id FROM workflow_runs WHERE repo_ref = ? ORDER BY updated_at DESC LIMIT 500",
        )
        .bind(repo_ref)
        .fetch_all(&state.db)
        .await
        .map_err(internal);
    }

    if matches!(query.scope.as_deref(), Some("active")) {
        return sqlx::query_scalar::<_, String>(
            "SELECT id FROM workflow_runs WHERE status IN ('queued', 'running', 'waiting', 'paused') ORDER BY updated_at DESC LIMIT 500",
        )
        .fetch_all(&state.db)
        .await
        .map_err(internal);
    }

    Ok(Vec::new())
}

fn runtime_filter_allows_empty_run_set(query: &RuntimeEventQuery) -> bool {
    query.run_id.is_none()
        && query.supervisor_run_id.is_none()
        && query.workflow_key.as_deref().unwrap_or("").trim().is_empty()
        && query.repo_ref.as_deref().unwrap_or("").trim().is_empty()
        && !matches!(query.scope.as_deref(), Some("active") | Some("workflow") | Some("workflow_run"))
}

async fn supervisor_child_workflow_run_ids(
    state: &AppState,
    supervisor_run_id: Uuid,
) -> Result<Vec<String>, (axum::http::StatusCode, String)> {
    let mut run_ids = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT sf.current_workflow_run_id
         FROM sprint_features sf
         JOIN sprints s ON s.id = sf.sprint_id
         WHERE s.supervisor_run_id = ?
           AND TRIM(COALESCE(sf.current_workflow_run_id, '')) != ''",
    )
    .bind(supervisor_run_id.to_string())
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    if let Some(integration_run_id) = sqlx::query_scalar::<_, Option<String>>(
        "SELECT integration_run_id FROM supervisor_runs WHERE id = ?",
    )
    .bind(supervisor_run_id.to_string())
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .flatten()
    .filter(|value| !value.trim().is_empty())
    {
        run_ids.push(integration_run_id);
    }

    run_ids.sort();
    run_ids.dedup();
    Ok(run_ids)
}

async fn runtime_event_allowed(
    state: &AppState,
    query: &RuntimeEventQuery,
    run_id: &str,
) -> Result<bool, (axum::http::StatusCode, String)> {
    let run_ids = runtime_filter_run_ids(state, query).await?;
    if run_ids.is_empty() {
        return Ok(runtime_filter_allows_empty_run_set(query));
    }
    Ok(run_ids.iter().any(|id| id == run_id))
}

async fn runtime_event_envelope(
    state: &AppState,
    item: StageChainEvent,
) -> Result<Option<RuntimeEventEnvelope>, (axum::http::StatusCode, String)> {
    let row = sqlx::query(
        "SELECT repo_ref, workflow_key FROM workflow_runs WHERE id = ?",
    )
    .bind(&item.run_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?;

    let Some(row) = row else {
        return Ok(None);
    };

    let supervisor_run_id = sqlx::query_scalar::<_, String>(
        "SELECT s.supervisor_run_id
         FROM sprint_features sf
         JOIN sprints s ON s.id = sf.sprint_id
         WHERE sf.current_workflow_run_id = ?
           AND TRIM(COALESCE(s.supervisor_run_id, '')) != ''
         LIMIT 1",
    )
    .bind(&item.run_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .or_else(|| None);

    let supervisor_run_id = if supervisor_run_id.is_some() {
        supervisor_run_id
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT id FROM supervisor_runs WHERE integration_run_id = ? LIMIT 1",
        )
        .bind(&item.run_id)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?
    };

    let scope = if item.capability_invocation_id.is_some() {
        "capability_invocation"
    } else if item.stage_execution_id.is_some() {
        "stage_execution"
    } else {
        "workflow_run"
    };

    Ok(Some(RuntimeEventEnvelope {
        scope: scope.to_string(),
        node_key: workflow_node_key(&item.run_id),
        run_id: Some(item.run_id.clone()),
        supervisor_run_id,
        workflow_key: row.get("workflow_key"),
        repo_ref: row.get("repo_ref"),
        event: item,
    }))
}

async fn latest_runtime_sequence(
    state: &AppState,
    run_ids: &[String],
) -> Result<i64, (axum::http::StatusCode, String)> {
    if run_ids.is_empty() {
        return sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(sequence_no), 0) FROM workflow_events",
        )
        .fetch_one(&state.db)
        .await
        .map_err(internal);
    }

    let mut latest = 0;
    for run_id in run_ids {
        let value = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(sequence_no), 0) FROM workflow_events WHERE run_id = ?",
        )
        .bind(run_id)
        .fetch_one(&state.db)
        .await
        .map_err(internal)?;
        latest = latest.max(value);
    }
    Ok(latest)
}

fn stream_error_sse(message: impl Into<String>) -> Event {
    Event::default()
        .event("stream_error")
        .data(json!({ "message": message.into() }).to_string())
}

fn runtime_snapshot_sse(snapshot: &RuntimeSnapshotResponse) -> Option<Event> {
    serde_json::to_string(snapshot)
        .ok()
        .map(|text| Event::default().event("runtime_snapshot").retry(std::time::Duration::from_secs(5)).data(text))
}

fn runtime_event_sse(envelope: &RuntimeEventEnvelope) -> Option<Event> {
    serde_json::to_string(envelope)
        .ok()
        .map(|text| Event::default().event("runtime_event").retry(std::time::Duration::from_secs(5)).data(text))
}

fn runtime_projection_sse(projection: &EventChainSummaryResponse) -> Option<Event> {
    serde_json::to_string(projection)
        .ok()
        .map(|text| Event::default().event("runtime_projection").retry(std::time::Duration::from_secs(5)).data(text))
}

fn workflow_node_key(id: &str) -> String {
    format!("workflow_run:{}", id)
}

fn supervisor_node_key(id: &str) -> String {
    format!("supervisor_run:{}", id)
}

fn workflow_event_sse(item: &StageChainEvent) -> Option<Event> {
    serde_json::to_string(item)
        .ok()
        .map(|text| Event::default().event("workflow_event").data(text))
}

fn monitor_snapshot_sse(summary: &EventChainSummaryResponse) -> Option<Event> {
    serde_json::to_string(summary)
        .ok()
        .map(|text| Event::default().event("monitor_snapshot").data(text))
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
