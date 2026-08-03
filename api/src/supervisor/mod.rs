pub mod lifecycle;
pub mod models;
pub mod patches;
pub mod repo_snapshot;
pub mod workflow_spawn;

use std::{collections::{HashMap, HashSet}, fs, hash::{Hash, Hasher}, path::{Path, PathBuf}};

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
        set_planner_feature_refinement_workflow_run, ExecutionPlanItem, FeaturePlanItem,
        FeaturePlanItemStatus,
    },
    models::{RunStatus, SprintEventStreamItem, WorkflowTemplateDefinition},
};
use models::{CreateSupervisorRunRequest, CreateSupervisorWorkUnitRequest, EnsureSupervisorPlannerRequest, EnsureSupervisorPlannerResponse, SupervisorActionRequest, SupervisorExecutionStrategy, SupervisorFeatureWorkflow, SupervisorRun, SupervisorStatus, SupervisorWorkPoolKind};
use lifecycle::{SupervisorPoolKind, SupervisorWorkflowSpawnRequest};

pub async fn list_supervisor_runs(state: &AppState) -> Result<Vec<SupervisorRun>> {
    let rows = sqlx::query("SELECT * FROM supervisor_runs WHERE archived_at IS NULL ORDER BY updated_at DESC")
        .fetch_all(&state.db)
        .await?;
    let mut runs = rows.into_iter().map(row_to_supervisor_run).collect::<Result<Vec<_>>>()?;
    for run in &mut runs {
        if is_repo_planner_run(run) {
            hydrate_supervisor_planner_from_repo(state, run).await?;
        }
        hydrate_supervisor_feature_workflows(state, run).await?;
    }
    Ok(runs)
}

pub async fn load_supervisor_run(state: &AppState, id: Uuid) -> Result<SupervisorRun> {
    let row = sqlx::query("SELECT * FROM supervisor_runs WHERE id = ? AND archived_at IS NULL")
        .bind(id.to_string())
        .fetch_one(&state.db)
        .await?;
    let mut run = row_to_supervisor_run(row)?;
    if is_repo_planner_run(&run) {
        hydrate_supervisor_planner_from_repo(state, &mut run).await?;
    }
    hydrate_supervisor_feature_workflows(state, &mut run).await?;
    Ok(run)
}

pub async fn load_supervisor_run_reconciled(state: &AppState, id: Uuid) -> Result<SupervisorRun> {
    load_supervisor_run(state, id).await
}

pub async fn list_supervisor_runs_reconciled(state: &AppState) -> Result<Vec<SupervisorRun>> {
    list_supervisor_runs(state).await
}

pub async fn create_supervisor_run(state: &AppState, req: CreateSupervisorRunRequest) -> Result<SupervisorRun> {
    let id = Uuid::new_v4();
    let now = Utc::now();
    let mut context = req.context;
    if !context.is_object() {
        context = json!({});
    }
    if let Some(obj) = context.as_object_mut() {
        if let Some(template_id) = req.workflow_template_id {
            obj.insert("workflow_template_id".to_string(), Value::String(template_id.to_string()));
        }
        if let Some(template_id) = req.integration_template_id {
            obj.insert("integration_template_id".to_string(), Value::String(template_id.to_string()));
        }
    }
    let execution_plan_items = req.execution_plan_items;
    let run = SupervisorRun {
        id,
        strategy: req.strategy,
        status: SupervisorStatus::Created,
        title: req.title,
        root_repo_path: req.root_repo_path,
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
    Ok(run)
}

pub async fn ensure_supervisor_planner_run(state: &AppState, req: EnsureSupervisorPlannerRequest) -> Result<EnsureSupervisorPlannerResponse> {
    let normalized_root = normalize_repo_root(&req.root_repo_path);
    if normalized_root.is_empty() {
        return Err(anyhow!("root_repo_path is required"));
    }

    let runs = list_supervisor_runs(state).await?;
    if let Some(mut run) = runs.into_iter().find(|run| repo_roots_match(&run.root_repo_path, &normalized_root) && is_repo_planner_run(run)) {
        hydrate_supervisor_planner_from_repo(state, &mut run).await?;
        update_supervisor_run(state, &run).await?;
        return Ok(EnsureSupervisorPlannerResponse {
            created: false,
            supervisor_run: run,
        });
    }

    let mut context = json!({
        "planner_kind": "repo_root",
        "repo_root_key": normalized_root,
    });
    if let Some(obj) = context.as_object_mut() {
        obj.insert("root_repo_path".to_string(), Value::String(normalized_root.clone()));
    }

    let persisted_features = load_repo_feature_plan_items(state, &normalized_root).await?;
    let run = create_supervisor_run(state, CreateSupervisorRunRequest {
        title: req.title.filter(|value| !value.trim().is_empty()).unwrap_or_else(|| repo_planner_title(&normalized_root)),
        root_repo_path: normalized_root,
        strategy: SupervisorExecutionStrategy::Series,
        workflow_template_id: None,
        integration_template_id: None,
        feature_plan_items: persisted_features,
        execution_plan_items: Vec::new(),
        context,
    }).await?;

    Ok(EnsureSupervisorPlannerResponse {
        created: true,
        supervisor_run: run,
    })
}

fn repo_roots_match(left: &str, right: &str) -> bool {
    normalize_repo_root(left) == normalize_repo_root(right)
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

fn repo_planner_title(root: &str) -> String {
    let name = root
        .rsplit('/')
        .find(|part| !part.trim().is_empty())
        .unwrap_or("Repo");
    format!("{} Planner", name)
}

fn is_repo_planner_run(run: &SupervisorRun) -> bool {
    run.context
        .get("planner_kind")
        .and_then(Value::as_str)
        .map(|value| value == "repo_root")
        .unwrap_or_else(|| run.integration_run_id.is_none() && run.final_patch_path.is_none())
}

fn planner_repo_key(root: &str) -> String {
    let normalized = normalize_repo_root(root);
    let mut key = normalized
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_lowercase() } else { '-' })
        .collect::<String>();
    while key.contains("--") {
        key = key.replace("--", "-");
    }
    let key = key.trim_matches('-').to_string();
    if key.is_empty() { "repo".to_string() } else { key }
}

fn sprint_key_for(root: &str, sprint_id: &str) -> String {
    let prefix = planner_repo_key(root).replace('-', "_").to_ascii_uppercase();
    let suffix = sprint_id.chars().filter(|ch| *ch != '-').take(12).collect::<String>().to_ascii_uppercase();
    format!("{}-SPRINT-{}", prefix, suffix)
}

async fn ensure_planner_repo_id(state: &AppState, root: &str) -> Result<String> {
    let normalized_root = normalize_repo_root(root);
    if normalized_root.is_empty() {
        return Err(anyhow!("root_repo_path is required"));
    }
    if let Some(row) = sqlx::query("SELECT id FROM planner_workspaces WHERE root_repo_path = ? ORDER BY is_default DESC, updated_at DESC, created_at DESC LIMIT 1")
        .bind(&normalized_root)
        .fetch_optional(&state.db)
        .await?
    {
        return Ok(row.get("id"));
    }
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let repo_key = planner_repo_key(&normalized_root);
    sqlx::query("INSERT INTO planner_workspaces (id, root_repo_path, repo_key, title, is_default, created_at, updated_at) VALUES (?, ?, ?, ?, 1, ?, ?)")
        .bind(&id)
        .bind(&normalized_root)
        .bind(repo_key)
        .bind(supervisor_planner_title(&normalized_root))
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await?;
    Ok(id)
}

async fn load_repo_feature_plan_items(state: &AppState, root: &str) -> Result<Vec<FeaturePlanItem>> {
    let normalized_root = normalize_repo_root(root);
    if normalized_root.trim().is_empty() {
        return Ok(Vec::new());
    }

    let planner_id = ensure_planner_repo_id(state, &normalized_root).await?;
    let rows = sqlx::query("SELECT payload_json FROM planner_features WHERE planner_id = ? AND id NOT LIKE 'manual-%' AND COALESCE(status, '') != 'deleted' ORDER BY sort_order ASC, created_at ASC")
        .bind(planner_id)
        .fetch_all(&state.db)
        .await?;
    rows.into_iter()
        .map(|row| {
            let mut item = serde_json::from_str::<FeaturePlanItem>(row.get::<String, _>("payload_json").as_str())?;
            if matches!(item.status, FeaturePlanItemStatus::Scheduled) {
                item.status = FeaturePlanItemStatus::Fine;
            }
            Ok(item)
        })
        .collect()
}

async fn save_repo_feature_plan_items(state: &AppState, root: &str, items: &[FeaturePlanItem]) -> Result<()> {
    let normalized_root = normalize_repo_root(root);
    let planner_id = ensure_planner_repo_id(state, &normalized_root).await?;
    let now = Utc::now().to_rfc3339();
    let planner_features = items
        .iter()
        .filter(|item| !item.id.starts_with("manual-"))
        .cloned()
        .map(|mut item| {
            if matches!(item.status, FeaturePlanItemStatus::Scheduled) {
                item.status = FeaturePlanItemStatus::Fine;
            }
            item
        })
        .collect::<Vec<_>>();
    for (index, item) in planner_features.iter().enumerate() {
        sqlx::query("INSERT INTO planner_features (id, planner_id, title, status, sort_order, payload_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET planner_id = excluded.planner_id, title = excluded.title, status = excluded.status, sort_order = excluded.sort_order, payload_json = excluded.payload_json, updated_at = excluded.updated_at")
            .bind(&item.id)
            .bind(&planner_id)
            .bind(&item.title)
            .bind(serde_json::to_value(&item.status)?.as_str().unwrap_or("fine"))
            .bind(index as i64)
            .bind(serde_json::to_string(item)?)
            .bind(&now)
            .bind(&now)
            .execute(&state.db)
            .await?;
    }
    sqlx::query("UPDATE planner_features SET status = 'deleted', updated_at = ? WHERE planner_id = ? AND id NOT LIKE 'manual-%' AND id NOT IN (SELECT value FROM json_each(?))")
        .bind(&now)
        .bind(&planner_id)
        .bind(serde_json::to_string(&planner_features.iter().map(|item| item.id.clone()).collect::<Vec<_>>())?)
        .execute(&state.db)
        .await?;
    sqlx::query("UPDATE planner_workspaces SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&planner_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

fn sync_execution_plan_from_scheduled_features(run: &mut SupervisorRun) -> bool {
    let existing_by_feature_id = run
        .execution_plan_items
        .iter()
        .map(|item| (item.feature_plan_item_id.as_str(), item))
        .collect::<HashMap<_, _>>();

    let next = run
        .feature_plan_items
        .iter()
        .enumerate()
        .filter(|(_, item)| matches!(item.status, FeaturePlanItemStatus::Scheduled))
        .map(|(index, item)| {
            let existing = existing_by_feature_id.get(item.id.as_str()).copied();
            ExecutionPlanItem {
                feature_plan_item_id: item.id.clone(),
                workflow_template_id: existing.and_then(|value| value.workflow_template_id),
                order_index: existing.and_then(|value| value.order_index).or(Some(index as i64)),
            }
        })
        .collect::<Vec<_>>();

    let changed = run.execution_plan_items.len() != next.len()
        || run.execution_plan_items.iter().zip(next.iter()).any(|(left, right)| {
            left.feature_plan_item_id != right.feature_plan_item_id
                || left.workflow_template_id != right.workflow_template_id
                || left.order_index != right.order_index
        });

    if changed {
        run.execution_plan_items = next;
    }

    changed
}

async fn hydrate_supervisor_planner_from_repo(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    let persisted_features = load_repo_feature_plan_items(state, &run.root_repo_path).await?;
    if !persisted_features.is_empty() {
        run.feature_plan_items = persisted_features;
        let feature_ids = run.feature_plan_items.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
        run.execution_plan_items.retain(|item| feature_ids.iter().any(|id| id == &item.feature_plan_item_id));
    } else if !run.feature_plan_items.is_empty() {
        save_repo_feature_plan_items(state, &run.root_repo_path, &run.feature_plan_items).await?;
    }
    Ok(())
}

async fn upsert_sprint_record(_state: &AppState, _run: &SupervisorRun, _sprint_id: &str, _sprint_key: &str, _title: &str, _status: &str, _sprint_started_at: Option<&str>, _sprint_completed_at: Option<&str>) -> Result<()> {
    Ok(())
}

async fn save_sprint_features(state: &AppState, run: &SupervisorRun, _sprint_id: &str, _completed_at: Option<&str>) -> Result<()> {
    save_repo_feature_plan_items(state, &run.root_repo_path, &run.feature_plan_items).await?;
    Ok(())
}

async fn clear_planner_feature_development_for_restart(state: &AppState, _root: &str, feature_id: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE planner_features SET locked_supervisor_run_id = NULL, locked_at = NULL, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(feature_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

async fn mark_planner_feature_preserved(state: &AppState, _root: &str, feature_id: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE planner_features SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(feature_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

async fn append_sprint_event(state: &AppState, sprint_id: &str, event_type: &str, event_time: &str, feature_id: Option<&str>, message: &str, payload: Value) -> Result<SprintEventStreamItem> {
    let event = SprintEventStreamItem {
        id: Uuid::new_v4().to_string(),
        sprint_id: sprint_id.to_string(),
        sequence_no: Utc::now().timestamp_millis(),
        event_type: event_type.to_string(),
        event_time: event_time.to_string(),
        feature_id: feature_id.map(str::to_string),
        actor: "system".to_string(),
        message: message.to_string(),
        payload,
        created_at: Utc::now().to_rfc3339(),
    };
    state.publish_sprint_event(event.clone());
    Ok(event)
}

fn import_string(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or("").trim().to_string()
}

fn import_string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::trim).filter(|value| !value.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SupervisorQueuedFeatureRef {
    feature_id: String,
    planner_id: String,
    planner_title: String,
}

fn import_queued_features(value: Option<&Value>) -> Vec<SupervisorQueuedFeatureRef> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<SupervisorQueuedFeatureRef>(item.clone()).ok())
                .map(|mut item| {
                    item.feature_id = item.feature_id.trim().to_string();
                    item.planner_id = item.planner_id.trim().to_string();
                    item.planner_title = item.planner_title.trim().to_string();
                    item
                })
                .filter(|item| !item.feature_id.is_empty() && !item.feature_id.starts_with("manual-") && !item.planner_id.is_empty())
                .fold(Vec::<SupervisorQueuedFeatureRef>::new(), |mut acc, item| {
                    if !acc.iter().any(|existing| existing.feature_id == item.feature_id) {
                        acc.push(item);
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

fn normalize_queue_workspace_features(features_json: &str) -> Vec<FeaturePlanItem> {
    serde_json::from_str::<Vec<FeaturePlanItem>>(features_json)
        .unwrap_or_default()
        .into_iter()
        .filter(|item| !item.id.trim().is_empty() && !item.id.starts_with("manual-"))
        .map(|mut item| {
            if matches!(item.status, FeaturePlanItemStatus::Scheduled) {
                item.status = FeaturePlanItemStatus::Fine;
            }
            item
        })
        .collect()
}

async fn queue_planner_workspace_by_id(state: &AppState, planner_id: &str) -> Result<Option<(String, String, Vec<FeaturePlanItem>)>> {
    ensure_planner_workspace_table(state).await?;
    let row = sqlx::query("SELECT root_repo_path, title FROM planner_workspaces WHERE id = ? LIMIT 1")
        .bind(planner_id)
        .fetch_optional(&state.db)
        .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let rows = sqlx::query("SELECT id, title, status, payload_json FROM planner_features WHERE planner_id = ? AND id NOT LIKE 'manual-%' AND COALESCE(status, '') != 'deleted' ORDER BY sort_order ASC, created_at ASC")
        .bind(planner_id)
        .fetch_all(&state.db)
        .await?;

    let items = rows
        .into_iter()
        .filter_map(|row| {
            let payload_json: String = row.get("payload_json");
            let mut item = serde_json::from_str::<FeaturePlanItem>(&payload_json).ok()?;
            if item.id.trim().is_empty() {
                item.id = row.get("id");
            }
            if item.title.trim().is_empty() {
                item.title = row.get("title");
            }
            let status_text: String = row.get("status");
            if let Ok(status) = serde_json::from_value::<FeaturePlanItemStatus>(Value::String(status_text)) {
                item.status = status;
            }
            if matches!(item.status, FeaturePlanItemStatus::Scheduled) {
                item.status = FeaturePlanItemStatus::Fine;
            }
            Some(item)
        })
        .collect::<Vec<_>>();

    Ok(Some((normalize_repo_root(row.get::<String, _>("root_repo_path").as_str()), row.get("title"), items)))
}

fn imported_feature_values(payload: &Value) -> Result<Vec<Value>> {
    if let Some(items) = payload.as_array() {
        return Ok(items.clone());
    }
    if let Some(items) = payload.get("features").and_then(Value::as_array) {
        return Ok(items.clone());
    }
    Err(anyhow!("planner import must be a JSON feature array or an object with a features array"))
}

fn import_status(value: &str) -> FeaturePlanItemStatus {
    match value {
        "fine" | "refined" | "approved" => FeaturePlanItemStatus::Fine,
        "scheduled" => FeaturePlanItemStatus::Scheduled,
        "applied" => FeaturePlanItemStatus::Applied,
        "completed" => FeaturePlanItemStatus::Completed,
        _ => FeaturePlanItemStatus::Rough,
    }
}

fn normalize_imported_feature(value: &Value, index: usize) -> Result<FeaturePlanItem> {
    let item = value.as_object().ok_or_else(|| anyhow!("feature import item {} must be an object", index + 1))?;
    let title = import_string(item.get("title"));
    if title.is_empty() {
        return Err(anyhow!("missing required title"));
    }
    let status = import_status(import_string(item.get("status")).as_str());
    let summary = import_string(item.get("summary"));
    let id = import_string(item.get("id"));
    let rough_summary = item
        .get("rough_summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| if matches!(status, FeaturePlanItemStatus::Rough) { Some(summary.clone()) } else { None });
    Ok(FeaturePlanItem {
        id: if id.is_empty() { Uuid::new_v4().to_string() } else { id },
        title,
        status,
        summary,
        rough_summary,
        refinement_workflow_run_id: item.get("refinement_workflow_run_id").and_then(Value::as_str).and_then(|value| Uuid::parse_str(value).ok()),
        applied_sprint_id: item.get("applied_sprint_id").and_then(Value::as_str).map(str::to_string),
        applied_sprint_title: item.get("applied_sprint_title").and_then(Value::as_str).map(str::to_string),
        applied_at: item.get("applied_at").and_then(Value::as_str).map(str::to_string),
        requirements: import_string_array(item.get("requirements")),
        acceptance_criteria: import_string_array(item.get("acceptance_criteria")),
        implementation_notes: import_string_array(item.get("implementation_notes")),
        review_expectations: import_string_array(item.get("review_expectations")),
        target_files_or_areas: import_string_array(item.get("target_files_or_areas")),
        dependencies: Vec::new(),
    })
}

fn normalized_text_fingerprint(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn feature_title_key(feature: &FeaturePlanItem) -> String {
    normalized_text_fingerprint(&feature.title)
}

fn feature_content_fingerprint(feature: &FeaturePlanItem) -> String {
    let value = json!({
        "title": normalized_text_fingerprint(&feature.title),
        "summary": normalized_text_fingerprint(&feature.summary),
        "requirements": feature.requirements.iter().map(|value| normalized_text_fingerprint(value)).collect::<Vec<_>>(),
        "acceptance_criteria": feature.acceptance_criteria.iter().map(|value| normalized_text_fingerprint(value)).collect::<Vec<_>>(),
        "implementation_notes": feature.implementation_notes.iter().map(|value| normalized_text_fingerprint(value)).collect::<Vec<_>>(),
        "review_expectations": feature.review_expectations.iter().map(|value| normalized_text_fingerprint(value)).collect::<Vec<_>>(),
        "target_files_or_areas": feature.target_files_or_areas.iter().map(|value| normalized_text_fingerprint(value)).collect::<Vec<_>>()
    });
    let encoded = serde_json::to_string(&value).unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    encoded.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn import_preview_items(run: &SupervisorRun, payload: &Value) -> Result<Vec<Value>> {
    let values = imported_feature_values(payload)?;
    let existing_by_id = run.feature_plan_items.iter().map(|item| (item.id.clone(), item)).collect::<HashMap<_, _>>();
    let mut existing_by_title = HashMap::<String, &FeaturePlanItem>::new();
    for item in &run.feature_plan_items {
        let key = feature_title_key(item);
        if !key.is_empty() {
            existing_by_title.entry(key).or_insert(item);
        }
    }
    let mut seen_import_ids = HashSet::<String>::new();
    let mut seen_import_titles = HashSet::<String>::new();
    let mut out = Vec::new();

    for (index, value) in values.iter().enumerate() {
        let imported = match normalize_imported_feature(value, index) {
            Ok(feature) => feature,
            Err(err) => {
                out.push(json!({
                    "import_index": index,
                    "status": "invalid",
                    "default_action": "reject",
                    "reason": err.to_string(),
                    "raw": value
                }));
                continue;
            }
        };

        let title_key = feature_title_key(&imported);
        let content_fingerprint = feature_content_fingerprint(&imported);

        if !seen_import_ids.insert(imported.id.clone()) {
            out.push(json!({
                "import_index": index,
                "status": "invalid",
                "default_action": "reject",
                "reason": "duplicate feature id inside uploaded file",
                "feature": imported,
                "content_fingerprint": content_fingerprint
            }));
            continue;
        }

        if !title_key.is_empty() && !seen_import_titles.insert(title_key.clone()) {
            out.push(json!({
                "import_index": index,
                "status": "invalid",
                "default_action": "reject",
                "reason": "duplicate feature title inside uploaded file",
                "feature": imported,
                "content_fingerprint": content_fingerprint
            }));
            continue;
        }

        if let Some(existing) = existing_by_id.get(&imported.id) {
            let existing_fingerprint = feature_content_fingerprint(existing);
            if existing_fingerprint == content_fingerprint {
                out.push(json!({
                    "import_index": index,
                    "status": "duplicate",
                    "default_action": "skip",
                    "reason": "feature id and content already exist",
                    "existing_feature_id": existing.id,
                    "existing_title": existing.title,
                    "feature": imported,
                    "content_fingerprint": content_fingerprint
                }));
            } else {
                out.push(json!({
                    "import_index": index,
                    "status": "conflict",
                    "default_action": "skip",
                    "reason": "feature id already exists with different content",
                    "existing_feature_id": existing.id,
                    "existing_title": existing.title,
                    "feature": imported,
                    "content_fingerprint": content_fingerprint
                }));
            }
            continue;
        }

        if let Some(existing) = existing_by_title.get(&title_key) {
            let existing_fingerprint = feature_content_fingerprint(existing);
            if existing_fingerprint == content_fingerprint {
                out.push(json!({
                    "import_index": index,
                    "status": "duplicate",
                    "default_action": "skip",
                    "reason": "feature title and content already exist",
                    "existing_feature_id": existing.id,
                    "existing_title": existing.title,
                    "feature": imported,
                    "content_fingerprint": content_fingerprint
                }));
            } else {
                out.push(json!({
                    "import_index": index,
                    "status": "conflict",
                    "default_action": "skip",
                    "reason": "feature title matches an existing edited feature with different content",
                    "existing_feature_id": existing.id,
                    "existing_title": existing.title,
                    "feature": imported,
                    "content_fingerprint": content_fingerprint
                }));
            }
            continue;
        }

        out.push(json!({
            "import_index": index,
            "status": "accepted",
            "default_action": "create",
            "reason": "new feature",
            "feature": imported,
            "content_fingerprint": content_fingerprint
        }));
    }

    Ok(out)
}

fn import_summary(items: &[Value]) -> Value {
    let count = |status: &str| -> usize {
        items.iter().filter(|item| item.get("status").and_then(Value::as_str) == Some(status)).count()
    };
    json!({
        "total": items.len(),
        "accepted": count("accepted"),
        "duplicates": count("duplicate"),
        "conflicts": count("conflict"),
        "invalid": count("invalid")
    })
}

pub async fn preview_supervisor_planner_import(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let run = load_supervisor_run(state, id).await?;
    let items = import_preview_items(&run, &payload)?;
    Ok(json!({
        "ok": true,
        "summary": import_summary(&items),
        "items": items
    }))
}

pub async fn apply_supervisor_planner_import(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let preview_payload = payload.get("import").cloned().unwrap_or_else(|| payload.clone());
    let preview_items = import_preview_items(&run, &preview_payload)?;
    let decisions = payload
        .get("decisions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let decision_by_index = decisions
        .iter()
        .filter_map(|item| {
            let index = item.get("import_index").and_then(Value::as_u64)? as usize;
            let action = item.get("action").and_then(Value::as_str)?.to_string();
            let existing_feature_id = item.get("existing_feature_id").and_then(Value::as_str).map(str::to_string);
            Some((index, (action, existing_feature_id)))
        })
        .collect::<HashMap<_, _>>();

    let mut created = Vec::<Value>::new();
    let mut replaced = Vec::<Value>::new();
    let mut skipped = Vec::<Value>::new();
    let mut rejected = Vec::<Value>::new();

    for item in preview_items {
        let index = item.get("import_index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let status = item.get("status").and_then(Value::as_str).unwrap_or("invalid");
        let default_action = item.get("default_action").and_then(Value::as_str).unwrap_or("reject");
        let (action, requested_existing_id) = decision_by_index
            .get(&index)
            .cloned()
            .unwrap_or_else(|| (default_action.to_string(), None));

        let Some(feature_value) = item.get("feature").cloned() else {
            rejected.push(item);
            continue;
        };
        let mut feature: FeaturePlanItem = serde_json::from_value(feature_value)?;

        match action.as_str() {
            "create" if status == "accepted" => {
                run.feature_plan_items.push(feature.clone());
                created.push(json!({ "import_index": index, "feature_id": feature.id, "title": feature.title }));
            }
            "create_copy" if status == "accepted" || status == "duplicate" || status == "conflict" => {
                feature.id = Uuid::new_v4().to_string();
                run.feature_plan_items.push(feature.clone());
                created.push(json!({ "import_index": index, "feature_id": feature.id, "title": feature.title, "copied": true }));
            }
            "replace_existing" if status == "conflict" => {
                let existing_id = requested_existing_id
                    .or_else(|| item.get("existing_feature_id").and_then(Value::as_str).map(str::to_string))
                    .ok_or_else(|| anyhow!("existing_feature_id is required to replace import item {}", index))?;
                let existing_index = run
                    .feature_plan_items
                    .iter()
                    .position(|existing| existing.id == existing_id)
                    .ok_or_else(|| anyhow!("existing feature {} is missing", existing_id))?;
                feature.id = existing_id.clone();
                run.feature_plan_items[existing_index] = feature.clone();
                replaced.push(json!({ "import_index": index, "feature_id": existing_id, "title": feature.title }));
            }
            "skip" => skipped.push(item),
            "reject" => rejected.push(item),
            other => {
                rejected.push(json!({
                    "import_index": index,
                    "status": status,
                    "default_action": default_action,
                    "reason": format!("unsupported import action {} for status {}", other, status),
                    "feature": feature
                }));
            }
        }
    }

    let feature_ids = run.feature_plan_items.iter().map(|item| item.id.clone()).collect::<HashSet<_>>();
    run.execution_plan_items.retain(|item| feature_ids.contains(&item.feature_plan_item_id));
    save_repo_feature_plan_items(state, &run.root_repo_path, &run.feature_plan_items).await?;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;

    Ok(json!({
        "ok": true,
        "summary": {
            "created": created.len(),
            "replaced": replaced.len(),
            "skipped": skipped.len(),
            "rejected": rejected.len()
        },
        "created": created,
        "replaced": replaced,
        "skipped": skipped,
        "rejected": rejected,
        "supervisor_run": run
    }))
}

pub async fn update_supervisor_plan(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let planner_value = payload
        .get("planner_log_items")
        .or_else(|| payload.get("feature_plan_items"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    let sprint_value = payload
        .get("sprint_items")
        .or_else(|| payload.get("execution_plan_items"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    let mut planner_items: Vec<FeaturePlanItem> = serde_json::from_value(planner_value)?;
    let sprint_items: Vec<ExecutionPlanItem> = serde_json::from_value(sprint_value)?;
    if let Some(strategy) = payload.get("sprint_strategy").or_else(|| payload.get("strategy")).and_then(Value::as_str) {
        run.strategy = parse_strategy(strategy);
    }
    if !run.context.is_object() {
        run.context = json!({});
    }
    if let Some(obj) = run.context.as_object_mut() {
        if let Some(template_id) = payload.get("workflow_template_id").and_then(Value::as_str).filter(|value| !value.is_empty()) {
            obj.insert("workflow_template_id".to_string(), Value::String(template_id.to_string()));
        } else if payload.get("workflow_template_id").is_some() {
            obj.remove("workflow_template_id");
        }
        if let Some(start_step_id) = payload.get("workflow_start_step_id").and_then(Value::as_str).filter(|value| !value.is_empty()) {
            obj.insert("workflow_start_step_id".to_string(), Value::String(start_step_id.to_string()));
        } else if payload.get("workflow_start_step_id").is_some() {
            obj.remove("workflow_start_step_id");
        }
        if let Some(template_id) = payload.get("planner_refinement_template_id").and_then(Value::as_str).filter(|value| !value.is_empty()) {
            obj.insert("planner_refinement_template_id".to_string(), Value::String(template_id.to_string()));
        } else if payload.get("planner_refinement_template_id").is_some() {
            obj.remove("planner_refinement_template_id");
        }
        if let Some(template_id) = payload.get("integration_template_id").and_then(Value::as_str).filter(|value| !value.is_empty()) {
            obj.insert("integration_template_id".to_string(), Value::String(template_id.to_string()));
        } else if payload.get("integration_template_id").is_some() {
            obj.remove("integration_template_id");
        }
        if let Some(feature_concurrency) = payload.get("feature_concurrency").and_then(Value::as_u64) {
            obj.insert("feature_concurrency".to_string(), Value::Number(feature_concurrency.max(1).min(64).into()));
        }
        if let Some(integration_policy) = payload.get("integration_policy").and_then(Value::as_str).filter(|value| matches!(*value, "auto" | "manual")) {
            obj.insert("integration_policy".to_string(), Value::String(integration_policy.to_string()));
        }
    }
    for sprint_item in &sprint_items {
        if !planner_items.iter().any(|item| item.id == sprint_item.feature_plan_item_id) {
            return Err(anyhow!("sprint item {} is not present in the planner log", sprint_item.feature_plan_item_id));
        }
    }
    let scheduled_feature_ids = sprint_items
        .iter()
        .map(|item| item.feature_plan_item_id.clone())
        .collect::<Vec<_>>();
    let previously_scheduled_feature_ids = run
        .execution_plan_items
        .iter()
        .map(|item| item.feature_plan_item_id.clone())
        .collect::<Vec<_>>();
    for item in &mut planner_items {
        let is_scheduled = scheduled_feature_ids.iter().any(|id| id == &item.id);
        if is_scheduled && !matches!(item.status, FeaturePlanItemStatus::Completed | FeaturePlanItemStatus::Applied) {
            item.status = FeaturePlanItemStatus::Scheduled;
        } else if !is_scheduled && matches!(item.status, FeaturePlanItemStatus::Scheduled) {
            item.status = FeaturePlanItemStatus::Fine;
        }
    }
    run.feature_plan_items = planner_items;
    run.execution_plan_items = sprint_items;
    save_repo_feature_plan_items(state, &run.root_repo_path, &run.feature_plan_items).await?;
    if scheduled_feature_ids != previously_scheduled_feature_ids {
        invalidate_supervisor_integration(state, &mut run).await?;
    }
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

fn shard_has_development_diff(shard_path: Option<&str>) -> bool {
    let Some(shard_path) = shard_path.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let path = std::path::Path::new(shard_path);
    if !path.exists() || !path.is_dir() {
        return false;
    }

    let unstaged = std::process::Command::new("git")
        .arg("diff")
        .arg("--quiet")
        .current_dir(path)
        .output();
    let staged = std::process::Command::new("git")
        .arg("diff")
        .arg("--cached")
        .arg("--quiet")
        .current_dir(path)
        .output();

    let has_unstaged = match unstaged {
        Ok(output) => output.status.code() == Some(1),
        Err(_) => true,
    };
    let has_staged = match staged {
        Ok(output) => output.status.code() == Some(1),
        Err(_) => true,
    };

    has_unstaged || has_staged
}

async fn queue_planner_features(state: &AppState, root: &str, planner_id: Option<&str>) -> Result<(Option<String>, Option<String>, Vec<FeaturePlanItem>)> {
    let normalized_root = normalize_repo_root(root);
    let Some(requested_planner_id) = planner_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok((None, None, Vec::new()));
    };
    if normalized_root.trim().is_empty() {
        return Ok((Some(requested_planner_id.to_string()), None, Vec::new()));
    }

    if let Some((workspace_root, title, items)) = queue_planner_workspace_by_id(state, requested_planner_id).await? {
        if workspace_root == normalized_root
            || normalized_root.starts_with(format!("{}/", workspace_root).as_str())
            || workspace_root.starts_with(format!("{}/", normalized_root).as_str())
        {
            return Ok((Some(requested_planner_id.to_string()), Some(title), items));
        }
    }

    Ok((Some(requested_planner_id.to_string()), None, Vec::new()))
}

pub async fn supervisor_queue_projection(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    kick_feature_pool_if_running(state, &mut run).await?;

    let current_planner_id = selected_planner_id_for_supervisor(state, id)
        .await?
        .unwrap_or_default();

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
            wu.workflow_run_id AS current_workflow_run_id,
            wu.patch_id AS current_patch_id,
            wu.state AS development_state,
            COALESCE(NULLIF(wu.workspace_path, ''), NULLIF(wu.shard_path, ''), NULLIF(wu.integration_path, '')) AS shard_path
        FROM planner_features pf
        JOIN planner_workspaces pw ON pw.id = pf.planner_id
        LEFT JOIN supervisor_work_units wu
          ON wu.feature_id = pf.id
         AND wu.supervisor_run_id = ?
         AND wu.kind = 'feature_development'
         AND wu.archived_at IS NULL
         AND wu.state NOT IN ('deleted', 'archived')
        WHERE pf.id NOT LIKE 'manual-%'
          AND COALESCE(pf.status, '') != 'deleted'
          AND COALESCE(pf.completed_at, '') = ''
          AND (
            (
              pf.planner_id = ?
              AND pf.status IN ('fine', 'refined', 'approved', 'scheduled')
            )
            OR pf.locked_supervisor_run_id = ?
          )
        ORDER BY
          CASE WHEN pf.locked_supervisor_run_id = ? THEN 0 ELSE 1 END,
          pf.sort_order ASC,
          pf.created_at ASC
        "#,
    )
    .bind(run.id.to_string())
    .bind(&current_planner_id)
    .bind(run.id.to_string())
    .bind(run.id.to_string())
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
        let completed_at = row.get::<Option<String>, _>("completed_at");
        let shard_path = row.get::<Option<String>, _>("shard_path");
        let queued = locked_by_this;
        let has_development_diff = shard_has_development_diff(shard_path.as_deref());
        let dequeue_without_prompt = queued && !has_development_diff;

        let feature_payload = serde_json::from_str::<Value>(&payload_json).unwrap_or_else(|_| json!({}));
        let summary = feature_payload.get("summary").and_then(Value::as_str).map(str::to_string);
        let can_queue = !queued && !locked_by_other && matches!(status.as_str(), "fine" | "refined" | "approved" | "scheduled");
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
            "current_sprint_id": Value::Null,
            "current_workflow_run_id": workflow_run_id,
            "current_patch_id": patch_id,
            "development_state": development_state,
            "scheduled_at": locked_at,
            "locked_at": locked_at,
            "completed_at": completed_at,
            "development_started_at": Value::Null,
            "development_completed_at": completed_at,
            "integration_completed_at": Value::Null,
            "applied_at": completed_at
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

async fn refresh_feature_pool_work_unit_statuses(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    let rows = sqlx::query(
        r#"
        SELECT id, feature_id, workflow_run_id
        FROM supervisor_work_units
        WHERE supervisor_run_id = ?
          AND kind = 'feature_development'
          AND state NOT IN ('deleted', 'archived')
          AND TRIM(COALESCE(workflow_run_id, '')) != ''
        "#,
    )
    .bind(run.id.to_string())
    .fetch_all(&state.db)
    .await?;

    let now = Utc::now().to_rfc3339();

    for row in rows {
        let work_unit_id: String = row.get("id");
        let feature_id = row.try_get::<Option<String>, _>("feature_id").ok().flatten();
        let workflow_run_id_text: String = row.get("workflow_run_id");
        let Ok(workflow_run_id) = Uuid::parse_str(&workflow_run_id_text) else {
            continue;
        };
        let Ok(child_run) = engine::load_run(state, workflow_run_id).await else {
            continue;
        };

        let work_unit_state = match child_run.status {
            RunStatus::Draft => "draft",
            RunStatus::Queued => "queued",
            RunStatus::Running => "running",
            RunStatus::Waiting => "waiting",
            RunStatus::Paused => "paused",
            RunStatus::Success => "complete",
            RunStatus::Error => "error",
            RunStatus::Cancelled => "cancelled",
        };

        sqlx::query("UPDATE supervisor_work_units SET state = ?, updated_at = ? WHERE id = ? AND state NOT IN ('deleted', 'archived')")
            .bind(work_unit_state)
            .bind(&now)
            .bind(&work_unit_id)
            .execute(&state.db)
            .await?;

        if let Some(feature_id) = feature_id.as_deref() {
            sqlx::query("UPDATE planner_features SET locked_supervisor_run_id = COALESCE(locked_supervisor_run_id, ?), locked_at = COALESCE(locked_at, ?), completed_at = CASE WHEN ? = 'complete' THEN COALESCE(completed_at, ?) ELSE completed_at END, updated_at = ? WHERE id = ?")
                .bind(run.id.to_string())
                .bind(&now)
                .bind(work_unit_state)
                .bind(&now)
                .bind(&now)
                .bind(feature_id)
                .execute(&state.db)
                .await?;
        }
    }

    *run = load_supervisor_run(state, run.id).await?;
    Ok(())
}

async fn start_next_feature_pool_work_units(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    let feature_concurrency = supervisor_feature_concurrency(run);
    let mut active_count = sqlx::query(
        r#"
        SELECT COUNT(*) AS count
        FROM supervisor_work_units wu
        LEFT JOIN workflow_runs wr ON wr.id = wu.workflow_run_id
        WHERE wu.supervisor_run_id = ?
          AND wu.kind = 'feature_development'
          AND wu.archived_at IS NULL
          AND (
              wu.state = 'starting'
              OR wr.status IN ('running', 'waiting', 'paused')
          )
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
          AND wu.kind = 'feature_development'
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

    refresh_feature_pool_work_unit_statuses(state, run).await?;
    start_next_feature_pool_work_units(state, run).await?;
    run.status = SupervisorStatus::RunningChildren;
    run.updated_at = Utc::now();
    update_supervisor_run(state, run).await?;
    Ok(())
}

fn workflow_template_ref_from_object(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| {
            value
                .get(*key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
}

fn flight_deck_pool_template_ref(settings: &Value, pool_key: &str) -> Option<String> {
    settings
        .get("pools")
        .and_then(|value| value.get(pool_key))
        .and_then(|value| workflow_template_ref_from_object(value, &["template_id", "workflow_template_id", "template"]))
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

async fn resolve_flight_deck_pool_template_id(state: &AppState, payload: &Value, settings: &Value, pool_key: &str, top_level_keys: &[&str]) -> Result<Option<Uuid>> {
    let candidate = workflow_template_ref_from_object(payload, top_level_keys)
        .or_else(|| workflow_template_ref_from_object(settings, top_level_keys))
        .or_else(|| flight_deck_pool_template_ref(settings, pool_key));
    match candidate {
        Some(value) => resolve_workflow_template_ref_id(state, &value).await,
        None => Ok(None),
    }
}

async fn resolve_supervisor_integration_template_id(state: &AppState, run: &SupervisorRun) -> Result<Option<Uuid>> {
    if let Some(template_id) = context_uuid(&run.context, "integration_template_id") {
        return Ok(Some(template_id));
    }
    let candidate = run
        .context
        .get("flight_deck_settings")
        .and_then(|settings| flight_deck_pool_template_ref(settings, "integration"));
    match candidate {
        Some(value) => resolve_workflow_template_ref_id(state, &value).await,
        None => Ok(None),
    }
}

pub async fn update_supervisor_flight_deck_settings(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    if !run.context.is_object() {
        run.context = json!({});
    }

    let mut settings = payload
        .get("flight_deck_settings")
        .or_else(|| payload.get("settings"))
        .cloned()
        .unwrap_or_else(|| payload.clone());
    let selected_planner_id = payload
        .get("selected_planner_id")
        .or_else(|| settings.get("selected_planner_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let feature_pool_settings = settings
        .get("pools")
        .and_then(|value| value.get("feature_development"))
        .cloned();
    let integration_pool_settings = settings
        .get("pools")
        .and_then(|value| value.get("integration"))
        .cloned();
    let feature_concurrency = payload
        .get("feature_concurrency")
        .and_then(Value::as_u64)
        .or_else(|| settings.get("feature_concurrency").and_then(Value::as_u64))
        .or_else(|| feature_pool_settings.as_ref().and_then(|value| value.get("feature_concurrency")).and_then(Value::as_u64))
        .or_else(|| feature_pool_settings.as_ref().and_then(|value| value.get("concurrency")).and_then(Value::as_u64))
        .or_else(|| feature_pool_settings.as_ref().and_then(|value| value.get("max_concurrency")).and_then(Value::as_u64));
    let integration_policy = payload
        .get("integration_policy")
        .and_then(Value::as_str)
        .or_else(|| settings.get("integration_policy").and_then(Value::as_str))
        .or_else(|| integration_pool_settings.as_ref().and_then(|value| value.get("integration_policy")).and_then(Value::as_str))
        .or_else(|| integration_pool_settings.as_ref().and_then(|value| value.get("mode")).and_then(Value::as_str))
        .filter(|value| matches!(*value, "auto" | "manual"))
        .map(str::to_string);
    let feature_template_id = resolve_flight_deck_pool_template_id(
        state,
        &payload,
        &settings,
        "feature_development",
        &["workflow_template_id", "template_id"],
    ).await?;
    let integration_template_id = resolve_flight_deck_pool_template_id(
        state,
        &payload,
        &settings,
        "integration",
        &["integration_template_id", "template_id"],
    ).await?;
    let refinement_template_id = resolve_flight_deck_pool_template_id(
        state,
        &payload,
        &settings,
        "refine",
        &["planner_refinement_template_id"],
    ).await?;

    if let Some(obj) = settings.as_object_mut() {
        let execution_event_limit = obj
            .get("execution_event_limit")
            .and_then(Value::as_u64)
            .unwrap_or(100)
            .clamp(10, 1000);
        obj.insert(
            "execution_event_limit".to_string(),
            Value::Number(execution_event_limit.into()),
        );
    }

    if let Some(obj) = settings.as_object_mut() {
        obj.remove("selected_planner_id");
        obj.remove("queue_planner_id");
        obj.remove("planner_workspace_id");
        obj.remove("planner_id");
        obj.remove("active_planner_id");
    }

    if let Some(obj) = run.context.as_object_mut() {
        obj.insert("flight_deck_settings".to_string(), settings.clone());
        obj.remove("queue_planner_id");
        obj.remove("selected_planner_id");
        obj.remove("planner_workspace_id");
        obj.remove("planner_id");
        obj.remove("active_planner_id");
        if let Some(template_id) = feature_template_id {
            obj.insert("workflow_template_id".to_string(), Value::String(template_id.to_string()));
        }
        if let Some(template_id) = integration_template_id {
            obj.insert("integration_template_id".to_string(), Value::String(template_id.to_string()));
        }
        if let Some(template_id) = refinement_template_id {
            obj.insert("planner_refinement_template_id".to_string(), Value::String(template_id.to_string()));
        }
        if let Some(feature_concurrency) = feature_concurrency {
            obj.insert("feature_concurrency".to_string(), Value::Number(feature_concurrency.max(1).min(64).into()));
        }
        if let Some(integration_policy) = integration_policy {
            obj.insert("integration_policy".to_string(), Value::String(integration_policy.to_string()));
        }
    }

    kick_feature_pool_if_running(state, &mut run).await?;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;

    if let Some(planner_id) = selected_planner_id.as_deref() {
        sqlx::query(
            "UPDATE supervisor_runs SET selected_planner_id = ?, updated_at = ? WHERE id = ?",
        )
        .bind(planner_id)
        .bind(run.updated_at.to_rfc3339())
        .bind(id.to_string())
        .execute(&state.db)
        .await?;
    }

    let run = load_supervisor_run(state, id).await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

async fn selected_planner_id_for_supervisor(state: &AppState, id: Uuid) -> Result<Option<String>> {
    let planner_id = sqlx::query_scalar::<_, String>(
        r#"
        SELECT selected_planner_id
        FROM supervisor_runs
        WHERE id = ?
          AND archived_at IS NULL
          AND TRIM(COALESCE(selected_planner_id, '')) != ''
        LIMIT 1
        "#,
    )
    .bind(id.to_string())
    .fetch_optional(&state.db)
    .await?;

    Ok(planner_id)
}

async fn resolve_feature_pool_template_id(
    state: &AppState,
    run: &SupervisorRun,
) -> Result<Uuid> {
    let candidate = context_uuid(&run.context, "workflow_template_id")
        .map(|value| value.to_string())
        .or_else(|| run.context.get("flight_deck_settings").and_then(|settings| flight_deck_pool_template_ref(settings, "feature_development")))
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
          AND kind = 'feature_development'
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

pub async fn select_supervisor_feature_pool(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let pool_was_paused = matches!(run.status, SupervisorStatus::Paused);
    let persisted_planner_id = selected_planner_id_for_supervisor(state, id).await?;
    let active_planner_id = persisted_planner_id
        .as_deref()
        .ok_or_else(|| anyhow!("supervisor has no selected planner"))?;
    let (_queue_planner_id, _queue_planner_title, persisted_features) =
        queue_planner_features(state, &run.root_repo_path, Some(active_planner_id)).await?;
    if !persisted_features.is_empty() {
        let mut merged = run.feature_plan_items.clone();
        for feature in persisted_features {
            if let Some(existing) = merged.iter_mut().find(|item| item.id == feature.id) {
                *existing = feature;
            } else {
                merged.push(feature);
            }
        }
        run.feature_plan_items = merged;
    }

    let mut queued_features = import_queued_features(payload.get("queued_features"));
    let mut selected_seen = HashSet::<String>::new();
    queued_features.retain(|item| selected_seen.insert(item.feature_id.clone()));
    let selected_feature_ids = queued_features.iter().map(|item| item.feature_id.clone()).collect::<Vec<_>>();

    let existing_rows = sqlx::query(
        r#"
        SELECT id AS feature_id
        FROM planner_features
        WHERE locked_supervisor_run_id = ?
          AND COALESCE(completed_at, '') = ''
          AND COALESCE(status, '') != 'deleted'
        "#,
    )
    .bind(run.id.to_string())
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
        let is_existing_queue_member = existing_queued_feature_ids.contains(&queued_feature.feature_id);

        if !is_existing_queue_member && queued_feature.planner_id != active_planner_id {
            return Err(anyhow!(
                "new feature '{}' must belong to the currently selected planner '{}'",
                queued_feature.feature_id,
                active_planner_id
            ));
        }

        let row = sqlx::query(
            r#"
            SELECT id, planner_id, title, status, payload_json, locked_supervisor_run_id
            FROM planner_features
            WHERE planner_id = ?
              AND id = ?
              AND id NOT LIKE 'manual-%'
              AND COALESCE(status, '') != 'deleted'
            LIMIT 1
            "#,
        )
        .bind(&queued_feature.planner_id)
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

        if !matches!(status.as_str(), "fine" | "refined" | "approved" | "scheduled") {
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

    if !run.context.is_object() {
        run.context = json!({});
    }
    if let Some(obj) = run.context.as_object_mut() {
        if let Some(template_id) = requested_template_id {
            obj.insert("workflow_template_id".to_string(), Value::String(template_id.to_string()));
        }
        if let Some(template_id) = payload.get("integration_template_id").and_then(Value::as_str).filter(|value| !value.trim().is_empty()) {
            obj.insert("integration_template_id".to_string(), Value::String(template_id.to_string()));
        }
        if let Some(feature_concurrency) = payload.get("feature_concurrency").and_then(Value::as_u64) {
            obj.insert("feature_concurrency".to_string(), Value::Number(feature_concurrency.max(1).min(64).into()));
        }
        if let Some(integration_policy) = payload.get("integration_policy").and_then(Value::as_str).filter(|value| matches!(*value, "auto" | "manual")) {
            obj.insert("integration_policy".to_string(), Value::String(integration_policy.to_string()));
        }
    }

    run.execution_plan_items = selected_feature_ids
        .iter()
        .enumerate()
        .map(|(index, feature_id)| ExecutionPlanItem {
            feature_plan_item_id: feature_id.clone(),
            workflow_template_id: requested_template_id,
            order_index: Some(index as i64),
        })
        .collect();

    let next_feature_ids = run
        .execution_plan_items
        .iter()
        .map(|item| item.feature_plan_item_id.clone())
        .collect::<Vec<_>>();
    queued_features.retain(|item| next_feature_ids.iter().any(|feature_id| feature_id == &item.feature_id));
    if let Some(obj) = run.context.as_object_mut() {
        obj.insert("queued_features".to_string(), serde_json::to_value(&queued_features)?);
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
              AND COALESCE(status, '') != 'deleted'
              AND COALESCE(completed_at, '') = ''
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
              AND COALESCE(status, '') != 'deleted'
              AND COALESCE(completed_at, '') = ''
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
            SELECT feature_id, shard_path
            FROM supervisor_work_units
            WHERE supervisor_run_id = ?
              AND kind = 'feature_development'
              AND feature_id IN (SELECT value FROM json_each(?))
              AND state NOT IN ('deleted', 'archived')
              AND TRIM(COALESCE(shard_path, '')) != ''
            "#,
        )
        .bind(run.id.to_string())
        .bind(&next_json)
        .fetch_all(&state.db)
        .await?;

        let existing_shard_by_feature_id = existing_rows
            .into_iter()
            .map(|row| (row.get::<String, _>("feature_id"), row.get::<String, _>("shard_path")))
            .collect::<HashMap<_, _>>();

        let mut materialized_feature_units = Vec::new();
        for (index, feature_id) in next_feature_ids.iter().enumerate() {
            if let Some(existing_shard_path) = existing_shard_by_feature_id.get(feature_id) {
                materialized_feature_units.push(json!({
                    "feature_id": feature_id,
                    "queue_position": index,
                    "shard_path": existing_shard_path
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
                ? || ':' || pf.id AS id,
                ? AS supervisor_run_id,
                NULL AS repo_id,
                pf.id AS feature_id,
                NULL AS workflow_run_id,
                NULL AS patch_id,
                'feature_development' AS kind,
                pf.title,
                'queued' AS state,
                pw.root_repo_path,
                json_extract(json_each.value, '$.shard_path') AS shard_path,
                NULL AS integration_path,
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
                    'workflow_type', 'feature_development',
                    'pool_key', 'feature_development',
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

pub async fn refine_supervisor_feature(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let feature_id = payload
        .get("feature_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("feature_id is required"))?
        .to_string();
    let planner_id = selected_planner_id_for_supervisor(state, id)
        .await?
        .ok_or_else(|| anyhow!("supervisor has no selected planner"))?;
    let planner_feature = sqlx::query(
        "SELECT id, title, payload_json FROM planner_features WHERE planner_id = ? AND id = ? AND id NOT LIKE 'manual-%' AND COALESCE(status, '') != 'deleted' LIMIT 1",
    )
    .bind(&planner_id)
    .bind(&feature_id)
    .fetch_optional(&state.db)
    .await?
    .and_then(|row| {
        let payload_json: String = row.get("payload_json");
        let mut item = serde_json::from_str::<FeaturePlanItem>(&payload_json).ok()?;
        item.id = row.get("id");
        item.title = row.get("title");
        Some(item)
    });
    let feature = planner_feature.ok_or_else(|| {
        anyhow!(
            "planner feature '{}' was not found in planner '{}'",
            feature_id,
            planner_id
        )
    })?;
    let workflow_template_id = payload
        .get("workflow_template_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .and_then(|value| Uuid::parse_str(value).ok())
        .or_else(|| context_uuid(&run.context, "planner_refinement_template_id"))
        .or_else(|| context_uuid(&run.context, "workflow_template_id"));
    let workflow_template_id = match workflow_template_id {
        Some(value) => value,
        None => default_refinement_workflow_template_id(state)
            .await?
            .ok_or_else(|| anyhow!("workflow_template_id is required for feature refinement"))?,
    };
    let work_unit_id = format!("{}:refine:{}", run.id, feature.id);
    let spawn_result = lifecycle::spawn_supervisor_workflow(
        state,
        SupervisorWorkflowSpawnRequest {
            supervisor_run_id: run.id,
            root_repo_path: run.root_repo_path.clone(),
            pool_kind: SupervisorPoolKind::Refine,
            work_unit_id: work_unit_id.clone(),
            shard_id: None,
            feature_id: Some(feature.id.clone()),
            title: feature.title.clone(),
            item: feature.clone(),
            template_id: Some(workflow_template_id),
            workflow_context: json!({
                "supervisor_run_id": run.id,
                "planner_id": planner_id,
                "feature_id": feature.id,
                "input_source": "supervisor_planner_feature"
            }),
            work_unit_context: json!({
                "source": "supervisor_planner_feature",
                "pool_key": "refine",
                "feature_id": feature.id,
                "template_id": workflow_template_id
            }),
            initial_state: "queued".to_string(),
            priority: 0,
            queue_position: None,
        },
    )
    .await?;
    let workflow_run_id = spawn_result.workflow_run_id;
    set_planner_feature_refinement_workflow_run(
        &state.db,
        &planner_id,
        &feature_id,
        workflow_run_id,
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

async fn workflow_run_definition_snapshot(state: &AppState, run_id: &str) -> Result<Option<WorkflowTemplateDefinition>> {
    let row = sqlx::query("SELECT definition_json FROM workflow_runs WHERE id = ?")
        .bind(run_id)
        .fetch_optional(&state.db)
        .await?;

    row.map(|row| serde_json::from_str::<WorkflowTemplateDefinition>(row.get::<String, _>("definition_json").as_str()).map_err(Into::into))
        .transpose()
}

pub async fn restart_current_supervisor_sprint(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    if run.execution_plan_items.is_empty() {
        return Err(anyhow!("sprint has no scheduled planner items"));
    }
    if matches!(run.status, SupervisorStatus::Applied) {
        return Err(anyhow!("applied sprints cannot be restarted; start the next sprint instead"));
    }

    let sprint_id = run.context.get("current_sprint_id").and_then(Value::as_str).map(str::to_string);
    let mut workflow_run_ids = if let Some(sprint_id) = sprint_id.as_deref() {
        sprint_feature_workflow_ids(state, sprint_id).await?
    } else {
        Vec::new()
    };
    if let Some(integration_run_id) = run.integration_run_id {
        workflow_run_ids.push(integration_run_id);
    }
    workflow_run_ids.sort();
    workflow_run_ids.dedup();
    for workflow_run_id in workflow_run_ids {
        delete_supervisor_workflow_run_records(state, workflow_run_id).await?;
    }

    if let Ok(workspace) = repo_snapshot::workspace_for(&run.root_repo_path, run.id) {
        if workspace.root.exists() {
            fs::remove_dir_all(&workspace.root)
                .with_context(|| format!("failed to clear {}", workspace.root.display()))?;
        }
    }

    run.integration_run_id = None;
    run.final_patch_path = None;
    run.merge_report = json!({});
    run.validation_report = json!({});
    run.snapshot_path = None;
    run.integration_path = None;
    run.status = SupervisorStatus::Created;
    if let Some(obj) = run.context.as_object_mut() {
        obj.remove("current_sprint_id");
        obj.remove("current_sprint_key");
        obj.remove("current_sprint_started_at");
    }
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;

    start_supervisor_run(state, id).await
}

pub async fn start_next_supervisor_sprint(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    if !matches!(run.status, SupervisorStatus::Applied | SupervisorStatus::ReadyToApply | SupervisorStatus::Failed | SupervisorStatus::Cancelled) {
        return Err(anyhow!("current sprint must be completed, ready, failed, or cancelled before starting another sprint"));
    }
    run.execution_plan_items.clear();
    run.integration_run_id = None;
    run.final_patch_path = None;
    run.merge_report = json!({});
    run.validation_report = json!({});
    run.snapshot_path = None;
    run.integration_path = None;
    run.status = SupervisorStatus::Created;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

pub async fn delete_supervisor_run(state: &AppState, id: Uuid) -> Result<()> {
    sqlx::query("DELETE FROM supervisor_runs WHERE id = ?")
        .bind(id.to_string())
        .execute(&state.db)
        .await?;
    Ok(())
}

pub async fn cancel_supervisor_run(state: &AppState, id: Uuid) -> Result<Value> {
    update_status(state, id, SupervisorStatus::Cancelled).await?;
    Ok(json!({ "ok": true, "status": "cancelled" }))
}

fn supervisor_start_is_idempotent(status: &SupervisorStatus) -> bool {
    matches!(
        status,
        SupervisorStatus::Snapshotting
            | SupervisorStatus::RunningChildren
            | SupervisorStatus::DevelopmentComplete
            | SupervisorStatus::RunningIntegration
            | SupervisorStatus::Validating
            | SupervisorStatus::ReadyToApply
            | SupervisorStatus::Applied
    )
}

pub async fn start_supervisor_run(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;

    if supervisor_start_is_idempotent(&run.status) {
        if matches!(run.status, SupervisorStatus::RunningChildren | SupervisorStatus::DevelopmentComplete | SupervisorStatus::RunningIntegration | SupervisorStatus::Validating) {
            let _ = advance_supervisor_run(state, id).await?;
        }
        let run = load_supervisor_run(state, id).await?;
        return Ok(json!({
            "ok": true,
            "idempotent": true,
            "supervisor_run": run
        }));
    }

    if run.execution_plan_items.is_empty() {
        return Err(anyhow!("integration batch has no selected Flight Deck feature-pool features"));
    }
    run.status = SupervisorStatus::Snapshotting;
    if !run.context.is_object() {
        run.context = json!({});
    }
    let sprint_id = Uuid::new_v4().to_string();
    let sprint_key = sprint_key_for(&run.root_repo_path, &sprint_id);
    let sprint_started_at = Utc::now().to_rfc3339();
    if let Some(obj) = run.context.as_object_mut() {
        obj.insert("current_sprint_id".to_string(), Value::String(sprint_id.clone()));
        obj.insert("current_sprint_key".to_string(), Value::String(sprint_key.clone()));
        obj.insert("current_sprint_started_at".to_string(), Value::String(sprint_started_at.clone()));
        obj.insert("current_integration_batch_id".to_string(), Value::String(sprint_id.clone()));
        obj.insert("current_integration_batch_key".to_string(), Value::String(sprint_key.clone()));
        obj.insert("current_integration_batch_started_at".to_string(), Value::String(sprint_started_at.clone()));
    }
    let scheduled_items = scheduled_feature_plan_items(&run)?;
    upsert_sprint_record(state, &run, &sprint_id, &sprint_key, &format!("Integration batch {}", sprint_key), "running", Some(&sprint_started_at), None).await?;
    save_sprint_features(state, &run, &sprint_id, None).await?;
    append_sprint_event(state, &sprint_id, "integration_batch_started", &sprint_started_at, None, "integration batch started", json!({ "integration_batch_key": sprint_key })).await?;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;

    let workspace = match repo_snapshot::create_workspace(&run.root_repo_path, run.id, &scheduled_items) {
        Ok(workspace) => workspace,
        Err(err) => {
            run.status = SupervisorStatus::Failed;
            run.updated_at = Utc::now();
            update_supervisor_run(state, &run).await?;
            return Err(err);
        }
    };
    let workspace = repo_snapshot::refresh_integration_from_worktree(&run.root_repo_path, run.id)?;
    patches::create_baseline(&workspace.integration)?;
    let workflow_template_id = context_uuid(&run.context, "workflow_template_id");
    let integration_template_id = match run.strategy {
        SupervisorExecutionStrategy::Parallel => resolve_supervisor_integration_template_id(state, &run).await?,
        SupervisorExecutionStrategy::Series => None,
    };

    for item in &scheduled_items {
        let shard_id = Uuid::new_v4();
        let shard = repo_snapshot::refresh_shard_from_worktree(&run.root_repo_path, run.id, shard_id)?;
        patches::create_baseline(&shard)?;
        let shard_path = shard.to_string_lossy().to_string();
        let template_id = run.execution_plan_items
            .iter()
            .find(|execution_item| execution_item.feature_plan_item_id == item.id)
            .and_then(|execution_item| execution_item.workflow_template_id)
            .or(workflow_template_id);
        let workflow_run_id = workflow_spawn::spawn_feature_plan_item_workflow(
            state,
            item,
            &shard_path,
            template_id,
            supervisor_context(&run, &workspace),
        ).await?;
        sqlx::query("UPDATE sprint_features SET supervisor_run_id = ?, current_workflow_run_id = ?, shard_path = ?, status = 'scheduled', development_state = 'scheduled', updated_at = ? WHERE sprint_id = ? AND feature_id = ?")
            .bind(run.id.to_string())
            .bind(workflow_run_id.to_string())
            .bind(&shard_path)
            .bind(Utc::now().to_rfc3339())
            .bind(&sprint_id)
            .bind(&item.id)
            .execute(&state.db)
            .await?;
        upsert_supervisor_work_unit_for_feature(state, run.id, &sprint_id, &item.id).await?;
    }
    run.status = SupervisorStatus::RunningChildren;

    if matches!(run.strategy, SupervisorExecutionStrategy::Parallel) {
        if let Some(template_id) = integration_template_id {
            if let Some(obj) = run.context.as_object_mut() {
                obj.insert("integration_template_id".to_string(), Value::String(template_id.to_string()));
            }
        }
    }

    run.snapshot_path = None;
    run.integration_path = Some(workspace.integration.to_string_lossy().to_string());
    save_repo_feature_plan_items(state, &run.root_repo_path, &run.feature_plan_items).await?;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;

    start_next_series_child(state, &mut run).await?;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;

    Ok(json!({ "ok": true, "supervisor_run": run }))
}

fn supervisor_context_uuid(context: &Value, key: &str) -> Option<Uuid> {
    context.get(key).and_then(Value::as_str).and_then(|value| Uuid::parse_str(value).ok())
}

fn supervisor_context_string(context: &Value, key: &str) -> Option<String> {
    context.get(key).and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty()).map(str::to_string)
}

fn workflow_terminal_event_type(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Success => "workflow_completed",
        RunStatus::Error => "workflow_failed",
        RunStatus::Cancelled => "workflow_cancelled",
        _ => "workflow_terminal",
    }
}

fn workflow_terminal_event_message(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Success => "workflow completed",
        RunStatus::Error => "workflow failed",
        RunStatus::Cancelled => "workflow cancelled",
        _ => "workflow reached terminal status",
    }
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
            "sprint_id": supervisor_context.get("sprint_id").cloned().unwrap_or(Value::Null),
            "feature_id": supervisor_context.get("feature_id").cloned().unwrap_or(Value::Null),
            "input_source": supervisor_context.get("input_source").cloned().unwrap_or(Value::Null),
            "workflow_status": status_str(&status)
        }),
    ).await?;

    let mut run = load_supervisor_run(state, supervisor_id).await?;
    let now = Utc::now().to_rfc3339();
    let input_source = supervisor_context.get("input_source").and_then(Value::as_str).unwrap_or("");
    let pool_type = supervisor_context
        .get("pool_type")
        .or_else(|| supervisor_context.get("pool_key"))
        .and_then(Value::as_str)
        .unwrap_or("");

    let terminal_work_unit_state = match status {
        RunStatus::Success => Some("complete"),
        RunStatus::Error => Some("error"),
        RunStatus::Cancelled => Some("cancelled"),
        _ => None,
    };

    if let Some(next_state) = terminal_work_unit_state {
        let blocked_reason = match status {
            RunStatus::Error => Some("workflow failed"),
            RunStatus::Cancelled => Some("workflow cancelled"),
            _ => None,
        };

        let changed = sqlx::query(
            r#"
            UPDATE supervisor_work_units
            SET state = ?,
                blocked_reason = ?,
                context_json = json_set(
                    CASE WHEN json_valid(context_json) THEN context_json ELSE '{}' END,
                    '$.current_step_id', ?,
                    '$.workflow_status', ?,
                    '$.terminal_at', ?
                ),
                updated_at = ?
            WHERE supervisor_run_id = ?
              AND workflow_run_id = ?
              AND kind IN ('feature_development', 'manual_shard')
              AND state NOT IN ('deleted', 'archived')
            "#,
        )
        .bind(next_state)
        .bind(blocked_reason)
        .bind(current_step_id)
        .bind(status_str(&status))
        .bind(&now)
        .bind(&now)
        .bind(supervisor_id.to_string())
        .bind(workflow_run_id.to_string())
        .execute(&state.db)
        .await?
        .rows_affected();

        if changed > 0 {
            if matches!(run.status, SupervisorStatus::RunningChildren) {
                start_next_feature_pool_work_units(state, &mut run).await?;
            }
            run.updated_at = Utc::now();
            update_supervisor_run(state, &run).await?;
            publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "supervisor work-unit terminal event processed").await?;
            return Ok(());
        }
    }

    if input_source == "supervisor_sprint_feature" {
        let sprint_id = supervisor_context_string(&supervisor_context, "sprint_id")
            .or_else(|| run.context.get("current_sprint_id").and_then(Value::as_str).map(str::to_string));
        let feature_id = supervisor_context_string(&supervisor_context, "feature_id");
        if let Some(feature_id) = feature_id.as_deref() {
            match status {
                RunStatus::Success => {
                    if let Some(item) = run.feature_plan_items.iter_mut().find(|item| item.id == feature_id) {
                        item.status = FeaturePlanItemStatus::Completed;
                    }
                    if let Some(sprint_id) = sprint_id.as_deref() {
                        update_sprint_feature_workflow_state(state, sprint_id, feature_id, Some(workflow_run_id), "completed", "development_succeeded", current_step_id, None).await?;
                    }
                }
                RunStatus::Error => {
                    if let Some(sprint_id) = sprint_id.as_deref() {
                        update_sprint_feature_workflow_state(state, sprint_id, feature_id, Some(workflow_run_id), "error", "development_failed", current_step_id, Some("workflow failed")).await?;
                    }
                }
                RunStatus::Cancelled => {
                    if let Some(sprint_id) = sprint_id.as_deref() {
                        update_sprint_feature_workflow_state(state, sprint_id, feature_id, Some(workflow_run_id), "cancelled", "development_failed", current_step_id, Some("workflow cancelled")).await?;
                    }
                }
                _ => {}
            }
        }

        if let Some(sprint_id) = sprint_id.as_deref() {
            append_sprint_event(
                state,
                sprint_id,
                workflow_terminal_event_type(&status),
                &now,
                feature_id.as_deref(),
                workflow_terminal_event_message(&status),
                json!({
                    "workflow_run_id": workflow_run_id,
                    "workflow_status": status_str(&status),
                    "current_step_id": current_step_id
                }),
            ).await?;
        }

        if let Some(sprint_id) = sprint_id.as_deref() {
            let (total, succeeded, failed) = sprint_development_terminal_counts(state, sprint_id).await?;
            if failed > 0 {
                run.status = SupervisorStatus::Failed;
            } else if total > 0 && succeeded >= total {
                run.status = SupervisorStatus::DevelopmentComplete;
                sqlx::query("UPDATE sprints SET status = ?, development_completed_at = COALESCE(development_completed_at, ?), updated_at = ? WHERE id = ?")
                    .bind("development_complete")
                    .bind(&now)
                    .bind(&now)
                    .bind(sprint_id)
                    .execute(&state.db)
                    .await?;
                append_sprint_event(
                    state,
                    sprint_id,
                    "development_completed",
                    &now,
                    None,
                    "all feature workflows completed",
                    json!({
                        "supervisor_run_id": run.id,
                        "execution_source": "sprint_features",
                        "integration_policy": supervisor_integration_policy(&run)
                    }),
                ).await?;
                if supervisor_integration_policy(&run) == "auto" {
                    spawn_live_integration_workflow(state, &mut run).await?;
                }
            } else {
                start_next_series_child(state, &mut run).await?;
            }
        }

        run.updated_at = Utc::now();
        update_supervisor_run(state, &run).await?;
        publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "feature workflow terminal event processed").await?;
        return Ok(());
    }

    if run.integration_run_id == Some(workflow_run_id) || pool_type == "integration" {
        match status {
            RunStatus::Success => {
                run.status = SupervisorStatus::ReadyToApply;
                if let Some(sprint_id) = run.context.get("current_sprint_id").and_then(Value::as_str) {
                    sqlx::query("UPDATE sprints SET status = ?, integration_completed_at = COALESCE(integration_completed_at, ?), updated_at = ? WHERE id = ?")
                        .bind("ready_to_apply")
                        .bind(&now)
                        .bind(&now)
                        .bind(sprint_id)
                        .execute(&state.db)
                        .await?;
                    append_sprint_event(state, sprint_id, "integration_completed", &now, None, "integration workflow completed", json!({
                        "workflow_run_id": workflow_run_id,
                        "patch_source": "integration_workflow_runtime"
                    })).await?;
                }
            }
            RunStatus::Error | RunStatus::Cancelled => {
                run.status = SupervisorStatus::Failed;
                if let Some(sprint_id) = run.context.get("current_sprint_id").and_then(Value::as_str) {
                    append_sprint_event(state, sprint_id, workflow_terminal_event_type(&status), &now, None, workflow_terminal_event_message(&status), json!({ "workflow_run_id": workflow_run_id })).await?;
                }
            }
            _ => {}
        }
        run.updated_at = Utc::now();
        update_supervisor_run(state, &run).await?;
        publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "integration workflow terminal event processed").await?;
    }

    Ok(())
}

fn payload_feature_id(payload: &Value) -> Result<String> {
    payload
        .get("feature_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("feature_id is required"))
}

async fn supervisor_child_workflow_row(
    state: &AppState,
    sprint_id: &str,
    feature_id: &str,
) -> Result<(String, Uuid, String)> {
    let row = sqlx::query(
        "SELECT feature_id, current_workflow_run_id, development_state
         FROM sprint_features
         WHERE sprint_id = ? AND feature_id = ?",
    )
    .bind(sprint_id)
    .bind(feature_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| anyhow!("feature workflow is missing"))?;

    let feature_id: String = row.get("feature_id");
    let workflow_run_id_text: String = row
        .try_get::<Option<String>, _>("current_workflow_run_id")
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("feature workflow run is missing"))?;
    let development_state: String = row.get("development_state");
    let workflow_run_id = Uuid::parse_str(&workflow_run_id_text)?;

    Ok((feature_id, workflow_run_id, development_state))
}

async fn resolve_manual_shard_template_id(state: &AppState, run: &SupervisorRun, payload: &Value) -> Result<Uuid> {
    let candidates = ["workflow_template_id", "template_id", "template"];
    for key in candidates {
        let Some(raw) = payload.get(key).and_then(Value::as_str).filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        if let Ok(uuid) = Uuid::parse_str(raw) {
            return Ok(uuid);
        }
        if let Some(row) = sqlx::query("SELECT id FROM workflow_templates WHERE id = ? OR name = ? LIMIT 1")
            .bind(raw)
            .bind(raw)
            .fetch_optional(&state.db)
            .await?
        {
            let id_text: String = row.get("id");
            return Ok(Uuid::parse_str(&id_text)?);
        }
    }

    context_uuid(&run.context, "workflow_template_id")
        .ok_or_else(|| anyhow!("manual shard workflow template is required"))
}

pub async fn create_supervisor_manual_shard(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let template_id = resolve_manual_shard_template_id(state, &run, &payload).await?;
    let now = Utc::now().to_rfc3339();
    let manual_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM supervisor_work_units WHERE supervisor_run_id = ? AND kind = 'manual_shard'",
    )
    .bind(id.to_string())
    .fetch_one(&state.db)
    .await?
    .max(0);
    let manual_id = format!("manual-{}", Uuid::new_v4());
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Manual shard {}", manual_count + 1));

    let mut item = FeaturePlanItem {
        id: manual_id.clone(),
        title: title.clone(),
        status: FeaturePlanItemStatus::Scheduled,
        summary: "Operator-created manual shard.".to_string(),
        rough_summary: None,
        refinement_workflow_run_id: None,
        applied_sprint_id: None,
        applied_sprint_title: None,
        applied_at: None,
        requirements: Vec::new(),
        acceptance_criteria: Vec::new(),
        implementation_notes: vec!["Manual shard created from flight deck.".to_string()],
        review_expectations: Vec::new(),
        target_files_or_areas: Vec::new(),
        dependencies: Vec::new(),
    };

    if let Some(summary) = payload.get("summary").and_then(Value::as_str).filter(|value| !value.trim().is_empty()) {
        item.summary = summary.to_string();
    }

    let workspace = repo_snapshot::workspace_for(&run.root_repo_path, run.id)?;
    let work_unit_id = format!("{}:manual:{}", run.id, manual_id);
    let mut supervisor_ctx = supervisor_context(&run, &workspace);
    if let Some(obj) = supervisor_ctx.as_object_mut() {
        obj.insert("input_source".to_string(), Value::String("supervisor_manual_shard".to_string()));
        obj.insert("manual_shard_id".to_string(), Value::String(manual_id.clone()));
        obj.insert("manual_shard".to_string(), Value::Bool(true));
    }

    let promised_context = json!({
        "source": "manual_shard",
        "workflow_type": "manual_shard",
        "pool_key": "manual_shard",
        "template_id": template_id,
        "planned_workflow_template_id": template_id,
        "manual_shard_id": manual_id,
        "feature_id": manual_id,
        "status": "queued",
        "materialization_state": "pending"
    });

    lifecycle::promise_supervisor_work_unit(
        state,
        run.id,
        &run.root_repo_path,
        SupervisorPoolKind::ManualShard,
        &work_unit_id,
        Some(&manual_id),
        &title,
        Some(template_id),
        promised_context,
        0,
        None,
    )
    .await?;

    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(
        state,
        &run,
        "supervisor_snapshot",
        "manual shard queued for materialization",
    )
    .await?;

    let spawn_request = SupervisorWorkflowSpawnRequest {
        supervisor_run_id: run.id,
        root_repo_path: run.root_repo_path.clone(),
        pool_kind: SupervisorPoolKind::ManualShard,
        work_unit_id: work_unit_id.clone(),
        shard_id: None,
        feature_id: Some(manual_id.clone()),
        title: title.clone(),
        item: item.clone(),
        template_id: Some(template_id),
        workflow_context: supervisor_ctx,
        work_unit_context: json!({
            "source": "manual_shard",
            "workflow_type": "manual_shard",
            "pool_key": "manual_shard",
            "template_id": template_id,
            "planned_workflow_template_id": template_id,
            "manual_shard_id": manual_id,
            "feature_id": manual_id,
            "status": "queued",
            "materialization_state": "pending"
        }),
        initial_state: "queued".to_string(),
        priority: 0,
        queue_position: None,
    };

    let materialization_state = state.clone();
    let materialization_supervisor_id = run.id;
    let materialization_work_unit_id = work_unit_id.clone();

    tokio::spawn(async move {
        match lifecycle::spawn_supervisor_workflow(
            &materialization_state,
            spawn_request,
        )
        .await
        {
            Ok(_) => {
                if let Ok(mut refreshed_run) = load_supervisor_run(
                    &materialization_state,
                    materialization_supervisor_id,
                )
                .await
                {
                    refreshed_run.updated_at = Utc::now();
                    let _ = update_supervisor_run(
                        &materialization_state,
                        &refreshed_run,
                    )
                    .await;
                    let _ = publish_supervisor_snapshot(
                        &materialization_state,
                        &refreshed_run,
                        "supervisor_snapshot",
                        "manual shard materialized",
                    )
                    .await;
                }
            }
            Err(err) => {
                let now = Utc::now().to_rfc3339();
                let error_text = format!("{:#}", err);
                let _ = sqlx::query(
                    r#"
                    UPDATE supervisor_work_units
                    SET state = 'failed',
                        blocked_reason = ?,
                        context_json = json_set(
                            COALESCE(NULLIF(context_json, ''), '{}'),
                            '$.materialization_state',
                            'failed',
                            '$.materialization_error',
                            ?
                        ),
                        updated_at = ?
                    WHERE id = ?
                    "#,
                )
                .bind(&error_text)
                .bind(&error_text)
                .bind(&now)
                .bind(&materialization_work_unit_id)
                .execute(&materialization_state.db)
                .await;

                tracing::error!(
                    supervisor_run_id = %materialization_supervisor_id,
                    work_unit_id = %materialization_work_unit_id,
                    error = %error_text,
                    "failed to materialize promised manual shard"
                );

                if let Ok(refreshed_run) = load_supervisor_run(
                    &materialization_state,
                    materialization_supervisor_id,
                )
                .await
                {
                    let _ = publish_supervisor_snapshot(
                        &materialization_state,
                        &refreshed_run,
                        "supervisor_snapshot",
                        "manual shard materialization failed",
                    )
                    .await;
                }
            }
        }
    });

    Ok(json!({
        "ok": true,
        "manual_shard_id": manual_id,
        "workflow_run_id": null,
        "work_unit_id": work_unit_id,
        "state": "queued",
        "materialization_state": "pending",
        "supervisor_run": run
    }))
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
        SupervisorWorkPoolKind::FeatureDevelopment => SupervisorPoolKind::FeatureDevelopment,
        SupervisorWorkPoolKind::ManualShard => SupervisorPoolKind::ManualShard,
        SupervisorWorkPoolKind::Integration => SupervisorPoolKind::Integration,
    }
}

fn pool_key_from_action_kind(kind: SupervisorWorkPoolKind) -> &'static str {
    match kind {
        SupervisorWorkPoolKind::Refine => "refine",
        SupervisorWorkPoolKind::FeatureDevelopment => "feature_development",
        SupervisorWorkPoolKind::ManualShard => "manual_shard",
        SupervisorWorkPoolKind::Integration => "integration",
    }
}

fn work_unit_running_state(kind: &str) -> &'static str {
    match kind {
        "integration" => "integrating",
        "feature_development" => "development_running",
        _ => "running",
    }
}

fn work_unit_paused_state(kind: &str) -> &'static str {
    match kind {
        "manual_shard" => "draft",
        _ => "waiting_user",
    }
}

async fn load_supervisor_work_unit_row(state: &AppState, supervisor_id: Uuid, work_unit_id: &str) -> Result<sqlx::sqlite::SqliteRow> {
    sqlx::query(
        r#"
        SELECT id, kind, feature_id, title, workflow_run_id, shard_path, context_json, state
        FROM supervisor_work_units
        WHERE supervisor_run_id = ? AND id = ? AND state NOT IN ('deleted', 'archived')
        LIMIT 1
        "#,
    )
    .bind(supervisor_id.to_string())
    .bind(work_unit_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| anyhow!("work unit not found"))
}

pub async fn create_supervisor_work_unit(state: &AppState, id: Uuid, request: CreateSupervisorWorkUnitRequest) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let template_id = request
        .template_id
        .or_else(|| context_uuid(&run.context, "workflow_template_id"))
        .ok_or_else(|| anyhow!("work unit workflow template is required"))?;
    let pool_key = pool_key_from_action_kind(request.pool_kind);
    let feature_id = request
        .feature_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{}-{}", pool_key.replace('_', "-"), Uuid::new_v4()));
    let title = if request.name.trim().is_empty() {
        format!("{} {}", pool_key.replace('_', " "), feature_id)
    } else {
        request.name.trim().to_string()
    };

    let item = FeaturePlanItem {
        id: feature_id.clone(),
        title: title.clone(),
        status: FeaturePlanItemStatus::Scheduled,
        summary: format!("Supervisor managed {} work unit.", pool_key.replace('_', " ")),
        rough_summary: None,
        refinement_workflow_run_id: None,
        applied_sprint_id: None,
        applied_sprint_title: None,
        applied_at: None,
        requirements: Vec::new(),
        acceptance_criteria: Vec::new(),
        implementation_notes: Vec::new(),
        review_expectations: Vec::new(),
        target_files_or_areas: Vec::new(),
        dependencies: Vec::new(),
    };

    let workspace = repo_snapshot::workspace_for(&run.root_repo_path, run.id)?;
    let work_unit_id = format!("{}:{}:{}", run.id, pool_key, feature_id);
    let mut supervisor_ctx = supervisor_context(&run, &workspace);
    if let Some(obj) = supervisor_ctx.as_object_mut() {
        obj.insert("input_source".to_string(), Value::String("supervisor_work_unit".to_string()));
        obj.insert("workflow_type".to_string(), Value::String(pool_key.to_string()));
        obj.insert("pool_key".to_string(), Value::String(pool_key.to_string()));
        obj.insert("template_id".to_string(), Value::String(template_id.to_string()));
        obj.insert("feature_id".to_string(), Value::String(feature_id.clone()));
    }

    let promised_pool_kind = action_pool_kind_to_lifecycle(request.pool_kind);
    let promised_context = json!({
        "source": "supervisor_work_unit",
        "workflow_type": pool_key,
        "pool_key": pool_key,
        "template_id": template_id,
        "planned_workflow_template_id": template_id,
        "feature_id": feature_id,
        "created_from_action": "create_work_unit",
        "status": "queued",
        "materialization_state": "pending"
    });

    lifecycle::promise_supervisor_work_unit(
        state,
        run.id,
        &run.root_repo_path,
        promised_pool_kind,
        &work_unit_id,
        Some(&feature_id),
        &title,
        Some(template_id),
        promised_context,
        0,
        None,
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

    let spawn_request = SupervisorWorkflowSpawnRequest {
        supervisor_run_id: run.id,
        root_repo_path: run.root_repo_path.clone(),
        pool_kind: promised_pool_kind,
        work_unit_id: work_unit_id.clone(),
        shard_id: None,
        feature_id: Some(feature_id.clone()),
        title: title.clone(),
        item,
        template_id: Some(template_id),
        workflow_context: supervisor_ctx,
        work_unit_context: json!({
            "source": "supervisor_work_unit",
            "workflow_type": pool_key,
            "pool_key": pool_key,
            "template_id": template_id,
            "planned_workflow_template_id": template_id,
            "feature_id": feature_id,
            "created_from_action": "create_work_unit",
            "status": "queued",
            "materialization_state": "pending"
        }),
        initial_state: "queued".to_string(),
        priority: 0,
        queue_position: None,
    };

    let materialization_state = state.clone();
    let materialization_supervisor_id = run.id;
    let materialization_work_unit_id = work_unit_id.clone();

    tokio::spawn(async move {
        match lifecycle::spawn_supervisor_workflow(
            &materialization_state,
            spawn_request,
        )
        .await
        {
            Ok(_) => {
                if let Ok(mut refreshed_run) = load_supervisor_run(
                    &materialization_state,
                    materialization_supervisor_id,
                )
                .await
                {
                    refreshed_run.updated_at = Utc::now();
                    let _ = update_supervisor_run(
                        &materialization_state,
                        &refreshed_run,
                    )
                    .await;
                    let _ = publish_supervisor_snapshot(
                        &materialization_state,
                        &refreshed_run,
                        "supervisor_snapshot",
                        "work unit materialized",
                    )
                    .await;
                }
            }
            Err(err) => {
                let now = Utc::now().to_rfc3339();
                let error_text = format!("{:#}", err);
                let _ = sqlx::query(
                    r#"
                    UPDATE supervisor_work_units
                    SET state = 'failed',
                        blocked_reason = ?,
                        context_json = json_set(
                            COALESCE(NULLIF(context_json, ''), '{}'),
                            '$.materialization_state',
                            'failed',
                            '$.materialization_error',
                            ?
                        ),
                        updated_at = ?
                    WHERE id = ?
                    "#,
                )
                .bind(&error_text)
                .bind(&error_text)
                .bind(&now)
                .bind(&materialization_work_unit_id)
                .execute(&materialization_state.db)
                .await;

                tracing::error!(
                    supervisor_run_id = %materialization_supervisor_id,
                    work_unit_id = %materialization_work_unit_id,
                    error = %error_text,
                    "failed to materialize promised supervisor work unit"
                );

                if let Ok(refreshed_run) = load_supervisor_run(
                    &materialization_state,
                    materialization_supervisor_id,
                )
                .await
                {
                    let _ = publish_supervisor_snapshot(
                        &materialization_state,
                        &refreshed_run,
                        "supervisor_snapshot",
                        "work unit materialization failed",
                    )
                    .await;
                }
            }
        }
    });

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
    let kind: String = row.get("kind");
    let feature_id = row.try_get::<Option<String>, _>("feature_id").ok().flatten();
    let workflow_run_id = row.try_get::<Option<String>, _>("workflow_run_id").ok().flatten().and_then(|value| Uuid::parse_str(value.as_str()).ok());
    let shard_path = row.try_get::<Option<String>, _>("shard_path").ok().flatten();

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

    if let Some(path_text) = shard_path.as_deref().filter(|value| !value.trim().is_empty()) {
        let path = PathBuf::from(path_text);
        if path.exists() {
            let result = if path.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
            if let Err(err) = result {
                tracing::warn!(path = %path.display(), error = %format!("{:#}", err), "failed to delete work unit workspace");
            }
        }
    }

    let archived_at = Utc::now().to_rfc3339();
    sqlx::query("UPDATE supervisor_work_units SET state = 'deleted', archived_at = ?, archived_reason = 'work unit deleted', updated_at = ? WHERE id = ?")
        .bind(&archived_at)
        .bind(&archived_at)
        .bind(&work_unit_id)
        .execute(&state.db)
        .await?;

    if let Some(feature_id) = feature_id.as_deref() {
        if kind == "refine" {
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

    if kind != "refine" {
        invalidate_supervisor_integration(state, &mut run).await?;
    }
    let result_action = if kind == "feature_development" { "unqueue_work_unit" } else { "delete_work_unit" };
    let snapshot_message = if kind == "feature_development" { "work unit unqueued" } else { "work unit deleted" };
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", snapshot_message).await?;

    Ok(json!({ "ok": true, "action": result_action, "work_unit_id": work_unit_id, "kind": kind, "supervisor_run": run }))
}

pub async fn regenerate_supervisor_work_unit(state: &AppState, id: Uuid, work_unit_id: String) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let row = load_supervisor_work_unit_row(state, id, &work_unit_id).await?;
    let kind: String = row.get("kind");
    let title: String = row.get("title");
    let feature_id = row.try_get::<Option<String>, _>("feature_id").ok().flatten();
    let workflow_run_id = row.try_get::<Option<String>, _>("workflow_run_id").ok().flatten().and_then(|value| Uuid::parse_str(value.as_str()).ok());
    let shard_path = row.try_get::<Option<String>, _>("shard_path").ok().flatten();
    let mut work_unit_context = serde_json::from_str::<Value>(&row.get::<String, _>("context_json")).unwrap_or_else(|_| json!({}));

    if kind == "refine" && selected_planner_id_for_supervisor(state, id).await?.is_none() {
        return Err(anyhow!("supervisor has no selected planner"));
    }

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

    if let Some(path_text) = shard_path.as_deref().filter(|value| !value.trim().is_empty()) {
        let path = PathBuf::from(path_text);
        if path.exists() {
            let result = if path.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
            if let Err(err) = result {
                tracing::warn!(path = %path.display(), error = %format!("{:#}", err), "failed to delete stale work unit workspace during regenerate");
            }
        }
    }

    if kind == "integration" {
        invalidate_supervisor_integration(state, &mut run).await?;
        sqlx::query("DELETE FROM supervisor_work_units WHERE supervisor_run_id = ? AND kind = 'integration'")
            .bind(id.to_string())
            .execute(&state.db)
            .await?;
        run.integration_run_id = None;
        run.integration_path = None;
        run.final_patch_path = None;
        run.merge_report = json!({});
        run.validation_report = json!({});
        run.status = SupervisorStatus::DevelopmentComplete;
        run.updated_at = Utc::now();
        update_supervisor_run(state, &run).await?;
        spawn_live_integration_workflow(state, &mut run).await?;
        run.updated_at = Utc::now();
        update_supervisor_run(state, &run).await?;
        publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "integration workflow regenerated").await?;
        return Ok(json!({ "ok": true, "action": "regenerate_work_unit", "work_unit_id": work_unit_id, "kind": kind, "state": "integrating", "workflow_run_id": run.integration_run_id.map(|value| Value::String(value.to_string())).unwrap_or(Value::Null), "supervisor_run": run }));
    }

    let now = Utc::now().to_rfc3339();
    let next_state = if kind == "manual_shard" { "draft" } else { "queued" };
    sqlx::query(
        r#"
        UPDATE supervisor_work_units
        SET workflow_run_id = NULL,
            patch_id = NULL,
            shard_path = NULL,
            workspace_path = NULL,
            state = ?,
            blocked_reason = NULL,
            context_json = json_set(
                CASE WHEN json_valid(context_json) THEN context_json ELSE '{}' END,
                '$.integration_input', json('false'),
                '$.staged_to_integration', json('false'),
                '$.integration_skipped', json('false'),
                '$.regenerated_at', ?
            ),
            updated_at = ?
        WHERE id = ?
        "#,
    )
    .bind(next_state)
    .bind(&now)
    .bind(&now)
    .bind(&work_unit_id)
    .execute(&state.db)
    .await?;

    let mut regenerated_workflow_run_id: Option<Uuid> = None;

    if let Some(feature_id) = feature_id.as_deref() {
        if kind == "refine" {
            if let Some(feature_item) = run.feature_plan_items.iter_mut().find(|item| item.id == feature_id) {
                feature_item.refinement_workflow_run_id = None;
            }
            run.updated_at = Utc::now();
            update_supervisor_run(state, &run).await?;

            let workflow_template_id = context_uuid(&work_unit_context, "template_id")
                .or_else(|| context_uuid(&work_unit_context, "planned_workflow_template_id"))
                .or_else(|| context_uuid(&run.context, "planner_refinement_template_id"))
                .or_else(|| context_uuid(&run.context, "workflow_template_id"));

            let refine_result = refine_supervisor_feature(state, id, json!({
                "feature_id": feature_id,
                "workflow_template_id": workflow_template_id.map(|value| value.to_string())
            })).await?;
            regenerated_workflow_run_id = refine_result
                .get("workflow_run_id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok());
            run = load_supervisor_run(state, id).await?;
        } else {
        sqlx::query("UPDATE planner_features SET locked_supervisor_run_id = COALESCE(locked_supervisor_run_id, ?), locked_at = COALESCE(locked_at, ?), completed_at = NULL, updated_at = ? WHERE id = ?")
            .bind(run.id.to_string())
            .bind(&now)
            .bind(&now)
            .bind(feature_id)
            .execute(&state.db)
            .await?;

        if kind == "manual_shard" {
            let template_id = context_uuid(&work_unit_context, "planned_workflow_template_id")
                .or_else(|| context_uuid(&work_unit_context, "template_id"))
                .or_else(|| context_uuid(&run.context, "workflow_template_id"))
                .ok_or_else(|| anyhow!("manual shard work unit template_id is missing"))?;
            let workspace = repo_snapshot::workspace_for(&run.root_repo_path, run.id)?;
            let feature = FeaturePlanItem {
                id: feature_id.to_string(),
                title: title.clone(),
                status: FeaturePlanItemStatus::Scheduled,
                summary: title.clone(),
                rough_summary: None,
                refinement_workflow_run_id: None,
                applied_sprint_id: None,
                applied_sprint_title: None,
                applied_at: None,
                requirements: Vec::new(),
                acceptance_criteria: Vec::new(),
                implementation_notes: vec!["Manual shard regenerated from flight deck.".to_string()],
                review_expectations: Vec::new(),
                target_files_or_areas: Vec::new(),
                dependencies: Vec::new(),
            };
            let mut workflow_context = supervisor_context(&run, &workspace);
            if let Some(obj) = workflow_context.as_object_mut() {
                obj.insert("input_source".to_string(), Value::String("supervisor_manual_shard".to_string()));
                obj.insert("workflow_type".to_string(), Value::String("manual_shard".to_string()));
                obj.insert("pool_key".to_string(), Value::String("manual_shard".to_string()));
                obj.insert("manual_shard_id".to_string(), Value::String(feature_id.to_string()));
                obj.insert("work_unit_id".to_string(), Value::String(work_unit_id.clone()));
                obj.insert("template_id".to_string(), Value::String(template_id.to_string()));
                obj.insert("manual_shard".to_string(), Value::Bool(true));
            }
            if !work_unit_context.is_object() {
                work_unit_context = json!({});
            }
            if let Some(obj) = work_unit_context.as_object_mut() {
                obj.insert("source".to_string(), Value::String("manual_shard".to_string()));
                obj.insert("workflow_type".to_string(), Value::String("manual_shard".to_string()));
                obj.insert("pool_key".to_string(), Value::String("manual_shard".to_string()));
                obj.insert("manual_shard_id".to_string(), Value::String(feature_id.to_string()));
                obj.insert("template_id".to_string(), Value::String(template_id.to_string()));
                obj.insert("planned_workflow_template_id".to_string(), Value::String(template_id.to_string()));
                obj.insert("integration_input".to_string(), Value::Bool(false));
                obj.insert("staged_to_integration".to_string(), Value::Bool(false));
                obj.insert("integration_skipped".to_string(), Value::Bool(false));
            }

            let spawn_result = lifecycle::spawn_supervisor_workflow(
                state,
                SupervisorWorkflowSpawnRequest {
                    supervisor_run_id: run.id,
                    root_repo_path: run.root_repo_path.clone(),
                    pool_kind: SupervisorPoolKind::ManualShard,
                    work_unit_id: work_unit_id.clone(),
                    shard_id: None,
                    feature_id: Some(feature_id.to_string()),
                    title: title.clone(),
                    item: feature,
                    template_id: Some(template_id),
                    workflow_context,
                    work_unit_context,
                    initial_state: "draft".to_string(),
                    priority: 0,
                    queue_position: row.try_get::<Option<i64>, _>("queue_position").ok().flatten(),
                },
            )
            .await?;

            regenerated_workflow_run_id = Some(spawn_result.workflow_run_id);
        } else         if kind == "feature_development" {
            let template_id = context_uuid(&work_unit_context, "planned_workflow_template_id")
                .or_else(|| context_uuid(&work_unit_context, "template_id"))
                .or_else(|| context_uuid(&run.context, "workflow_template_id"))
                .ok_or_else(|| anyhow!("feature work unit template_id is missing"))?;
            let feature = run
                .feature_plan_items
                .iter()
                .find(|item| item.id == feature_id)
                .cloned()
                .unwrap_or_else(|| FeaturePlanItem {
                    id: feature_id.to_string(),
                    title: title.clone(),
                    status: FeaturePlanItemStatus::Scheduled,
                    summary: title.clone(),
                    rough_summary: None,
                    refinement_workflow_run_id: None,
                    applied_sprint_id: None,
                    applied_sprint_title: None,
                    applied_at: None,
                    requirements: Vec::new(),
                    acceptance_criteria: Vec::new(),
                    implementation_notes: Vec::new(),
                    review_expectations: Vec::new(),
                    target_files_or_areas: Vec::new(),
                    dependencies: Vec::new(),
                });
            let workspace = repo_snapshot::workspace_for(&run.root_repo_path, run.id)?;
            let mut workflow_context = supervisor_context(&run, &workspace);
            if let Some(obj) = workflow_context.as_object_mut() {
                obj.insert("input_source".to_string(), Value::String("supervisor_feature_queue".to_string()));
                obj.insert("workflow_type".to_string(), Value::String("feature_development".to_string()));
                obj.insert("pool_key".to_string(), Value::String("feature_development".to_string()));
                obj.insert("feature_id".to_string(), Value::String(feature_id.to_string()));
                obj.insert("work_unit_id".to_string(), Value::String(work_unit_id.clone()));
                obj.insert("template_id".to_string(), Value::String(template_id.to_string()));
            }
            if !work_unit_context.is_object() {
                work_unit_context = json!({});
            }
            if let Some(obj) = work_unit_context.as_object_mut() {
                obj.insert("source".to_string(), Value::String("supervisor_feature_queue".to_string()));
                obj.insert("status".to_string(), Value::String("queued".to_string()));
                obj.insert("development_state".to_string(), Value::String("queued".to_string()));
                obj.insert("planner_feature_id".to_string(), Value::String(feature_id.to_string()));
                obj.insert("feature_id".to_string(), Value::String(feature_id.to_string()));
                obj.insert("template_id".to_string(), Value::String(template_id.to_string()));
                obj.insert("planned_workflow_template_id".to_string(), Value::String(template_id.to_string()));
                obj.insert("workflow_type".to_string(), Value::String("feature_development".to_string()));
                obj.insert("pool_key".to_string(), Value::String("feature_development".to_string()));
                obj.insert("planned_workflow".to_string(), Value::Bool(true));
            }

            let spawn_result = lifecycle::spawn_supervisor_workflow(
                state,
                SupervisorWorkflowSpawnRequest {
                    supervisor_run_id: run.id,
                    root_repo_path: run.root_repo_path.clone(),
                    pool_kind: SupervisorPoolKind::FeatureDevelopment,
                    work_unit_id: work_unit_id.clone(),
                    shard_id: None,
                    feature_id: Some(feature_id.to_string()),
                    title: title.clone(),
                    item: feature,
                    template_id: Some(template_id),
                    workflow_context,
                    work_unit_context,
                    initial_state: "queued".to_string(),
                    priority: 0,
                    queue_position: row.try_get::<Option<i64>, _>("queue_position").ok().flatten(),
                },
            )
            .await?;

            sqlx::query("UPDATE planner_features SET locked_supervisor_run_id = COALESCE(locked_supervisor_run_id, ?), locked_at = COALESCE(locked_at, ?), completed_at = NULL, updated_at = ? WHERE id = ?")
                .bind(run.id.to_string())
                .bind(&now)
                .bind(&now)
                .bind(feature_id)
                .execute(&state.db)
                .await?;

            regenerated_workflow_run_id = Some(spawn_result.workflow_run_id);
        }
        }
    }

    if kind != "refine" {
        invalidate_supervisor_integration(state, &mut run).await?;
    }
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "work unit regenerated").await?;

    Ok(json!({ "ok": true, "action": "regenerate_work_unit", "work_unit_id": work_unit_id, "kind": kind, "state": next_state, "workflow_run_id": regenerated_workflow_run_id, "supervisor_run": run }))
}

pub async fn start_supervisor_work_unit(state: &AppState, id: Uuid, work_unit_id: String) -> Result<Value> {
    let requested_work_unit_id = work_unit_id.trim();
    let integration_work_unit_id = format!("{}:integration", id);
    let integration_draft_work_unit_id = format!("{}:integration:draft", id);
    if requested_work_unit_id.is_empty()
        || requested_work_unit_id == integration_work_unit_id
        || requested_work_unit_id == integration_draft_work_unit_id
    {
        return start_supervisor_integration_workflow(state, id).await;
    }

    let mut run = load_supervisor_run(state, id).await?;
    let row = load_supervisor_work_unit_row(state, id, &work_unit_id).await?;
    let kind: String = row.get("kind");
    let title: String = row.get("title");
    let feature_id = row.try_get::<Option<String>, _>("feature_id").ok().flatten();
    let mut work_unit_context = serde_json::from_str::<Value>(&row.get::<String, _>("context_json")).unwrap_or_else(|_| json!({}));

    let workflow_run_id = match row
        .try_get::<Option<String>, _>("workflow_run_id")
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .and_then(|value| Uuid::parse_str(value.as_str()).ok())
    {
        Some(workflow_run_id) => workflow_run_id,
        None if kind == "feature_development" => {
            let claim_time = Utc::now().to_rfc3339();
            let claim_result = sqlx::query(
                r#"
                UPDATE supervisor_work_units
                SET state = 'starting',
                    blocked_reason = NULL,
                    updated_at = ?
                WHERE id = ?
                  AND supervisor_run_id = ?
                  AND kind = 'feature_development'
                  AND workflow_run_id IS NULL
                  AND archived_at IS NULL
                  AND state IN (
                      'queued',
                      'paused',
                      'waiting_user',
                      'failed',
                      'development_failed'
                  )
                "#,
            )
            .bind(&claim_time)
            .bind(&work_unit_id)
            .bind(run.id.to_string())
            .execute(&state.db)
            .await?;

            if claim_result.rows_affected() == 0 {
                let current_row = load_supervisor_work_unit_row(
                    state,
                    id,
                    &work_unit_id,
                )
                .await?;
                let current_state: String = current_row.get("state");
                let current_workflow_run_id = current_row
                    .try_get::<Option<String>, _>("workflow_run_id")
                    .ok()
                    .flatten()
                    .filter(|value| !value.trim().is_empty());

                return Ok(json!({
                    "ok": true,
                    "action": "start_work_unit",
                    "work_unit_id": work_unit_id,
                    "workflow_run_id": current_workflow_run_id,
                    "state": current_state,
                    "already_starting": current_workflow_run_id.is_none(),
                    "already_materialized": current_workflow_run_id.is_some(),
                    "supervisor_run": run
                }));
            }

            tracing::info!(
                supervisor_run_id = %run.id,
                work_unit_id = %work_unit_id,
                "claimed queued feature work unit for workflow materialization"
            );
            let feature_id = feature_id
                .clone()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| anyhow!("feature work unit has no feature_id"))?;
            let template_id = context_uuid(&work_unit_context, "planned_workflow_template_id")
                .or_else(|| context_uuid(&work_unit_context, "template_id"))
                .or_else(|| context_uuid(&run.context, "workflow_template_id"))
                .ok_or_else(|| anyhow!("feature work unit template_id is missing"))?;
            let feature = run
                .feature_plan_items
                .iter()
                .find(|item| item.id == feature_id)
                .cloned()
                .unwrap_or_else(|| FeaturePlanItem {
                    id: feature_id.clone(),
                    title: title.clone(),
                    status: FeaturePlanItemStatus::Scheduled,
                    summary: title.clone(),
                    rough_summary: None,
                    refinement_workflow_run_id: None,
                    applied_sprint_id: None,
                    applied_sprint_title: None,
                    applied_at: None,
                    requirements: Vec::new(),
                    acceptance_criteria: Vec::new(),
                    implementation_notes: Vec::new(),
                    review_expectations: Vec::new(),
                    target_files_or_areas: Vec::new(),
                    dependencies: Vec::new(),
                });
            let workspace = repo_snapshot::workspace_for(&run.root_repo_path, run.id)?;
            let mut workflow_context = supervisor_context(&run, &workspace);
            if let Some(obj) = workflow_context.as_object_mut() {
                obj.insert("input_source".to_string(), Value::String("supervisor_feature_pool".to_string()));
                obj.insert("workflow_type".to_string(), Value::String("feature_development".to_string()));
                obj.insert("pool_key".to_string(), Value::String("feature_development".to_string()));
                obj.insert("feature_id".to_string(), Value::String(feature_id.clone()));
                obj.insert("work_unit_id".to_string(), Value::String(work_unit_id.clone()));
                obj.insert("template_id".to_string(), Value::String(template_id.to_string()));
            }
            if !work_unit_context.is_object() {
                work_unit_context = json!({});
            }
            if let Some(obj) = work_unit_context.as_object_mut() {
                obj.insert("source".to_string(), Value::String("supervisor_feature_pool".to_string()));
                obj.insert("workflow_type".to_string(), Value::String("feature_development".to_string()));
                obj.insert("pool_key".to_string(), Value::String("feature_development".to_string()));
                obj.insert("feature_id".to_string(), Value::String(feature_id.clone()));
                obj.insert("template_id".to_string(), Value::String(template_id.to_string()));
            }

            let spawn_result = match lifecycle::spawn_supervisor_workflow(
                state,
                SupervisorWorkflowSpawnRequest {
                    supervisor_run_id: run.id,
                    root_repo_path: run.root_repo_path.clone(),
                    pool_kind: SupervisorPoolKind::FeatureDevelopment,
                    work_unit_id: work_unit_id.clone(),
                    shard_id: None,
                    feature_id: Some(feature_id.clone()),
                    title: title.clone(),
                    item: feature,
                    template_id: Some(template_id),
                    workflow_context,
                    work_unit_context,
                    initial_state: "queued".to_string(),
                    priority: 0,
                    queue_position: None,
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
                        SET state = 'development_failed',
                            blocked_reason = ?,
                            updated_at = ?
                        WHERE id = ?
                          AND workflow_run_id IS NULL
                          AND state = 'starting'
                        "#,
                    )
                    .bind(&failure_message)
                    .bind(&failure_time)
                    .bind(&work_unit_id)
                    .execute(&state.db)
                    .await?;

                    tracing::error!(
                        supervisor_run_id = %run.id,
                        work_unit_id = %work_unit_id,
                        error = %failure_message,
                        "failed to materialize claimed feature work unit"
                    );

                    return Err(err);
                }
            };

            let now = Utc::now().to_rfc3339();
            sqlx::query("UPDATE planner_features SET locked_supervisor_run_id = COALESCE(NULLIF(locked_supervisor_run_id, ''), ?), locked_at = COALESCE(locked_at, ?), updated_at = ? WHERE id = ?")
                .bind(run.id.to_string())
                .bind(&now)
                .bind(&now)
                .bind(&feature_id)
                .execute(&state.db)
                .await?;

            spawn_result.workflow_run_id
        }
        None => return Err(anyhow!("work unit has no workflow_run_id; regenerate or recreate it before starting")),
    };

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

    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE supervisor_work_units SET state = ?, blocked_reason = ?, updated_at = ? WHERE id = ?")
        .bind(if waiting_on_operator_checkpoint {
            work_unit_paused_state(&kind)
        } else {
            work_unit_running_state(&kind)
        })
        .bind(if waiting_on_operator_checkpoint {
            Some("Waiting on operator checkpoint")
        } else {
            None
        })
        .bind(&now)
        .bind(&work_unit_id)
        .execute(&state.db)
        .await?;

    if kind == "integration" {
        run.status = SupervisorStatus::RunningIntegration;
        run.integration_run_id = Some(workflow_run_id);
    } else if kind == "feature_development" {
        run.status = SupervisorStatus::RunningChildren;
    }
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "work unit start requested").await?;

    Ok(json!({ "ok": true, "action": "start_work_unit", "work_unit_id": work_unit_id, "workflow_run_id": workflow_run_id, "start_result": start_result, "supervisor_run": run }))
}

pub async fn pause_supervisor_work_unit(state: &AppState, id: Uuid, work_unit_id: String) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let row = load_supervisor_work_unit_row(state, id, &work_unit_id).await?;
    let kind: String = row.get("kind");
    let workflow_run_id = row.try_get::<Option<String>, _>("workflow_run_id").ok().flatten().and_then(|value| Uuid::parse_str(value.as_str()).ok());

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

    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE supervisor_work_units SET state = ?, updated_at = ? WHERE id = ?")
        .bind(work_unit_paused_state(&kind))
        .bind(&now)
        .bind(&work_unit_id)
        .execute(&state.db)
        .await?;

    if kind == "integration" {
        run.status = SupervisorStatus::DevelopmentComplete;
    }
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "work unit pause requested").await?;

    Ok(json!({ "ok": true, "action": "pause_work_unit", "work_unit_id": work_unit_id, "pause_result": pause_result, "supervisor_run": run }))
}

pub async fn stage_supervisor_work_unit(state: &AppState, id: Uuid, work_unit_id: String, staged: bool) -> Result<Value> {
    let row = load_supervisor_work_unit_row(state, id, &work_unit_id).await?;
    let kind: String = row.get("kind");
    let title: String = row.get("title");
    let shard_path = row.try_get::<Option<String>, _>("shard_path").ok().flatten();
    let mut context = serde_json::from_str::<Value>(&row.get::<String, _>("context_json")).unwrap_or_else(|_| json!({}));

    if kind == "manual_shard" && staged {
        let path = shard_path.as_deref().filter(|value| !value.trim().is_empty()).ok_or_else(|| anyhow!("manual shard has no shard_path"))?;
        if !manual_shard_has_staged_changes(path)? {
            return Err(anyhow!("manual shard '{}' cannot be staged to integration because it has no staged git changes", title));
        }
    }

    if let Some(obj) = context.as_object_mut() {
        obj.insert("integration_input".to_string(), Value::Bool(staged));
        obj.insert("staged_to_integration".to_string(), Value::Bool(staged));
        obj.insert("workflow_type".to_string(), Value::String(kind.clone()));
        obj.insert("pool_key".to_string(), Value::String(kind.clone()));
        obj.insert(if staged { "staged_to_integration_at" } else { "unstaged_from_integration_at" }.to_string(), Value::String(Utc::now().to_rfc3339()));
    }

    let now = Utc::now().to_rfc3339();
    let state_text = if staged { "ready_for_integration" } else { "draft" };
    sqlx::query("UPDATE supervisor_work_units SET state = ?, context_json = ?, updated_at = ? WHERE id = ?")
        .bind(state_text)
        .bind(serde_json::to_string(&context)?)
        .bind(&now)
        .bind(&work_unit_id)
        .execute(&state.db)
        .await?;

    let run = load_supervisor_run(state, id).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", if staged { "work unit staged for integration" } else { "work unit unstaged from integration" }).await?;

    Ok(json!({ "ok": true, "action": "stage_work_unit", "work_unit_id": work_unit_id, "staged": staged, "state": state_text, "supervisor_run": run }))
}

pub async fn set_supervisor_work_unit_integration_skipped(state: &AppState, id: Uuid, work_unit_id: String, skipped: bool) -> Result<Value> {
    let row = load_supervisor_work_unit_row(state, id, &work_unit_id).await?;
    let kind: String = row.get("kind");
    let feature_id = row.try_get::<Option<String>, _>("feature_id").ok().flatten().filter(|value| !value.trim().is_empty()).ok_or_else(|| anyhow!("work unit has no feature_id"))?;
    let payload = if kind == "manual_shard" { json!({ "manual_shard_id": feature_id }) } else { json!({ "feature_id": feature_id }) };
    set_feature_integration_skipped(state, id, payload, skipped).await
}

fn manual_shard_has_staged_changes(shard_path: &str) -> Result<bool> {
    let output = std::process::Command::new("git")
        .arg("diff")
        .arg("--cached")
        .arg("--quiet")
        .current_dir(shard_path)
        .output()?;

    match output.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(anyhow!(
            "failed to validate staged manual shard changes: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

async fn set_feature_integration_skipped(state: &AppState, id: Uuid, payload: Value, skipped: bool) -> Result<Value> {
    let feature_id = payload
        .get("feature_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let manual_shard_id = payload
        .get("manual_shard_id")
        .or_else(|| payload.get("feature_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);

    let now = Utc::now().to_rfc3339();

    if let Some(feature_id) = feature_id {
        let changed = sqlx::query(
            "UPDATE sprint_features
             SET integration_skipped = ?, updated_at = ?
             WHERE supervisor_run_id = ?
               AND feature_id = ?
               AND status != 'unscheduled'",
        )
        .bind(if skipped { 1 } else { 0 })
        .bind(&now)
        .bind(id.to_string())
        .bind(&feature_id)
        .execute(&state.db)
        .await?
        .rows_affected();

        let row = sqlx::query(
            "SELECT id, context_json FROM supervisor_work_units WHERE supervisor_run_id = ? AND kind = 'feature_development' AND feature_id = ? LIMIT 1",
        )
        .bind(id.to_string())
        .bind(&feature_id)
        .fetch_optional(&state.db)
        .await?;

        if let Some(row) = row {
            let work_unit_id: String = row.get("id");
            let mut context = serde_json::from_str::<Value>(&row.get::<String, _>("context_json"))
                .unwrap_or_else(|_| json!({}));
            if let Some(obj) = context.as_object_mut() {
                obj.insert("workflow_type".to_string(), Value::String("feature_development".to_string()));
                obj.insert("pool_key".to_string(), Value::String("feature_pool".to_string()));
                obj.insert("integration_input".to_string(), Value::Bool(true));
                obj.insert("integration_skipped".to_string(), Value::Bool(skipped));
                obj.insert(
                    if skipped { "skipped_for_integration_at" } else { "unskipped_for_integration_at" }.to_string(),
                    Value::String(now.clone()),
                );
            }
            sqlx::query("UPDATE supervisor_work_units SET context_json = ?, updated_at = ? WHERE id = ?")
                .bind(serde_json::to_string(&context)?)
                .bind(&now)
                .bind(&work_unit_id)
                .execute(&state.db)
                .await?;
        }

        let run = load_supervisor_run(state, id).await?;
        publish_supervisor_snapshot(
            state,
            &run,
            "supervisor_snapshot",
            if skipped { "integration input skipped" } else { "integration input unskipped" },
        ).await?;

        return Ok(json!({
            "ok": true,
            "kind": "feature_development",
            "feature_id": feature_id,
            "integration_skipped": skipped,
            "sprint_features_updated": changed,
            "planner_status_unchanged": true
        }));
    }

    let manual_shard_id = manual_shard_id.ok_or_else(|| anyhow!("feature_id or manual_shard_id is required"))?;
    let row = sqlx::query(
        "SELECT id, context_json FROM supervisor_work_units WHERE supervisor_run_id = ? AND kind = 'manual_shard' AND feature_id = ? LIMIT 1",
    )
    .bind(id.to_string())
    .bind(&manual_shard_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| anyhow!("manual shard integration input not found"))?;

    let work_unit_id: String = row.get("id");
    let mut context = serde_json::from_str::<Value>(&row.get::<String, _>("context_json"))
        .unwrap_or_else(|_| json!({}));
    if let Some(obj) = context.as_object_mut() {
        obj.insert("workflow_type".to_string(), Value::String("manual_shard".to_string()));
        obj.insert("pool_key".to_string(), Value::String("manual_shard".to_string()));
        obj.insert("integration_input".to_string(), Value::Bool(true));
        obj.insert("staged_to_integration".to_string(), Value::Bool(true));
        obj.insert("integration_skipped".to_string(), Value::Bool(skipped));
        obj.insert(
            if skipped { "skipped_for_integration_at" } else { "unskipped_for_integration_at" }.to_string(),
            Value::String(now.clone()),
        );
    }

    sqlx::query("UPDATE supervisor_work_units SET context_json = ?, updated_at = ? WHERE id = ?")
        .bind(serde_json::to_string(&context)?)
        .bind(&now)
        .bind(&work_unit_id)
        .execute(&state.db)
        .await?;

    let run = load_supervisor_run(state, id).await?;
    publish_supervisor_snapshot(
        state,
        &run,
        "supervisor_snapshot",
        if skipped { "manual integration input skipped" } else { "manual integration input unskipped" },
    ).await?;

    Ok(json!({
        "ok": true,
        "kind": "manual_shard",
        "manual_shard_id": manual_shard_id,
        "work_unit_id": work_unit_id,
        "integration_skipped": skipped,
        "planner_status_unchanged": true
    }))
}

pub async fn skip_supervisor_integration_input(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    set_feature_integration_skipped(state, id, payload, true).await
}

pub async fn unskip_supervisor_integration_input(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    set_feature_integration_skipped(state, id, payload, false).await
}

pub async fn stage_supervisor_manual_shard(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let manual_shard_id = payload
        .get("manual_shard_id")
        .or_else(|| payload.get("feature_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("manual_shard_id is required"))?
        .to_string();

    let row = sqlx::query(
        "SELECT id, title, shard_path, context_json FROM supervisor_work_units WHERE supervisor_run_id = ? AND kind = 'manual_shard' AND feature_id = ? LIMIT 1",
    )
    .bind(id.to_string())
    .bind(&manual_shard_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| anyhow!("manual shard not found"))?;

    let work_unit_id: String = row.get("id");
    let title: String = row.get("title");
    let shard_path: String = row
        .get::<Option<String>, _>("shard_path")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("manual shard has no shard_path"))?;

    if !manual_shard_has_staged_changes(&shard_path)? {
        return Err(anyhow!("manual shard '{}' cannot be staged to integration because it has no staged git changes", title));
    }

    let mut context = serde_json::from_str::<Value>(&row.get::<String, _>("context_json"))
        .unwrap_or_else(|_| json!({}));
    if let Some(obj) = context.as_object_mut() {
        obj.insert("workflow_type".to_string(), Value::String("manual_shard".to_string()));
        obj.insert("pool_key".to_string(), Value::String("manual_shard".to_string()));
        obj.insert("integration_input".to_string(), Value::Bool(true));
        obj.insert("staged_to_integration".to_string(), Value::Bool(true));
        obj.insert("staged_to_integration_at".to_string(), Value::String(Utc::now().to_rfc3339()));
    }

    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE supervisor_work_units SET state = 'ready_for_integration', context_json = ?, updated_at = ? WHERE id = ?",
    )
    .bind(serde_json::to_string(&context)?)
    .bind(&now)
    .bind(&work_unit_id)
    .execute(&state.db)
    .await?;

    let run = load_supervisor_run(state, id).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "manual shard staged for integration").await?;

    Ok(json!({
        "ok": true,
        "action": "stage_manual_shard",
        "manual_shard_id": manual_shard_id,
        "work_unit_id": work_unit_id,
        "state": "ready_for_integration"
    }))
}

pub async fn unstage_supervisor_manual_shard(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let manual_shard_id = payload
        .get("manual_shard_id")
        .or_else(|| payload.get("feature_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("manual_shard_id is required"))?
        .to_string();

    let row = sqlx::query(
        "SELECT id, context_json FROM supervisor_work_units WHERE supervisor_run_id = ? AND kind = 'manual_shard' AND feature_id = ? LIMIT 1",
    )
    .bind(id.to_string())
    .bind(&manual_shard_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| anyhow!("manual shard not found"))?;

    let work_unit_id: String = row.get("id");
    let mut context = serde_json::from_str::<Value>(&row.get::<String, _>("context_json"))
        .unwrap_or_else(|_| json!({}));
    if let Some(obj) = context.as_object_mut() {
        obj.insert("workflow_type".to_string(), Value::String("manual_shard".to_string()));
        obj.insert("pool_key".to_string(), Value::String("manual_shard".to_string()));
        obj.insert("integration_input".to_string(), Value::Bool(false));
        obj.insert("staged_to_integration".to_string(), Value::Bool(false));
        obj.insert("unstaged_from_integration_at".to_string(), Value::String(Utc::now().to_rfc3339()));
    }

    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE supervisor_work_units SET state = 'draft', context_json = ?, updated_at = ? WHERE id = ?",
    )
    .bind(serde_json::to_string(&context)?)
    .bind(&now)
    .bind(&work_unit_id)
    .execute(&state.db)
    .await?;

    let run = load_supervisor_run(state, id).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "manual shard unstaged from integration").await?;

    Ok(json!({
        "ok": true,
        "action": "unstage_manual_shard",
        "manual_shard_id": manual_shard_id,
        "work_unit_id": work_unit_id,
        "state": "draft"
    }))
}

pub async fn advance_supervisor_run(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    match run.status {
        SupervisorStatus::RunningChildren => kick_feature_pool_if_running(state, &mut run).await?,
        SupervisorStatus::RunningIntegration | SupervisorStatus::Validating => tick_integration(state, &mut run).await?,
        _ => {}
    }
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "supervisor advanced").await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

pub async fn apply_supervisor_final_patch(state: &AppState, id: Uuid) -> Result<Value> {
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
          AND kind = 'integration'
          AND archived_at IS NULL
          AND TRIM(COALESCE(workflow_run_id, '')) != ''
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(id.to_string())
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
    let integration_work_unit_state: String = integration_row.get("state");
    let integration_run = engine::load_run(state, integration_run_id).await?;
    if !matches!(integration_run.status, RunStatus::Success)
        || !matches!(integration_work_unit_state.as_str(), "integrated" | "patch_ready" | "ready_to_apply")
    {
        return Err(anyhow!("current integration workflow must complete successfully before applying integration batch"));
    }
    if !matches!(run.status, SupervisorStatus::ReadyToApply) {
        run.status = SupervisorStatus::ReadyToApply;
        run.updated_at = Utc::now();
        update_supervisor_run(state, &run).await?;
    }

    let integration_batch_id = run.id.to_string();
    let patch_paths = supervisor_patch_paths(state, &run).await?;
    if patch_paths.is_empty() {
        return Err(anyhow!("integration has no ready unskipped inputs in the integration pool"));
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
        "applied_at": now_text
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
                '$.applied_at', ?
            ),
            updated_at = ?
        WHERE supervisor_run_id = ?
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
    .execute(&state.db)
    .await?;
    run.final_patch_path = None;
    run.merge_report = merge_report;
    let completed_at = Utc::now();
    let completed_at_text = completed_at.to_rfc3339();
    let scheduled_feature_ids = patch_paths
        .iter()
        .filter_map(|item| {
            item.get("feature_id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    let sprint_id = integration_batch_id.clone();
    let sprint_key = run
        .context
        .get("current_sprint_key")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| sprint_key_for(&run.root_repo_path, &sprint_id));
    let sprint_title = format!("Integration batch {} applied {}", sprint_key, completed_at_text);

    for item in &mut run.feature_plan_items {
        if scheduled_feature_ids.iter().any(|id| id == &item.id) {
            item.status = FeaturePlanItemStatus::Applied;
            item.applied_sprint_id = Some(sprint_id.clone());
            item.applied_sprint_title = Some(sprint_title.clone());
            item.applied_at = Some(completed_at_text.clone());
        }
    }

    let completed_features = run
        .feature_plan_items
        .iter()
        .filter(|item| scheduled_feature_ids.iter().any(|id| id == &item.id))
        .map(|item| json!({
            "id": item.id,
            "title": item.title,
            "applied_at": completed_at_text,
            "applied_sprint_id": sprint_id,
            "applied_sprint_title": sprint_title
        }))
        .collect::<Vec<_>>();

    let sprint_record = json!({
        "sprint_id": sprint_id,
        "integration_batch_id": sprint_id,
        "title": sprint_title,
        "status": "applied",
        "applied_at": completed_at_text,
        "root_repo_path": run.root_repo_path,
        "snapshot_path": run.snapshot_path,
        "integration_path": run.integration_path,
        "integration_run_id": run.integration_run_id,
        "final_patch_path": run.final_patch_path,
        "features": completed_features,
        "execution_source": "supervisor_work_units"
    });

    save_repo_feature_plan_items(state, &run.root_repo_path, &run.feature_plan_items).await?;
    upsert_sprint_record(state, &run, &sprint_id, &sprint_key, &sprint_title, "applied", None, Some(&completed_at_text)).await?;
    save_sprint_features(state, &run, &sprint_id, Some(&completed_at_text)).await?;
    let repo_id = ensure_planner_repo_id(state, &run.root_repo_path).await?;
    for feature_id in &scheduled_feature_ids {
        let row = sqlx::query("SELECT current_workflow_run_id, current_patch_id FROM sprint_features WHERE sprint_id = ? AND feature_id = ?")
            .bind(&sprint_id)
            .bind(feature_id)
            .fetch_optional(&state.db)
            .await?;
        let workflow_run_id = row.as_ref().and_then(|row| row.try_get::<String, _>("current_workflow_run_id").ok()).filter(|value| !value.trim().is_empty());
        let patch_id = row.as_ref().and_then(|row| row.try_get::<String, _>("current_patch_id").ok()).filter(|value| !value.trim().is_empty());
        sqlx::query("UPDATE planner_features SET current_sprint_id = ?, current_supervisor_run_id = ?, current_workflow_run_id = ?, current_patch_id = ?, development_state = 'applied', integration_completed_at = COALESCE(integration_completed_at, ?), applied_at = COALESCE(applied_at, ?), updated_at = ? WHERE repo_id = ? AND id = ?")
            .bind(&sprint_id)
            .bind(run.id.to_string())
            .bind(workflow_run_id)
            .bind(patch_id)
            .bind(&completed_at_text)
            .bind(&completed_at_text)
            .bind(&completed_at_text)
            .bind(&repo_id)
            .bind(feature_id)
            .execute(&state.db)
            .await?;
    }
    sqlx::query("UPDATE sprint_features SET status = 'archived', development_state = 'applied', completed_at = COALESCE(completed_at, ?), updated_at = ? WHERE sprint_id = ? AND feature_id IN (SELECT value FROM json_each(?)) AND COALESCE(integration_skipped, 0) = 0")
        .bind(&completed_at_text)
        .bind(&completed_at_text)
        .bind(&sprint_id)
        .bind(serde_json::to_string(&scheduled_feature_ids)?)
        .execute(&state.db)
        .await?;
    sqlx::query("UPDATE supervisor_work_units SET state = 'archived', updated_at = ? WHERE supervisor_run_id = ? AND kind IN ('feature_development', 'manual_shard') AND state IN ('patch_ready', 'ready_for_integration', 'integrated') AND COALESCE(json_extract(context_json, '$.integration_skipped'), 0) = 0")
        .bind(&completed_at_text)
        .bind(run.id.to_string())
        .execute(&state.db)
        .await?;

    sqlx::query("UPDATE workflow_runs SET status = 'archived', updated_at = ? WHERE id = ?")
        .bind(&completed_at_text)
        .bind(integration_run_id.to_string())
        .execute(&state.db)
        .await?;

    if let Ok(workspace) = repo_snapshot::workspace_for(&run.root_repo_path, run.id) {
        let path = &workspace.integration;
        if path.exists() {
            if path.is_dir() {
                fs::remove_dir_all(path)
                    .with_context(|| format!("failed to clean supervisor integration workspace {}", path.display()))?;
            } else {
                fs::remove_file(path)
                    .with_context(|| format!("failed to clean supervisor integration workspace file {}", path.display()))?;
            }
        }
    }

    sqlx::query("DELETE FROM supervisor_work_units WHERE supervisor_run_id = ? AND kind = 'integration'")
        .bind(run.id.to_string())
        .execute(&state.db)
        .await?;
    append_sprint_event(state, &sprint_id, "integration_batch_applied", &completed_at_text, None, "integration batch applied", json!({ "integration_batch_key": sprint_key, "features": completed_features })).await?;

    if !run.context.is_object() {
        run.context = json!({});
    }
    if let Some(obj) = run.context.as_object_mut() {
        obj.insert("integration_batch_applied_at".to_string(), Value::String(completed_at_text.clone()));
        obj.insert("completed_features".to_string(), sprint_record.get("features").cloned().unwrap_or_else(|| json!([])));
        let history = obj.entry("integration_batch_history".to_string()).or_insert_with(|| json!([]));
        if let Some(items) = history.as_array_mut() {
            items.push(sprint_record.clone());
        }
        let sprint_history = obj.entry("sprint_history".to_string()).or_insert_with(|| json!([]));
        if let Some(items) = sprint_history.as_array_mut() {
            items.push(sprint_record);
        }
        obj.remove("current_sprint_id");
        obj.remove("current_sprint_key");
        obj.remove("current_sprint_started_at");
        obj.remove("current_integration_batch_id");
        obj.remove("current_integration_batch_key");
        obj.remove("current_integration_batch_started_at");
    }
    run.execution_plan_items.clear();
    run.integration_run_id = None;
    run.final_patch_path = None;
    run.merge_report = json!({});
    run.validation_report = json!({});
    run.snapshot_path = None;
    run.integration_path = None;
    run.feature_workflows.clear();
    run.status = SupervisorStatus::Created;
    run.updated_at = completed_at;
    update_supervisor_run(state, &run).await?;
    Ok(json!({ "ok": true, "status": "applied", "integration_batch_applied_at": completed_at_text, "supervisor_run": run }))
}

async fn refresh_supervisor_final_patch(
    state: &AppState,
    run: &SupervisorRun,
    sprint_id: &str,
    integration_path: &str,
) -> Result<String> {
    let patch_text = patches::generate_patch_text(Path::new(integration_path))?;
    let patch_hash = patches::patch_content_hash(&patch_text);
    let patch_dir = Path::new(&run.root_repo_path)
        .join(".mdev")
        .join("supervisors")
        .join(run.id.to_string())
        .join("patches");
    fs::create_dir_all(&patch_dir)?;
    let patch_path = patch_dir.join(format!(
        "final-{}.patch",
        repo_snapshot::sanitize_path_segment(sprint_id)
    ));
    fs::write(&patch_path, patch_text.as_bytes())?;
    let now = Utc::now().to_rfc3339();
    let final_patch_path = patch_path.to_string_lossy().replace('\\', "/");
    let mut report = run.merge_report.clone();
    if !report.is_object() {
        report = json!({});
    }
    if let Some(obj) = report.as_object_mut() {
        obj.insert("ok".to_string(), Value::Bool(true));
        obj.insert("status".to_string(), Value::String("ready_to_apply".to_string()));
        obj.insert("source".to_string(), Value::String("final_apply_refresh".to_string()));
        obj.insert("sprint_id".to_string(), Value::String(sprint_id.to_string()));
        obj.insert("root_repo_path".to_string(), Value::String(run.root_repo_path.clone()));
        obj.insert("integration_path".to_string(), Value::String(integration_path.to_string()));
        obj.insert("final_patch_path".to_string(), Value::String(final_patch_path.clone()));
        obj.insert("final_patch_hash".to_string(), Value::String(patch_hash.clone()));
        obj.insert("final_patch_bytes".to_string(), Value::Number((patch_text.len() as u64).into()));
        obj.insert("refreshed_at".to_string(), Value::String(now.clone()));
    }
    sqlx::query(
        r#"
        UPDATE supervisor_work_units
        SET patch_id = COALESCE(NULLIF(patch_id, ''), ?),
            integration_path = COALESCE(NULLIF(integration_path, ''), ?),
            state = CASE WHEN state IN ('deleted', 'archived') THEN state ELSE 'patch_ready' END,
            context_json = json_set(
                CASE WHEN json_valid(context_json) THEN context_json ELSE '{}' END,
                '$.final_patch_path', ?,
                '$.merge_report', json(?),
                '$.final_patch_hash', ?,
                '$.workflow_type', 'integration',
                '$.pool_key', 'integration'
            ),
            updated_at = ?
        WHERE supervisor_run_id = ?
          AND kind = 'integration'
          AND archived_at IS NULL
        "#,
    )
    .bind(&patch_hash)
    .bind(integration_path)
    .bind(&final_patch_path)
    .bind(serde_json::to_string(&report)?)
    .bind(&patch_hash)
    .bind(&now)
    .bind(run.id.to_string())
    .execute(&state.db)
    .await?;
    Ok(final_patch_path)
}

async fn log_sqlite_foreign_key_check(state: &AppState, label: &str) {
    match sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&state.db)
        .await
    {
        Ok(rows) if rows.is_empty() => {
            tracing::info!(label, "sqlite foreign_key_check passed");
        }
        Ok(rows) => {
            for row in rows {
                let table_name = row.try_get::<String, _>("table").unwrap_or_else(|_| "<unknown>".to_string());
                let rowid = row.try_get::<i64, _>("rowid").unwrap_or(-1);
                let parent = row.try_get::<String, _>("parent").unwrap_or_else(|_| "<unknown>".to_string());
                let fkid = row.try_get::<i64, _>("fkid").unwrap_or(-1);
                tracing::error!(label, table = %table_name, rowid, parent = %parent, fkid, "sqlite foreign_key_check violation");
            }
        }
        Err(err) => {
            tracing::error!(label, error = %format!("{:#}", err), "sqlite foreign_key_check failed to run");
        }
    }
}

fn supervisor_dynamic_feature_pool_ids(run: &SupervisorRun) -> Vec<String> {
    let mut ids = run
        .execution_plan_items
        .iter()
        .map(|item| item.feature_plan_item_id.clone())
        .collect::<Vec<_>>();
    ids.extend(import_string_array(run.context.get("feature_pool_ids")));
    ids.extend(import_string_array(run.context.get("queued_feature_ids")));
    let mut seen = HashSet::<String>::new();
    ids.retain(|id| !id.trim().is_empty() && !id.starts_with("manual-") && seen.insert(id.clone()));
    ids
}

async fn supervisor_patch_paths(state: &AppState, run: &SupervisorRun) -> Result<Vec<Value>> {
    let mut inputs = Vec::new();

    let feature_rows = sqlx::query(
        r#"
        SELECT wu.feature_id,
               wu.title,
               wu.patch_id,
               wu.workflow_run_id,
               wu.shard_path,
               wu.state
        FROM supervisor_work_units wu
        WHERE wu.supervisor_run_id = ?
          AND wu.kind = 'feature_development'
          AND wu.state IN ('patch_ready', 'ready_for_integration', 'development_succeeded')
          AND TRIM(COALESCE(wu.shard_path, '')) != ''
          AND COALESCE(json_extract(wu.context_json, '$.integration_skipped'), 0) = 0
        ORDER BY wu.queue_position ASC, wu.updated_at ASC
        "#,
    )
    .bind(run.id.to_string())
    .fetch_all(&state.db)
    .await?;

    for row in feature_rows {
        let feature_id: String = row.get("feature_id");
        inputs.push(json!({
            "execution_item_id": feature_id,
            "feature_id": feature_id,
            "title": row.get::<String, _>("title"),
            "patch_path": null,
            "patch_id": row.get::<Option<String>, _>("patch_id"),
            "shard_path": row.get::<Option<String>, _>("shard_path"),
            "workflow_run_id": row.get::<Option<String>, _>("workflow_run_id"),
            "workflow_type": "feature_development",
            "patch_owner": "feature_pool",
            "planner_feature_backed": true,
            "pool_state": row.get::<String, _>("state")
        }));
    }

    let manual_rows = sqlx::query(
        r#"
        SELECT feature_id, title, workflow_run_id, patch_id, shard_path, state
        FROM supervisor_work_units
        WHERE supervisor_run_id = ?
          AND kind = 'manual_shard'
          AND state IN ('ready_for_integration', 'integrating', 'integrated')
          AND TRIM(COALESCE(shard_path, '')) != ''
          AND COALESCE(json_extract(context_json, '$.integration_skipped'), 0) = 0
          AND (
              COALESCE(json_extract(context_json, '$.integration_input'), 0) = 1
              OR COALESCE(json_extract(context_json, '$.staged_to_integration'), 0) = 1
          )
        ORDER BY updated_at ASC
        "#,
    )
    .bind(run.id.to_string())
    .fetch_all(&state.db)
    .await?;

    for row in manual_rows {
        let manual_id = row
            .get::<Option<String>, _>("feature_id")
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        inputs.push(json!({
            "execution_item_id": manual_id,
            "manual_shard_id": manual_id,
            "feature_id": Value::Null,
            "title": row.get::<String, _>("title"),
            "patch_path": null,
            "patch_id": row.get::<Option<String>, _>("patch_id"),
            "shard_path": row.get::<Option<String>, _>("shard_path"),
            "workflow_run_id": row.get::<Option<String>, _>("workflow_run_id"),
            "workflow_type": "manual_shard",
            "patch_owner": "supervisor_manual_shard",
            "planner_feature_backed": false,
            "pool_state": row.get::<String, _>("state")
        }));
    }

    Ok(inputs)
}

async fn mark_manual_integration_inputs_running(state: &AppState, run: &SupervisorRun, patch_paths: &[Value]) -> Result<()> {
    let manual_ids = patch_paths
        .iter()
        .filter(|item| item.get("workflow_type").and_then(Value::as_str) == Some("manual_shard"))
        .filter_map(|item| item.get("manual_shard_id").and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();

    if manual_ids.is_empty() {
        return Ok(());
    }

    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE supervisor_work_units
        SET state = 'integrating',
            context_json = json_set(
                CASE WHEN json_valid(context_json) THEN context_json ELSE '{}' END,
                '$.workflow_type', 'manual_shard',
                '$.pool_key', 'manual_shard',
                '$.integration_input', json('true'),
                '$.staged_to_integration', json('true'),
                '$.integration_started_at', ?
            ),
            updated_at = ?
        WHERE supervisor_run_id = ?
          AND kind = 'manual_shard'
          AND feature_id IN (SELECT value FROM json_each(?))
          AND state IN ('ready_for_integration', 'patch_ready', 'integrating')
        "#,
    )
    .bind(&now)
    .bind(&now)
    .bind(run.id.to_string())
    .bind(serde_json::to_string(&manual_ids)?)
    .execute(&state.db)
    .await?;

    Ok(())
}

async fn upsert_supervisor_work_unit_for_integration(
    state: &AppState,
    run: &SupervisorRun,
    integration_run_id: Uuid,
    integration_path: &str,
    patch_paths: &[Value],
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let work_unit_id = format!("{}:integration", run.id);
    sqlx::query(
        r#"
        INSERT INTO supervisor_work_units (
            id, supervisor_run_id, repo_id, feature_id, workflow_run_id, patch_id,
            kind, title, state, root_repo_path, shard_path, integration_path,
            priority, queue_position, blocked_reason, waiting_user_input_json, context_json,
            created_at, updated_at
        )
        VALUES (?, ?, NULL, NULL, ?, NULL, 'integration', 'Integration', 'integrating', ?, NULL, ?, 0, NULL, NULL, '{}', ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            workflow_run_id = excluded.workflow_run_id,
            title = excluded.title,
            state = excluded.state,
            root_repo_path = excluded.root_repo_path,
            integration_path = excluded.integration_path,
            context_json = excluded.context_json,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(work_unit_id)
    .bind(run.id.to_string())
    .bind(integration_run_id.to_string())
    .bind(&run.root_repo_path)
    .bind(integration_path)
    .bind(serde_json::to_string(&json!({
        "supervisor_id": run.id,
        "pool_type": "integration"
    }))?)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn reconcile_integration_start_error(
    state: &AppState,
    run: &mut SupervisorRun,
    integration_run_id: Uuid,
    err: &anyhow::Error,
) -> Result<bool> {
    let row = sqlx::query("SELECT status, current_step_id FROM workflow_runs WHERE id = ?")
        .bind(integration_run_id.to_string())
        .fetch_optional(&state.db)
        .await?;

    let Some(row) = row else {
        return Ok(false);
    };

    let status: String = row.get("status");
    let current_step_id: Option<String> = row.try_get::<Option<String>, _>("current_step_id").ok().flatten();
    if !matches!(status.as_str(), "error" | "cancelled" | "success") {
        return Ok(false);
    }

    let now = Utc::now().to_rfc3339();
    let work_unit_state = match status.as_str() {
        "success" => "integrated",
        "cancelled" => "cancelled",
        _ => "development_failed",
    };
    let supervisor_status = match status.as_str() {
        "success" => SupervisorStatus::ReadyToApply,
        _ => SupervisorStatus::Failed,
    };
    let blocked_reason = match status.as_str() {
        "success" => None,
        "cancelled" => Some("integration workflow cancelled"),
        _ => Some("integration workflow failed"),
    };

    sqlx::query(
        r#"
        UPDATE supervisor_work_units
        SET state = ?,
            blocked_reason = ?,
            context_json = json_set(
                CASE WHEN json_valid(context_json) THEN context_json ELSE '{}' END,
                '$.workflow_status', ?,
                '$.current_step_id', ?,
                '$.terminal_at', ?,
                '$.start_error', ?
            ),
            updated_at = ?
        WHERE supervisor_run_id = ?
          AND kind = 'integration'
          AND workflow_run_id = ?
        "#,
    )
    .bind(work_unit_state)
    .bind(blocked_reason)
    .bind(&status)
    .bind(current_step_id.as_deref())
    .bind(&now)
    .bind(format!("{:#}", err))
    .bind(&now)
    .bind(run.id.to_string())
    .bind(integration_run_id.to_string())
    .execute(&state.db)
    .await?;

    run.status = supervisor_status;
    if status == "success" {
        run.final_patch_path = run.integration_path.clone();
    }
    run.updated_at = Utc::now();
    update_supervisor_run(state, run).await?;
    publish_supervisor_snapshot(state, run, "supervisor_snapshot", "integration workflow terminal state reconciled after start error").await?;
    Ok(true)
}

async fn spawn_live_integration_workflow(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    let patch_paths = supervisor_patch_paths(state, run).await?;
    if patch_paths.is_empty() {
        return Err(anyhow!("integration has no unskipped completed feature or staged manual inputs"));
    }

    mark_manual_integration_inputs_running(state, run, &patch_paths).await?;

    let integration_template_id = resolve_supervisor_integration_template_id(state, run).await?;
    let work_unit_id = format!("{}:integration", run.id);
    let workspace = repo_snapshot::workspace_for(&run.root_repo_path, run.id)?;
    let mut workflow_context = supervisor_context(&run, &workspace);
    if let Some(obj) = workflow_context.as_object_mut() {
        obj.insert("supervisor_id".to_string(), Value::String(run.id.to_string()));
        obj.insert("supervisor_run_id".to_string(), Value::String(run.id.to_string()));
        obj.insert("pool_type".to_string(), Value::String("integration".to_string()));
        obj.insert("pool_key".to_string(), Value::String("integration".to_string()));
        obj.insert("workflow_type".to_string(), Value::String("integration".to_string()));
        obj.insert("work_unit_id".to_string(), Value::String(work_unit_id.clone()));
        obj.insert("input_source".to_string(), Value::String("supervisor_work_units".to_string()));
        obj.insert("integration_inputs".to_string(), Value::Array(patch_paths.clone()));
    }

    let integration_item = FeaturePlanItem {
        id: work_unit_id.clone(),
        title: "Integration".to_string(),
        status: FeaturePlanItemStatus::Scheduled,
        summary: format!("{} merge integration", run.title),
        rough_summary: None,
        refinement_workflow_run_id: None,
        applied_sprint_id: None,
        applied_sprint_title: None,
        applied_at: None,
        requirements: Vec::new(),
        acceptance_criteria: Vec::new(),
        implementation_notes: Vec::new(),
        review_expectations: Vec::new(),
        target_files_or_areas: Vec::new(),
        dependencies: Vec::new(),
    };

    let spawn_result = lifecycle::spawn_supervisor_workflow(
        state,
        SupervisorWorkflowSpawnRequest {
            supervisor_run_id: run.id,
            root_repo_path: run.root_repo_path.clone(),
            pool_kind: SupervisorPoolKind::Integration,
            work_unit_id: work_unit_id.clone(),
            shard_id: None,
            feature_id: Some("integration".to_string()),
            title: "Integration".to_string(),
            item: integration_item,
            template_id: integration_template_id,
            workflow_context,
            work_unit_context: json!({
                "source": "run_integration",
                "workflow_type": "integration",
                "pool_type": "integration",
                "pool_key": "integration",
                "integration_inputs": patch_paths
            }),
            initial_state: "integrating".to_string(),
            priority: 0,
            queue_position: None,
        },
    ).await?;

    run.integration_path = Some(spawn_result.workspace_path.clone());
    run.integration_run_id = Some(spawn_result.workflow_run_id);
    run.final_patch_path = None;
    run.status = SupervisorStatus::RunningIntegration;
    run.updated_at = Utc::now();
    update_supervisor_run(state, run).await?;

    if let Err(err) = Box::pin(crate::engine::workflow_lifecycle::execute_workflow_command_value(
        state,
        spawn_result.workflow_run_id,
        crate::engine::workflow_lifecycle::WorkflowCommand::Start {
            mode: crate::engine::workflow_lifecycle::WorkflowExecutionMode::MultiStage,
            step_id: None,
        },
    ))
    .await
    {
        tracing::error!(
            supervisor_run_id = %run.id,
            integration_run_id = %spawn_result.workflow_run_id,
            input_count = patch_paths.len(),
            input_source = "supervisor_work_units",
            error = %format!("{:#}", err),
            "integration workflow start returned an error"
        );
        log_sqlite_foreign_key_check(state, "integration_start_failed").await;
        if reconcile_integration_start_error(state, run, spawn_result.workflow_run_id, &err).await? {
            return Ok(());
        }
        return Err(err);
    }
    Ok(())
}

pub async fn reopen_supervisor_development(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    invalidate_supervisor_integration(state, &mut run).await?;
    run.status = if development_has_remaining_work(&run) {
        SupervisorStatus::RunningChildren
    } else {
        SupervisorStatus::DevelopmentComplete
    };
    reconcile_development_runtime(state, &mut run).await?;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

pub async fn start_supervisor_integration_workflow(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    if matches!(run.status, SupervisorStatus::RunningChildren | SupervisorStatus::DevelopmentComplete | SupervisorStatus::RunningIntegration | SupervisorStatus::ReadyToApply | SupervisorStatus::Failed) {
        refresh_supervisor_child_run_statuses(state, &mut run).await?;
        run.updated_at = Utc::now();
        update_supervisor_run(state, &run).await?;
        run = load_supervisor_run(state, id).await?;
    }
    let patch_paths = supervisor_patch_paths(state, &run).await?;
    if patch_paths.is_empty() {
        return Err(anyhow!("integration has no ready unskipped inputs in the integration pool"));
    }

    if let Some(integration_run_id) = run.integration_run_id.take() {
        sqlx::query("DELETE FROM supervisor_work_units WHERE supervisor_run_id = ? AND kind = 'integration' AND workflow_run_id = ?")
            .bind(run.id.to_string())
            .bind(integration_run_id.to_string())
            .execute(&state.db)
            .await?;
        delete_supervisor_workflow_run_records(state, integration_run_id).await?;
    }
    sqlx::query("DELETE FROM supervisor_work_units WHERE supervisor_run_id = ? AND kind = 'integration'")
        .bind(run.id.to_string())
        .execute(&state.db)
        .await?;
    run.final_patch_path = None;
    run.merge_report = json!({});
    run.validation_report = json!({});

    spawn_live_integration_workflow(state, &mut run).await?;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "integration workflow started with current unskipped inputs").await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

fn supervisor_child_terminal(status: &str) -> bool {
    matches!(status, "success" | "error" | "cancelled")
}

fn supervisor_feature_concurrency(run: &SupervisorRun) -> usize {
    run.context
        .get("feature_concurrency")
        .and_then(Value::as_u64)
        .map(|value| value.max(1).min(64) as usize)
        .unwrap_or(1)
}

fn supervisor_integration_policy(run: &SupervisorRun) -> &str {
    run.context
        .get("integration_policy")
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "auto" | "manual"))
        .unwrap_or("manual")
}

fn development_feature_done(item: &FeaturePlanItem) -> bool {
    matches!(item.status, FeaturePlanItemStatus::Completed | FeaturePlanItemStatus::Applied)
}

fn scheduled_development_feature_ids(run: &SupervisorRun) -> HashSet<String> {
    run.execution_plan_items
        .iter()
        .map(|item| item.feature_plan_item_id.clone())
        .collect()
}

fn development_progress_counts(run: &SupervisorRun) -> (usize, usize) {
    let scheduled_ids = scheduled_development_feature_ids(run);
    let total = scheduled_ids.len();
    let completed = run
        .feature_plan_items
        .iter()
        .filter(|item| scheduled_ids.contains(&item.id) && development_feature_done(item))
        .count();
    (completed, total)
}

fn development_has_remaining_work(run: &SupervisorRun) -> bool {
    let (completed, total) = development_progress_counts(run);
    total > 0 && completed < total
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

async fn current_sprint_id(run: &SupervisorRun) -> Result<String> {
    run.context
        .get("current_sprint_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("current sprint id is missing"))
}

async fn sprint_integration_queue_not_ready_count(state: &AppState, run: &SupervisorRun) -> Result<usize> {
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) AS count
        FROM supervisor_work_units wu
        WHERE wu.supervisor_run_id = ?
          AND wu.kind IN ('feature_development', 'manual_shard')
          AND COALESCE(json_extract(wu.context_json, '$.integration_skipped'), 0) = 0
          AND wu.state NOT IN ('patch_ready', 'ready_for_integration', 'integrating', 'integrated', 'archived')
        "#,
    )
    .bind(run.id.to_string())
    .fetch_one(&state.db)
    .await?;
    Ok(row.get::<i64, _>("count").max(0) as usize)
}

async fn sprint_development_active_count(state: &AppState, sprint_id: &str) -> Result<usize> {
    let row = sqlx::query("SELECT COUNT(*) AS count FROM sprint_features WHERE sprint_id = ? AND development_state IN ('development_queued', 'development_running')")
        .bind(sprint_id)
        .fetch_one(&state.db)
        .await?;
    Ok(row.get::<i64, _>("count").max(0) as usize)
}

async fn sprint_development_terminal_counts(state: &AppState, sprint_id: &str) -> Result<(usize, usize, usize)> {
    let rows = sqlx::query(
        "SELECT development_state, COUNT(*) AS count
         FROM sprint_features
         WHERE sprint_id = ?
           AND status != 'unscheduled'
           AND COALESCE(integration_skipped, 0) = 0
         GROUP BY development_state",
    )
    .bind(sprint_id)
    .fetch_all(&state.db)
    .await?;
    let mut total = 0usize;
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    for row in rows {
        let state_value: String = row.get("development_state");
        let count = row.get::<i64, _>("count").max(0) as usize;
        total += count;
        if matches!(state_value.as_str(), "development_succeeded" | "integrated" | "applied") {
            succeeded += count;
        }
        if state_value == "development_failed" {
            failed += count;
        }
    }
    Ok((total, succeeded, failed))
}

async fn upsert_supervisor_work_unit_for_feature(
    state: &AppState,
    supervisor_run_id: Uuid,
    sprint_id: &str,
    feature_id: &str,
) -> Result<()> {
    let row = sqlx::query(
        r#"
        SELECT sf.feature_id,
               COALESCE(pf.title, sf.feature_id) AS title,
               pf.repo_id,
               sf.current_workflow_run_id,
               sf.current_patch_id,
               sf.shard_path,
               sf.status,
               sf.development_state,
               sf.last_error,
               sf.created_at,
               sf.updated_at,
               sr.root_repo_path,
               sr.integration_path
        FROM sprint_features sf
        LEFT JOIN planner_features pf ON pf.id = sf.feature_id
        LEFT JOIN supervisor_runs sr ON sr.id = sf.supervisor_run_id
        WHERE sf.sprint_id = ?
          AND sf.feature_id = ?
          AND sf.supervisor_run_id = ?
          AND sf.status != 'unscheduled'
        LIMIT 1
        "#,
    )
    .bind(sprint_id)
    .bind(feature_id)
    .bind(supervisor_run_id.to_string())
    .fetch_optional(&state.db)
    .await?;

    let Some(row) = row else {
        return Ok(());
    };

    let now = Utc::now().to_rfc3339();
    let status = row.try_get::<String, _>("status").unwrap_or_else(|_| "scheduled".to_string());
    let development_state = row.try_get::<String, _>("development_state").unwrap_or_else(|_| "scheduled".to_string());
    let last_error = row.try_get::<Option<String>, _>("last_error").ok().flatten();
    let work_unit_state = supervisor_work_unit_state(status.as_str(), development_state.as_str(), last_error.as_deref());
    let work_unit_id = format!("{}:{}", supervisor_run_id, feature_id);

    sqlx::query(
        r#"
        INSERT INTO supervisor_work_units (
            id, supervisor_run_id, repo_id, feature_id, workflow_run_id, patch_id,
            kind, title, state, root_repo_path, shard_path, integration_path,
            priority, queue_position, blocked_reason, waiting_user_input_json, context_json,
            created_at, updated_at
        )
        VALUES (?, ?, ?, ?, ?, ?, 'feature_development', ?, ?, ?, ?, ?, 0, NULL, ?, '{}', ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            repo_id = excluded.repo_id,
            workflow_run_id = excluded.workflow_run_id,
            patch_id = excluded.patch_id,
            title = excluded.title,
            root_repo_path = excluded.root_repo_path,
            shard_path = excluded.shard_path,
            integration_path = excluded.integration_path,
            blocked_reason = excluded.blocked_reason,
            context_json = excluded.context_json,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(work_unit_id)
    .bind(supervisor_run_id.to_string())
    .bind(row.try_get::<Option<String>, _>("repo_id").ok().flatten())
    .bind(row.get::<String, _>("feature_id"))
    .bind(row.try_get::<Option<String>, _>("current_workflow_run_id").ok().flatten())
    .bind(row.try_get::<Option<String>, _>("current_patch_id").ok().flatten())
    .bind(row.get::<String, _>("title"))
    .bind(work_unit_state)
    .bind(row.get::<String, _>("root_repo_path"))
    .bind(row.try_get::<Option<String>, _>("shard_path").ok().flatten())
    .bind(row.try_get::<Option<String>, _>("integration_path").ok().flatten())
    .bind(last_error.clone())
    .bind(serde_json::to_string(&json!({
        "sprint_id": sprint_id,
        "source": "supervisor_runtime",
        "status": status,
        "development_state": development_state,
        "last_error": last_error
    }))?)
    .bind(row.try_get::<String, _>("created_at").unwrap_or_else(|_| now.clone()))
    .bind(now)
    .execute(&state.db)
    .await?;

    Ok(())
}

fn supervisor_work_unit_state(status: &str, development_state: &str, last_error: Option<&str>) -> String {
    if last_error.is_some_and(|value| !value.trim().is_empty()) {
        return "failed".to_string();
    }

    match development_state {
        "development_running" | "running" | "active" => "running".to_string(),
        "waiting" | "waiting_user" | "paused" => "waiting_user".to_string(),
        "development_failed" | "failed" | "blocked" => "failed".to_string(),
        "development_succeeded" => "ready_for_integration".to_string(),
        "integrating" | "integration_running" => "integrating".to_string(),
        "integrated" => "integrated".to_string(),
        "applied" => "archived".to_string(),
        "patch_ready" => "patch_ready".to_string(),
        "deleted_for_restart" | "unscheduled" => "deleted".to_string(),
        _ => match status {
            "active" | "running" => "running".to_string(),
            "waiting" | "paused" => "waiting_user".to_string(),
            "failed" | "blocked" => "failed".to_string(),
            "completed" => "ready_for_integration".to_string(),
            "scheduled" => "queued".to_string(),
            _ => "queued".to_string(),
        },
    }
}

async fn update_sprint_feature_workflow_state(
    state: &AppState,
    sprint_id: &str,
    feature_id: &str,
    workflow_run_id: Option<Uuid>,
    status: &str,
    development_state: &str,
    current_step_id: Option<&str>,
    last_error: Option<&str>,
) -> Result<()> {
    let previous = sqlx::query("SELECT status, development_state, current_workflow_run_id, current_step_id, last_error FROM sprint_features WHERE sprint_id = ? AND feature_id = ?")
        .bind(sprint_id)
        .bind(feature_id)
        .fetch_optional(&state.db)
        .await?;

    let previous_status = previous.as_ref().and_then(|row| row.try_get::<String, _>("status").ok());
    let previous_development_state = previous.as_ref().and_then(|row| row.try_get::<String, _>("development_state").ok());
    let previous_workflow_run_id = previous.as_ref().and_then(|row| row.try_get::<Option<String>, _>("current_workflow_run_id").ok()).flatten();
    let previous_step_id = previous.as_ref().and_then(|row| row.try_get::<Option<String>, _>("current_step_id").ok()).flatten();
    let previous_last_error = previous.as_ref().and_then(|row| row.try_get::<Option<String>, _>("last_error").ok()).flatten();
    let next_workflow_run_id = workflow_run_id.map(|id| id.to_string()).or(previous_workflow_run_id.clone());
    let now = Utc::now().to_rfc3339();

    let update = sqlx::query("UPDATE sprint_features SET current_workflow_run_id = COALESCE(?, current_workflow_run_id), status = ?, development_state = ?, current_step_id = ?, last_error = ?, development_completed_at = CASE WHEN ? IN ('development_succeeded', 'development_failed') THEN COALESCE(development_completed_at, ?) ELSE development_completed_at END, completed_at = CASE WHEN ? = 'completed' THEN COALESCE(completed_at, ?) ELSE completed_at END, updated_at = ? WHERE sprint_id = ? AND feature_id = ?")
        .bind(next_workflow_run_id.clone())
        .bind(status)
        .bind(development_state)
        .bind(current_step_id)
        .bind(last_error)
        .bind(development_state)
        .bind(&now)
        .bind(status)
        .bind(&now)
        .bind(&now)
        .bind(sprint_id)
        .bind(feature_id)
        .execute(&state.db)
        .await?;

    if update.rows_affected() > 0
        && (previous_status.as_deref() != Some(status)
            || previous_development_state.as_deref() != Some(development_state)
            || previous_workflow_run_id != next_workflow_run_id
            || previous_step_id.as_deref() != current_step_id
            || previous_last_error.as_deref() != last_error)
    {
        append_sprint_event(
            state,
            sprint_id,
            "feature_status_changed",
            &now,
            Some(feature_id),
            "feature status changed",
            json!({
                "feature_id": feature_id,
                "workflow_run_id": next_workflow_run_id,
                "status": status,
                "development_state": development_state,
                "current_step_id": current_step_id,
                "last_error": last_error,
                "previous_status": previous_status,
                "previous_development_state": previous_development_state
            }),
        )
        .await?;
    }

    Ok(())
}

async fn sprint_feature_workflow_ids(state: &AppState, sprint_id: &str) -> Result<Vec<Uuid>> {
    let rows = sqlx::query("SELECT current_workflow_run_id FROM sprint_features WHERE sprint_id = ? AND TRIM(COALESCE(current_workflow_run_id, '')) != ''")
        .bind(sprint_id)
        .fetch_all(&state.db)
        .await?;
    rows.into_iter()
        .filter_map(|row| Uuid::parse_str(row.get::<String, _>("current_workflow_run_id").as_str()).ok())
        .collect::<Vec<_>>()
        .pipe(Ok)
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T { f(self) }
}
impl<T> Pipe for T {}

async fn ensure_scheduled_development_children(state: &AppState, run: &mut SupervisorRun) -> Result<bool> {
    let sprint_id = current_sprint_id(run).await?;
    let mut changed = false;

    if run.execution_plan_items.is_empty() {
        return Ok(false);
    }

    let scheduled_items = scheduled_feature_plan_items(run)?;
    let workspace = if !repo_snapshot::workspace_for(&run.root_repo_path, run.id)?.integration.is_dir() {
        let workspace = repo_snapshot::create_workspace(&run.root_repo_path, run.id, &scheduled_items)?;
        patches::create_baseline(&workspace.integration)?;
        run.snapshot_path = None;
        run.integration_path = Some(workspace.integration.to_string_lossy().to_string());
        workspace
    } else {
        repo_snapshot::workspace_for(&run.root_repo_path, run.id)?
    };

    let mut sprint_items = run.execution_plan_items.clone();
    sprint_items.sort_by_key(|item| item.order_index.unwrap_or(i64::MAX));

    for sprint_item in sprint_items {
        let feature_id = sprint_item.feature_plan_item_id.clone();
        let existing = sqlx::query("SELECT current_workflow_run_id FROM sprint_features WHERE sprint_id = ? AND feature_id = ?")
            .bind(&sprint_id)
            .bind(&feature_id)
            .fetch_optional(&state.db)
            .await?;
        if existing
            .as_ref()
            .and_then(|row| row.try_get::<String, _>("current_workflow_run_id").ok())
            .filter(|value| !value.trim().is_empty())
            .is_some()
        {
            continue;
        }

        let Some(feature) = run.feature_plan_items.iter().find(|item| item.id == feature_id).cloned() else {
            continue;
        };
        if development_feature_done(&feature) {
            continue;
        }

        let shard = repo_snapshot::create_shard_from_workspace_snapshot(&workspace, Uuid::new_v4())?;
        patches::create_baseline(&shard)?;
        let shard_path = shard.to_string_lossy().to_string();
        let workflow_run_id = workflow_spawn::spawn_feature_plan_item_workflow(
            state,
            &feature,
            &shard_path,
            sprint_item.workflow_template_id.or_else(|| context_uuid(&run.context, "workflow_template_id")),
            supervisor_context(run, &workspace),
        ).await?;

        sqlx::query("UPDATE sprint_features SET supervisor_run_id = ?, current_workflow_run_id = ?, shard_path = ?, status = 'scheduled', development_state = 'scheduled', updated_at = ? WHERE sprint_id = ? AND feature_id = ?")
            .bind(run.id.to_string())
            .bind(workflow_run_id.to_string())
            .bind(&shard_path)
            .bind(Utc::now().to_rfc3339())
            .bind(&sprint_id)
            .bind(&feature.id)
            .execute(&state.db)
            .await?;

        if let Some(item) = run.feature_plan_items.iter_mut().find(|item| item.id == feature.id) {
            item.status = FeaturePlanItemStatus::Scheduled;
        }
        changed = true;
    }

    Ok(changed)
}

async fn reconcile_development_runtime(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    if !matches!(
        run.status,
        SupervisorStatus::RunningChildren
            | SupervisorStatus::DevelopmentComplete
            | SupervisorStatus::RunningIntegration
            | SupervisorStatus::Validating
            | SupervisorStatus::ReadyToApply
            | SupervisorStatus::Failed
    ) {
        return Ok(());
    }

    refresh_supervisor_child_run_statuses(state, run).await?;
    let spawned = ensure_scheduled_development_children(state, run).await?;
    let sprint_id = current_sprint_id(run).await?;
    let (total, succeeded, failed) = sprint_development_terminal_counts(state, &sprint_id).await?;
    let remaining = total == 0 || succeeded < total;

    if spawned || remaining {
        if matches!(run.status, SupervisorStatus::DevelopmentComplete | SupervisorStatus::RunningIntegration | SupervisorStatus::Validating | SupervisorStatus::ReadyToApply | SupervisorStatus::Failed) {
            invalidate_supervisor_integration(state, run).await?;
        }
        run.status = SupervisorStatus::RunningChildren;
        tick_children(state, run).await?;
        start_next_series_child(state, run).await?;
    } else if failed > 0 {
        run.status = SupervisorStatus::Failed;
    } else if total > 0 && succeeded >= total {
        run.status = SupervisorStatus::DevelopmentComplete;
    }

    if let Some(sprint_id) = run.context.get("current_sprint_id").and_then(Value::as_str).map(str::to_string) {
        save_sprint_features(state, run, &sprint_id, None).await?;
    }

    Ok(())
}

async fn refresh_supervisor_child_run_statuses(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    let sprint_id = match current_sprint_id(run).await {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let rows = sqlx::query("SELECT feature_id, current_workflow_run_id FROM sprint_features WHERE sprint_id = ? AND TRIM(COALESCE(current_workflow_run_id, '')) != ''")
        .bind(&sprint_id)
        .fetch_all(&state.db)
        .await?;
    for row in rows {
        let feature_id: String = row.get("feature_id");
        let workflow_run_id_text: String = row.get("current_workflow_run_id");
        let Ok(workflow_run_id) = Uuid::parse_str(&workflow_run_id_text) else {
            continue;
        };
        match engine::load_run(state, workflow_run_id).await {
            Ok(child_run) => {
                match child_run.status {
                    RunStatus::Success => update_sprint_feature_workflow_state(state, &sprint_id, &feature_id, Some(workflow_run_id), "completed", "development_succeeded", child_run.current_step_id.as_deref(), None).await?,
                    RunStatus::Error => update_sprint_feature_workflow_state(state, &sprint_id, &feature_id, Some(workflow_run_id), "error", "development_failed", child_run.current_step_id.as_deref(), Some("workflow failed")).await?,
                    RunStatus::Cancelled => update_sprint_feature_workflow_state(state, &sprint_id, &feature_id, Some(workflow_run_id), "cancelled", "development_failed", child_run.current_step_id.as_deref(), Some("workflow cancelled")).await?,
                    RunStatus::Queued | RunStatus::Waiting | RunStatus::Paused | RunStatus::Draft => {}
                    RunStatus::Running => update_sprint_feature_workflow_state(state, &sprint_id, &feature_id, Some(workflow_run_id), "development_running", "development_running", child_run.current_step_id.as_deref(), None).await?,
                }
            }
            Err(err) => {
                tracing::warn!(
                    supervisor_run_id = %run.id,
                    workflow_run_id = %workflow_run_id,
                    error = %format!("{:#}", err),
                    "supervisor could not refresh feature workflow status"
                );
            }
        }
    }
    Ok(())
}

async fn start_next_series_child(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    refresh_supervisor_child_run_statuses(state, run).await?;
    let sprint_id = current_sprint_id(run).await?;
    let feature_concurrency = supervisor_feature_concurrency(run);
    let mut active_count = sprint_development_active_count(state, &sprint_id).await?;
    if active_count >= feature_concurrency {
        return Ok(());
    }

    let rows = sqlx::query(
        r#"
        SELECT sf.feature_id, sf.current_workflow_run_id
        FROM sprint_features sf
        LEFT JOIN supervisor_work_units wu
          ON wu.supervisor_run_id = sf.supervisor_run_id
         AND wu.kind = 'feature_development'
         AND wu.feature_id = sf.feature_id
         AND wu.state NOT IN ('deleted', 'archived')
        WHERE sf.sprint_id = ?
          AND sf.supervisor_run_id = ?
          AND sf.development_state = 'scheduled'
          AND TRIM(COALESCE(sf.current_workflow_run_id, '')) != ''
        ORDER BY COALESCE(wu.queue_position, sf.sort_order) ASC, sf.sort_order ASC, sf.created_at ASC
        "#,
    )
        .bind(&sprint_id)
        .bind(run.id.to_string())
        .fetch_all(&state.db)
        .await?;

    for row in rows {
        if active_count >= feature_concurrency {
            break;
        }
        let feature_id: String = row.get("feature_id");
        let workflow_run_id_text: String = row.get("current_workflow_run_id");
        let child_run_id = Uuid::parse_str(&workflow_run_id_text)?;
        let now = Utc::now().to_rfc3339();
        let claim = sqlx::query("UPDATE sprint_features SET status = 'development_queued', development_state = 'development_queued', development_started_at = COALESCE(development_started_at, ?), updated_at = ? WHERE sprint_id = ? AND feature_id = ? AND development_state = 'scheduled'")
            .bind(&now)
            .bind(&now)
            .bind(&sprint_id)
            .bind(&feature_id)
            .execute(&state.db)
            .await?;
        if claim.rows_affected() != 1 {
            continue;
        }

        append_sprint_event(
            state,
            &sprint_id,
            "feature_status_changed",
            &now,
            Some(&feature_id),
            "feature development queued",
            json!({
                "feature_id": feature_id,
                "workflow_run_id": workflow_run_id_text,
                "status": "development_queued",
                "development_state": "development_queued",
                "previous_status": "scheduled",
                "previous_development_state": "scheduled"
            }),
        )
        .await?;

        let supervisor_run_id = run.id;
        let state_for_task = state.clone();
        let sprint_id_for_task = sprint_id.clone();
        let feature_id_for_task = feature_id.clone();
        active_count += 1;

        tracing::info!(
            supervisor_run_id = %supervisor_run_id,
            workflow_run_id = %child_run_id,
            execution_item_id = %feature_id,
            feature_concurrency = feature_concurrency,
            "supervisor claimed feature workflow development slot"
        );

        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            handle.block_on(async move {
                let _ = update_sprint_feature_workflow_state(&state_for_task, &sprint_id_for_task, &feature_id_for_task, Some(child_run_id), "development_running", "development_running", None, None).await;
                match crate::engine::workflow_lifecycle::execute_workflow_command_value(
                    &state_for_task,
                    child_run_id,
                    crate::engine::workflow_lifecycle::WorkflowCommand::Start {
                        mode: crate::engine::workflow_lifecycle::WorkflowExecutionMode::MultiStage,
                        step_id: None,
                    },
                )
                .await
                {
                    Ok(start_result) => {
                        if start_result.get("blocked_on").and_then(Value::as_str) == Some("pause_after_stage") {
                            tracing::warn!(
                                supervisor_run_id = %supervisor_run_id,
                                workflow_run_id = %child_run_id,
                                execution_item_id = %feature_id_for_task,
                                "supervisor autonomous progression blocked by workflow pause-after-stage checkpoint"
                            );
                        }
                    }
                    Err(err) => {
                        let error_text = format!("{:#}", err);
                        let _ = update_sprint_feature_workflow_state(&state_for_task, &sprint_id_for_task, &feature_id_for_task, Some(child_run_id), "error", "development_failed", None, Some(&error_text)).await;
                        tracing::error!(
                            supervisor_run_id = %supervisor_run_id,
                            workflow_run_id = %child_run_id,
                            execution_item_id = %feature_id_for_task,
                            error = %error_text,
                            "supervisor autonomous workflow progression task failed"
                        );
                    }
                }
            });
        });
    }

    Ok(())
}

async fn tick_children(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    refresh_supervisor_child_run_statuses(state, run).await?;
    let sprint_id = current_sprint_id(run).await?;
    let (total, succeeded, failed) = sprint_development_terminal_counts(state, &sprint_id).await?;

    if failed > 0 {
        run.status = SupervisorStatus::Failed;
        return Ok(());
    }

    if total > 0 && succeeded >= total {
        run.status = SupervisorStatus::DevelopmentComplete;
        if supervisor_integration_policy(run) == "auto" {
            spawn_live_integration_workflow(state, run).await?;
        }
    } else {
        run.status = SupervisorStatus::RunningChildren;
        start_next_series_child(state, run).await?;
    }
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

fn scheduled_feature_plan_items(run: &SupervisorRun) -> Result<Vec<FeaturePlanItem>> {
    let mut execution_items = run.execution_plan_items.clone();
    execution_items.sort_by_key(|item| item.order_index.unwrap_or(i64::MAX));
    execution_items
        .iter()
        .map(|execution_item| {
            run.feature_plan_items
                .iter()
                .find(|item| item.id == execution_item.feature_plan_item_id)
                .cloned()
                .ok_or_else(|| anyhow!("feature plan item {} is missing", execution_item.feature_plan_item_id))
        })
        .collect()
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

fn context_uuid(context: &Value, key: &str) -> Option<Uuid> {
    context.get(key).and_then(Value::as_str).and_then(|value| Uuid::parse_str(value).ok())
}

fn supervisor_context(run: &SupervisorRun, workspace: &repo_snapshot::SupervisorWorkspace) -> Value {
    json!({
        "supervisor_run_id": run.id,
        "sprint_id": run.context.get("current_sprint_id").cloned().unwrap_or(Value::Null),
        "sprint_key": run.context.get("current_sprint_key").cloned().unwrap_or(Value::Null),
        "strategy": run.strategy,
        "root_repo_path": run.root_repo_path,
        "snapshot_path": workspace.snapshot,
        "integration_path": workspace.integration,
        "patches_path": workspace.patches,
        "input_source": "supervisor_sprint_feature"
    })
}

async fn insert_supervisor_run(state: &AppState, run: &SupervisorRun) -> Result<()> {
    sqlx::query("INSERT INTO supervisor_runs (id, mode, status, title, root_repo_path, selected_planner_id, flight_deck_json, context_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(run.id.to_string())
        .bind(strategy_str(&run.strategy))
        .bind(status_supervisor_str(&run.status))
        .bind(&run.title)
        .bind(&run.root_repo_path)
        .bind(run.context.get("selected_planner_id").or_else(|| run.context.get("queue_planner_id")).and_then(Value::as_str))
        .bind(serde_json::to_string(run.context.get("flight_deck_settings").unwrap_or(&json!({})))?)
        .bind(serde_json::to_string(&run.context)?)
        .bind(run.created_at.to_rfc3339())
        .bind(run.updated_at.to_rfc3339())
        .execute(&state.db)
        .await?;
    Ok(())
}

pub(crate) async fn update_supervisor_run(state: &AppState, run: &SupervisorRun) -> Result<()> {
    sqlx::query("UPDATE supervisor_runs SET mode = ?, status = ?, title = ?, root_repo_path = ?, context_json = ?, selected_planner_id = COALESCE(NULLIF(?, ''), selected_planner_id), flight_deck_json = ?, updated_at = ? WHERE id = ?")
        .bind(strategy_str(&run.strategy))
        .bind(status_supervisor_str(&run.status))
        .bind(&run.title)
        .bind(&run.root_repo_path)
        .bind(serde_json::to_string(&run.context)?)
        .bind(run.context.get("selected_planner_id").or_else(|| run.context.get("queue_planner_id")).and_then(Value::as_str).unwrap_or(""))
        .bind(serde_json::to_string(run.context.get("flight_deck_settings").unwrap_or(&json!({})))?)
        .bind(run.updated_at.to_rfc3339())
        .bind(run.id.to_string())
        .execute(&state.db)
        .await?;
    Ok(())
}

async fn publish_supervisor_snapshot(state: &AppState, run: &SupervisorRun, event_type: &str, message: &str) -> Result<()> {
    let Some(sprint_id) = run
        .context
        .get("current_sprint_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(());
    };

    let mut snapshot = run.clone();
    hydrate_supervisor_feature_workflows(state, &mut snapshot).await?;
    let event_time = Utc::now().to_rfc3339();
    let payload = json!({
        "supervisor_run_id": run.id,
        "supervisor_run": snapshot,
        "snapshot": true
    });
    append_sprint_event(state, sprint_id, event_type, &event_time, None, message, payload).await?;
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
