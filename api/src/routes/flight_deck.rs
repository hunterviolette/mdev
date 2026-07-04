use axum::{extract::{Query, State}, http::StatusCode, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use std::collections::HashSet;
use std::process::Command;

use crate::app_state::AppState;

#[derive(Debug, Clone, Deserialize)]
struct FlightDeckQuery {
    supervisor_id: Option<String>,
    root_repo_path: Option<String>,
    state: Option<String>,
    kind: Option<String>,
    include_deleted: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Default)]
struct FlightDeckTotals {
    supervisors: usize,
    work_units: usize,
    running: usize,
    waiting_user: usize,
    failed: usize,
    ready_for_integration: usize,
    integrating: usize,
}

#[derive(Debug, Clone, Serialize)]
struct FlightDeckResponse {
    supervisors: Vec<FlightDeckSupervisor>,
    alerts: Vec<FlightDeckAlert>,
    totals: FlightDeckTotals,
}

#[derive(Debug, Clone, Serialize)]
struct FlightDeckSupervisor {
    id: String,
    mode: String,
    status: String,
    title: String,
    root_repo_path: String,
    snapshot_path: Option<String>,
    integration_path: Option<String>,
    integration_run_id: Option<String>,
    topology: Vec<FlightDeckTopologyNode>,
    work_units: Vec<FlightDeckWorkUnit>,
    alerts: Vec<FlightDeckAlert>,
    integration: Value,
    context: Value,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
struct FlightDeckTopologyNode {
    id: String,
    parent_id: Option<String>,
    kind: String,
    label: String,
    state: String,
    work_unit_id: Option<String>,
    workflow_run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct FlightDeckWorkUnit {
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
    shard_path: Option<String>,
    integration_path: Option<String>,
    telemetry: Value,
    alerts: Vec<FlightDeckAlert>,
    created_at: Option<String>,
    updated_at: Option<String>,
    workflow_deleted: bool,
}

#[derive(Debug, Clone, Serialize)]
struct FlightDeckAlert {
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
    shard_path: Option<String>,
    integration_path: Option<String>,
    context: Value,
    created_at: Option<String>,
    updated_at: Option<String>,
    workflow_deleted: bool,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/flight-deck", get(get_flight_deck))
}

async fn get_flight_deck(
    State(state): State<AppState>,
    Query(query): Query<FlightDeckQuery>,
) -> Result<Json<FlightDeckResponse>, (StatusCode, String)> {
    build_flight_deck(&state, query).await.map(Json).map_err(internal)
}

async fn build_flight_deck(state: &AppState, query: FlightDeckQuery) -> anyhow::Result<FlightDeckResponse> {
    let supervisors = load_supervisors(state, &query).await?;
    let mut response_supervisors = Vec::new();
    let mut response_alerts = Vec::new();
    let mut totals = FlightDeckTotals::default();
    let include_deleted = query.include_deleted.unwrap_or(false);

    for supervisor in supervisors {
        sync_supervisor_work_units(state, &supervisor).await?;
        let mut seeds = load_work_unit_seeds(state, &supervisor, include_deleted).await?;

        if let Some(kind) = query.kind.as_deref().filter(|value| !value.trim().is_empty()) {
            seeds.retain(|unit| unit.kind == kind);
        }
        if let Some(state_filter) = query.state.as_deref().filter(|value| !value.trim().is_empty()) {
            seeds.retain(|unit| unit.state == state_filter);
        }

        let mut work_units = Vec::new();
        let mut supervisor_alerts = Vec::new();

        for seed in seeds {
            let telemetry = match seed.workflow_run_id.as_deref() {
                Some(workflow_run_id) if !seed.workflow_deleted => workflow_telemetry(state, workflow_run_id).await?,
                _ => empty_telemetry(),
            };
            let telemetry = enrich_projection_context_telemetry(telemetry, &seed.context);
            let telemetry = if seed.workflow_type == "manual_shard" {
                enrich_manual_shard_telemetry(telemetry, seed.shard_path.as_deref())
            } else {
                telemetry
            };

            let mut alerts = Vec::new();
            if seed.state == "waiting_user" {
                alerts.push(FlightDeckAlert {
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
            if seed.state == "failed" || seed.state == "blocked" {
                alerts.push(FlightDeckAlert {
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
                alerts.push(FlightDeckAlert {
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

            let projected_state = flight_deck_work_unit_state(&seed);
            supervisor_alerts.extend(alerts.iter().cloned());
            work_units.push(FlightDeckWorkUnit {
                id: seed.id,
                supervisor_id: seed.supervisor_id,
                repo_id: seed.repo_id,
                feature_id: seed.feature_id,
                workflow_run_id: seed.workflow_run_id,
                patch_id: seed.patch_id,
                kind: seed.kind,
                workflow_type: seed.workflow_type,
                title: seed.title,
                state: projected_state,
                root_repo_path: seed.root_repo_path,
                shard_path: seed.shard_path,
                integration_path: seed.integration_path,
                telemetry,
                alerts,
                created_at: seed.created_at,
                updated_at: seed.updated_at,
                workflow_deleted: seed.workflow_deleted,
            });
        }

        let wants_integration = query.kind.as_deref()
            .map(|value| value.trim().is_empty() || value == "integration")
            .unwrap_or(true);
        let wants_draft = query.state.as_deref()
            .map(|value| value.trim().is_empty() || value == "draft")
            .unwrap_or(true);
        if wants_integration && wants_draft && !work_units.iter().any(|unit| unit.kind == "integration") {
            work_units.push(integration_draft_work_unit(&supervisor));
        }

        let topology = build_topology(&supervisor, &work_units);
        let pending_patch_count = work_units.iter().filter(|unit| {
            unit.patch_id.is_some() && (unit.state == "patch_ready" || unit.state == "ready_for_integration")
        }).count();
        let integration_state_value = integration_state(&supervisor.status);
        let integration = json!({
            "state": integration_state_value,
            "workflow_run_id": supervisor.integration_run_id,
            "pending_patch_count": pending_patch_count,
            "merge_summary": summary_field(&supervisor.merge_report),
            "validation_summary": summary_field(&supervisor.validation_report)
        });

        totals.supervisors += 1;
        totals.work_units += work_units.len();
        for unit in &work_units {
            match unit.state.as_str() {
                "running" => totals.running += 1,
                "waiting_user" => totals.waiting_user += 1,
                "failed" | "blocked" => totals.failed += 1,
                "patch_ready" | "ready_for_integration" => totals.ready_for_integration += 1,
                "integrating" => totals.integrating += 1,
                _ => {}
            }
        }

        response_alerts.extend(supervisor_alerts.iter().cloned());
        response_supervisors.push(FlightDeckSupervisor {
            id: supervisor.id,
            mode: supervisor.mode,
            status: supervisor.status,
            title: supervisor.title,
            root_repo_path: supervisor.root_repo_path,
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
        });
    }

    Ok(FlightDeckResponse {
        supervisors: response_supervisors,
        alerts: response_alerts,
        totals,
    })
}

async fn load_supervisors(state: &AppState, query: &FlightDeckQuery) -> anyhow::Result<Vec<SupervisorRow>> {
    let rows = sqlx::query(
        r#"
        SELECT id, mode, status, title, root_repo_path, snapshot_path, integration_path, integration_run_id,
               merge_report_json, validation_report_json, context_json, created_at, updated_at
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
        if query.root_repo_path.as_deref().is_some_and(|filter| !filter.is_empty() && filter != root_repo_path) {
            continue;
        }
        supervisors.push(SupervisorRow {
            id,
            mode: row.get("mode"),
            status: row.get("status"),
            title: row.get("title"),
            root_repo_path,
            snapshot_path: row.get("snapshot_path"),
            integration_path: row.get("integration_path"),
            integration_run_id: row.get("integration_run_id"),
            merge_report: parse_json(row.get::<String, _>("merge_report_json")),
            validation_report: parse_json(row.get::<String, _>("validation_report_json")),
            context: parse_json(row.get::<String, _>("context_json")),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        });
    }
    Ok(supervisors)
}

async fn sync_supervisor_work_units(state: &AppState, supervisor: &SupervisorRow) -> anyhow::Result<()> {
    let current_sprint_id = sqlx::query(
        r#"
        SELECT id
        FROM sprints
        WHERE supervisor_run_id = ?
          AND status != 'archived'
        ORDER BY updated_at DESC, created_at DESC
        LIMIT 1
        "#,
    )
    .bind(supervisor.id.as_str())
    .fetch_optional(&state.db)
    .await?
    .map(|row| row.get::<String, _>("id"));

    sqlx::query(
        r#"
        DELETE FROM supervisor_work_units
        WHERE supervisor_run_id = ?
          AND kind = 'feature_development'
          AND json_extract(context_json, '$.source') = 'flight_deck_projection_sync'
        "#,
    )
    .bind(supervisor.id.as_str())
    .execute(&state.db)
    .await?;

    let Some(current_sprint_id) = current_sprint_id else {
        return Ok(());
    };

    sqlx::query(
        r#"
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
            sr.id || ':' || sf.feature_id,
            sr.id,
            pf.repo_id,
            sf.feature_id,
            sf.current_workflow_run_id,
            sf.current_patch_id,
            'feature_development',
            COALESCE(pf.title, sf.feature_id),
            CASE
                WHEN TRIM(COALESCE(sf.last_error, '')) != '' THEN 'failed'
                WHEN sf.development_state IN ('development_running', 'running', 'active') THEN 'running'
                WHEN sf.development_state IN ('waiting', 'waiting_user', 'paused') THEN 'waiting_user'
                WHEN sf.development_state IN ('development_failed', 'failed', 'blocked') THEN 'failed'
                WHEN sf.development_state IN ('development_succeeded', 'completed') THEN 'ready_for_integration'
                WHEN sf.development_state IN ('integrating', 'integration_running') THEN 'integrating'
                WHEN sf.development_state IN ('integrated', 'applied') THEN 'integrated'
                WHEN sf.development_state = 'patch_ready' THEN 'patch_ready'
                WHEN sf.status IN ('active', 'running') THEN 'running'
                WHEN sf.status IN ('waiting', 'paused') THEN 'waiting_user'
                WHEN sf.status IN ('failed', 'blocked') THEN 'failed'
                WHEN sf.status = 'completed' THEN 'ready_for_integration'
                ELSE 'queued'
            END,
            sr.root_repo_path,
            sf.shard_path,
            sr.integration_path,
            0,
            sf.sort_order,
            sf.last_error,
            '{}',
            json_object(
                'source', 'flight_deck_projection_sync',
                'sprint_id', sf.sprint_id,
                'feature_status', sf.status,
                'development_state', sf.development_state,
                'integration_skipped', COALESCE(sf.integration_skipped, 0)
            ),
            sf.created_at,
            sf.updated_at
        FROM sprint_features sf
        JOIN supervisor_runs sr ON sr.id = sf.supervisor_run_id
        LEFT JOIN planner_features pf ON pf.id = sf.feature_id
        WHERE sf.supervisor_run_id = ?
          AND sf.sprint_id = ?
          AND sf.status != 'unscheduled'
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
    .bind(current_sprint_id.as_str())
    .execute(&state.db)
    .await?;

    Ok(())
}

async fn load_work_unit_seeds(state: &AppState, supervisor: &SupervisorRow, include_deleted: bool) -> anyhow::Result<Vec<WorkUnitSeed>> {
    let rows = sqlx::query(
        r#"
        SELECT wu.id, wu.supervisor_run_id, wu.repo_id, wu.feature_id, wu.workflow_run_id, wu.patch_id,
               wu.kind, COALESCE(json_extract(wu.context_json, '$.workflow_type'), wu.kind) AS workflow_type,
               wu.title, wu.state, wr.status AS workflow_status, wu.root_repo_path, wu.shard_path, wu.integration_path,
               wu.context_json, wu.created_at, wu.updated_at,
               CASE WHEN wu.workflow_run_id IS NULL OR wr.id IS NOT NULL THEN 1 ELSE 0 END AS workflow_exists
        FROM supervisor_work_units wu
        LEFT JOIN workflow_runs wr ON wr.id = wu.workflow_run_id
        WHERE wu.supervisor_run_id = ?
        ORDER BY wu.priority DESC, wu.queue_position ASC, wu.updated_at DESC
        "#,
    )
    .bind(supervisor.id.as_str())
    .fetch_all(&state.db)
    .await?;

    let mut seeds = Vec::new();
    for row in rows {
        let workflow_run_id: Option<String> = row.get("workflow_run_id");
        let workflow_exists = row.get::<i64, _>("workflow_exists") == 1;
        if workflow_run_id.is_some() && !workflow_exists && !include_deleted {
            continue;
        }
        seeds.push(WorkUnitSeed {
            id: row.get("id"),
            supervisor_id: row.get("supervisor_run_id"),
            repo_id: row.get("repo_id"),
            feature_id: row.get("feature_id"),
            workflow_run_id,
            patch_id: row.get("patch_id"),
            kind: row.get("kind"),
            workflow_type: row.get("workflow_type"),
            title: row.get("title"),
            state: if workflow_exists { row.get("state") } else { "deleted".to_string() },
            workflow_status: row.get("workflow_status"),
            root_repo_path: row.get("root_repo_path"),
            shard_path: row.get("shard_path"),
            integration_path: row.get("integration_path"),
            context: parse_json(row.get::<String, _>("context_json")),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
            workflow_deleted: !workflow_exists,
        });
    }

    let has_integration_work_unit = seeds.iter().any(|seed| seed.kind == "integration");
    if !has_integration_work_unit {
        if let Some(integration_run_id) = supervisor.integration_run_id.as_deref().filter(|value| !value.trim().is_empty()) {
            let workflow_exists = workflow_exists(state, integration_run_id).await?;
            if workflow_exists || include_deleted {
                seeds.push(WorkUnitSeed {
                    id: format!("integration-{}", supervisor.id),
                    supervisor_id: supervisor.id.clone(),
                    repo_id: None,
                    feature_id: None,
                    workflow_run_id: Some(integration_run_id.to_string()),
                    patch_id: None,
                    kind: "integration".to_string(),
                    workflow_type: "integration".to_string(),
                    title: "Integration".to_string(),
                    state: if workflow_exists { integration_state(&supervisor.status) } else { "deleted".to_string() },
                    workflow_status: None,
                    root_repo_path: supervisor.root_repo_path.clone(),
                    shard_path: None,
                    integration_path: supervisor.integration_path.clone(),
                    context: json!({ "workflow_type": "integration", "pool_key": "integration" }),
                    created_at: Some(supervisor.created_at.clone()),
                    updated_at: Some(supervisor.updated_at.clone()),
                    workflow_deleted: !workflow_exists,
                });
            }
        }
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
        return (Vec::new(), Some("workflow run is missing definition_json; Flight Deck cannot render template stages".to_string()));
    };

    let definition = match serde_json::from_str::<Value>(definition_json) {
        Ok(value) => value,
        Err(err) => return (Vec::new(), Some(format!("workflow definition_json is invalid: {:#}", err))),
    };

    let Some(steps) = definition.get("steps").and_then(Value::as_array) else {
        return (Vec::new(), Some("workflow definition_json is missing steps[]; Flight Deck cannot render template stages".to_string()));
    };

    if steps.is_empty() {
        return (Vec::new(), Some("workflow template has no steps; Flight Deck cannot render template stages".to_string()));
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

async fn workflow_telemetry(state: &AppState, workflow_run_id: &str) -> anyhow::Result<Value> {
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
        SELECT step_id, stage_execution_id, capability_invocation_id, parent_invocation_id,
               level, kind, message, payload_json, created_at, sequence_no
        FROM workflow_events
        WHERE run_id = ?
        ORDER BY sequence_no DESC
        LIMIT 200
        "#,
    )
    .bind(workflow_run_id)
    .fetch_all(&state.db)
    .await?;

    let mut seen_stages = HashSet::new();
    let mut recent_stage_executions = Vec::new();
    let mut current_stage_recent_capabilities = Vec::new();

    for row in rows {
        let step_id: Option<String> = row.get("step_id");
        let stage_execution_id: Option<String> = row.get("stage_execution_id");
        let capability_invocation_id: Option<String> = row.get("capability_invocation_id");
        let parent_invocation_id: Option<String> = row.get("parent_invocation_id");
        let level: String = row.get("level");
        let kind: String = row.get("kind");
        let message: String = row.get("message");
        let payload = parse_json(row.get::<String, _>("payload_json"));
        let created_at: String = row.get("created_at");

        if let Some(stage_id) = stage_execution_id.clone() {
            if recent_stage_executions.len() < 4 && seen_stages.insert(stage_id.clone()) {
                recent_stage_executions.push(json!({
                    "stage_execution_id": stage_id,
                    "step_id": step_id,
                    "status": event_state(&level, &kind, &message),
                    "level": level,
                    "kind": kind,
                    "message": message,
                    "created_at": created_at
                }));
            }
        }

        if current_stage_recent_capabilities.len() < 16 && current_step_id.as_deref() == step_id.as_deref() {
            if let Some(capability_id) = capability_invocation_id.clone() {
                current_stage_recent_capabilities.push(json!({
                    "capability_invocation_id": capability_id,
                    "parent_invocation_id": parent_invocation_id,
                    "capability": payload.get("capability").and_then(Value::as_str).unwrap_or("capability"),
                    "status": event_state(&level, &kind, &message),
                    "level": level,
                    "kind": kind,
                    "message": message,
                    "created_at": created_at
                }));
            }
        }
    }

    Ok(json!({
        "status": status,
        "current_step_id": current_step_id,
        "stage_template": stage_template,
        "stage_template_error": stage_template_error,
        "recent_stage_executions": recent_stage_executions,
        "current_stage_recent_capabilities": current_stage_recent_capabilities
    }))
}

fn manual_shard_has_staged_changes(shard_path: Option<&str>) -> bool {
    let Some(shard_path) = shard_path.filter(|value| !value.trim().is_empty()) else {
        return false;
    };

    let Ok(output) = Command::new("git")
        .arg("diff")
        .arg("--cached")
        .arg("--quiet")
        .current_dir(shard_path)
        .output()
    else {
        return false;
    };

    output.status.code() == Some(1)
}

fn enrich_manual_shard_telemetry(mut telemetry: Value, shard_path: Option<&str>) -> Value {
    let has_staged_changes = manual_shard_has_staged_changes(shard_path);
    if let Some(obj) = telemetry.as_object_mut() {
        obj.insert("manual_shard_has_staged_changes".to_string(), Value::Bool(has_staged_changes));
        obj.insert("manual_shard_stageable".to_string(), Value::Bool(has_staged_changes));
    }
    telemetry
}

fn enrich_projection_context_telemetry(mut telemetry: Value, context: &Value) -> Value {
    if let Some(obj) = telemetry.as_object_mut() {
        if let Some(value) = context.get("integration_skipped") {
            let skipped = value.as_bool().unwrap_or_else(|| value.as_i64().unwrap_or(0) != 0);
            obj.insert("integration_skipped".to_string(), Value::Bool(skipped));
        }
    }
    telemetry
}

fn empty_telemetry() -> Value {
    json!({
        "status": null,
        "current_step_id": null,
        "recent_stage_executions": [],
        "current_stage_recent_capabilities": []
    })
}

fn integration_draft_work_unit(supervisor: &SupervisorRow) -> FlightDeckWorkUnit {
    FlightDeckWorkUnit {
        id: format!("{}:integration:draft", supervisor.id),
        supervisor_id: supervisor.id.clone(),
        repo_id: None,
        feature_id: None,
        workflow_run_id: None,
        patch_id: None,
        kind: "integration".to_string(),
        workflow_type: "integration".to_string(),
        title: "Draft integration workflow".to_string(),
        state: "draft".to_string(),
        root_repo_path: supervisor.root_repo_path.clone(),
        shard_path: None,
        integration_path: supervisor.integration_path.clone(),
        telemetry: integration_draft_telemetry(),
        alerts: Vec::new(),
        created_at: Some(supervisor.created_at.clone()),
        updated_at: Some(supervisor.updated_at.clone()),
        workflow_deleted: false,
    }
}

fn integration_draft_telemetry() -> Value {
    json!({
        "status": "draft",
        "pool_key": "integration",
        "current_step_id": null,
        "recent_stage_executions": [],
        "current_stage_recent_capabilities": []
    })
}

fn build_topology(supervisor: &SupervisorRow, work_units: &[FlightDeckWorkUnit]) -> Vec<FlightDeckTopologyNode> {
    let root_id = format!("root-{}", supervisor.id);
    let supervisor_id = format!("supervisor-{}", supervisor.id);
    let mut nodes = vec![
        FlightDeckTopologyNode {
            id: root_id.clone(),
            parent_id: None,
            kind: "root".to_string(),
            label: supervisor.root_repo_path.clone(),
            state: "root".to_string(),
            work_unit_id: None,
            workflow_run_id: None,
        },
        FlightDeckTopologyNode {
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
        nodes.push(FlightDeckTopologyNode {
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

fn flight_deck_work_unit_state(seed: &WorkUnitSeed) -> String {
    let workflow_status = seed.workflow_status.as_deref().unwrap_or("").trim().to_ascii_lowercase();
    match workflow_status.as_str() {
        "waiting" | "waiting_user" | "paused" => "waiting_user".to_string(),
        "running" => "running".to_string(),
        "failed" | "error" => "failed".to_string(),
        "cancelled" | "canceled" => "cancelled".to_string(),
        "completed" | "complete" | "success" => {
            if seed.kind == "manual_shard" && seed.state == "ready_for_integration" {
                "ready_for_integration".to_string()
            } else if seed.kind == "integration" {
                "integrated".to_string()
            } else if seed.state == "patch_ready" || seed.state == "ready_for_integration" {
                seed.state.clone()
            } else {
                "patch_ready".to_string()
            }
        }
        _ => seed.state.clone(),
    }
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

fn event_state(level: &str, kind: &str, message: &str) -> String {
    let haystack = format!("{} {} {}", level, kind, message).to_ascii_lowercase();
    if haystack.contains("fail") || haystack.contains("error") {
        "failed".to_string()
    } else if haystack.contains("wait") || haystack.contains("pause") || haystack.contains("input") {
        "waiting_user".to_string()
    } else if haystack.contains("success") || haystack.contains("complete") {
        "success".to_string()
    } else if haystack.contains("start") || haystack.contains("running") {
        "running".to_string()
    } else {
        "event".to_string()
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
    tracing::error!(error = %message, "flight deck route error");
    (StatusCode::INTERNAL_SERVER_ERROR, message)
}
