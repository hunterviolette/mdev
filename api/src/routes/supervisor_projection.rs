use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    routing::get,
    Json, Router,
};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use std::{collections::HashSet, convert::Infallible, path::Path, time::Duration};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::{
    app_state::AppState,
    supervisor::{
        models::IntegrationInputState,
        patches,
    },
};

#[derive(Debug, Clone, Deserialize)]
struct SupervisorProjectionQuery {
    supervisor_id: Option<String>,
    supervisor_ids: Option<String>,
    root_repo_path: Option<String>,
    state: Option<String>,
    kind: Option<String>,
    include_deleted: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Default)]
struct SupervisorProjectionTotals {
    supervisors: usize,
    work_units: usize,
    running: usize,
    waiting_user: usize,
    failed: usize,
    ready_for_integration: usize,
    integrating: usize,
}

#[derive(Debug, Clone, Serialize)]
struct SupervisorProjectionResponse {
    supervisors: Vec<SupervisorProjection>,
    alerts: Vec<SupervisorAlert>,
    totals: SupervisorProjectionTotals,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SupervisorProjection {
    id: String,
    mode: String,
    status: String,
    title: String,
    root_repo_path: String,
    selected_planner_id: Option<String>,
    snapshot_path: Option<String>,
    integration_path: Option<String>,
    integration_run_id: Option<String>,
    topology: Vec<SupervisorTopologyNode>,
    work_units: Vec<SupervisorWorkUnitProjection>,
    alerts: Vec<SupervisorAlert>,
    integration: Value,
    context: Value,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
struct SupervisorTopologyNode {
    id: String,
    parent_id: Option<String>,
    kind: String,
    label: String,
    state: String,
    work_unit_id: Option<String>,
    workflow_run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SupervisorWorkUnitProjection {
    id: String,
    supervisor_id: String,
    repo_id: Option<String>,
    feature_id: Option<String>,
    workflow_run_id: Option<String>,
    patch_id: Option<String>,
    kind: String,
    workflow_type: String,
    title: String,
    state: String,
    root_repo_path: String,
    workspace_path: Option<String>,
    integration_path: Option<String>,
    has_staged_changes: bool,
    has_workspace_changes: bool,
    integration_state: IntegrationInputState,
    integration_apply_available: bool,
    applied_at: Option<String>,
    telemetry: Value,
    alerts: Vec<SupervisorAlert>,
    created_at: Option<String>,
    updated_at: Option<String>,
    workflow_deleted: bool,
}

#[derive(Debug, Clone, Serialize)]
struct SupervisorAlert {
    id: String,
    supervisor_id: String,
    work_unit_id: Option<String>,
    workflow_run_id: Option<String>,
    level: String,
    kind: String,
    message: String,
    created_at: Option<String>,
}

#[derive(Debug, Clone)]
struct SupervisorRow {
    id: String,
    mode: String,
    status: String,
    title: String,
    root_repo_path: String,
    selected_planner_id: Option<String>,
    snapshot_path: Option<String>,
    integration_path: Option<String>,
    integration_run_id: Option<String>,
    merge_report: Value,
    validation_report: Value,
    context: Value,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone)]
struct WorkUnitSeed {
    id: String,
    supervisor_id: String,
    repo_id: Option<String>,
    feature_id: Option<String>,
    workflow_run_id: Option<String>,
    patch_id: Option<String>,
    kind: String,
    workflow_type: String,
    title: String,
    state: String,
    workflow_status: Option<String>,
    root_repo_path: String,
    workspace_path: Option<String>,
    integration_path: Option<String>,
    integration_state: IntegrationInputState,
    context: Value,
    created_at: Option<String>,
    updated_at: Option<String>,
    workflow_deleted: bool,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/supervisors/projection", get(get_supervisor_projection))
        .route("/api/supervisors/projection/stream", get(stream_supervisor_projection))
}

async fn get_supervisor_projection(
    State(state): State<AppState>,
    Query(query): Query<SupervisorProjectionQuery>,
) -> Result<Json<SupervisorProjectionResponse>, (StatusCode, String)> {
    build_supervisor_projection_response(&state, query).await.map(Json).map_err(internal)
}

async fn send_supervisor_stream_event(
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    event_name: &'static str,
    payload: Value,
) -> bool {
    tx.send(Ok(Event::default().event(event_name).data(payload.to_string())))
        .await
        .is_ok()
}

async fn produce_supervisor_projection_stream(
    state: AppState,
    query: SupervisorProjectionQuery,
    tx: mpsc::Sender<Result<Event, Infallible>>,
) -> anyhow::Result<()> {
    let supervisors = load_supervisors(&state, &query).await?;
    let include_deleted = query.include_deleted.unwrap_or(false);

    if !send_supervisor_stream_event(
        &tx,
        "supervisor_projection_begin",
        json!({ "supervisor_count": supervisors.len() }),
    )
    .await
    {
        return Ok(());
    }

    for supervisor in supervisors {
        let mut seeds = load_work_unit_seeds(&state, &supervisor, include_deleted).await?;
        filter_work_unit_seeds(&mut seeds, &query);

        let placeholders = seeds
            .iter()
            .map(|seed| placeholder_work_unit(&supervisor, seed))
            .collect::<Vec<_>>();
        let shell = build_supervisor_projection(supervisor.clone(), placeholders);

        if !send_supervisor_stream_event(
            &tx,
            "supervisor_projection",
            serde_json::to_value(&shell)?,
        )
        .await
        {
            return Ok(());
        }

        let work_unit_count = seeds.len();
        let mut hydrated = vec![None; work_unit_count];
        let hydration_state = state.clone();
        let hydration_supervisor = supervisor.clone();
        let builds = stream::iter(seeds.into_iter().enumerate().map(move |(index, seed)| {
            let state = hydration_state.clone();
            let supervisor = hydration_supervisor.clone();
            async move {
                let unit = build_work_unit_projection(&state, &supervisor, seed).await?;
                Ok::<_, anyhow::Error>((index, unit))
            }
        }))
        .buffer_unordered(6);

        tokio::pin!(builds);

        while let Some(result) = builds.next().await {
            let (index, unit) = result?;
            hydrated[index] = Some(unit.clone());

            if !send_supervisor_stream_event(
                &tx,
                "supervisor_work_unit",
                json!({
                    "supervisor_id": supervisor.id,
                    "index": index,
                    "work_unit": unit,
                }),
            )
            .await
            {
                return Ok(());
            }
        }

        let work_units = hydrated.into_iter().flatten().collect::<Vec<_>>();
        let complete = build_supervisor_projection(supervisor, work_units);

        if !send_supervisor_stream_event(
            &tx,
            "supervisor_projection_complete",
            serde_json::to_value(&complete)?,
        )
        .await
        {
            return Ok(());
        }
    }

    send_supervisor_stream_event(&tx, "supervisor_hydration_complete", json!({ "ok": true })).await;
    Ok(())
}

async fn stream_supervisor_projection(
    State(state): State<AppState>,
    Query(query): Query<SupervisorProjectionQuery>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = mpsc::channel(32);

    tokio::spawn(async move {
        let stream_tx = tx.clone();
        if let Err(error) = produce_supervisor_projection_stream(state, query, stream_tx).await {
            let _ = send_supervisor_stream_event(
                &tx,
                "supervisor_projection_error",
                json!({ "message": error.to_string() }),
            )
            .await;
        }
    });

    Sse::new(ReceiverStream::new(rx)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("supervisor-projection"),
    )
}

pub(crate) async fn build_supervisor_projection_by_id(
    state: &AppState,
    supervisor_run_id: &str,
) -> anyhow::Result<Option<SupervisorProjection>> {
    let response = build_supervisor_projection_response(
        state,
        SupervisorProjectionQuery {
            supervisor_id: Some(supervisor_run_id.to_string()),
            supervisor_ids: None,
            root_repo_path: None,
            state: None,
            kind: None,
            include_deleted: Some(false),
        },
    )
    .await?;

    Ok(response.supervisors.into_iter().next())
}

fn filter_work_unit_seeds(seeds: &mut Vec<WorkUnitSeed>, query: &SupervisorProjectionQuery) {
    if let Some(kind) = query.kind.as_deref().filter(|value| !value.trim().is_empty()) {
        seeds.retain(|unit| unit.kind == kind);
    }
    if let Some(state_filter) = query.state.as_deref().filter(|value| !value.trim().is_empty()) {
        seeds.retain(|unit| unit.state == state_filter);
    }
}

fn work_unit_alerts(supervisor: &SupervisorRow, seed: &WorkUnitSeed) -> Vec<SupervisorAlert> {
    let mut alerts = Vec::new();

    if seed.state == "waiting" {
        alerts.push(SupervisorAlert {
            id: format!("waiting-user-{}", seed.id),
            supervisor_id: supervisor.id.clone(),
            work_unit_id: Some(seed.id.clone()),
            workflow_run_id: seed.workflow_run_id.clone(),
            level: "warning".to_string(),
            kind: "waiting_user".to_string(),
            message: format!("{} is waiting for user input", seed.title),
            created_at: seed.updated_at.clone(),
        });
    }

    if seed.state == "error" || seed.state == "failed" || seed.state == "blocked" {
        alerts.push(SupervisorAlert {
            id: format!("blocked-{}", seed.id),
            supervisor_id: supervisor.id.clone(),
            work_unit_id: Some(seed.id.clone()),
            workflow_run_id: seed.workflow_run_id.clone(),
            level: "error".to_string(),
            kind: seed.state.clone(),
            message: format!("{} is {}", seed.title, seed.state),
            created_at: seed.updated_at.clone(),
        });
    }

    if seed.workflow_deleted {
        alerts.push(SupervisorAlert {
            id: format!("deleted-workflow-{}", seed.id),
            supervisor_id: supervisor.id.clone(),
            work_unit_id: Some(seed.id.clone()),
            workflow_run_id: seed.workflow_run_id.clone(),
            level: "warning".to_string(),
            kind: "deleted_workflow".to_string(),
            message: format!("{} references a deleted workflow", seed.title),
            created_at: seed.updated_at.clone(),
        });
    }

    alerts
}

fn placeholder_work_unit(supervisor: &SupervisorRow, seed: &WorkUnitSeed) -> SupervisorWorkUnitProjection {
    let applied_at = seed
        .context
        .get("applied_at")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let integration_apply_available = seed
        .context
        .get("integration_apply_available")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let integration_apply_available = seed
        .context
        .get("integration_apply_available")
        .and_then(Value::as_bool)
        .unwrap_or(false);


    SupervisorWorkUnitProjection {
        id: seed.id.clone(),
        supervisor_id: seed.supervisor_id.clone(),
        repo_id: seed.repo_id.clone(),
        feature_id: seed.feature_id.clone(),
        workflow_run_id: seed.workflow_run_id.clone(),
        patch_id: seed.patch_id.clone(),
        kind: seed.kind.clone(),
        workflow_type: seed.workflow_type.clone(),
        title: seed.title.clone(),
        state: seed.state.clone(),
        root_repo_path: seed.root_repo_path.clone(),
        workspace_path: seed.workspace_path.clone(),
        integration_path: seed.integration_path.clone(),
        has_staged_changes: false,
        has_workspace_changes: false,
        integration_state: seed.integration_state.clone(),
        integration_apply_available,
        applied_at,
        telemetry: json!({ "hydrating": true }),
        alerts: work_unit_alerts(supervisor, seed),
        created_at: seed.created_at.clone(),
        updated_at: seed.updated_at.clone(),
        workflow_deleted: seed.workflow_deleted,
    }
}

async fn build_work_unit_projection(
    state: &AppState,
    supervisor: &SupervisorRow,
    seed: WorkUnitSeed,
) -> anyhow::Result<SupervisorWorkUnitProjection> {
    let execution_event_limit = supervisor
        .context
        .get("execution_event_limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(10, 1000) as usize;

    let telemetry = match seed.workflow_run_id.as_deref() {
        Some(workflow_run_id) if !seed.workflow_deleted => {
            workflow_telemetry(state, workflow_run_id, execution_event_limit).await?
        }
        _ => draft_workflow_telemetry(state, supervisor, &seed.context).await?,
    };

    let change_status = seed
        .workspace_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .and_then(|path| patches::change_status(Path::new(path)).ok());

    let mut unit = placeholder_work_unit(supervisor, &seed);
    unit.has_staged_changes = change_status
        .as_ref()
        .is_some_and(|status| status.has_staged_changes());
    unit.has_workspace_changes = change_status
        .as_ref()
        .is_some_and(|status| status.has_changes());
    unit.telemetry = telemetry;
    Ok(unit)
}

fn build_supervisor_projection(
    supervisor: SupervisorRow,
    work_units: Vec<SupervisorWorkUnitProjection>,
) -> SupervisorProjection {
    let supervisor_alerts = work_units
        .iter()
        .flat_map(|unit| unit.alerts.iter().cloned())
        .collect::<Vec<_>>();
    let topology = build_topology(&supervisor, &work_units);
    let pending_patch_count = work_units
        .iter()
        .filter(|unit| {
            unit.patch_id.is_some()
                && (unit.state == "patch_ready" || unit.state == "ready_for_integration")
        })
        .count();
    let integration_workflow_run_id = work_units
        .iter()
        .find(|unit| unit.kind == "integration")
        .and_then(|unit| unit.workflow_run_id.clone());
    let integration = json!({
        "state": integration_state(&supervisor.status),
        "workflow_run_id": integration_workflow_run_id,
        "pending_patch_count": pending_patch_count,
        "merge_summary": null,
        "validation_summary": null
    });

    SupervisorProjection {
        id: supervisor.id,
        mode: supervisor.mode,
        status: supervisor.status,
        title: supervisor.title,
        root_repo_path: supervisor.root_repo_path,
        selected_planner_id: supervisor.selected_planner_id,
        snapshot_path: supervisor.snapshot_path,
        integration_path: supervisor.integration_path,
        integration_run_id: supervisor.integration_run_id,
        topology,
        work_units,
        alerts: supervisor_alerts,
        integration,
        context: supervisor.context,
        created_at: supervisor.created_at,
        updated_at: supervisor.updated_at,
    }
}

async fn build_supervisor_projection_response(state: &AppState, query: SupervisorProjectionQuery) -> anyhow::Result<SupervisorProjectionResponse> {
    let supervisors = load_supervisors(state, &query).await?;
    let mut response_supervisors = Vec::new();
    let include_deleted = query.include_deleted.unwrap_or(false);

    for supervisor in supervisors {
        let mut seeds = load_work_unit_seeds(state, &supervisor, include_deleted).await?;
        filter_work_unit_seeds(&mut seeds, &query);

        let mut work_units = Vec::with_capacity(seeds.len());
        for seed in seeds {
            work_units.push(build_work_unit_projection(state, &supervisor, seed).await?);
        }

        response_supervisors.push(build_supervisor_projection(supervisor, work_units));
    }

    let response_alerts = response_supervisors
        .iter()
        .flat_map(|supervisor| supervisor.alerts.iter().cloned())
        .collect::<Vec<_>>();
    let mut totals = SupervisorProjectionTotals::default();
    totals.supervisors = response_supervisors.len();

    for supervisor in &response_supervisors {
        totals.work_units += supervisor.work_units.len();
        for unit in &supervisor.work_units {
            match unit.state.as_str() {
                "running" => totals.running += 1,
                "waiting" => totals.waiting_user += 1,
                "failed" | "error" => totals.failed += 1,
                "ready_for_integration" => totals.ready_for_integration += 1,
                "integrating" => totals.integrating += 1,
                _ => {}
            }
        }
    }

    Ok(SupervisorProjectionResponse {
        supervisors: response_supervisors,
        alerts: response_alerts,
        totals,
    })
}

async fn load_supervisors(state: &AppState, query: &SupervisorProjectionQuery) -> anyhow::Result<Vec<SupervisorRow>> {
    let supervisor_ids = query
        .supervisor_ids
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        r#"
        SELECT id, mode, status, title, root_repo_path, selected_planner_id, context_json, created_at, updated_at
        FROM supervisor_runs
        ORDER BY updated_at DESC
        "#,
    )
    .fetch_all(&state.db)
    .await?;

    let mut supervisors = Vec::new();
    for row in rows {
        let id: String = row.get("id");
        let root_repo_path: String = row.get("root_repo_path");
        if query.supervisor_id.as_deref().is_some_and(|filter| !filter.is_empty() && filter != id) {
            continue;
        }
        if !supervisor_ids.is_empty() && !supervisor_ids.iter().any(|filter| *filter == id) {
            continue;
        }
        if query.root_repo_path.as_deref().is_some_and(|filter| !filter.is_empty() && filter != root_repo_path) {
            continue;
        }
        supervisors.push(SupervisorRow {
            id,
            mode: row.get("mode"),
            status: row.get("status"),
            title: row.get("title"),
            root_repo_path,
            selected_planner_id: row.get("selected_planner_id"),
            snapshot_path: None,
            integration_path: None,
            integration_run_id: None,
            merge_report: json!({}),
            validation_report: json!({}),
            context: parse_json(row.get::<String, _>("context_json")),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        });
    }
    Ok(supervisors)
}

async fn sync_supervisor_work_units(state: &AppState, supervisor: &SupervisorRow) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        DELETE FROM supervisor_work_units
        WHERE supervisor_run_id = ?
          AND kind = 'feature_development'
        "#,
    )
    .bind(supervisor.id.as_str())
    .execute(&state.db)
    .await?;

    sqlx::query(
        r#"
        WITH queued_ids AS (
            SELECT DISTINCT value AS feature_id, CAST(key AS INTEGER) AS queue_position
            FROM json_each((SELECT context_json FROM supervisor_runs WHERE id = ?), '$.queued_feature_ids')
            WHERE type = 'text'
            UNION
            SELECT DISTINCT value AS feature_id, CAST(key AS INTEGER) AS queue_position
            FROM json_each((SELECT context_json FROM supervisor_runs WHERE id = ?), '$.feature_pool_ids')
            WHERE type = 'text'
        )
        INSERT INTO supervisor_work_units (
            id,
            supervisor_run_id,
            repo_id,
            feature_id,
            workflow_run_id,
            patch_id,
            kind,
            title,
            state,
            root_repo_path,
            shard_path,
            integration_path,
            priority,
            queue_position,
            blocked_reason,
            waiting_user_input_json,
            context_json,
            created_at,
            updated_at
        )
        SELECT
            sr.id || ':' || pf.id,
            sr.id,
            pf.repo_id,
            pf.id,
            pf.current_workflow_run_id,
            pf.current_patch_id,
            'feature_development',
            pf.title,
            CASE
                WHEN pf.development_state IN ('development_running', 'running', 'active') THEN 'running'
                WHEN pf.development_state IN ('waiting', 'waiting_user', 'paused') THEN 'waiting_user'
                WHEN pf.development_state IN ('development_failed', 'failed', 'blocked') THEN 'failed'
                WHEN pf.development_state IN ('development_succeeded', 'completed') THEN 'ready_for_integration'
                WHEN pf.development_state IN ('integrating', 'integration_running') THEN 'integrating'
                WHEN pf.development_state IN ('integrated', 'applied') THEN 'integrated'
                WHEN pf.development_state = 'patch_ready' THEN 'patch_ready'
                ELSE 'queued'
            END,
            sr.root_repo_path,
            NULL,
            sr.integration_path,
            0,
            queued_ids.queue_position,
            NULL,
            '{}',
            json_object(
                'source', 'supervisor_projection_sync',
                'queue_source', 'supervisor_execution_queue',
                'workflow_type', 'feature_development',
                'pool_key', 'feature_development',
                'planned_workflow', 1,
                'planned_workflow_template_id', json_extract(sr.context_json, '$.pools.feature_development.template_id'),
                'development_state', pf.development_state,
                'feature_status', pf.status
            ),
            COALESCE(pf.scheduled_at, pf.updated_at, sr.created_at),
            sr.updated_at
        FROM queued_ids
        JOIN supervisor_runs sr ON sr.id = ?
        JOIN planner_features pf ON pf.id = queued_ids.feature_id
        WHERE pf.id NOT LIKE 'manual-%'
          AND COALESCE(pf.status, '') != 'deleted'
        ON CONFLICT(id) DO UPDATE SET
            repo_id = excluded.repo_id,
            workflow_run_id = excluded.workflow_run_id,
            patch_id = excluded.patch_id,
            title = excluded.title,
            state = excluded.state,
            root_repo_path = excluded.root_repo_path,
            shard_path = excluded.shard_path,
            integration_path = excluded.integration_path,
            queue_position = excluded.queue_position,
            blocked_reason = excluded.blocked_reason,
            context_json = excluded.context_json,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(supervisor.id.as_str())
    .bind(supervisor.id.as_str())
    .bind(supervisor.id.as_str())
    .execute(&state.db)
    .await?;

    Ok(())
}

async fn load_work_unit_seeds(state: &AppState, supervisor: &SupervisorRow, include_deleted: bool) -> anyhow::Result<Vec<WorkUnitSeed>> {
    let rows = sqlx::query(
        r#"
        SELECT wu.id, wu.supervisor_run_id, wu.repo_id, wu.feature_id, wu.workflow_run_id, wu.patch_id,
               wu.kind, COALESCE(json_extract(wu.context_json, '$.workflow_type'), wu.kind) AS workflow_type,
               wu.title, wu.state, wr.status AS workflow_status, wu.root_repo_path, wu.workspace_path, wu.integration_path,
               wu.integration_state, wu.context_json, wu.created_at, wu.updated_at,
               CASE WHEN wu.workflow_run_id IS NULL OR wr.id IS NOT NULL THEN 1 ELSE 0 END AS workflow_exists
        FROM supervisor_work_units wu
        LEFT JOIN workflow_runs wr ON wr.id = wu.workflow_run_id
        WHERE wu.supervisor_run_id = ?
        ORDER BY wu.priority DESC, wu.queue_position ASC, wu.created_at ASC, wu.id ASC
        "#,
    )
    .bind(supervisor.id.as_str())
    .fetch_all(&state.db)
    .await?;

    let mut seeds = Vec::new();
    for row in rows {
        let workflow_run_id: Option<String> = row.get("workflow_run_id");
        let workflow_status = row
            .try_get::<Option<String>, _>("workflow_status")
            .ok()
            .flatten()
            .map(|status| match status.as_str() {
                "success" => "complete".to_string(),
                _ => status,
            });
        let kind_text: String = row.get("kind");
        let integration_state_text: String = row.get("integration_state");
        let integration_state = IntegrationInputState::try_from(integration_state_text.as_str()).map_err(anyhow::Error::msg)?;
        let workflow_exists = row.get::<i64, _>("workflow_exists") == 1;
        let persisted_state: String = row.get("state");
        let has_workflow_reference = workflow_run_id
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());

        let state_text = if !has_workflow_reference {
            persisted_state
        } else if !workflow_exists {
            "deleted".to_string()
        } else {
            workflow_status
                .clone()
                .ok_or_else(|| anyhow::anyhow!("workflow {} exists without a status", workflow_run_id.as_deref().unwrap_or_default()))?
        };
        if workflow_run_id.is_some() && !workflow_exists && !include_deleted {
            continue;
        }
        if !include_deleted && matches!(state_text.as_str(), "archived" | "applied" | "deleted" | "removed" | "skipped") {
            continue;
        }
        seeds.push(WorkUnitSeed {
            id: row.get("id"),
            supervisor_id: row.get("supervisor_run_id"),
            repo_id: row.get("repo_id"),
            feature_id: row.get("feature_id"),
            workflow_run_id,
            patch_id: row.get("patch_id"),
            kind: kind_text,
            workflow_type: row.get("workflow_type"),
            title: row.get("title"),
            state: state_text,
            workflow_status,
            root_repo_path: row.get("root_repo_path"),
            workspace_path: row.get("workspace_path"),
            integration_path: row.get("integration_path"),
            integration_state,
            context: parse_json(row.get::<String, _>("context_json")),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
            workflow_deleted: !workflow_exists,
        });
    }

    Ok(seeds)
}

async fn workflow_exists(state: &AppState, workflow_run_id: &str) -> anyhow::Result<bool> {
    let exists = sqlx::query("SELECT 1 FROM workflow_runs WHERE id = ? LIMIT 1")
        .bind(workflow_run_id)
        .fetch_optional(&state.db)
        .await?
        .is_some();
    Ok(exists)
}

fn workflow_stage_key(step: &Value, index: usize) -> String {
    let raw = step
        .get("step_type")
        .and_then(Value::as_str)
        .or_else(|| step.get("id").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    let mut key = raw
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    while key.contains("__") {
        key = key.replace("__", "_");
    }
    let key = key.trim_matches('_').to_string();
    if key.is_empty() { format!("stage_{}", index + 1) } else { key }
}

fn workflow_stage_template(definition_json: Option<&str>) -> (Vec<Value>, Option<String>) {
    let Some(definition_json) = definition_json.filter(|value| !value.trim().is_empty()) else {
        return (Vec::new(), Some("workflow run is missing definition_json; Supervisor cannot render template stages".to_string()));
    };

    let definition = match serde_json::from_str::<Value>(definition_json) {
        Ok(value) => value,
        Err(err) => return (Vec::new(), Some(format!("workflow definition_json is invalid: {:#}", err))),
    };

    let Some(steps) = definition.get("steps").and_then(Value::as_array) else {
        return (Vec::new(), Some("workflow definition_json is missing steps[]; Supervisor cannot render template stages".to_string()));
    };

    if steps.is_empty() {
        return (Vec::new(), Some("workflow template has no steps; Supervisor cannot render template stages".to_string()));
    }

    let stages = steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let key = workflow_stage_key(step, index);
            json!({
                "key": key,
                "step_id": step.get("id").cloned().unwrap_or(Value::Null),
                "step_type": step.get("step_type").cloned().unwrap_or(Value::Null),
                "label": step.get("name").and_then(Value::as_str).filter(|value| !value.trim().is_empty()).unwrap_or_else(|| key.as_str()),
                "state": "future"
            })
        })
        .collect::<Vec<_>>();

    (stages, None)
}

fn execution_duration_ms(started_at: Option<&str>, finished_at: Option<&str>) -> Option<i64> {
    let started_at = started_at
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())?;
    let finished_at = finished_at
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())?;

    Some(
        finished_at
            .signed_duration_since(started_at)
            .num_milliseconds()
            .max(0),
    )
}

async fn workflow_telemetry(
    state: &AppState,
    workflow_run_id: &str,
    execution_event_limit: usize,
) -> anyhow::Result<Value> {
    let run_row = sqlx::query("SELECT status, current_step_id, definition_json FROM workflow_runs WHERE id = ?")
        .bind(workflow_run_id)
        .fetch_optional(&state.db)
        .await?;

    let status = run_row.as_ref().map(|row| row.get::<String, _>("status"));
    let current_step_id = run_row.as_ref().and_then(|row| row.get::<Option<String>, _>("current_step_id"));
    let definition_json = run_row
        .as_ref()
        .and_then(|row| row.try_get::<String, _>("definition_json").ok())
        .filter(|value| !value.trim().is_empty());
    let (stage_template, stage_template_error) = workflow_stage_template(definition_json.as_deref());

    let rows = sqlx::query(
        r#"
        SELECT
            step_id,
            stage_execution_id,
            capability_invocation_id,
            parent_invocation_id,
            is_header_event,
            level,
            kind,
            message,
            payload_json,
            created_at,
            sequence_no,
            MIN(created_at) OVER (
                PARTITION BY stage_execution_id
            ) AS stage_started_at,
            MAX(created_at) OVER (
                PARTITION BY stage_execution_id
            ) AS stage_finished_at,
            MIN(created_at) OVER (
                PARTITION BY capability_invocation_id
            ) AS capability_started_at,
            MAX(created_at) OVER (
                PARTITION BY capability_invocation_id
            ) AS capability_finished_at
        FROM workflow_events
        WHERE run_id = ?
        ORDER BY sequence_no DESC
        LIMIT ?
        "#,
    )
    .bind(workflow_run_id)
    .bind(execution_event_limit.max(10).min(1000) as i64)
    .fetch_all(&state.db)
    .await?;

    let mut seen_stages = HashSet::new();
    let mut seen_capabilities = HashSet::new();
    let mut recent_stage_executions = Vec::new();
    let mut stage_execution_fallbacks = Vec::<(String, Value)>::new();
    let mut recent_capability_executions = Vec::new();

    for row in rows {
        let step_id: Option<String> = row.get("step_id");
        let stage_execution_id: Option<String> = row.get("stage_execution_id");
        let capability_invocation_id: Option<String> = row.get("capability_invocation_id");
        let parent_invocation_id: Option<String> = row.get("parent_invocation_id");
        let is_header_event: i64 = row.get("is_header_event");
        let level: String = row.get("level");
        let kind: String = row.get("kind");
        let message: String = row.get("message");
        let payload = parse_json(row.get::<String, _>("payload_json"));
        let created_at: String = row.get("created_at");
        let stage_started_at: Option<String> = row.try_get("stage_started_at").ok().flatten();
        let stage_finished_at: Option<String> = row.try_get("stage_finished_at").ok().flatten();
        let capability_started_at: Option<String> = row.try_get("capability_started_at").ok().flatten();
        let capability_finished_at: Option<String> = row.try_get("capability_finished_at").ok().flatten();
        let stage_duration_ms = execution_duration_ms(
            stage_started_at.as_deref(),
            stage_finished_at.as_deref(),
        );
        let capability_duration_ms = payload
            .get("duration_ms")
            .or_else(|| payload.get("elapsed_ms"))
            .and_then(Value::as_i64)
            .or_else(|| {
                execution_duration_ms(
                    capability_started_at.as_deref(),
                    capability_finished_at.as_deref(),
                )
            });

        if let Some(stage_id) = stage_execution_id.clone() {
            let stage_projection = json!({
                "stage_execution_id": stage_id,
                "step_id": step_id,
                "status": event_state(&level, &kind, &message, &payload),
                "level": level,
                "kind": kind,
                "message": message,
                "created_at": created_at,
                "started_at": stage_started_at,
                "finished_at": stage_finished_at,
                "duration_ms": stage_duration_ms
            });

            if is_header_event != 0 {
                if recent_stage_executions.len() < execution_event_limit
                    && seen_stages.insert(stage_id.clone())
                {
                    recent_stage_executions.push(stage_projection);
                }
            } else if !stage_execution_fallbacks
                .iter()
                .any(|(fallback_stage_id, _)| fallback_stage_id == &stage_id)
            {
                stage_execution_fallbacks.push((stage_id, stage_projection));
            }
        }

        if recent_capability_executions.len() < execution_event_limit {
            if let Some(capability_id) = capability_invocation_id.clone() {
                if seen_capabilities.insert(capability_id.clone()) {
                    recent_capability_executions.push(json!({
                        "capability_invocation_id": capability_id,
                        "parent_invocation_id": parent_invocation_id,
                        "stage_execution_id": stage_execution_id,
                        "step_id": step_id,
                        "capability": payload.get("capability").and_then(Value::as_str).unwrap_or("capability"),
                        "status": event_state(&level, &kind, &message, &payload),
                        "level": level,
                        "kind": kind,
                        "message": message,
                        "created_at": created_at,
                        "started_at": capability_started_at,
                        "finished_at": capability_finished_at,
                        "duration_ms": capability_duration_ms
                    }));
                }
            }
        }
    }

    for (stage_id, fallback) in stage_execution_fallbacks {
        if recent_stage_executions.len() >= execution_event_limit {
            break;
        }

        if seen_stages.insert(stage_id) {
            recent_stage_executions.push(fallback);
        }
    }

    Ok(json!({
        "status": status,
        "current_step_id": current_step_id,
        "stage_template": stage_template,
        "stage_template_error": stage_template_error,
        "recent_stage_executions": recent_stage_executions,
        "recent_capability_executions": recent_capability_executions,
        "execution_event_limit": execution_event_limit
    }))
}

fn context_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn supervisor_pool_template_id(supervisor: &SupervisorRow, pool_key: &str) -> Option<String> {
    supervisor
        .context
        .get("pools")
        .and_then(|value| value.get(pool_key))
        .and_then(|value| context_string(value, "template_id"))
}

async fn draft_workflow_telemetry(
    state: &AppState,
    supervisor: &SupervisorRow,
    context: &Value,
) -> anyhow::Result<Value> {
    let pool_key = context_string(context, "pool_key").unwrap_or_else(|| "workflow".to_string());
    let template_id = context_string(context, "planned_workflow_template_id")
        .or_else(|| context_string(context, "template_id"))
        .or_else(|| context_string(context, "workflow_template_id"))
        .or_else(|| supervisor_pool_template_id(supervisor, &pool_key));
    let materialization_state = context_string(context, "materialization_state")
        .unwrap_or_else(|| "pending".to_string());
    let regenerated_at = context_string(context, "regenerated_at");
    let materialization_started_at = context_string(context, "materialization_started_at");
    let status = match materialization_state.as_str() {
        "materializing" => "running",
        "failed" => "failed",
        _ => "queued",
    };

    let Some(template_id) = template_id else {
        return Ok(json!({
            "status": status,
            "pool_key": pool_key,
            "planned_workflow": true,
            "planned_workflow_template_id": Value::Null,
            "materialization_state": materialization_state,
            "materialization_started_at": materialization_started_at,
            "regenerated_at": regenerated_at,
            "current_step_id": Value::Null,
            "stage_template": [],
            "stage_template_error": "No workflow template is configured for this supervisor pool.",
            "recent_stage_executions": [],
            "recent_capability_executions": []
        }));
    };

    let row = sqlx::query("SELECT name, definition_json FROM workflow_templates WHERE id = ? OR name = ? LIMIT 1")
        .bind(template_id.as_str())
        .bind(template_id.as_str())
        .fetch_optional(&state.db)
        .await?;

    let Some(row) = row else {
        return Ok(json!({
            "status": status,
            "pool_key": pool_key,
            "planned_workflow": true,
            "planned_workflow_template_id": template_id,
            "materialization_state": materialization_state,
            "materialization_started_at": materialization_started_at,
            "regenerated_at": regenerated_at,
            "current_step_id": Value::Null,
            "stage_template": [],
            "stage_template_error": "Configured workflow template was not found.",
            "recent_stage_executions": [],
            "recent_capability_executions": []
        }));
    };

    let template_name: String = row.get("name");
    let definition_json: String = row.get("definition_json");
    let (stage_template, stage_template_error) = workflow_stage_template(Some(definition_json.as_str()));

    Ok(json!({
        "status": status,
        "pool_key": pool_key,
        "planned_workflow": true,
        "planned_workflow_template_id": template_id,
        "planned_workflow_template_name": template_name,
        "materialization_state": materialization_state,
        "materialization_started_at": materialization_started_at,
        "regenerated_at": regenerated_at,
        "current_step_id": Value::Null,
        "stage_template": stage_template,
        "stage_template_error": stage_template_error,
        "recent_stage_executions": [],
        "recent_capability_executions": []
    }))
}

fn empty_telemetry() -> Value {
    json!({
        "status": null,
        "current_step_id": null,
        "recent_stage_executions": [],
        "recent_capability_executions": []
    })
}

fn build_topology(supervisor: &SupervisorRow, work_units: &[SupervisorWorkUnitProjection]) -> Vec<SupervisorTopologyNode> {
    let root_id = format!("root-{}", supervisor.id);
    let supervisor_id = format!("supervisor-{}", supervisor.id);
    let mut nodes = vec![
        SupervisorTopologyNode {
            id: root_id.clone(),
            parent_id: None,
            kind: "root".to_string(),
            label: supervisor.root_repo_path.clone(),
            state: "root".to_string(),
            work_unit_id: None,
            workflow_run_id: None,
        },
        SupervisorTopologyNode {
            id: supervisor_id.clone(),
            parent_id: Some(root_id),
            kind: "supervisor".to_string(),
            label: supervisor.title.clone(),
            state: supervisor.status.clone(),
            work_unit_id: None,
            workflow_run_id: None,
        },
    ];

    for unit in work_units {
        nodes.push(SupervisorTopologyNode {
            id: format!("work-unit-{}", unit.id),
            parent_id: Some(supervisor_id.clone()),
            kind: unit.kind.clone(),
            label: unit.title.clone(),
            state: unit.state.clone(),
            work_unit_id: Some(unit.id.clone()),
            workflow_run_id: unit.workflow_run_id.clone(),
        });
    }

    nodes
}

fn integration_state(status: &str) -> String {
    match status {
        "running_integration" | "validating" => "integrating".to_string(),
        "development_complete" | "ready_to_apply" => "ready_for_integration".to_string(),
        "applied" | "completed" => "integrated".to_string(),
        "failed" => "failed".to_string(),
        _ => "idle".to_string(),
    }
}

fn event_state(level: &str, kind: &str, message: &str, payload: &Value) -> String {
    let normalized_kind = kind.trim().to_ascii_lowercase();
    let normalized_level = level.trim().to_ascii_lowercase();
    let payload_status = payload
        .get("status")
        .and_then(Value::as_str)
        .or_else(|| payload.get("result").and_then(|value| value.get("status")).and_then(Value::as_str));
    let payload_disposition = payload
        .get("disposition")
        .and_then(Value::as_str)
        .or_else(|| payload.get("result").and_then(|value| value.get("disposition")).and_then(Value::as_str));
    let execution_state = payload
        .get("execution_state")
        .and_then(Value::as_str)
        .or_else(|| payload.get("result").and_then(|value| value.get("execution_state")).and_then(Value::as_str));

    if payload_status == Some("paused") || payload_disposition == Some("pause_error") {
        return "paused".to_string();
    }
    if execution_state == Some("awaiting_user_input") {
        return "user_input".to_string();
    }
    if payload_status == Some("error")
        || payload_status.is_some_and(|status| status.starts_with("error_code:"))
        || normalized_kind.ends_with("_failed")
        || normalized_kind.ends_with("_error")
        || normalized_level == "error"
    {
        return "failed".to_string();
    }
    if matches!(payload_status, Some("success") | Some("complete") | Some("completed")) {
        return "completed".to_string();
    }
    if normalized_kind.ends_with("_completed")
        || normalized_kind.ends_with("_complete")
        || normalized_kind.ends_with("_succeeded")
        || normalized_kind.ends_with("_success")
    {
        return "completed".to_string();
    }
    if normalized_kind.ends_with("_started") || normalized_kind.ends_with("_running") {
        return "running".to_string();
    }
    if normalized_kind.contains("input_required") || normalized_kind.contains("checkpoint_required") {
        return "user_input".to_string();
    }
    if normalized_kind.contains("waiting") {
        return "waiting".to_string();
    }

    let haystack = format!("{} {}", level, message).to_ascii_lowercase();
    if haystack.contains("fail") || haystack.contains("error") {
        "failed".to_string()
    } else if haystack.contains("pause") {
        "paused".to_string()
    } else if haystack.contains("success") || haystack.contains("complete") {
        "completed".to_string()
    } else if haystack.contains("start") || haystack.contains("running") {
        "running".to_string()
    } else if haystack.contains("wait") {
        "waiting".to_string()
    } else {
        "unknown".to_string()
    }
}

fn summary_field(value: &Value) -> Option<String> {
    value.get("summary")
        .and_then(Value::as_str)
        .or_else(|| value.get("display_summary").and_then(Value::as_str))
        .or_else(|| value.get("status").and_then(Value::as_str))
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn parse_json(value: String) -> Value {
    serde_json::from_str(&value).unwrap_or_else(|_| json!({}))
}

fn internal(err: impl std::fmt::Display) -> (StatusCode, String) {
    let message = err.to_string();
    tracing::error!(error = %message, "supervisor projection route error");
    (StatusCode::INTERNAL_SERVER_ERROR, message)
}
