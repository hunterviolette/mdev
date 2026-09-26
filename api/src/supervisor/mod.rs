pub mod lifecycle;
pub mod models;
pub mod patches;
pub mod repo_snapshot;
pub mod workflow_spawn;

use std::{collections::{HashMap, HashSet}, fs, path::{Path, PathBuf}};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine,
    engine::capabilities::planner::{
        ExecutionPlanItem,
        FeaturePlanItem,
        FeaturePlanItemStatus,
    },
    models::{RunStatus, SupervisorEventPayload, SupervisorEventStreamItem},
};
use models::{integration_work_unit_id, CreateSupervisorRunRequest, CreateSupervisorWorkUnitRequest, IntegrationInputState, SupervisorExecutionStrategy, SupervisorFeatureWorkflow, SupervisorIntegrationCandidate, SupervisorIntegrationInput, SupervisorRun, SupervisorStatus, SupervisorWorkPoolKind, SupervisorWorkUnitRecord, SupervisorWorkUnitState, SupervisorWorkUnitStoredContext};
use lifecycle::{SupervisorPoolKind, SupervisorWorkUnitPromiseRequest, SupervisorWorkflowSpawnRequest};



pub async fn load_supervisor_run(state: &AppState, id: Uuid) -> Result<SupervisorRun> {
    let row = sqlx::query("SELECT * FROM supervisor_runs WHERE id = ? AND archived_at IS NULL")
        .bind(id.to_string())
        .fetch_one(&state.db)
        .await?;
    let mut run = row_to_supervisor_run(row)?;
    hydrate_supervisor_planner(state, &mut run).await?;
    hydrate_supervisor_feature_workflows(state, &mut run).await?;
    Ok(run)
}





async fn ensure_supervisor_integration_work_unit(
    state: &AppState,
    run: &SupervisorRun,
) -> Result<()> {
    let template_id = supervisor_pool_template_uuid(&run.context, SupervisorWorkPoolKind::Integration.as_str());
    let work_unit_id = integration_work_unit_id(run.id);

    lifecycle::promise_supervisor_work_unit(
        state,
        SupervisorWorkUnitPromiseRequest {
            supervisor_run_id: run.id,
            root_repo_path: run.root_repo_path.clone(),
            pool_kind: SupervisorPoolKind::Integration,
            work_unit_id,
            feature_id: None,
            title: "Integration".to_string(),
            template_id,
            context: SupervisorWorkUnitStoredContext {
                template_id,
                planned_workflow_template_id: template_id,
                ..Default::default()
            },
            priority: 0,
            queue_position: None,
        },
    )
    .await
}

pub async fn create_supervisor_run(state: &AppState, req: CreateSupervisorRunRequest) -> Result<SupervisorRun> {
    let id = Uuid::new_v4();
    let now = Utc::now();
    let mut context = req.context;
    if !context.is_object() {
        context = json!({});
    }
    let execution_plan_items = req.execution_plan_items;
    let run = SupervisorRun {
        id,
        strategy: req.strategy,
        status: SupervisorStatus::Created,
        title: req.title,
        root_repo_path: req.root_repo_path,
        selected_planner_id: None,
        snapshot_path: None,
        integration_path: None,
        feature_plan_items: req.feature_plan_items,
        execution_plan_items,
        feature_workflows: Vec::new(),
        integration_run_id: None,
        final_patch_path: None,
        merge_report: json!({}),
        validation_report: json!({}),
        context,
        created_at: now,
        updated_at: now,
    };
    insert_supervisor_run(state, &run).await?;
    ensure_supervisor_integration_work_unit(state, &run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_created", "supervisor created").await?;
    Ok(run)
}





fn normalize_repo_root(value: &str) -> String {
    let replaced = value.trim().replace('\\', "/");
    let trimmed = replaced.trim_end_matches('/').to_string();
    if cfg!(windows) {
        trimmed.to_lowercase()
    } else {
        trimmed
    }
}




async fn hydrate_supervisor_planner(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    let Some(planner_id) = run.selected_planner_id.clone() else {
        return Ok(());
    };

    if !state.planner().belongs_to_scope(&planner_id, &run.root_repo_path).await? {
        return Err(anyhow!(
            "selected planner '{}' does not belong to supervisor repo_ref '{}'",
            planner_id,
            run.root_repo_path
        ));
    }

    let persisted_features = state.planner().features(&planner_id).await?;
    if !persisted_features.is_empty() {
        run.feature_plan_items = persisted_features;
        let feature_ids = run
            .feature_plan_items
            .iter()
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        run.execution_plan_items
            .retain(|item| feature_ids.iter().any(|id| id == &item.feature_plan_item_id));
    } else if !run.feature_plan_items.is_empty() {
        state.planner().replace_features(&planner_id, &run.feature_plan_items).await?;
    }

    Ok(())
}





async fn append_supervisor_event(
    state: &AppState,
    supervisor_run_id: Uuid,
    event_type: &str,
    message: &str,
    payload: SupervisorEventPayload,
) -> Result<SupervisorEventStreamItem> {
    let event_time = Utc::now();
    let event = SupervisorEventStreamItem {
        id: Uuid::new_v4().to_string(),
        supervisor_run_id: supervisor_run_id.to_string(),
        sequence_no: event_time.timestamp_micros(),
        event_type: event_type.to_string(),
        event_time: event_time.to_rfc3339(),
        message: message.to_string(),
        payload,
        created_at: event_time.to_rfc3339(),
    };
    state.publish_supervisor_event(event.clone());
    Ok(event)
}





#[derive(Debug, Clone, Serialize, Deserialize)]
struct SupervisorQueuedFeatureRef {
    feature_id: String,
    planner_id: String,
    planner_title: String,
}

fn import_feature_ids(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|feature_id| !feature_id.is_empty() && !feature_id.starts_with("manual-"))
                .fold(Vec::<String>::new(), |mut acc, feature_id| {
                    if !acc.iter().any(|existing| existing == feature_id) {
                        acc.push(feature_id.to_string());
                    }
                    acc
                })
        })
        .unwrap_or_default()
}

fn supervisor_planner_title(root: &str) -> String {
    let normalized = normalize_repo_root(root);
    let name = normalized
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Planner");
    format!("{} Planner", name)
}

async fn ensure_planner_workspace_table(state: &AppState) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS planner_workspaces (
            id TEXT PRIMARY KEY,
            root_repo_path TEXT NOT NULL,
            repo_key TEXT NOT NULL DEFAULT '',
            title TEXT NOT NULL,
            is_default INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(&state.db)
    .await?;
    Ok(())
}



























fn workspace_has_changes(workspace_path: Option<&str>) -> bool {
    workspace_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .and_then(|path| patches::change_status(Path::new(path)).ok())
        .is_some_and(|status| status.has_changes())
}

async fn queue_planner_features(state: &AppState, repo_ref: &str, planner_id: Option<&str>) -> Result<(Option<String>, Option<String>, Vec<FeaturePlanItem>)> {
    let Some(requested_planner_id) = planner_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok((None, None, Vec::new()));
    };

    if !state.planner().belongs_to_scope(requested_planner_id, repo_ref).await? {
        return Ok((Some(requested_planner_id.to_string()), None, Vec::new()));
    }

    let Some(planner) = state.planner().get(requested_planner_id).await? else {
        return Ok((Some(requested_planner_id.to_string()), None, Vec::new()));
    };

    Ok((
        Some(planner.id),
        Some(planner.title),
        planner.features,
    ))
}

pub async fn supervisor_queue_projection(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    kick_feature_pool_if_running(state, &mut run).await?;

    let current_planner_id = run.selected_planner_id.clone().unwrap_or_default();

    let rows = sqlx::query(
        r#"
        SELECT
            pf.id,
            pf.planner_id,
            pw.title AS planner_title,
            pf.title,
            pf.status,
            pf.payload_json,
            pf.locked_supervisor_run_id,
            pf.locked_at,
            pf.completed_at,
            wu.id AS current_work_unit_id,
            wu.workflow_run_id AS current_workflow_run_id,
            wu.patch_id AS current_patch_id,
            wu.state AS development_state,
            wu.workspace_path
        FROM planner_features pf
        JOIN planner_workspaces pw ON pw.id = pf.planner_id
        LEFT JOIN supervisor_work_units wu
          ON wu.feature_id = pf.id
         AND wu.supervisor_run_id = ?
         AND wu.kind = 'feature'
         AND wu.archived_at IS NULL
         AND wu.state NOT IN ('cancelled', 'archived')
        WHERE pf.id NOT LIKE 'manual-%'
          AND COALESCE(pf.status, '') != 'archived'
          AND (
              pf.planner_id = ?
              OR wu.id IS NOT NULL
          )
        ORDER BY
          pf.sort_order ASC,
          pf.created_at ASC
        "#,
    )
    .bind(run.id.to_string())
    .bind(&current_planner_id)
    .fetch_all(&state.db)
    .await?;

    let mut queued_features = Vec::<Value>::new();
    let mut feature_ids = Vec::<String>::new();
    let mut items = Vec::<Value>::new();
    let mut seen = HashSet::<String>::new();

    for row in rows {
        let feature_id: String = row.get("id");
        if !seen.insert(feature_id.clone()) {
            continue;
        }

        let planner_id: String = row.get("planner_id");
        let planner_title: String = row.get("planner_title");
        let title: String = row.get("title");
        let status: String = row.get("status");
        let payload_json: String = row.get("payload_json");
        let locked_owner = row.get::<Option<String>, _>("locked_supervisor_run_id");
        let locked_by_this = locked_owner.as_deref() == Some(run.id.to_string().as_str());
        let locked_by_other = locked_owner.as_ref().map(|owner| !owner.trim().is_empty()).unwrap_or(false) && !locked_by_this;
        let is_current_planner = planner_id == current_planner_id;
        let work_unit_id = row.get::<Option<String>, _>("current_work_unit_id");
        let workflow_run_id = row.get::<Option<String>, _>("current_workflow_run_id");

        tracing::warn!(
            supervisor_run_id = %run.id,
            feature_id = %feature_id,
            planner_id = %planner_id,
            planner_status = %status,
            current_planner = is_current_planner,
            locked_owner = ?locked_owner,
            locked_by_this,
            locked_by_other,
            current_workflow_run_id = ?workflow_run_id,
            "projecting supervisor queue feature"
        );
        let patch_id = row.get::<Option<String>, _>("current_patch_id");
        let development_state = row.get::<Option<String>, _>("development_state");
        let locked_at = row.get::<Option<String>, _>("locked_at");
        let workspace_path = row.get::<Option<String>, _>("workspace_path");
        let queued = work_unit_id.is_some();
        let has_development_diff = workspace_has_changes(workspace_path.as_deref());
        let dequeue_without_prompt = queued && !has_development_diff;

        let feature_payload = serde_json::from_str::<Value>(&payload_json).unwrap_or_else(|_| json!({}));
        let summary = feature_payload.get("summary").and_then(Value::as_str).map(str::to_string);
        let can_queue = !queued && !locked_by_other && status == "fine";
        let queue_state = if queued {
            development_state.clone().unwrap_or_else(|| "queued".to_string())
        } else if locked_by_other {
            "locked".to_string()
        } else {
            "available".to_string()
        };

        if queued {
            queued_features.push(json!({
                "feature_id": feature_id,
                "planner_id": planner_id,
                "planner_title": planner_title
            }));
        }

        feature_ids.push(feature_id.clone());
        items.push(json!({
            "feature_id": feature_id,
            "planner_id": planner_id,
            "planner_title": planner_title,
            "is_current_planner": is_current_planner,
            "title": title,
            "summary": summary,
            "planner_status": status,
            "queue_state": queue_state,
            "queued": queued,
            "can_queue": can_queue,
            "can_dequeue": queued,
            "dequeue_without_prompt": dequeue_without_prompt,
            "has_development_diff": has_development_diff,
            "locked_by_other": locked_by_other,
            "lock_owner_supervisor_run_id": locked_owner,
            "disabled_reason": if locked_by_other { Some("Feature is checked out by another supervisor".to_string()) } else { None },
            "work_unit_id": work_unit_id,
            "current_workflow_run_id": workflow_run_id,
            "current_patch_id": patch_id,
            "development_state": development_state,
            "scheduled_at": locked_at,
            "locked_at": locked_at,
            "workspace_path": workspace_path,
            "development_started_at": Value::Null,
            "development_completed_at": Value::Null,
            "integration_completed_at": Value::Null,
            "applied_at": Value::Null
        }));
    }

    let planner_rows = sqlx::query(
        r#"
        SELECT id, root_repo_path, title, is_default, created_at, updated_at
        FROM planner_workspaces
        WHERE LOWER(REPLACE(root_repo_path, char(92), '/')) = LOWER(REPLACE(?, char(92), '/'))
        ORDER BY is_default DESC, updated_at DESC, created_at DESC
        "#,
    )
    .bind(&run.root_repo_path)
    .fetch_all(&state.db)
    .await?;

    let planners = planner_rows
        .into_iter()
        .map(|row| json!({
            "id": row.get::<String, _>("id"),
            "root_repo_path": row.get::<String, _>("root_repo_path"),
            "title": row.get::<String, _>("title"),
            "is_default": row.get::<i64, _>("is_default") != 0,
            "feature_plan_items": [],
            "created_at": row.get::<String, _>("created_at"),
            "updated_at": row.get::<String, _>("updated_at")
        }))
        .collect::<Vec<_>>();

    tracing::warn!(
        supervisor_run_id = %run.id,
        current_planner_id = %current_planner_id,
        projected_queued_features = ?queued_features,
        projected_feature_ids = ?feature_ids,
        "completed supervisor queue projection"
    );

    Ok(json!({
        "ok": true,
        "supervisor_run_id": run.id,
        "root_repo_path": run.root_repo_path,
        "current_planner_id": current_planner_id,
        "planners": planners,
        "queued_features": queued_features,
        "feature_ids": feature_ids,
        "items": items
    }))
}

async fn start_next_feature_pool_work_units(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    let feature_concurrency = supervisor_feature_concurrency(run);
    let mut active_count = sqlx::query(
        r#"
        SELECT COUNT(*) AS count
        FROM supervisor_work_units wu
        LEFT JOIN workflow_runs wr ON wr.id = wu.workflow_run_id
        WHERE wu.supervisor_run_id = ?
          AND wu.kind = 'feature'
          AND wu.archived_at IS NULL
          AND wr.status IN ('running', 'waiting', 'paused')
        "#,
    )
    .bind(run.id.to_string())
    .fetch_one(&state.db)
    .await?
    .get::<i64, _>("count")
    .max(0) as usize;

    if active_count >= feature_concurrency {
        return Ok(());
    }

    let rows = sqlx::query(
        r#"
        SELECT wu.id
        FROM supervisor_work_units wu
        LEFT JOIN workflow_runs wr ON wr.id = wu.workflow_run_id
        WHERE wu.supervisor_run_id = ?
          AND wu.kind = 'feature'
          AND wu.archived_at IS NULL
          AND wu.state = 'queued'
          AND (
              wu.workflow_run_id IS NULL
              OR wr.status IN ('draft', 'queued')
          )
        ORDER BY COALESCE(wu.queue_position, 9223372036854775807) ASC, wu.created_at ASC
        "#,
    )
    .bind(run.id.to_string())
    .fetch_all(&state.db)
    .await?;

    for row in rows {
        if active_count >= feature_concurrency {
            break;
        }
        let work_unit_id: String = row.get("id");
        start_supervisor_work_unit(state, run.id, work_unit_id).await?;
        active_count += 1;
    }

    *run = load_supervisor_run(state, run.id).await?;
    Ok(())
}

async fn kick_feature_pool_if_running(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    if !matches!(run.status, SupervisorStatus::RunningChildren) {
        return Ok(());
    }

    start_next_feature_pool_work_units(state, run).await?;
    run.status = SupervisorStatus::RunningChildren;
    run.updated_at = Utc::now();
    update_supervisor_run(state, run).await?;
    Ok(())
}

fn supervisor_pool_config<'a>(context: &'a Value, pool_key: &str) -> Option<&'a Value> {
    context
        .get("pools")
        .and_then(|value| value.get(pool_key))
}

fn supervisor_pool_template_ref(context: &Value, pool_key: &str) -> Option<String> {
    supervisor_pool_config(context, pool_key)
        .and_then(|value| value.get("template_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn supervisor_pool_template_uuid(context: &Value, pool_key: &str) -> Option<Uuid> {
    supervisor_pool_template_ref(context, pool_key)
        .and_then(|value| Uuid::parse_str(&value).ok())
}

async fn resolve_workflow_template_ref_id(state: &AppState, value: &str) -> Result<Option<Uuid>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if let Ok(id) = Uuid::parse_str(trimmed) {
        return Ok(Some(id));
    }
    let row = sqlx::query("SELECT id FROM workflow_templates WHERE id = ? OR name = ? LIMIT 1")
        .bind(trimmed)
        .bind(trimmed)
        .fetch_optional(&state.db)
        .await?;
    row.map(|row| Uuid::parse_str(row.get::<String, _>("id").as_str()).map_err(Into::into))
        .transpose()
}


pub async fn update_supervisor_config(state: &AppState, id: Uuid, mut config: Value) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    if !config.is_object() {
        return Err(anyhow!("supervisor config must be an object"));
    }
    if !run.context.is_object() {
        run.context = json!({});
    }

    let execution_event_limit = config
        .get("execution_event_limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(10, 1000);
    let pools = config
        .get_mut("pools")
        .map(std::mem::take)
        .unwrap_or_else(|| json!({}));
    if !pools.is_object() {
        return Err(anyhow!("supervisor pools config must be an object"));
    }

    if let Some(obj) = run.context.as_object_mut() {
        obj.insert("execution_event_limit".to_string(), Value::Number(execution_event_limit.into()));
        obj.insert("pools".to_string(), pools);
    }

    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;

    kick_feature_pool_if_running(state, &mut run).await?;

    let run = load_supervisor_run(state, id).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_config_updated", "supervisor configuration updated").await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

pub async fn select_supervisor_planner(state: &AppState, id: Uuid, planner_id: String) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let planner_id = planner_id.trim();
    if planner_id.is_empty() {
        return Err(anyhow!("planner_id is required"));
    }

    if !state.planner().belongs_to_scope(planner_id, &run.root_repo_path).await? {
        return Err(anyhow!("planner does not belong to this supervisor repo_ref"));
    }

    run.selected_planner_id = Some(planner_id.to_string());
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;

    let run = load_supervisor_run(state, id).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_planner_selected", "supervisor planner selected").await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

async fn resolve_feature_pool_template_id(
    state: &AppState,
    run: &SupervisorRun,
) -> Result<Uuid> {
    let candidate = supervisor_pool_template_ref(&run.context, "feature")
        .ok_or_else(|| anyhow!("feature pool template is not configured"))?;

    resolve_workflow_template_ref_id(state, &candidate)
        .await?
        .ok_or_else(|| anyhow!("feature pool template '{}' was not found", candidate))
}

async fn archive_feature_pool_work_units_for_feature_ids(
    state: &AppState,
    run: &SupervisorRun,
    feature_ids: &[String],
    reason: &str,
) -> Result<()> {
    if feature_ids.is_empty() {
        return Ok(());
    }

    let feature_ids_json = serde_json::to_string(feature_ids)?;
    let rows = sqlx::query(
        r#"
        SELECT id
        FROM supervisor_work_units
        WHERE supervisor_run_id = ?
          AND kind = 'feature'
          AND feature_id IN (SELECT value FROM json_each(?))
          AND archived_at IS NULL
        "#,
    )
    .bind(run.id.to_string())
    .bind(&feature_ids_json)
    .fetch_all(&state.db)
    .await?;

    for row in rows {
        let work_unit_id: String = row.get("id");
        lifecycle::archive_supervisor_workflow_by_work_unit(
            state,
            run.id,
            &run.root_repo_path,
            &work_unit_id,
            reason,
        )
        .await?;
    }

    Ok(())
}

pub async fn enqueue_supervisor_feature(
    state: &AppState,
    id: Uuid,
    planner_id: String,
    feature_id: String,
) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let planner_id = planner_id.trim().to_string();
    let feature_id = feature_id.trim().to_string();

    if planner_id.is_empty() {
        return Err(anyhow!("planner_id is required"));
    }
    if feature_id.is_empty() {
        return Err(anyhow!("feature_id is required"));
    }

    if !state.planner().belongs_to_scope(&planner_id, &run.root_repo_path).await? {
        return Err(anyhow!("planner does not belong to this supervisor repo_ref"));
    }

    let feature_row = sqlx::query(
        r#"
        SELECT pf.title,
               pf.status,
               pf.locked_supervisor_run_id
        FROM planner_features pf
        WHERE pf.planner_id = ?
          AND pf.id = ?
          AND pf.id NOT LIKE 'manual-%'
          AND COALESCE(pf.status, '') != 'archived'
        LIMIT 1
        "#,
    )
    .bind(&planner_id)
    .bind(&feature_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| anyhow!("feature '{}' was not found in planner '{}'", feature_id, planner_id))?;

    let title: String = feature_row.get("title");
    let status: String = feature_row.get("status");
    let prior_lock_owner = feature_row.get::<Option<String>, _>("locked_supervisor_run_id");

    if status != "fine" {
        return Err(anyhow!(
            "feature '{}' has planner status '{}' and is not available for the supervisor feature pool",
            feature_id,
            status
        ));
    }

    if let Some(owner) = prior_lock_owner
        .as_deref()
        .map(str::trim)
        .filter(|owner| !owner.is_empty() && *owner != run.id.to_string())
    {
        return Err(anyhow!(
            "feature '{}' is already checked out by supervisor '{}'",
            feature_id,
            owner
        ));
    }

    let existing_work_unit = sqlx::query_scalar::<_, String>(
        r#"
        SELECT id
        FROM supervisor_work_units
        WHERE supervisor_run_id = ?
          AND kind = 'feature'
          AND feature_id = ?
          AND archived_at IS NULL
          AND state NOT IN ('deleted', 'archived', 'cancelled')
        LIMIT 1
        "#,
    )
    .bind(run.id.to_string())
    .bind(&feature_id)
    .fetch_optional(&state.db)
    .await?;

    if let Some(work_unit_id) = existing_work_unit {
        return Ok(json!({
            "ok": true,
            "action": "enqueue_feature",
            "reused": true,
            "work_unit_id": work_unit_id,
            "supervisor_run": run
        }));
    }

    let template_id = resolve_feature_pool_template_id(state, &run).await?;
    let queue_position = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COALESCE(MAX(queue_position), -1) + 1
        FROM supervisor_work_units
        WHERE supervisor_run_id = ?
          AND kind = 'feature'
          AND archived_at IS NULL
        "#,
    )
    .bind(run.id.to_string())
    .fetch_one(&state.db)
    .await?;

    let now = Utc::now().to_rfc3339();
    let lock_result = sqlx::query(
        r#"
        UPDATE planner_features
        SET locked_supervisor_run_id = ?,
            locked_at = COALESCE(locked_at, ?),
            updated_at = ?
        WHERE planner_id = ?
          AND id = ?
          AND status = 'fine'
          AND (
              TRIM(COALESCE(locked_supervisor_run_id, '')) = ''
              OR locked_supervisor_run_id = ?
          )
        "#,
    )
    .bind(run.id.to_string())
    .bind(&now)
    .bind(&now)
    .bind(&planner_id)
    .bind(&feature_id)
    .bind(run.id.to_string())
    .execute(&state.db)
    .await?;

    if lock_result.rows_affected() != 1 {
        return Err(anyhow!(
            "failed to acquire planner feature lock for '{}'",
            feature_id
        ));
    }

    let work_unit_id = format!("{}:{}", run.id, feature_id);
    let promise_result = lifecycle::promise_supervisor_work_unit(
        state,
        SupervisorWorkUnitPromiseRequest {
            supervisor_run_id: run.id,
            root_repo_path: run.root_repo_path.clone(),
            pool_kind: SupervisorPoolKind::Feature,
            work_unit_id: work_unit_id.clone(),
            feature_id: Some(feature_id.clone()),
            title,
            template_id: Some(template_id),
            context: SupervisorWorkUnitStoredContext {
                template_id: Some(template_id),
                planned_workflow_template_id: Some(template_id),
                planner_id: Some(planner_id.clone()),
                feature_id: Some(feature_id.clone()),
            },
            priority: 0,
            queue_position: Some(queue_position),
        },
    )
    .await;

    if let Err(err) = promise_result {
        if prior_lock_owner
            .as_deref()
            .map(str::trim)
            .filter(|owner| !owner.is_empty())
            .is_none()
        {
            let _ = sqlx::query(
                r#"
                UPDATE planner_features
                SET locked_supervisor_run_id = NULL,
                    locked_at = NULL,
                    updated_at = ?
                WHERE planner_id = ?
                  AND id = ?
                  AND locked_supervisor_run_id = ?
                "#,
            )
            .bind(Utc::now().to_rfc3339())
            .bind(&planner_id)
            .bind(&feature_id)
            .bind(run.id.to_string())
            .execute(&state.db)
            .await;
        }
        return Err(err);
    }

    run = load_supervisor_run(state, id).await?;
    kick_feature_pool_if_running(state, &mut run).await?;
    let run = load_supervisor_run(state, id).await?;

    Ok(json!({
        "ok": true,
        "action": "enqueue_feature",
        "reused": false,
        "work_unit_id": work_unit_id,
        "planner_id": planner_id,
        "feature_id": feature_id,
        "supervisor_run": run
    }))
}

pub async fn dequeue_supervisor_feature(
    state: &AppState,
    id: Uuid,
    planner_id: String,
    feature_id: String,
) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let planner_id = planner_id.trim().to_string();
    let feature_id = feature_id.trim().to_string();

    if planner_id.is_empty() || feature_id.is_empty() {
        return Err(anyhow!("planner_id and feature_id are required"));
    }

    let work_unit_id = sqlx::query_scalar::<_, String>(
        r#"
        SELECT id
        FROM supervisor_work_units
        WHERE supervisor_run_id = ?
          AND kind = 'feature'
          AND feature_id = ?
          AND archived_at IS NULL
        LIMIT 1
        "#,
    )
    .bind(run.id.to_string())
    .bind(&feature_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| anyhow!("feature '{}' is not queued in this supervisor", feature_id))?;

    let work_unit = load_supervisor_work_unit_row(state, id, &work_unit_id).await?;
    if let Some(work_unit_planner_id) = work_unit.context.planner_id.as_deref() {
        if work_unit_planner_id != planner_id {
            return Err(anyhow!(
                "feature work unit belongs to planner '{}' rather than '{}'",
                work_unit_planner_id,
                planner_id
            ));
        }
    }

    lifecycle::archive_supervisor_workflow_by_work_unit(
        state,
        run.id,
        &run.root_repo_path,
        &work_unit_id,
        "feature removed from supervisor feature pool",
    )
    .await?;

    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE planner_features
        SET locked_supervisor_run_id = NULL,
            locked_at = NULL,
            updated_at = ?
        WHERE planner_id = ?
          AND id = ?
          AND locked_supervisor_run_id = ?
        "#,
    )
    .bind(&now)
    .bind(&planner_id)
    .bind(&feature_id)
    .bind(run.id.to_string())
    .execute(&state.db)
    .await?;

    invalidate_supervisor_integration(state, &mut run).await?;
    run = load_supervisor_run(state, id).await?;
    kick_feature_pool_if_running(state, &mut run).await?;
    let run = load_supervisor_run(state, id).await?;

    Ok(json!({
        "ok": true,
        "action": "dequeue_feature",
        "work_unit_id": work_unit_id,
        "planner_id": planner_id,
        "feature_id": feature_id,
        "supervisor_run": run
    }))
}

pub async fn reorder_supervisor_feature_pool(
    state: &AppState,
    id: Uuid,
    feature_ids: Vec<String>,
) -> Result<Value> {
    let run = load_supervisor_run(state, id).await?;
    let mut seen = HashSet::<String>::new();
    let feature_ids = feature_ids
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && seen.insert(value.clone()))
        .collect::<Vec<_>>();

    let now = Utc::now().to_rfc3339();
    for (index, feature_id) in feature_ids.iter().enumerate() {
        sqlx::query(
            r#"
            UPDATE supervisor_work_units
            SET queue_position = ?,
                updated_at = ?
            WHERE supervisor_run_id = ?
              AND kind = 'feature'
              AND feature_id = ?
              AND archived_at IS NULL
            "#,
        )
        .bind(index as i64)
        .bind(&now)
        .bind(run.id.to_string())
        .bind(feature_id)
        .execute(&state.db)
        .await?;
    }

    Ok(json!({
        "ok": true,
        "action": "reorder_feature_pool",
        "feature_ids": feature_ids,
        "supervisor_run": load_supervisor_run(state, id).await?
    }))
}

pub async fn select_supervisor_feature_pool(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let active_planner_id = run
        .selected_planner_id
        .clone()
        .ok_or_else(|| anyhow!("supervisor has no selected planner"))?;
    let active_planner = state.planner().get(&active_planner_id)
        .await?
        .ok_or_else(|| anyhow!("selected supervisor planner '{}' was not found", active_planner_id))?;

    if !state.planner().belongs_to_scope(&active_planner_id, &run.root_repo_path).await? {
        return Err(anyhow!("selected supervisor planner does not belong to this supervisor repo_ref"));
    }

    if !active_planner.features.is_empty() {
        let mut merged = run.feature_plan_items.clone();
        for feature in active_planner.features.iter().cloned() {
            if let Some(existing) = merged.iter_mut().find(|item| item.id == feature.id) {
                *existing = feature;
            } else {
                merged.push(feature);
            }
        }
        run.feature_plan_items = merged;
    }

    let selected_feature_ids = import_feature_ids(payload.get("feature_ids"));
    let queued_features = selected_feature_ids
        .iter()
        .map(|feature_id| SupervisorQueuedFeatureRef {
            feature_id: feature_id.clone(),
            planner_id: active_planner_id.clone(),
            planner_title: active_planner.title.clone(),
        })
        .collect::<Vec<_>>();

    let existing_rows = sqlx::query(
        r#"
        SELECT id AS feature_id
        FROM planner_features
        WHERE locked_supervisor_run_id = ?
          AND planner_id = ?
          AND COALESCE(status, '') != 'archived'
        "#,
    )
    .bind(run.id.to_string())
    .bind(&active_planner_id)
    .fetch_all(&state.db)
    .await?;

    let existing_queued_feature_ids = existing_rows
        .into_iter()
        .map(|row| row.get::<String, _>("feature_id"))
        .collect::<HashSet<_>>();

    tracing::warn!(
        supervisor_run_id = %run.id,
        active_planner_id = %active_planner_id,
        requested_feature_ids = ?selected_feature_ids,
        existing_planner_locked_feature_ids = ?existing_queued_feature_ids,
        "feature queue mutation received"
    );

    let supervisor_id = run.id.to_string();
    let mut selected_features = HashMap::<String, FeaturePlanItem>::new();

    for queued_feature in &queued_features {
        let row = sqlx::query(
            r#"
            SELECT id, planner_id, title, status, payload_json, locked_supervisor_run_id
            FROM planner_features
            WHERE planner_id = ?
              AND id = ?
              AND id NOT LIKE 'manual-%'
              AND COALESCE(status, '') != 'archived'
            LIMIT 1
            "#,
        )
        .bind(&active_planner_id)
        .bind(&queued_feature.feature_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| {
            anyhow!(
                "feature '{}' was not found in planner '{}'",
                queued_feature.feature_id,
                queued_feature.planner_id
            )
        })?;

        let feature_id: String = row.get("id");
        let title: String = row.get("title");
        let status: String = row.get("status");
        let locked_owner = row.get::<Option<String>, _>("locked_supervisor_run_id");

        if let Some(owner) = locked_owner
            .as_deref()
            .map(str::trim)
            .filter(|owner| !owner.is_empty() && *owner != supervisor_id)
        {
            return Err(anyhow!(
                "feature {} ({}) is already checked out by supervisor {}",
                feature_id,
                title,
                owner
            ));
        }

        if status != "fine" {
            return Err(anyhow!(
                "feature {} has planner status '{}' and is not available for the supervisor feature pool",
                feature_id,
                status
            ));
        }

        let mut feature: FeaturePlanItem = serde_json::from_str(
            row.get::<String, _>("payload_json").as_str(),
        )?;
        feature.id = feature_id.clone();
        feature.title = title;
        if let Ok(parsed_status) = serde_json::from_value::<FeaturePlanItemStatus>(
            Value::String(status),
        ) {
            feature.status = parsed_status;
        }

        selected_features.insert(feature_id, feature);
    }

    for feature_id in &selected_feature_ids {
        let feature = selected_features.remove(feature_id).ok_or_else(|| {
            anyhow!("queued feature '{}' could not be loaded", feature_id)
        })?;

        if let Some(existing) = run
            .feature_plan_items
            .iter_mut()
            .find(|item| item.id == feature.id)
        {
            *existing = feature;
        } else {
            run.feature_plan_items.push(feature);
        }
    }

    let mut previous_feature_ids = existing_queued_feature_ids
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    previous_feature_ids.sort();
    let requested_template_id = if selected_feature_ids.is_empty() {
        None
    } else {
        Some(resolve_feature_pool_template_id(state, &run).await?)
    };

    run.execution_plan_items = selected_feature_ids
        .iter()
        .enumerate()
        .map(|(index, feature_id)| ExecutionPlanItem {
            feature_plan_item_id: feature_id.clone(),
            workflow_template_id: None,
            order_index: Some(index as i64),
        })
        .collect();

    let next_feature_ids = run
        .execution_plan_items
        .iter()
        .map(|item| item.feature_plan_item_id.clone())
        .collect::<Vec<_>>();
    let queued_features = queued_features
        .into_iter()
        .filter(|item| next_feature_ids.iter().any(|feature_id| feature_id == &item.feature_id))
        .collect::<Vec<_>>();
    if let Some(obj) = run.context.as_object_mut() {
        obj.insert("feature_ids".to_string(), serde_json::to_value(&next_feature_ids)?);
        obj.remove("queued_features");
    }

    let now = Utc::now().to_rfc3339();
    let next_json = serde_json::to_string(&next_feature_ids)?;
    let removed_feature_ids = previous_feature_ids
        .iter()
        .filter(|feature_id| !next_feature_ids.iter().any(|next_id| next_id == *feature_id))
        .cloned()
        .collect::<Vec<_>>();

    tracing::warn!(
        supervisor_run_id = %run.id,
        previous_feature_ids = ?previous_feature_ids,
        requested_feature_ids = ?selected_feature_ids,
        resulting_execution_plan_feature_ids = ?next_feature_ids,
        removed_feature_ids = ?removed_feature_ids,
        retained_queued_features = ?queued_features
            .iter()
            .map(|item| item.feature_id.as_str())
            .collect::<Vec<_>>(),
        "feature queue mutation calculated resulting queue"
    );

    for queued_feature in &queued_features {
        let lock_result = sqlx::query(
            r#"
            UPDATE planner_features
            SET locked_supervisor_run_id = ?,
                locked_at = COALESCE(locked_at, ?),
                updated_at = ?
            WHERE planner_id = ?
              AND id = ?
              AND id NOT LIKE 'manual-%'
              AND status = 'fine'
              AND (
                  TRIM(COALESCE(locked_supervisor_run_id, '')) = ''
                  OR locked_supervisor_run_id = ?
              )
            "#,
        )
        .bind(run.id.to_string())
        .bind(&now)
        .bind(&now)
        .bind(&queued_feature.planner_id)
        .bind(&queued_feature.feature_id)
        .bind(run.id.to_string())
        .execute(&state.db)
        .await?;

        if lock_result.rows_affected() != 1 {
            let owner = sqlx::query_scalar::<_, Option<String>>(
                "SELECT locked_supervisor_run_id FROM planner_features WHERE planner_id = ? AND id = ? LIMIT 1",
            )
            .bind(&queued_feature.planner_id)
            .bind(&queued_feature.feature_id)
            .fetch_optional(&state.db)
            .await?
            .flatten();

            return Err(match owner.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
                Some(owner) => anyhow!(
                    "feature '{}' in planner '{}' is already checked out by supervisor '{}'",
                    queued_feature.feature_id,
                    queued_feature.planner_id,
                    owner
                ),
                None => anyhow!(
                    "failed to acquire lock for feature '{}' in planner '{}'",
                    queued_feature.feature_id,
                    queued_feature.planner_id
                ),
            });
        }
    }

    if !next_feature_ids.is_empty() {
        let requested_template_id = requested_template_id.ok_or_else(|| anyhow!("feature pool template is not configured"))?;
        let existing_rows = sqlx::query(
            r#"
            SELECT feature_id, workspace_path
            FROM supervisor_work_units
            WHERE supervisor_run_id = ?
              AND kind = 'feature'
              AND feature_id IN (SELECT value FROM json_each(?))
              AND archived_at IS NULL
              AND TRIM(COALESCE(workspace_path, '')) != ''
            "#,
        )
        .bind(run.id.to_string())
        .bind(&next_json)
        .fetch_all(&state.db)
        .await?;

        let existing_workspace_by_feature_id = existing_rows
            .into_iter()
            .map(|row| (row.get::<String, _>("feature_id"), row.get::<String, _>("workspace_path")))
            .collect::<HashMap<_, _>>();

        let mut materialized_feature_units = Vec::new();
        for (index, feature_id) in next_feature_ids.iter().enumerate() {
            if let Some(existing_workspace_path) = existing_workspace_by_feature_id.get(feature_id) {
                materialized_feature_units.push(json!({
                    "feature_id": feature_id,
                    "queue_position": index,
                    "workspace_path": existing_workspace_path
                }));
                continue;
            }

            materialized_feature_units.push(json!({
                "feature_id": feature_id,
                "queue_position": index
            }));
        }
        let materialized_json = serde_json::to_string(&materialized_feature_units)?;

        let requested_template_id_text = requested_template_id.to_string();
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
                workspace_path,
                priority,
                queue_position,
                blocked_reason,
                waiting_user_input_json,
                context_json,
                created_at,
                updated_at
            )
            SELECT
                ? || ':' || pf.id AS id,
                ? AS supervisor_run_id,
                NULL AS repo_id,
                pf.id AS feature_id,
                NULL AS workflow_run_id,
                NULL AS patch_id,
                'feature' AS kind,
                pf.title,
                'queued' AS state,
                pw.root_repo_path,
                json_extract(json_each.value, '$.workspace_path') AS workspace_path,
                0 AS priority,
                CAST(json_extract(json_each.value, '$.queue_position') AS INTEGER) AS queue_position,
                NULL AS blocked_reason,
                '{}' AS waiting_user_input_json,
                json_object(
                    'source', 'supervisor_feature_queue',
                    'status', pf.status,
                    'development_state', 'queued',
                    'planner_feature_id', pf.id,
                    'planner_id', pf.planner_id,
                    'template_id', ?,
                    'planned_workflow_template_id', ?,
                    'workflow_type', 'feature',
                    'pool_key', 'feature',
                    'planned_workflow', 1
                ) AS context_json,
                ? AS created_at,
                ? AS updated_at
            FROM json_each(?)
            JOIN planner_features pf ON pf.id = json_extract(json_each.value, '$.feature_id')
            JOIN planner_workspaces pw ON pw.id = pf.planner_id
            WHERE COALESCE(pf.completed_at, '') = ''
              AND (TRIM(COALESCE(pf.locked_supervisor_run_id, '')) = '' OR pf.locked_supervisor_run_id = ?)
            ON CONFLICT(id) DO UPDATE SET
                supervisor_run_id = excluded.supervisor_run_id,
                feature_id = excluded.feature_id,
                workflow_run_id = NULL,
                patch_id = NULL,
                kind = excluded.kind,
                title = excluded.title,
                state = 'queued',
                root_repo_path = excluded.root_repo_path,
                workspace_path = NULL,
                shard_id = NULL,
                shard_path = NULL,
                integration_path = NULL,
                priority = excluded.priority,
                queue_position = excluded.queue_position,
                blocked_reason = NULL,
                waiting_user_input_json = '{}',
                context_json = excluded.context_json,
                archived_at = NULL,
                archived_reason = NULL,
                updated_at = excluded.updated_at
            WHERE supervisor_work_units.archived_at IS NOT NULL
               OR supervisor_work_units.state IN ('deleted', 'archived', 'cancelled', 'failed', 'development_failed')
            "#,
        )
        .bind(run.id.to_string())
        .bind(run.id.to_string())
        .bind(requested_template_id_text.as_str())
        .bind(requested_template_id_text.as_str())
        .bind(&now)
        .bind(&now)
        .bind(&materialized_json)
        .bind(run.id.to_string())
        .execute(&state.db)
        .await?;

        sqlx::query(
            r#"
            UPDATE planner_features
            SET locked_supervisor_run_id = ?,
                locked_at = COALESCE(locked_at, ?),
                updated_at = ?
            WHERE id IN (SELECT json_extract(value, '$.feature_id') FROM json_each(?))
              AND (TRIM(COALESCE(locked_supervisor_run_id, '')) = '' OR locked_supervisor_run_id = ?)
            "#,
        )
        .bind(run.id.to_string())
        .bind(&now)
        .bind(&now)
        .bind(&materialized_json)
        .bind(run.id.to_string())
        .execute(&state.db)
        .await?;

        let locked_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM planner_features
            WHERE id IN (SELECT json_extract(value, '$.feature_id') FROM json_each(?))
              AND locked_supervisor_run_id = ?
            "#,
        )
        .bind(&materialized_json)
        .bind(run.id.to_string())
        .fetch_one(&state.db)
        .await?;

        if locked_count != next_feature_ids.len() as i64 {
            archive_feature_pool_work_units_for_feature_ids(
                state,
                &run,
                &next_feature_ids,
                "failed to lock queued planner features",
            )
            .await?;
            return Err(anyhow!("failed to lock all queued planner features after creating supervisor work units"));
        }


    }

    if !removed_feature_ids.is_empty() {
        let removed_json = serde_json::to_string(&removed_feature_ids)?;
        let unlock_result = sqlx::query(
            r#"
            UPDATE planner_features
            SET locked_supervisor_run_id = NULL,
                locked_at = NULL,
                updated_at = ?
            WHERE id IN (SELECT value FROM json_each(?))
              AND locked_supervisor_run_id = ?
            "#,
        )
        .bind(&now)
        .bind(&removed_json)
        .bind(run.id.to_string())
        .execute(&state.db)
        .await?;

        let planner_rows_after_unlock = sqlx::query(
            r#"
            SELECT id, planner_id, status, locked_supervisor_run_id, locked_at, completed_at
            FROM planner_features
            WHERE id IN (SELECT value FROM json_each(?))
            ORDER BY id
            "#,
        )
        .bind(&removed_json)
        .fetch_all(&state.db)
        .await?;

        let planner_state_after_unlock = planner_rows_after_unlock
            .iter()
            .map(|row| {
                json!({
                    "feature_id": row.get::<String, _>("id"),
                    "planner_id": row.get::<String, _>("planner_id"),
                    "status": row.get::<String, _>("status"),
                    "locked_supervisor_run_id": row.get::<Option<String>, _>("locked_supervisor_run_id"),
                    "locked_at": row.get::<Option<String>, _>("locked_at"),
                    "completed_at": row.get::<Option<String>, _>("completed_at")
                })
            })
            .collect::<Vec<_>>();

        tracing::warn!(
            supervisor_run_id = %run.id,
            removed_feature_ids = ?removed_feature_ids,
            planner_unlock_rows_affected = unlock_result.rows_affected(),
            planner_state_after_unlock = ?planner_state_after_unlock,
            "released planner feature locks for dequeue"
        );

        archive_feature_pool_work_units_for_feature_ids(
            state,
            &run,
            &removed_feature_ids,
            "feature removed from supervisor feature pool",
        )
        .await?;

        let remaining_work_units = sqlx::query(
            r#"
            SELECT id, feature_id, workflow_run_id, state, archived_at, archived_reason
            FROM supervisor_work_units
            WHERE supervisor_run_id = ?
              AND kind = 'feature_development'
              AND feature_id IN (SELECT value FROM json_each(?))
            ORDER BY feature_id, created_at
            "#,
        )
        .bind(run.id.to_string())
        .bind(&removed_json)
        .fetch_all(&state.db)
        .await?;

        let remaining_work_unit_state = remaining_work_units
            .iter()
            .map(|row| {
                json!({
                    "work_unit_id": row.get::<String, _>("id"),
                    "feature_id": row.get::<Option<String>, _>("feature_id"),
                    "workflow_run_id": row.get::<Option<String>, _>("workflow_run_id"),
                    "state": row.get::<String, _>("state"),
                    "archived_at": row.get::<Option<String>, _>("archived_at"),
                    "archived_reason": row.get::<Option<String>, _>("archived_reason")
                })
            })
            .collect::<Vec<_>>();

        tracing::warn!(
            supervisor_run_id = %run.id,
            removed_feature_ids = ?removed_feature_ids,
            work_units_after_archive = ?remaining_work_unit_state,
            "completed dequeue work-unit archival"
        );


    }

    if previous_feature_ids != next_feature_ids {
        invalidate_supervisor_integration(state, &mut run).await?;
    }

    if matches!(run.status, SupervisorStatus::RunningChildren) {
        kick_feature_pool_if_running(state, &mut run).await?;
    }

    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    let run = load_supervisor_run(state, id).await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

pub async fn refine_supervisor_feature(
    state: &AppState,
    id: Uuid,
    feature_id: String,
    workflow_template_id: Option<Uuid>,
) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let feature_id = feature_id.trim().to_string();
    if feature_id.is_empty() {
        return Err(anyhow!("feature_id is required"));
    }
    let planner_id = run
        .selected_planner_id
        .clone()
        .ok_or_else(|| anyhow!("supervisor has no selected planner"))?;
    if !state.planner().belongs_to_scope(&planner_id, &run.root_repo_path).await? {
        return Err(anyhow!("selected supervisor planner does not belong to this supervisor repo_ref"));
    }
    let feature = state.planner().feature(&planner_id, &feature_id)
        .await?
        .ok_or_else(|| {
            anyhow!(
                "planner feature '{}' was not found in the supervisor selected planner '{}'",
                feature_id,
                planner_id
            )
        })?;
    let workflow_template_id = workflow_template_id
        .or_else(|| supervisor_pool_template_uuid(&run.context, "refine"));
    let workflow_template_id = match workflow_template_id {
        Some(value) => value,
        None => default_refinement_workflow_template_id(state)
            .await?
            .ok_or_else(|| anyhow!("workflow_template_id is required for feature refinement"))?,
    };
    let work_unit_id = format!("{}:refine:{}", run.id, feature.id);
    lifecycle::promise_supervisor_work_unit(
        state,
        SupervisorWorkUnitPromiseRequest {
            supervisor_run_id: run.id,
            root_repo_path: run.root_repo_path.clone(),
            pool_kind: SupervisorPoolKind::Refine,
            work_unit_id: work_unit_id.clone(),
            feature_id: Some(feature.id.clone()),
            title: feature.title.clone(),
            template_id: Some(workflow_template_id),
            context: SupervisorWorkUnitStoredContext {
                template_id: Some(workflow_template_id),
                planned_workflow_template_id: Some(workflow_template_id),
                planner_id: Some(planner_id.clone()),
                feature_id: Some(feature.id.clone()),
            },
            priority: 0,
            queue_position: None,
        },
    )
    .await?;

    let work_unit = load_supervisor_work_unit_row(state, id, &work_unit_id).await?;
    let workflow_run_id = materialize_supervisor_work_unit(
        state,
        &mut run,
        &work_unit,
    )
    .await?;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;

    Ok(json!({ "ok": true, "workflow_run_id": workflow_run_id, "reused": false }))
}

async fn delete_supervisor_workflow_run_records(state: &AppState, run_id: Uuid) -> Result<()> {
    let run_id_text = run_id.to_string();
    let now = Utc::now().to_rfc3339();

    sqlx::query("UPDATE supervisor_work_units SET workflow_run_id = NULL, updated_at = ? WHERE workflow_run_id = ?")
        .bind(&now)
        .bind(&run_id_text)
        .execute(&state.db)
        .await?;


    sqlx::query("DELETE FROM workflow_events WHERE run_id = ?")
        .bind(&run_id_text)
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM changeset_file_effects WHERE attempt_id IN (SELECT id FROM changeset_attempts WHERE run_id = ?)")
        .bind(&run_id_text)
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM changeset_attempts WHERE run_id = ?")
        .bind(&run_id_text)
        .execute(&state.db)
        .await?;
    sqlx::query("UPDATE planner_feature_patches SET workflow_run_id = NULL WHERE workflow_run_id = ?")
        .bind(&run_id_text)
        .execute(&state.db)
        .await?;

    tracing::info!(workflow_run_id = %run_id_text, "supervisor deleting workflow run records");
    sqlx::query("DELETE FROM workflow_runs WHERE id = ?")
        .bind(&run_id_text)
        .execute(&state.db)
        .await?;
    Ok(())
}





pub async fn delete_supervisor_run(state: &AppState, id: Uuid) -> Result<()> {
    sqlx::query("DELETE FROM supervisor_runs WHERE id = ?")
        .bind(id.to_string())
        .execute(&state.db)
        .await?;

    append_supervisor_event(
        state,
        id,
        "supervisor_deleted",
        "supervisor deleted",
        SupervisorEventPayload {
            supervisor_run_id: id,
            supervisor: None,
            deleted: true,
        },
    )
    .await?;

    Ok(())
}

pub async fn cancel_supervisor_run(state: &AppState, id: Uuid) -> Result<Value> {
    update_status(state, id, SupervisorStatus::Cancelled).await?;
    let run = load_supervisor_run(state, id).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_cancelled", "supervisor cancelled").await?;
    Ok(json!({ "ok": true, "status": "cancelled" }))
}



fn supervisor_context_uuid(context: &Value, key: &str) -> Option<Uuid> {
    context.get(key).and_then(Value::as_str).and_then(|value| Uuid::parse_str(value).ok())
}



fn workflow_terminal_event_message(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Success => "workflow completed",
        RunStatus::Error => "workflow failed",
        RunStatus::Cancelled => "workflow cancelled",
        _ => "workflow reached terminal status",
    }
}

async fn transition_integration_work_unit_terminal(
    state: &AppState,
    supervisor_run_id: Uuid,
    workflow_run_id: Uuid,
    status: &RunStatus,
) -> Result<bool> {
    let work_unit_id = integration_work_unit_id(supervisor_run_id);
    let now = Utc::now().to_rfc3339();
    let (state_value, available) = match status {
        RunStatus::Success => (SupervisorWorkUnitState::Completed.as_str(), true),
        RunStatus::Error => (SupervisorWorkUnitState::Failed.as_str(), false),
        RunStatus::Cancelled => (SupervisorWorkUnitState::Cancelled.as_str(), false),
        _ => return Ok(false),
    };

    let result = sqlx::query(
        r#"
        UPDATE supervisor_work_units
        SET state = ?,
            context_json = json_set(
                CASE
                    WHEN ? THEN json_remove(
                        CASE WHEN json_valid(context_json) THEN context_json ELSE '{}' END,
                        '$.applied_at'
                    )
                    WHEN json_valid(context_json) THEN context_json
                    ELSE '{}'
                END,
                '$.integration_apply_available', json(?)
            ),
            updated_at = ?
        WHERE id = ?
          AND supervisor_run_id = ?
          AND workflow_run_id = ?
          AND archived_at IS NULL
        "#,
    )
    .bind(state_value)
    .bind(available)
    .bind(if available { "true" } else { "false" })
    .bind(&now)
    .bind(&work_unit_id)
    .bind(supervisor_run_id.to_string())
    .bind(workflow_run_id.to_string())
    .execute(&state.db)
    .await?;

    if result.rows_affected() != 1 {
        return Err(anyhow!("integration workflow terminal transition did not resolve its integration work unit"));
    }

    Ok(available)
}

pub async fn handle_workflow_terminal_event(state: &AppState, workflow_run_id: Uuid, status: RunStatus, current_step_id: Option<&str>) -> Result<()> {
    let workflow_run = engine::load_run(state, workflow_run_id).await?;
    let supervisor_context = workflow_run.context.get("supervisor").cloned().unwrap_or_else(|| json!({}));
    let Some(supervisor_id) = supervisor_context_uuid(&supervisor_context, "supervisor_run_id")
        .or_else(|| supervisor_context_uuid(&supervisor_context, "supervisor_id"))
    else {
        return Ok(());
    };

    engine::append_engine_event(
        state,
        workflow_run_id,
        current_step_id,
        "info",
        "supervisor.workflow_terminal",
        workflow_terminal_event_message(&status),
        json!({
            "supervisor_run_id": supervisor_id,
            "feature_id": supervisor_context.get("feature_id").cloned().unwrap_or(Value::Null),
            "input_source": supervisor_context.get("input_source").cloned().unwrap_or(Value::Null),
            "workflow_status": status_str(&status)
        }),
    ).await?;

    let mut run = load_supervisor_run(state, supervisor_id).await?;
    let pool_type = supervisor_context
        .get("pool_type")
        .or_else(|| supervisor_context.get("pool_key"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let is_integration_workflow =
        run.integration_run_id == Some(workflow_run_id) || pool_type == "integration";

    if !is_integration_workflow {
        if matches!(run.status, SupervisorStatus::RunningChildren) {
            start_next_feature_pool_work_units(state, &mut run).await?;
        }
        run.updated_at = Utc::now();
        update_supervisor_run(state, &run).await?;
        publish_supervisor_snapshot(
            state,
            &run,
            "supervisor_snapshot",
            "workflow terminal event processed",
        )
        .await?;
        return Ok(());
    }


    if matches!(status, RunStatus::Success | RunStatus::Error | RunStatus::Cancelled) {
        let available = transition_integration_work_unit_terminal(
            state,
            supervisor_id,
            workflow_run_id,
            &status,
        )
        .await?;

        run.status = if available {
            SupervisorStatus::ReadyToApply
        } else {
            SupervisorStatus::Failed
        };
        run.updated_at = Utc::now();

        update_supervisor_run(state, &run).await?;

        publish_supervisor_snapshot(
            state,
            &run,
            if available { "integration_available" } else { "supervisor_snapshot" },
            "integration workflow terminal event processed",
        )
        .await?;
    }

    Ok(())
}









pub async fn pause_supervisor_feature_pool(state: &AppState, id: Uuid, _payload: Value) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    run.status = SupervisorStatus::Paused;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "feature pool paused; active child workflows were not interrupted").await?;

    Ok(json!({
        "ok": true,
        "paused": true,
        "interrupts_children": false,
        "supervisor_run": run
    }))
}

pub async fn resume_supervisor_feature_pool(state: &AppState, id: Uuid, _payload: Value) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    run.status = SupervisorStatus::RunningChildren;
    kick_feature_pool_if_running(state, &mut run).await?;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "feature pool resumed").await?;
    let run = load_supervisor_run(state, id).await?;

    Ok(json!({
        "ok": true,
        "paused": false,
        "supervisor_run": run
    }))
}

fn action_pool_kind_to_lifecycle(kind: SupervisorWorkPoolKind) -> SupervisorPoolKind {
    match kind {
        SupervisorWorkPoolKind::Refine => SupervisorPoolKind::Refine,
        SupervisorWorkPoolKind::Feature => SupervisorPoolKind::Feature,
        SupervisorWorkPoolKind::Manual => SupervisorPoolKind::Manual,
        SupervisorWorkPoolKind::Integration => SupervisorPoolKind::Integration,
    }
}

fn pool_key_from_action_kind(kind: SupervisorWorkPoolKind) -> &'static str {
    kind.as_str()
}

async fn load_supervisor_work_unit_row(state: &AppState, supervisor_id: Uuid, work_unit_id: &str) -> Result<SupervisorWorkUnitRecord> {
    let row = sqlx::query(
        r#"
        SELECT id, kind, feature_id, title, workflow_run_id, workspace_path, integration_state, context_json, state, queue_position
        FROM supervisor_work_units
        WHERE supervisor_run_id = ? AND id = ? AND state != 'archived' AND archived_at IS NULL
        LIMIT 1
        "#,
    )
    .bind(supervisor_id.to_string())
    .bind(work_unit_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| anyhow!("work unit not found"))?;

    let kind_text: String = row.try_get("kind")?;
    let state_text: String = row.try_get("state")?;
    let integration_state_text: String = row.try_get("integration_state")?;
    let workflow_run_id = row
        .try_get::<Option<String>, _>("workflow_run_id")?
        .filter(|value| !value.trim().is_empty())
        .map(|value| Uuid::parse_str(&value))
        .transpose()?;
    let context_json: String = row.try_get("context_json")?;

    Ok(SupervisorWorkUnitRecord {
        id: row.try_get("id")?,
        kind: SupervisorWorkPoolKind::try_from(kind_text.as_str()).map_err(anyhow::Error::msg)?,
        feature_id: row.try_get("feature_id")?,
        title: row.try_get("title")?,
        workflow_run_id,
        workspace_path: row.try_get::<Option<String>, _>("workspace_path")?.map(PathBuf::from),
        integration_state: IntegrationInputState::try_from(integration_state_text.as_str()).map_err(anyhow::Error::msg)?,
        state: SupervisorWorkUnitState::try_from(state_text.as_str()).map_err(anyhow::Error::msg)?,
        context: serde_json::from_str::<SupervisorWorkUnitStoredContext>(&context_json)?,
        queue_position: row.try_get("queue_position")?,
    })
}

async fn load_supervisor_integration_candidate(
    state: &AppState,
    supervisor_id: Uuid,
    work_unit_id: &str,
) -> Result<SupervisorIntegrationCandidate> {
    let row = sqlx::query(
        r#"
        SELECT id, kind, workspace_path, integration_state
        FROM supervisor_work_units
        WHERE supervisor_run_id = ?
          AND id = ?
          AND archived_at IS NULL
        LIMIT 1
        "#,
    )
    .bind(supervisor_id.to_string())
    .bind(work_unit_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| anyhow!("work unit not found"))?;

    let kind_text: String = row.try_get("kind")?;
    let integration_state_text: String = row.try_get("integration_state")?;

    Ok(SupervisorIntegrationCandidate {
        id: row.try_get("id")?,
        kind: SupervisorWorkPoolKind::try_from(kind_text.as_str()).map_err(anyhow::Error::msg)?,
        workspace_path: row.try_get::<Option<String>, _>("workspace_path")?.map(PathBuf::from),
        integration_state: IntegrationInputState::try_from(integration_state_text.as_str()).map_err(anyhow::Error::msg)?,
    })
}

async fn supervisor_work_unit_materialization_item(
    state: &AppState,
    run: &SupervisorRun,
    work_unit: &SupervisorWorkUnitRecord,
) -> Result<FeaturePlanItem> {
    if let Some(feature_id) = work_unit.feature_id.as_deref().filter(|value| !value.trim().is_empty()) {
        if let Some(feature) = run.feature_plan_items.iter().find(|item| item.id == feature_id) {
            return Ok(feature.clone());
        }

        if matches!(work_unit.kind, SupervisorWorkPoolKind::Refine | SupervisorWorkPoolKind::Feature) {
            let planner_id = match work_unit.kind {
                SupervisorWorkPoolKind::Feature => work_unit
                    .context
                    .planner_id
                    .as_deref()
                    .ok_or_else(|| anyhow!("feature work unit has no planner_id"))?,
                SupervisorWorkPoolKind::Refine => work_unit
                    .context
                    .planner_id
                    .as_deref()
                    .or(run.selected_planner_id.as_deref())
                    .ok_or_else(|| anyhow!("refine work unit has no planner_id"))?,
                _ => unreachable!(),
            };
            if let Some(feature) = state.planner().feature(planner_id, feature_id).await? {
                return Ok(feature);
            }
        }
    }

    let feature_id = work_unit
        .feature_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| work_unit.id.clone());

    Ok(FeaturePlanItem {
        id: feature_id,
        title: work_unit.title.clone(),
        status: FeaturePlanItemStatus::Scheduled,
        summary: work_unit.title.clone(),
        rough_summary: None,
        refinement_workflow_run_id: None,
        applied_at: None,
        requirements: Vec::new(),
        acceptance_criteria: Vec::new(),
        implementation_notes: Vec::new(),
        review_expectations: Vec::new(),
        target_files_or_areas: Vec::new(),
        dependencies: Vec::new(),
    })
}

async fn supervisor_work_unit_template_id(
    state: &AppState,
    run: &SupervisorRun,
    work_unit: &SupervisorWorkUnitRecord,
) -> Result<Uuid> {
    if let Some(template_id) = work_unit
        .context
        .workflow_template_id()
        .or_else(|| supervisor_pool_template_uuid(&run.context, work_unit.kind.as_str()))
    {
        return Ok(template_id);
    }

    if work_unit.kind == SupervisorWorkPoolKind::Refine {
        return default_refinement_workflow_template_id(state)
            .await?
            .ok_or_else(|| anyhow!("workflow template is required for refine work unit"));
    }

    Err(anyhow!(
        "workflow template is required for {} work unit",
        work_unit.kind.as_str()
    ))
}

async fn supervisor_work_unit_workflow_context(
    state: &AppState,
    run: &SupervisorRun,
    work_unit: &SupervisorWorkUnitRecord,
    template_id: Uuid,
) -> Result<Value> {
    let workspace = repo_snapshot::workspace_for(&run.root_repo_path, run.id)?;
    let mut context = supervisor_context(run, &workspace);
    let obj = context
        .as_object_mut()
        .ok_or_else(|| anyhow!("supervisor workflow context must be an object"))?;

    obj.insert(
        "input_source".to_string(),
        Value::String("supervisor_work_unit".to_string()),
    );
    obj.insert(
        "workflow_type".to_string(),
        Value::String(work_unit.kind.as_str().to_string()),
    );
    obj.insert(
        "pool_key".to_string(),
        Value::String(work_unit.kind.as_str().to_string()),
    );
    obj.insert(
        "work_unit_id".to_string(),
        Value::String(work_unit.id.clone()),
    );
    obj.insert(
        "template_id".to_string(),
        Value::String(template_id.to_string()),
    );

    if let Some(feature_id) = work_unit.feature_id.as_deref().filter(|value| !value.trim().is_empty()) {
        obj.insert(
            "feature_id".to_string(),
            Value::String(feature_id.to_string()),
        );
    }

    match work_unit.kind {
        SupervisorWorkPoolKind::Refine | SupervisorWorkPoolKind::Feature => {
            let planner_id = work_unit
                .context
                .planner_id
                .clone()
                .or_else(|| {
                    if work_unit.kind == SupervisorWorkPoolKind::Refine {
                        run.selected_planner_id.clone()
                    } else {
                        None
                    }
                })
                .ok_or_else(|| anyhow!("planner-backed work unit has no planner_id"))?;
            obj.insert("planner_id".to_string(), Value::String(planner_id));
        }
        SupervisorWorkPoolKind::Integration => {
            let inputs = load_supervisor_integration_inputs(state, run.id).await?;
            if inputs.is_empty() {
                return Err(anyhow!("integration has no included work-unit inputs"));
            }

            obj.insert(
                "pool_type".to_string(),
                Value::String(SupervisorWorkPoolKind::Integration.as_str().to_string()),
            );
            obj.insert(
                "integration_inputs".to_string(),
                serde_json::to_value(inputs)?,
            );
        }
        SupervisorWorkPoolKind::Manual => {}
    }

    Ok(context)
}

async fn after_supervisor_work_unit_materialized(
    state: &AppState,
    run: &mut SupervisorRun,
    work_unit: &SupervisorWorkUnitRecord,
    workflow_run_id: Uuid,
    workspace_path: &str,
) -> Result<()> {
    match work_unit.kind {
        SupervisorWorkPoolKind::Feature => {
            if let Some(feature_id) = work_unit.feature_id.as_deref().filter(|value| !value.trim().is_empty()) {
                let planner_id = work_unit
                    .context
                    .planner_id
                    .as_deref()
                    .ok_or_else(|| anyhow!("feature work unit has no planner_id"))?;
                let now = Utc::now().to_rfc3339();
                sqlx::query("UPDATE planner_features SET locked_supervisor_run_id = COALESCE(NULLIF(locked_supervisor_run_id, ''), ?), locked_at = COALESCE(locked_at, ?), completed_at = NULL, updated_at = ? WHERE planner_id = ? AND id = ?")
                    .bind(run.id.to_string())
                    .bind(&now)
                    .bind(&now)
                    .bind(planner_id)
                    .bind(feature_id)
                    .execute(&state.db)
                    .await?;
            }
        }
        SupervisorWorkPoolKind::Refine => {
            let feature_id = work_unit
                .feature_id
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| anyhow!("refine work unit has no feature_id"))?;
            let planner_id = work_unit
                .context
                .planner_id
                .clone()
                .or_else(|| run.selected_planner_id.clone())
                .ok_or_else(|| anyhow!("refine work unit has no planner_id"))?;
            state
                .planner()
                .set_refinement_workflow_run(
                    &planner_id,
                    feature_id,
                    workflow_run_id,
                )
                .await?;
        }
        SupervisorWorkPoolKind::Integration => {
            run.integration_run_id = Some(workflow_run_id);
            run.integration_path = Some(workspace_path.to_string());
            run.final_patch_path = None;
        }
        SupervisorWorkPoolKind::Manual => {}
    }

    run.updated_at = Utc::now();
    update_supervisor_run(state, run).await?;
    Ok(())
}

async fn materialize_supervisor_work_unit(
    state: &AppState,
    run: &mut SupervisorRun,
    work_unit: &SupervisorWorkUnitRecord,
) -> Result<Uuid> {
    if let Some(workflow_run_id) = work_unit.workflow_run_id {
        return Ok(workflow_run_id);
    }

    let claim_time = Utc::now().to_rfc3339();
    let claim = sqlx::query(
        r#"
        UPDATE supervisor_work_units
        SET context_json = json_set(
                CASE WHEN json_valid(context_json) THEN context_json ELSE '{}' END,
                '$.materialization_state', 'materializing',
                '$.materialization_started_at', ?
            ),
            blocked_reason = NULL,
            updated_at = ?
        WHERE id = ?
          AND supervisor_run_id = ?
          AND workflow_run_id IS NULL
          AND archived_at IS NULL
          AND state IN ('draft', 'queued', 'paused', 'waiting', 'failed')
        "#,
    )
    .bind(&claim_time)
    .bind(&claim_time)
    .bind(&work_unit.id)
    .bind(run.id.to_string())
    .execute(&state.db)
    .await?;

    if claim.rows_affected() == 0 {
        let current = load_supervisor_work_unit_row(state, run.id, &work_unit.id).await?;
        if let Some(workflow_run_id) = current.workflow_run_id {
            return Ok(workflow_run_id);
        }
        return Err(anyhow!("work unit cannot be materialized from its current state"));
    }

    let materializing_run = load_supervisor_run(state, run.id).await?;
    publish_supervisor_snapshot(
        state,
        &materializing_run,
        "work_unit_materializing",
        "work unit materialization started",
    )
    .await?;

    let prepared = async {
        let template_id = supervisor_work_unit_template_id(state, run, work_unit).await?;
        let item = supervisor_work_unit_materialization_item(state, run, work_unit).await?;
        let workflow_context = supervisor_work_unit_workflow_context(
            state,
            run,
            work_unit,
            template_id,
        )
        .await?;

        Ok::<_, anyhow::Error>((template_id, item, workflow_context))
    }
    .await;

    let (template_id, item, workflow_context) = match prepared {
        Ok(prepared) => prepared,
        Err(err) => {
            let failure_time = Utc::now().to_rfc3339();
            let failure_message = format!("{:#}", err);
            sqlx::query(
                r#"
                UPDATE supervisor_work_units
                SET state = 'failed',
                    blocked_reason = ?,
                    context_json = json_set(
                        CASE WHEN json_valid(context_json) THEN context_json ELSE '{}' END,
                        '$.materialization_state', 'failed',
                        '$.materialization_failed_at', ?
                    ),
                    updated_at = ?
                WHERE id = ?
                  AND supervisor_run_id = ?
                  AND workflow_run_id IS NULL
                "#,
            )
            .bind(&failure_message)
            .bind(&failure_time)
            .bind(&failure_time)
            .bind(&work_unit.id)
            .bind(run.id.to_string())
            .execute(&state.db)
            .await?;

            return Err(err);
        }
    };

    let spawn_result = match lifecycle::spawn_supervisor_workflow(
        state,
        SupervisorWorkflowSpawnRequest {
            supervisor_run_id: run.id,
            root_repo_path: run.root_repo_path.clone(),
            pool_kind: action_pool_kind_to_lifecycle(work_unit.kind),
            work_unit_id: work_unit.id.clone(),
            shard_id: None,
            feature_id: work_unit.feature_id.clone(),
            title: work_unit.title.clone(),
            item,
            template_id: Some(template_id),
            workflow_context,
            work_unit_context: serde_json::to_value(&work_unit.context)?,
            initial_state: SupervisorWorkUnitState::Queued,
            priority: 0,
            queue_position: work_unit.queue_position,
        },
    )
    .await
    {
        Ok(result) => result,
        Err(err) => {
            let failure_time = Utc::now().to_rfc3339();
            let failure_message = format!("{:#}", err);
            sqlx::query(
                r#"
                UPDATE supervisor_work_units
                SET state = 'failed',
                    blocked_reason = ?,
                    context_json = json_set(
                        CASE WHEN json_valid(context_json) THEN context_json ELSE '{}' END,
                        '$.materialization_state', 'failed',
                        '$.materialization_failed_at', ?
                    ),
                    updated_at = ?
                WHERE id = ?
                  AND supervisor_run_id = ?
                  AND workflow_run_id IS NULL
                "#,
            )
            .bind(&failure_message)
            .bind(&failure_time)
            .bind(&failure_time)
            .bind(&work_unit.id)
            .bind(run.id.to_string())
            .execute(&state.db)
            .await?;

            return Err(err);
        }
    };

    after_supervisor_work_unit_materialized(
        state,
        run,
        work_unit,
        spawn_result.workflow_run_id,
        &spawn_result.workspace_path,
    )
    .await?;

    Ok(spawn_result.workflow_run_id)
}

fn spawn_supervisor_work_unit_materialization(
    state: &AppState,
    supervisor_run_id: Uuid,
    work_unit_id: String,
) {
    let materialization_state = state.clone();

    tokio::spawn(async move {
        let result: Result<()> = async {
            let mut run = load_supervisor_run(&materialization_state, supervisor_run_id).await?;
            let work_unit = load_supervisor_work_unit_row(
                &materialization_state,
                supervisor_run_id,
                &work_unit_id,
            )
            .await?;

            materialize_supervisor_work_unit(
                &materialization_state,
                &mut run,
                &work_unit,
            )
            .await?;

            let refreshed_run = load_supervisor_run(
                &materialization_state,
                supervisor_run_id,
            )
            .await?;
            publish_supervisor_snapshot(
                &materialization_state,
                &refreshed_run,
                "supervisor_snapshot",
                "work unit materialized",
            )
            .await?;
            Ok(())
        }
        .await;

        if let Err(err) = result {
            tracing::error!(
                supervisor_run_id = %supervisor_run_id,
                work_unit_id = %work_unit_id,
                error = %format!("{:#}", err),
                "supervisor work unit materialization failed"
            );

            if let Ok(run) = load_supervisor_run(&materialization_state, supervisor_run_id).await {
                let _ = publish_supervisor_snapshot(
                    &materialization_state,
                    &run,
                    "supervisor_snapshot",
                    "work unit materialization failed",
                )
                .await;
            }
        }
    });
}

pub async fn create_supervisor_work_unit(state: &AppState, id: Uuid, request: CreateSupervisorWorkUnitRequest) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let pool_key = pool_key_from_action_kind(request.pool_kind);
    let template_id = request
        .template_id
        .or_else(|| supervisor_pool_template_uuid(&run.context, pool_key))
        .ok_or_else(|| anyhow!("work unit workflow template is required"))?;
    let feature_id = request
        .feature_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{}-{}", pool_key.replace('_', "-"), Uuid::new_v4()));
    let title = if request.name.trim().is_empty() {
        format!("{} {}", pool_key.replace('_', " "), feature_id)
    } else {
        request.name.trim().to_string()
    };

    let work_unit_id = format!("{}:{}:{}", run.id, pool_key, feature_id);

    let promised_pool_kind = action_pool_kind_to_lifecycle(request.pool_kind);
    lifecycle::promise_supervisor_work_unit(
        state,
        SupervisorWorkUnitPromiseRequest {
            supervisor_run_id: run.id,
            root_repo_path: run.root_repo_path.clone(),
            pool_kind: promised_pool_kind,
            work_unit_id: work_unit_id.clone(),
            feature_id: Some(feature_id.clone()),
            title: title.clone(),
            template_id: Some(template_id),
            context: SupervisorWorkUnitStoredContext {
                template_id: Some(template_id),
                planned_workflow_template_id: Some(template_id),
                feature_id: Some(feature_id.clone()),
                ..Default::default()
            },
            priority: 0,
            queue_position: None,
        },
    )
    .await?;

    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(
        state,
        &run,
        "supervisor_snapshot",
        "work unit queued for materialization",
    )
    .await?;

    spawn_supervisor_work_unit_materialization(state, run.id, work_unit_id.clone());

    Ok(json!({
        "ok": true,
        "action": "create_work_unit",
        "work_unit_id": work_unit_id,
        "feature_id": feature_id,
        "workflow_run_id": null,
        "state": "queued",
        "materialization_state": "pending",
        "supervisor_run": run
    }))
}

pub async fn delete_supervisor_work_unit(state: &AppState, id: Uuid, work_unit_id: String) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let row = load_supervisor_work_unit_row(state, id, &work_unit_id).await?;
    let kind = row.kind;
    let feature_id = row.feature_id.clone();
    let workflow_run_id = row.workflow_run_id;
    let workspace_path = row.workspace_path.clone();

    if let Some(workflow_run_id) = workflow_run_id {
        delete_supervisor_workflow_run_records(state, workflow_run_id).await?;
        if run.integration_run_id == Some(workflow_run_id) {
            run.integration_run_id = None;
            run.integration_path = None;
            run.final_patch_path = None;
            run.merge_report = json!({});
            run.validation_report = json!({});
        }
    }

    if let Some(path) = workspace_path.as_deref().filter(|value| !value.as_os_str().is_empty()) {
        let path_text = path.to_string_lossy();
        repo_snapshot::delete_supervisor_workspace_path(&run.root_repo_path, run.id, path_text.as_ref())?;
    }

    let archived_at = Utc::now().to_rfc3339();
    sqlx::query("UPDATE supervisor_work_units SET state = 'archived', archived_at = ?, archived_reason = 'work unit deleted', updated_at = ? WHERE id = ?")
        .bind(&archived_at)
        .bind(&archived_at)
        .bind(&work_unit_id)
        .execute(&state.db)
        .await?;

    if let Some(feature_id) = feature_id.as_deref() {
        if kind == SupervisorWorkPoolKind::Refine {
            if let Some(feature_item) = run.feature_plan_items.iter_mut().find(|item| item.id == feature_id) {
                feature_item.refinement_workflow_run_id = None;
            }
        } else {
            sqlx::query("UPDATE planner_features SET locked_supervisor_run_id = NULL, locked_at = NULL, updated_at = ? WHERE id = ? AND locked_supervisor_run_id = ?")
                .bind(&archived_at)
                .bind(feature_id)
                .bind(id.to_string())
                .execute(&state.db)
                .await?;

            run.execution_plan_items.retain(|item| item.feature_plan_item_id != feature_id);
            if let Some(obj) = run.context.as_object_mut() {
                if let Some(queued_features_value) = obj.get_mut("queued_features") {
                    if let Some(rows) = queued_features_value.as_array_mut() {
                        rows.retain(|item| item.get("feature_id").and_then(Value::as_str) != Some(feature_id));
                    }
                }
                if let Some(queued_feature_ids_value) = obj.get_mut("queued_feature_ids") {
                    if let Some(rows) = queued_feature_ids_value.as_array_mut() {
                        rows.retain(|item| item.as_str() != Some(feature_id));
                    }
                }
            }
        }
    }

    if kind != SupervisorWorkPoolKind::Refine {
        invalidate_supervisor_integration(state, &mut run).await?;
    }
    let result_action = if kind == SupervisorWorkPoolKind::Feature { "unqueue_work_unit" } else { "delete_work_unit" };
    let snapshot_message = if kind == SupervisorWorkPoolKind::Feature { "work unit unqueued" } else { "work unit deleted" };
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", snapshot_message).await?;

    Ok(json!({ "ok": true, "action": result_action, "work_unit_id": work_unit_id, "kind": kind.as_str(), "supervisor_run": run }))
}

pub async fn regenerate_supervisor_work_unit(state: &AppState, id: Uuid, work_unit_id: String) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let work_unit = load_supervisor_work_unit_row(state, id, &work_unit_id).await?;

    if matches!(work_unit.kind, SupervisorWorkPoolKind::Refine | SupervisorWorkPoolKind::Feature)
        && run.selected_planner_id.is_none()
    {
        return Err(anyhow!("planner-backed work unit requires a selected supervisor planner"));
    }

    if let Some(workflow_run_id) = work_unit.workflow_run_id {
        delete_supervisor_workflow_run_records(state, workflow_run_id).await?;
    }

    if let Some(workspace_path) = work_unit.workspace_path.as_deref().filter(|value| !value.as_os_str().is_empty()) {
        let workspace_path_text = workspace_path.to_string_lossy();
        repo_snapshot::delete_supervisor_workspace_path(&run.root_repo_path, run.id, workspace_path_text.as_ref())?;
    }

    if work_unit.kind == SupervisorWorkPoolKind::Refine {
        if let Some(feature_id) = work_unit.feature_id.as_deref() {
            if let Some(feature_item) = run.feature_plan_items.iter_mut().find(|item| item.id == feature_id) {
                feature_item.refinement_workflow_run_id = None;
            }
        }
    }

    if work_unit.kind == SupervisorWorkPoolKind::Integration {
        run.integration_run_id = None;
        run.integration_path = None;
        run.final_patch_path = None;
        run.merge_report = json!({});
        run.validation_report = json!({});
    }

    if work_unit.kind == SupervisorWorkPoolKind::Integration {
        sqlx::query(
            r#"
            UPDATE supervisor_work_units
            SET context_json = json_remove(
                    CASE WHEN json_valid(context_json) THEN context_json ELSE '{}' END,
                    '$.applied_at',
                    '$.final_patch_ref',
                    '$.final_patch_text',
                    '$.final_patch_hash',
                    '$.final_patch_bytes',
                    '$.merge_report',
                    '$.integration_apply_available'
                ),
                updated_at = ?
            WHERE id = ?
              AND supervisor_run_id = ?
              AND kind = 'integration'
              AND archived_at IS NULL
            "#,
        )
        .bind(Utc::now().to_rfc3339())
        .bind(&work_unit_id)
        .bind(id.to_string())
        .execute(&state.db)
        .await?;
    }

    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE supervisor_work_units
        SET workflow_run_id = NULL,
            patch_id = NULL,
            workspace_path = NULL,
            integration_state = 'available',
            state = 'queued',
            blocked_reason = NULL,
            context_json = json_set(
                CASE WHEN json_valid(context_json) THEN context_json ELSE '{}' END,
                '$.materialization_state', 'pending',
                '$.regenerated_at', ?
            ),
            updated_at = ?
        WHERE id = ?
          AND supervisor_run_id = ?
        "#,
    )
    .bind(&now)
    .bind(&now)
    .bind(&work_unit_id)
    .bind(id.to_string())
    .execute(&state.db)
    .await?;

    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;

    let refreshed = load_supervisor_work_unit_row(state, id, &work_unit_id).await?;
    publish_supervisor_snapshot(
        state,
        &run,
        "work_unit_regeneration_queued",
        "work unit queued for regeneration",
    )
    .await?;

    let workflow_run_id = materialize_supervisor_work_unit(state, &mut run, &refreshed).await?;

    publish_supervisor_snapshot(state, &run, "work_unit_regenerated", "work unit regenerated").await?;

    Ok(json!({
        "ok": true,
        "action": "regenerate_work_unit",
        "work_unit_id": work_unit_id,
        "kind": refreshed.kind.as_str(),
        "state": SupervisorWorkUnitState::Queued.as_str(),
        "workflow_run_id": workflow_run_id,
        "supervisor_run": run
    }))
}

pub async fn start_supervisor_work_unit(state: &AppState, id: Uuid, work_unit_id: String) -> Result<Value> {
    if work_unit_id.trim().is_empty() {
        return Err(anyhow!("work_unit_id is required"));
    }

    let mut run = load_supervisor_run(state, id).await?;
    let work_unit = load_supervisor_work_unit_row(state, id, &work_unit_id).await?;
    let workflow_run_id = materialize_supervisor_work_unit(
        state,
        &mut run,
        &work_unit,
    )
    .await?;

    let child_run = engine::load_run(state, workflow_run_id).await?;
    let waiting_on_operator_checkpoint = child_run
        .context
        .get("workflow_engine")
        .and_then(|value| value.get("run_state"))
        .and_then(|value| value.get("blocked_on"))
        .and_then(|value| value.get("kind"))
        .and_then(Value::as_str)
        == Some("operator_checkpoint");

    let start_result = if waiting_on_operator_checkpoint {
        json!({
            "ok": false,
            "status": "waiting",
            "blocked_on": "operator_checkpoint",
            "workflow_run_id": workflow_run_id,
            "message": "The workflow is waiting on an operator checkpoint and was not restarted."
        })
    } else if matches!(child_run.status, RunStatus::Waiting | RunStatus::Paused) {
        crate::engine::workflow_lifecycle::execute_workflow_command_value(
            state,
            workflow_run_id,
            crate::engine::workflow_lifecycle::WorkflowCommand::Resume,
        )
        .await?
    } else if matches!(child_run.status, RunStatus::Queued | RunStatus::Running) {
        json!({ "ok": true, "already_running": true })
    } else {
        crate::engine::workflow_lifecycle::execute_workflow_command_value(
            state,
            workflow_run_id,
            crate::engine::workflow_lifecycle::WorkflowCommand::Start {
                mode: crate::engine::workflow_lifecycle::WorkflowExecutionMode::MultiStage,
                step_id: None,
            },
        )
        .await?
    };

    let workflow_status = engine::load_run(state, workflow_run_id).await?.status;

    match work_unit.kind {
        SupervisorWorkPoolKind::Integration => {
            run.status = SupervisorStatus::RunningIntegration;
            run.integration_run_id = Some(workflow_run_id);
        }
        SupervisorWorkPoolKind::Feature => {
            run.status = SupervisorStatus::RunningChildren;
        }
        SupervisorWorkPoolKind::Refine | SupervisorWorkPoolKind::Manual => {}
    }

    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(
        state,
        &run,
        "supervisor_snapshot",
        "work unit start requested",
    )
    .await?;

    Ok(json!({
        "ok": true,
        "action": "start_work_unit",
        "work_unit_id": work_unit_id,
        "workflow_run_id": workflow_run_id,
        "workflow_status": status_str(&workflow_status),
        "start_result": start_result,
        "supervisor_run": run
    }))
}

pub async fn pause_supervisor_work_unit(state: &AppState, id: Uuid, work_unit_id: String) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let row = load_supervisor_work_unit_row(state, id, &work_unit_id).await?;
    let kind = row.kind;
    let workflow_run_id = row.workflow_run_id;

    let pause_result = if let Some(workflow_run_id) = workflow_run_id {
        crate::engine::workflow_lifecycle::execute_workflow_command_value(
            state,
            workflow_run_id,
            crate::engine::workflow_lifecycle::WorkflowCommand::Pause,
        )
        .await?
    } else {
        json!({ "ok": true, "paused": true, "workflow_run_id": null })
    };

    if workflow_run_id.is_none() {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE supervisor_work_units SET state = ?, updated_at = ? WHERE id = ?")
            .bind(SupervisorWorkUnitState::Paused.as_str())
            .bind(&now)
            .bind(&work_unit_id)
            .execute(&state.db)
            .await?;
    }

    if kind == SupervisorWorkPoolKind::Integration {
        run.status = SupervisorStatus::DevelopmentComplete;
    }
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "work unit pause requested").await?;

    Ok(json!({ "ok": true, "action": "pause_work_unit", "work_unit_id": work_unit_id, "pause_result": pause_result, "supervisor_run": run }))
}

pub async fn stage_supervisor_work_unit(state: &AppState, id: Uuid, work_unit_id: String, staged: bool) -> Result<Value> {
    let work_unit = load_supervisor_integration_candidate(state, id, &work_unit_id).await?;
    if !work_unit.supports_integration_input() {
        return Err(anyhow!("work unit kind cannot be used as an integration input"));
    }

    let integration_state = if staged {
        let workspace_path = work_unit
            .workspace_path
            .as_deref()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or_else(|| anyhow!("work unit has no workspace"))?;
        let change_status = patches::change_status(workspace_path)?;
        if !work_unit.can_stage_to_integration(&change_status) {
            return Err(anyhow!("work unit has no staged changes to add to integration or is already an integration input"));
        }
        IntegrationInputState::Included
    } else {
        if !work_unit.can_unstage_from_integration() {
            return Err(anyhow!("work unit is not staged to integration"));
        }
        IntegrationInputState::Available
    };

    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE supervisor_work_units SET integration_state = ?, updated_at = ? WHERE id = ? AND supervisor_run_id = ?")
        .bind(integration_state.as_str())
        .bind(&now)
        .bind(&work_unit_id)
        .bind(id.to_string())
        .execute(&state.db)
        .await?;

    let run = load_supervisor_run(state, id).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", if staged { "work unit staged for integration" } else { "work unit unstaged from integration" }).await?;

    Ok(json!({
        "ok": true,
        "action": "stage_work_unit",
        "work_unit_id": work_unit_id,
        "integration_state": integration_state,
        "supervisor_run": run
    }))
}

pub async fn set_supervisor_work_unit_integration_skipped(state: &AppState, id: Uuid, work_unit_id: String, skipped: bool) -> Result<Value> {
    let work_unit = load_supervisor_integration_candidate(state, id, &work_unit_id).await?;
    if !work_unit.supports_integration_input() {
        return Err(anyhow!("work unit kind cannot be used as an integration input"));
    }

    let integration_state = if skipped {
        if work_unit.integration_state != IntegrationInputState::Included {
            return Err(anyhow!("only included integration inputs can be skipped"));
        }
        IntegrationInputState::Skipped
    } else {
        if work_unit.integration_state != IntegrationInputState::Skipped {
            return Err(anyhow!("work unit is not skipped from integration"));
        }
        IntegrationInputState::Included
    };

    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE supervisor_work_units SET integration_state = ?, updated_at = ? WHERE id = ? AND supervisor_run_id = ?")
        .bind(integration_state.as_str())
        .bind(&now)
        .bind(&work_unit_id)
        .bind(id.to_string())
        .execute(&state.db)
        .await?;

    let run = load_supervisor_run(state, id).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", if skipped { "integration input skipped" } else { "integration input unskipped" }).await?;

    Ok(json!({
        "ok": true,
        "work_unit_id": work_unit_id,
        "kind": work_unit.kind,
        "integration_state": integration_state,
        "supervisor_run": run
    }))
}













pub async fn apply_supervisor_work_unit(
    state: &AppState,
    id: Uuid,
    work_unit_id: String,
    archive_integrated_workflows: bool,
) -> Result<Value> {
    let work_unit = load_supervisor_work_unit_row(state, id, &work_unit_id).await?;
    if work_unit.kind != SupervisorWorkPoolKind::Integration {
        return Err(anyhow!("only the integration work unit can be applied to the root repository"));
    }

    let mut run = load_supervisor_run(state, id).await?;
    if matches!(run.status, SupervisorStatus::RunningIntegration | SupervisorStatus::Validating) {
        tick_integration(state, &mut run).await?;
        run.updated_at = Utc::now();
        update_supervisor_run(state, &run).await?;
    }

    let integration_row = sqlx::query(
        r#"
        SELECT workflow_run_id, integration_path, state, context_json
        FROM supervisor_work_units
        WHERE supervisor_run_id = ?
          AND id = ?
          AND kind = 'integration'
          AND archived_at IS NULL
          AND TRIM(COALESCE(workflow_run_id, '')) != ''
        LIMIT 1
        "#,
    )
    .bind(id.to_string())
    .bind(&work_unit_id)
    .fetch_optional(&state.db)
    .await?;

    let Some(integration_row) = integration_row else {
        return Err(anyhow!("integration workflow must complete successfully before applying integration batch"));
    };

    let integration_run_id_text: String = integration_row.get("workflow_run_id");
    let integration_run_id = Uuid::parse_str(&integration_run_id_text)?;
    let integration_path = integration_row
        .get::<Option<String>, _>("integration_path")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("integration path is missing; re-run integration before applying final patch"))?;
    let integration_context_json: String = integration_row.get("context_json");
    let integration_context = serde_json::from_str::<Value>(&integration_context_json).unwrap_or_else(|_| json!({}));
    if integration_context
        .get("applied_at")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(anyhow!("integration work unit has already been applied to the root repository"));
    }
    let integration_apply_available = integration_context
        .get("integration_apply_available")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !integration_apply_available {
        return Err(anyhow!("current integration result is not available to apply"));
    }
    if !matches!(run.status, SupervisorStatus::ReadyToApply) {
        run.status = SupervisorStatus::ReadyToApply;
        run.updated_at = Utc::now();
        update_supervisor_run(state, &run).await?;
    }

    let integration_batch_id = run.id.to_string();
    let integrated_inputs = sqlx::query(
        r#"
        SELECT source_work_unit_id, feature_id
        FROM supervisor_integration_patches
        WHERE supervisor_run_id = ?
          AND integration_work_unit_id = ?
          AND integration_workflow_run_id = ?
        ORDER BY created_at ASC
        "#,
    )
    .bind(run.id.to_string())
    .bind(&work_unit_id)
    .bind(integration_run_id.to_string())
    .fetch_all(&state.db)
    .await?;

    if integrated_inputs.is_empty() {
        return Err(anyhow!("integration workflow has no successfully merged input patches"));
    }

    let patch_text = integration_context
        .get("final_patch_text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| patches::generate_patch_text(Path::new(&integration_path)).unwrap_or_default());
    if patch_text.trim().is_empty() {
        return Err(anyhow!("integration produced an empty final patch; re-run integration before applying"));
    }
    let patch_hash = patches::patch_content_hash(&patch_text);
    let final_patch_ref = format!("supervisor_work_units:{}:context_json.final_patch_text", integration_batch_id);
    patches::apply_patch_text(Path::new(&run.root_repo_path), &patch_text)?;
    let now_text = Utc::now().to_rfc3339();
    let merge_report = json!({
        "ok": true,
        "status": "applied",
        "source": "supervisor_work_units.context_json.final_patch_text",
        "final_patch_ref": final_patch_ref,
        "final_patch_hash": patch_hash,
        "final_patch_bytes": patch_text.len(),
        "integration_path": integration_path,
        "applied_at": now_text,
        "archive_integrated_workflows": archive_integrated_workflows
    });
    sqlx::query(
        r#"
        UPDATE supervisor_work_units
        SET patch_id = COALESCE(NULLIF(patch_id, ''), ?),
            context_json = json_set(
                CASE WHEN json_valid(context_json) THEN context_json ELSE '{}' END,
                '$.final_patch_ref', ?,
                '$.final_patch_text', ?,
                '$.final_patch_hash', ?,
                '$.final_patch_bytes', ?,
                '$.merge_report', json(?),
                '$.applied_at', ?,
                '$.integration_apply_available', json('false')
            ),
            updated_at = ?
        WHERE supervisor_run_id = ?
          AND id = ?
          AND kind = 'integration'
          AND archived_at IS NULL
        "#,
    )
    .bind(&patch_hash)
    .bind(&final_patch_ref)
    .bind(&patch_text)
    .bind(&patch_hash)
    .bind(patch_text.len() as i64)
    .bind(serde_json::to_string(&merge_report)?)
    .bind(&now_text)
    .bind(&now_text)
    .bind(run.id.to_string())
    .bind(&work_unit_id)
    .execute(&state.db)
    .await?;
    let applied_at = Utc::now();
    let applied_at_text = applied_at.to_rfc3339();
    let source_work_unit_ids = integrated_inputs
        .iter()
        .map(|row| row.get::<String, _>("source_work_unit_id"))
        .collect::<Vec<_>>();
    let feature_ids = integrated_inputs
        .iter()
        .filter_map(|row| row.get::<Option<String>, _>("feature_id"))
        .filter(|feature_id| !feature_id.trim().is_empty())
        .collect::<Vec<_>>();

    let source_work_unit_ids_json = serde_json::to_string(&source_work_unit_ids)?;
    let feature_ids_json = serde_json::to_string(&feature_ids)?;
    let mut tx = state.db.begin().await?;

    if archive_integrated_workflows {
        sqlx::query(
            r#"
            UPDATE supervisor_work_units
            SET state = 'archived',
                integration_state = 'available',
                archived_at = COALESCE(archived_at, ?),
                archived_reason = 'integration applied and archived by user',
                updated_at = ?
            WHERE supervisor_run_id = ?
              AND id IN (SELECT value FROM json_each(?))
              AND kind IN ('feature', 'manual')
              AND archived_at IS NULL
            "#,
        )
        .bind(&applied_at_text)
        .bind(&applied_at_text)
        .bind(run.id.to_string())
        .bind(&source_work_unit_ids_json)
        .execute(&mut *tx)
        .await?;

        if !feature_ids.is_empty() {
            sqlx::query(
                r#"
                UPDATE planner_features
                SET status = 'archived',
                    locked_supervisor_run_id = NULL,
                    locked_at = NULL,
                    completed_at = COALESCE(completed_at, ?),
                    updated_at = ?
                WHERE id IN (SELECT value FROM json_each(?))
                  AND locked_supervisor_run_id = ?
                "#,
            )
            .bind(&applied_at_text)
            .bind(&applied_at_text)
            .bind(&feature_ids_json)
            .bind(run.id.to_string())
            .execute(&mut *tx)
            .await?;
        }
    }

    sqlx::query(
        "UPDATE supervisor_runs SET status = 'applied', updated_at = ? WHERE id = ? AND archived_at IS NULL",
    )
    .bind(&applied_at_text)
    .bind(run.id.to_string())
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    run.final_patch_path = None;
    run.merge_report = merge_report;
    run.status = SupervisorStatus::Applied;
    run.updated_at = applied_at;
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(
        state,
        &run,
        "integration_consumed",
        "integration work unit applied to root repository",
    )
    .await?;

    let run = load_supervisor_run(state, id).await?;
    Ok(json!({
        "ok": true,
        "status": "applied",
        "work_unit_id": work_unit_id,
        "integration_workflow_run_id": integration_run_id,
        "applied_at": applied_at_text,
        "source_work_unit_ids": source_work_unit_ids,
        "archive_integrated_workflows": archive_integrated_workflows,
        "supervisor_run": run
    }))
}






pub async fn load_supervisor_integration_inputs(state: &AppState, supervisor_id: Uuid) -> Result<Vec<SupervisorIntegrationInput>> {
    let rows = sqlx::query(
        r#"
        SELECT id, kind, feature_id, workflow_run_id, workspace_path
        FROM supervisor_work_units
        WHERE supervisor_run_id = ?
          AND kind IN ('feature', 'manual')
          AND integration_state = 'included'
          AND archived_at IS NULL
          AND state NOT IN ('cancelled', 'archived')
          AND TRIM(COALESCE(workspace_path, '')) != ''
        ORDER BY COALESCE(queue_position, 9223372036854775807) ASC, updated_at ASC
        "#,
    )
    .bind(supervisor_id.to_string())
    .fetch_all(&state.db)
    .await?;

    rows.into_iter()
        .map(|row| {
            let kind_text: String = row.try_get("kind")?;
            let workflow_run_id = row
                .try_get::<Option<String>, _>("workflow_run_id")?
                .filter(|value| !value.trim().is_empty())
                .map(|value| Uuid::parse_str(&value))
                .transpose()?;

            Ok(SupervisorIntegrationInput {
                work_unit_id: row.try_get("id")?,
                feature_id: row.try_get("feature_id")?,
                workspace_path: PathBuf::from(row.try_get::<String, _>("workspace_path")?),
                workflow_run_id,
                kind: SupervisorWorkPoolKind::try_from(kind_text.as_str()).map_err(anyhow::Error::msg)?,
            })
        })
        .collect()
}












fn supervisor_feature_concurrency(run: &SupervisorRun) -> usize {
    run.context
        .get("feature_concurrency")
        .and_then(Value::as_u64)
        .map(|value| value.clamp(1, 64) as usize)
        .unwrap_or(1)
}











async fn invalidate_supervisor_integration(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    if let Some(integration_run_id) = run.integration_run_id.take() {
        sqlx::query("UPDATE workflow_runs SET status = 'archived', updated_at = ? WHERE id = ?")
            .bind(Utc::now().to_rfc3339())
            .bind(integration_run_id.to_string())
            .execute(&state.db)
            .await?;
    }
    run.integration_path = None;
    run.final_patch_path = None;
    run.merge_report = json!({});
    run.validation_report = json!({});
    Ok(())
}

















async fn tick_integration(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    let Some(integration_run_id) = run.integration_run_id else {
        return Ok(());
    };
    let integration_run = crate::engine::load_run(state, integration_run_id).await?;
    match integration_run.status {
        RunStatus::Success => {
            run.status = SupervisorStatus::ReadyToApply;
        }
        RunStatus::Waiting | RunStatus::Paused | RunStatus::Queued | RunStatus::Running | RunStatus::Draft => {
            run.status = SupervisorStatus::RunningIntegration;
        }
        RunStatus::Error | RunStatus::Cancelled => run.status = SupervisorStatus::Failed,
    }
    Ok(())
}



const DEFAULT_REFINEMENT_TEMPLATE_NAME: &str = "Default refinement workflow";

async fn default_refinement_workflow_template_id(state: &AppState) -> Result<Option<Uuid>> {
    let row = sqlx::query("SELECT id FROM workflow_templates WHERE name = ?")
        .bind(DEFAULT_REFINEMENT_TEMPLATE_NAME)
        .fetch_optional(&state.db)
        .await?;
    row.map(|row| Uuid::parse_str(row.get::<String, _>("id").as_str()).map_err(Into::into))
        .transpose()
}


fn supervisor_context(run: &SupervisorRun, workspace: &repo_snapshot::SupervisorWorkspace) -> Value {
    json!({
        "supervisor_run_id": run.id,
        "strategy": run.strategy,
        "root_repo_path": run.root_repo_path,
        "snapshot_path": workspace.snapshot,
        "integration_path": workspace.integration,
        "patches_path": workspace.patches,
        "input_source": "supervisor_work_unit"
    })
}

async fn insert_supervisor_run(state: &AppState, run: &SupervisorRun) -> Result<()> {
    sqlx::query("INSERT INTO supervisor_runs (id, mode, status, title, root_repo_path, selected_planner_id, context_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(run.id.to_string())
        .bind(strategy_str(&run.strategy))
        .bind(status_supervisor_str(&run.status))
        .bind(&run.title)
        .bind(&run.root_repo_path)
        .bind(run.selected_planner_id.as_deref())
        .bind(serde_json::to_string(&run.context)?)
        .bind(run.created_at.to_rfc3339())
        .bind(run.updated_at.to_rfc3339())
        .execute(&state.db)
        .await?;
    Ok(())
}

pub(crate) async fn update_supervisor_run(state: &AppState, run: &SupervisorRun) -> Result<()> {
    sqlx::query("UPDATE supervisor_runs SET mode = ?, status = ?, title = ?, root_repo_path = ?, context_json = ?, selected_planner_id = ?, updated_at = ? WHERE id = ?")
        .bind(strategy_str(&run.strategy))
        .bind(status_supervisor_str(&run.status))
        .bind(&run.title)
        .bind(&run.root_repo_path)
        .bind(serde_json::to_string(&run.context)?)
        .bind(run.selected_planner_id.as_deref())
        .bind(run.updated_at.to_rfc3339())
        .bind(run.id.to_string())
        .execute(&state.db)
        .await?;
    Ok(())
}

async fn publish_supervisor_snapshot(state: &AppState, run: &SupervisorRun, event_type: &str, message: &str) -> Result<()> {
    let projection = crate::routes::supervisor_projection::build_supervisor_projection_by_id(
        state,
        &run.id.to_string(),
    )
    .await?;

    append_supervisor_event(
        state,
        run.id,
        event_type,
        message,
        SupervisorEventPayload {
            supervisor_run_id: run.id,
            supervisor: projection,
            deleted: false,
        },
    )
    .await?;
    Ok(())
}

async fn update_status(state: &AppState, id: Uuid, status: SupervisorStatus) -> Result<()> {
    sqlx::query("UPDATE supervisor_runs SET status = ?, updated_at = ? WHERE id = ?")
        .bind(status_supervisor_str(&status))
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .execute(&state.db)
        .await?;
    Ok(())
}

async fn hydrate_supervisor_feature_workflows(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    run.feature_workflows.clear();

    let rows = sqlx::query(
        r#"
        SELECT feature_id,
               title,
               shard_path,
               workflow_run_id,
               state,
               patch_id,
               blocked_reason,
               context_json
        FROM supervisor_work_units
        WHERE supervisor_run_id = ?
          AND kind = 'feature_development'
          AND state NOT IN ('deleted', 'archived')
        ORDER BY queue_position ASC, created_at ASC
        "#,
    )
    .bind(run.id.to_string())
    .fetch_all(&state.db)
    .await?;

    run.feature_workflows = rows
        .into_iter()
        .map(|row| {
            let workflow_run_id = row
                .try_get::<Option<String>, _>("workflow_run_id")
                .ok()
                .flatten()
                .and_then(|value| Uuid::parse_str(value.as_str()).ok());
            let state_value = row.try_get::<String, _>("state").unwrap_or_else(|_| "queued".to_string());
            let context = serde_json::from_str::<Value>(&row.get::<String, _>("context_json"))
                .unwrap_or_else(|_| json!({}));
            SupervisorFeatureWorkflow {
                feature_id: row.get("feature_id"),
                title: row.get("title"),
                shard_path: row.try_get("shard_path").ok(),
                workflow_run_id,
                status: context
                    .get("status")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| state_value.clone()),
                development_state: state_value,
                current_step_id: context.get("current_step_id").and_then(Value::as_str).map(str::to_string),
                current_patch_id: row.try_get("patch_id").ok(),
                last_error: row.try_get("blocked_reason").ok(),
            }
        })
        .collect();
    Ok(())
}

fn row_to_supervisor_run(row: sqlx::sqlite::SqliteRow) -> Result<SupervisorRun> {
    let context: Value = serde_json::from_str(row.get::<String, _>("context_json").as_str())?;
    Ok(SupervisorRun {
        id: Uuid::parse_str(row.get::<String, _>("id").as_str())?,
        strategy: parse_strategy(row.get::<String, _>("mode").as_str()),
        status: parse_status(row.get::<String, _>("status").as_str()),
        title: row.get("title"),
        root_repo_path: row.get("root_repo_path"),
        selected_planner_id: row.get("selected_planner_id"),
        snapshot_path: None,
        integration_path: None,
        feature_plan_items: Vec::new(),
        execution_plan_items: Vec::new(),
        feature_workflows: Vec::new(),
        integration_run_id: None,
        final_patch_path: None,
        merge_report: json!({}),
        validation_report: json!({}),
        context,
        created_at: DateTime::parse_from_rfc3339(row.get::<String, _>("created_at").as_str())?.with_timezone(&Utc),
        updated_at: DateTime::parse_from_rfc3339(row.get::<String, _>("updated_at").as_str())?.with_timezone(&Utc),
    })
}

fn parse_strategy(value: &str) -> SupervisorExecutionStrategy {
    match value {
        "parallel" | "fanout_sharded" => SupervisorExecutionStrategy::Parallel,
        _ => SupervisorExecutionStrategy::Series,
    }
}

fn parse_status(value: &str) -> SupervisorStatus {
    match value {
        "snapshotting" => SupervisorStatus::Snapshotting,
        "running_children" => SupervisorStatus::RunningChildren,
        "paused" => SupervisorStatus::Paused,
        "development_complete" => SupervisorStatus::DevelopmentComplete,
        "running_integration" => SupervisorStatus::RunningIntegration,
        "validating" => SupervisorStatus::Validating,
        "ready_to_apply" => SupervisorStatus::ReadyToApply,
        "applied" => SupervisorStatus::Applied,
        "failed" => SupervisorStatus::Failed,
        "cancelled" => SupervisorStatus::Cancelled,
        _ => SupervisorStatus::Created,
    }
}

fn strategy_str(value: &SupervisorExecutionStrategy) -> &'static str {
    match value {
        SupervisorExecutionStrategy::Series => "series",
        SupervisorExecutionStrategy::Parallel => "parallel",
    }
}

fn status_supervisor_str(value: &SupervisorStatus) -> &'static str {
    match value {
        SupervisorStatus::Created => "created",
        SupervisorStatus::Snapshotting => "snapshotting",
        SupervisorStatus::RunningChildren => "running_children",
        SupervisorStatus::Paused => "paused",
        SupervisorStatus::DevelopmentComplete => "development_complete",
        SupervisorStatus::RunningIntegration => "running_integration",
        SupervisorStatus::Validating => "validating",
        SupervisorStatus::ReadyToApply => "ready_to_apply",
        SupervisorStatus::Applied => "applied",
        SupervisorStatus::Failed => "failed",
        SupervisorStatus::Cancelled => "cancelled",
    }
}

fn status_str(value: &RunStatus) -> &'static str {
    match value {
        RunStatus::Draft => "draft",
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::Waiting => "waiting",
        RunStatus::Paused => "paused",
        RunStatus::Success => "success",
        RunStatus::Error => "error",
        RunStatus::Cancelled => "cancelled",
    }
}
