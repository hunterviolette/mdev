pub mod models;
pub mod patches;
pub mod repo_snapshot;
pub mod workflow_spawn;

use std::{collections::{HashMap, HashSet}, fs, hash::{Hash, Hasher}, path::PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine,
    engine::capabilities::planner::{
        extract_inference_text, normalize_refined_feature_plan_item, ExecutionPlanItem,
        FeaturePlanItem, FeaturePlanItemStatus,
    },
    models::{RunStatus, SprintEventStreamItem, WorkflowRun},
};
use models::{CreateSupervisorRunRequest, EnsureSupervisorPlannerRequest, EnsureSupervisorPlannerResponse, SupervisorActionRequest, SupervisorExecutionStrategy, SupervisorFeatureWorkflow, SupervisorRun, SupervisorStatus};

pub async fn list_supervisor_runs(state: &AppState) -> Result<Vec<SupervisorRun>> {
    let rows = sqlx::query("SELECT * FROM supervisor_runs ORDER BY updated_at DESC")
        .fetch_all(&state.db)
        .await?;
    let mut runs = rows.into_iter().map(row_to_supervisor_run).collect::<Result<Vec<_>>>()?;
    for run in &mut runs {
        hydrate_supervisor_feature_workflows(state, run).await?;
    }
    Ok(runs)
}

pub async fn load_supervisor_run(state: &AppState, id: Uuid) -> Result<SupervisorRun> {
    let row = sqlx::query("SELECT * FROM supervisor_runs WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(&state.db)
        .await?;
    let mut run = row_to_supervisor_run(row)?;
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
    if let Some(row) = sqlx::query("SELECT id FROM planner_repos WHERE root_repo_path = ?")
        .bind(&normalized_root)
        .fetch_optional(&state.db)
        .await?
    {
        return Ok(row.get("id"));
    }
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let repo_key = planner_repo_key(&normalized_root);
    sqlx::query("INSERT INTO planner_repos (id, root_repo_path, repo_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?)")
        .bind(&id)
        .bind(&normalized_root)
        .bind(repo_key)
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await?;
    Ok(id)
}

async fn load_repo_feature_plan_items(state: &AppState, root: &str) -> Result<Vec<FeaturePlanItem>> {
    let repo_id = ensure_planner_repo_id(state, root).await?;
    let rows = sqlx::query("SELECT payload_json FROM planner_features WHERE repo_id = ? ORDER BY sort_order ASC, created_at ASC")
        .bind(repo_id)
        .fetch_all(&state.db)
        .await?;
    rows.into_iter()
        .map(|row| serde_json::from_str::<FeaturePlanItem>(row.get::<String, _>("payload_json").as_str()).map_err(Into::into))
        .collect()
}

async fn save_repo_feature_plan_items(state: &AppState, root: &str, items: &[FeaturePlanItem]) -> Result<()> {
    let repo_id = ensure_planner_repo_id(state, root).await?;
    let now = Utc::now().to_rfc3339();
    for (index, item) in items.iter().enumerate() {
        sqlx::query("INSERT INTO planner_features (id, repo_id, title, status, sort_order, payload_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET title = excluded.title, status = excluded.status, sort_order = excluded.sort_order, payload_json = excluded.payload_json, updated_at = excluded.updated_at")
            .bind(&item.id)
            .bind(&repo_id)
            .bind(&item.title)
            .bind(serde_json::to_value(&item.status)?.as_str().unwrap_or("planned"))
            .bind(index as i64)
            .bind(serde_json::to_string(item)?)
            .bind(&now)
            .bind(&now)
            .execute(&state.db)
            .await?;
    }
    sqlx::query("DELETE FROM planner_features WHERE repo_id = ? AND id NOT IN (SELECT value FROM json_each(?))")
        .bind(&repo_id)
        .bind(serde_json::to_string(&items.iter().map(|item| item.id.clone()).collect::<Vec<_>>())?)
        .execute(&state.db)
        .await?;
    sqlx::query("UPDATE planner_repos SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&repo_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

async fn hydrate_supervisor_planner_from_repo(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    let persisted_features = load_repo_feature_plan_items(state, &run.root_repo_path).await?;
    if !persisted_features.is_empty() {
        run.feature_plan_items = persisted_features;
        let feature_ids = run.feature_plan_items.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
        run.execution_plan_items.retain(|item| feature_ids.iter().any(|id| id == &item.feature_plan_item_id));
        run.updated_at = Utc::now();
    } else if !run.feature_plan_items.is_empty() {
        save_repo_feature_plan_items(state, &run.root_repo_path, &run.feature_plan_items).await?;
    }
    Ok(())
}

async fn upsert_sprint_record(state: &AppState, run: &SupervisorRun, sprint_id: &str, sprint_key: &str, title: &str, status: &str, sprint_started_at: Option<&str>, sprint_completed_at: Option<&str>) -> Result<()> {
    let repo_id = ensure_planner_repo_id(state, &run.root_repo_path).await?;
    let now = Utc::now().to_rfc3339();
    sqlx::query("INSERT INTO sprints (id, repo_id, sprint_key, title, status, supervisor_run_id, sprint_started_at, sprint_completed_at, created_at, updated_at, summary_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET title = excluded.title, status = excluded.status, supervisor_run_id = excluded.supervisor_run_id, sprint_started_at = COALESCE(sprints.sprint_started_at, excluded.sprint_started_at), sprint_completed_at = COALESCE(excluded.sprint_completed_at, sprints.sprint_completed_at), updated_at = excluded.updated_at, summary_json = excluded.summary_json")
        .bind(sprint_id)
        .bind(&repo_id)
        .bind(sprint_key)
        .bind(title)
        .bind(status)
        .bind(run.id.to_string())
        .bind(sprint_started_at)
        .bind(sprint_completed_at)
        .bind(&now)
        .bind(&now)
        .bind(serde_json::to_string(&json!({
            "root_repo_path": run.root_repo_path,
            "snapshot_path": run.snapshot_path,
            "integration_path": run.integration_path,
            "integration_run_id": run.integration_run_id,
            "final_patch_path": run.final_patch_path,
            "execution_source": "sprint_features"
        }))?)
        .execute(&state.db)
        .await?;
    Ok(())
}

async fn save_sprint_features(state: &AppState, run: &SupervisorRun, sprint_id: &str, completed_at: Option<&str>) -> Result<()> {
    save_repo_feature_plan_items(state, &run.root_repo_path, &run.feature_plan_items).await?;
    let now = Utc::now().to_rfc3339();
    let scheduled_feature_ids = run
        .execution_plan_items
        .iter()
        .map(|item| item.feature_plan_item_id.clone())
        .collect::<Vec<_>>();

    for (index, execution_item) in run.execution_plan_items.iter().enumerate() {
        let planned_status = run
            .feature_plan_items
            .iter()
            .find(|item| item.id == execution_item.feature_plan_item_id)
            .map(|item| serde_json::to_value(&item.status).ok().and_then(|value| value.as_str().map(str::to_string)).unwrap_or_else(|| "scheduled".to_string()))
            .unwrap_or_else(|| "scheduled".to_string());

        sqlx::query("INSERT INTO sprint_features (id, sprint_id, feature_id, supervisor_run_id, status, completed_at, sort_order, development_state, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(sprint_id, feature_id) DO UPDATE SET supervisor_run_id = excluded.supervisor_run_id, sort_order = excluded.sort_order, status = CASE WHEN sprint_features.status IN ('development_queued', 'development_running', 'completed', 'integrated', 'applied', 'error', 'cancelled') THEN sprint_features.status ELSE excluded.status END, development_state = CASE WHEN sprint_features.development_state IN ('development_queued', 'development_running', 'development_succeeded', 'development_failed', 'integration_running', 'integrated', 'applied') THEN sprint_features.development_state ELSE excluded.development_state END, completed_at = COALESCE(sprint_features.completed_at, excluded.completed_at), updated_at = excluded.updated_at")
            .bind(Uuid::new_v4().to_string())
            .bind(sprint_id)
            .bind(&execution_item.feature_plan_item_id)
            .bind(run.id.to_string())
            .bind(planned_status)
            .bind(completed_at)
            .bind(index as i64)
            .bind("scheduled")
            .bind(&now)
            .bind(&now)
            .execute(&state.db)
            .await?;
    }

    sqlx::query("UPDATE sprint_features SET status = 'unscheduled', development_state = CASE WHEN development_state IN ('development_queued', 'development_running', 'development_succeeded', 'integration_running', 'integrated', 'applied') THEN development_state ELSE 'unscheduled' END, updated_at = ? WHERE sprint_id = ? AND feature_id NOT IN (SELECT value FROM json_each(?))")
        .bind(&now)
        .bind(sprint_id)
        .bind(serde_json::to_string(&scheduled_feature_ids)?)
        .execute(&state.db)
        .await?;

    Ok(())
}

async fn clear_planner_feature_development_for_restart(state: &AppState, root: &str, feature_id: &str) -> Result<()> {
    let repo_id = ensure_planner_repo_id(state, root).await?;
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE planner_features SET current_sprint_id = NULL, current_supervisor_run_id = NULL, current_workflow_run_id = NULL, current_patch_id = NULL, development_state = 'deleted_for_restart', unscheduled_at = ?, restarted_at = ?, updated_at = ? WHERE repo_id = ? AND id = ?")
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .bind(&repo_id)
        .bind(feature_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

async fn mark_planner_feature_preserved(state: &AppState, root: &str, feature_id: &str) -> Result<()> {
    let repo_id = ensure_planner_repo_id(state, root).await?;
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE planner_features SET development_state = CASE WHEN current_workflow_run_id IS NULL THEN 'unscheduled' ELSE 'preserved' END, unscheduled_at = ?, updated_at = ? WHERE repo_id = ? AND id = ?")
        .bind(&now)
        .bind(&now)
        .bind(&repo_id)
        .bind(feature_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

async fn append_sprint_event(state: &AppState, sprint_id: &str, event_type: &str, event_time: &str, feature_id: Option<&str>, message: &str, payload: Value) -> Result<SprintEventStreamItem> {
    let payload_json = serde_json::to_string(&payload)?;
    let mut last_error = None;

    for _ in 0..16 {
        let sequence_no = sqlx::query("SELECT COALESCE(MAX(sequence_no), 0) + 1 AS next_sequence_no FROM sprint_events WHERE sprint_id = ?")
            .bind(sprint_id)
            .fetch_one(&state.db)
            .await?
            .get::<i64, _>("next_sequence_no");
        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now().to_rfc3339();

        let inserted = sqlx::query("INSERT INTO sprint_events (id, sprint_id, sequence_no, event_type, event_time, feature_id, actor, message, payload_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&id)
            .bind(sprint_id)
            .bind(sequence_no)
            .bind(event_type)
            .bind(event_time)
            .bind(feature_id)
            .bind("system")
            .bind(message)
            .bind(&payload_json)
            .bind(&created_at)
            .execute(&state.db)
            .await;

        match inserted {
            Ok(_) => {
                let event = SprintEventStreamItem {
                    id,
                    sprint_id: sprint_id.to_string(),
                    sequence_no,
                    event_type: event_type.to_string(),
                    event_time: event_time.to_string(),
                    feature_id: feature_id.map(str::to_string),
                    actor: "system".to_string(),
                    message: message.to_string(),
                    payload,
                    created_at,
                };
                state.publish_sprint_event(event.clone());
                return Ok(event);
            }
            Err(sqlx::Error::Database(err)) if err.code().as_deref() == Some("2067") => {
                last_error = Some(sqlx::Error::Database(err));
                tokio::task::yield_now().await;
                continue;
            }
            Err(err) => return Err(err.into()),
        }
    }

    Err(last_error.unwrap_or_else(|| sqlx::Error::RowNotFound).into())
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
    if let Some(sprint_id) = run.context.get("current_sprint_id").and_then(Value::as_str).map(str::to_string) {
        save_sprint_features(state, &run, &sprint_id, None).await?;
    }
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

pub async fn unschedule_supervisor_feature(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let feature_id = payload
        .get("feature_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("feature_id is required"))?
        .to_string();
    let mode = payload
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("preserve_development");

    let feature_index = run
        .feature_plan_items
        .iter()
        .position(|item| item.id == feature_id)
        .ok_or_else(|| anyhow!("feature {} is not present in the planner log", feature_id))?;

    if mode == "delete_development" && matches!(run.feature_plan_items[feature_index].status, FeaturePlanItemStatus::Applied) {
        return Err(anyhow!("applied features cannot delete development without a revert flow"));
    }

    run.execution_plan_items = run
        .execution_plan_items
        .into_iter()
        .filter(|item| item.feature_plan_item_id != feature_id)
        .enumerate()
        .map(|(index, mut item)| {
            item.order_index = Some(index as i64);
            item
        })
        .collect();

    let sprint_id = run.context.get("current_sprint_id").and_then(Value::as_str).map(str::to_string);
    let mut had_child = false;

    match mode {
        "preserve_development" => {}
        "delete_development" => {
            if let Some(sprint_id) = sprint_id.as_deref() {
                if let Some(row) = sqlx::query("SELECT current_workflow_run_id, shard_path FROM sprint_features WHERE sprint_id = ? AND feature_id = ?")
                    .bind(sprint_id)
                    .bind(&feature_id)
                    .fetch_optional(&state.db)
                    .await?
                {
                    had_child = true;
                    if let Ok(workflow_run_id_text) = row.try_get::<String, _>("current_workflow_run_id") {
                        if let Ok(workflow_run_id) = Uuid::parse_str(&workflow_run_id_text) {
                            delete_supervisor_workflow_run_records(state, workflow_run_id).await?;
                        }
                    }
                    if let Ok(shard_path) = row.try_get::<String, _>("shard_path") {
                        let path = PathBuf::from(shard_path);
                        if path.exists() {
                            if path.is_dir() {
                                fs::remove_dir_all(&path).with_context(|| format!("failed to delete stale shard {}", path.display()))?;
                            } else {
                                fs::remove_file(&path).with_context(|| format!("failed to delete stale shard file {}", path.display()))?;
                            }
                        }
                    }
                    invalidate_supervisor_integration(state, &mut run).await?;
                }
            }
        }
        other => return Err(anyhow!("unsupported unschedule mode {}", other)),
    }

    if let Some(feature) = run.feature_plan_items.get_mut(feature_index) {
        if mode == "delete_development" {
            feature.status = FeaturePlanItemStatus::Fine;
        } else if matches!(feature.status, FeaturePlanItemStatus::Scheduled) {
            feature.status = FeaturePlanItemStatus::Fine;
        }
    }

    if let Some(obj) = run.context.as_object_mut() {
        obj.insert("last_unscheduled_feature_id".to_string(), Value::String(feature_id.clone()));
        obj.insert("last_unschedule_mode".to_string(), Value::String(mode.to_string()));
        obj.insert("last_unschedule_had_child_workflow".to_string(), Value::Bool(had_child));
        obj.insert("last_unscheduled_at".to_string(), Value::String(Utc::now().to_rfc3339()));
    }

    reconcile_development_runtime(state, &mut run).await?;
    save_repo_feature_plan_items(state, &run.root_repo_path, &run.feature_plan_items).await?;
    match mode {
        "preserve_development" => mark_planner_feature_preserved(state, &run.root_repo_path, &feature_id).await?,
        "delete_development" => clear_planner_feature_development_for_restart(state, &run.root_repo_path, &feature_id).await?,
        _ => {}
    }
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

pub async fn refine_supervisor_feature(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let feature_id = payload
        .get("feature_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("feature_id is required"))?;
    let feature = run
        .feature_plan_items
        .iter()
        .find(|item| item.id == feature_id)
        .cloned()
        .ok_or_else(|| anyhow!("feature {} is missing", feature_id))?;
    let workflow_template_id = payload
        .get("workflow_template_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .or_else(|| context_uuid(&run.context, "planner_refinement_template_id"))
        .or_else(|| context_uuid(&run.context, "workflow_template_id"));
    let workflow_template_id = match workflow_template_id {
        Some(value) => value,
        None => default_refinement_workflow_template_id(state)
            .await?
            .ok_or_else(|| anyhow!("workflow_template_id is required for feature refinement"))?,
    };
    let refinement_path = run.root_repo_path.clone();
    let workflow_run_id = workflow_spawn::spawn_feature_plan_item_workflow(
        state,
        &feature,
        &refinement_path,
        Some(workflow_template_id),
        json!({
            "supervisor_run_id": run.id,
            "feature_id": feature.id,
            "input_source": "supervisor_planner_feature",
            "structured_output": {
                "enabled": true,
                "schema_armed": true,
                "schema_id": "supervisor_feature_plan_item_v1",
                "auto_apply_armed": true,
                "preserve_rough_definition": true,
                "apply_handler": "supervisor_planner_item",
                "rough_definition": feature.rough_summary.clone().unwrap_or_else(|| feature.summary.clone())
            }
        }),
    ).await?;
    if let Some(feature_item) = run.feature_plan_items.iter_mut().find(|item| item.id == feature_id) {
        feature_item.refinement_workflow_run_id = Some(workflow_run_id);
    }
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;

    Ok(json!({ "ok": true, "workflow_run_id": workflow_run_id, "reused": false }))
}

pub async fn apply_refined_feature_output_from_workflow(state: &AppState, workflow_run: &WorkflowRun, capability_results: &[Value]) -> Result<()> {
    let supervisor_context = workflow_run
        .context
        .get("supervisor")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let structured_output = supervisor_context
        .get("structured_output")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let global_capabilities = workflow_run
        .context
        .get("workflow_engine")
        .and_then(|value| value.get("global_state"))
        .and_then(|value| value.get("capabilities"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let planner = global_capabilities
        .get("planner")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let apply_handler = structured_output
        .get("apply_handler")
        .and_then(Value::as_str)
        .unwrap_or("supervisor_planner_item");
    if apply_handler != "supervisor_planner_item" {
        return Err(anyhow::anyhow!("planner apply handler mismatch: {}", apply_handler));
    }

    let schema_id = structured_output
        .get("schema_id")
        .and_then(Value::as_str)
        .or_else(|| planner.get("schema_id").and_then(Value::as_str))
        .unwrap_or("supervisor_feature_plan_item_v1");
    if schema_id != "supervisor_feature_plan_item_v1" {
        return Err(anyhow::anyhow!("planner schema mismatch: {}", schema_id));
    }

    let supervisor_run_id = supervisor_context
        .get("supervisor_run_id")
        .and_then(Value::as_str)
        .or_else(|| planner.get("supervisor_run_id").and_then(Value::as_str))
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| anyhow::anyhow!("supervisor_run_id is required for planner item apply"))?;
    let feature_id = supervisor_context
        .get("feature_id")
        .and_then(Value::as_str)
        .or_else(|| planner.get("selected_feature_id").and_then(Value::as_str))
        .ok_or_else(|| anyhow::anyhow!("feature_id is required for planner item apply"))?;
    let output_text = extract_inference_text(capability_results)
        .ok_or_else(|| anyhow::anyhow!("inference output is required for planner item apply"))?;
    let mut supervisor_run = load_supervisor_run(state, supervisor_run_id).await?;
    let existing_index = supervisor_run
        .feature_plan_items
        .iter()
        .position(|item| item.id == feature_id)
        .ok_or_else(|| anyhow::anyhow!("feature {} is missing", feature_id))?;
    let existing = supervisor_run.feature_plan_items[existing_index].clone();
    let preserved_rough = existing
        .rough_summary
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| match existing.status {
            FeaturePlanItemStatus::Rough => Some(existing.summary.clone()),
            _ => None,
        })
        .or_else(|| structured_output.get("rough_definition").and_then(Value::as_str).map(ToString::to_string));
    let refined = normalize_refined_feature_plan_item(
        &existing.id,
        &existing.title,
        preserved_rough,
        output_text.as_str(),
    )?;
    supervisor_run.feature_plan_items[existing_index] = refined;
    save_repo_feature_plan_items(state, &supervisor_run.root_repo_path, &supervisor_run.feature_plan_items).await?;
    supervisor_run.updated_at = Utc::now();
    update_supervisor_run(state, &supervisor_run).await?;
    Ok(())
}

async fn delete_supervisor_workflow_run_records(state: &AppState, run_id: Uuid) -> Result<()> {
    let run_id_text = run_id.to_string();
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
    sqlx::query("DELETE FROM workflow_runs WHERE id = ?")
        .bind(&run_id_text)
        .execute(&state.db)
        .await?;
    Ok(())
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
        return Err(anyhow!("sprint has no scheduled planner items"));
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
    }
    let scheduled_items = scheduled_feature_plan_items(&run)?;
    let scheduled_feature_ids = scheduled_items.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
    for item in &mut run.feature_plan_items {
        if scheduled_feature_ids.iter().any(|id| id == &item.id) && !matches!(item.status, FeaturePlanItemStatus::Completed | FeaturePlanItemStatus::Applied) {
            item.status = FeaturePlanItemStatus::Scheduled;
        }
    }
    upsert_sprint_record(state, &run, &sprint_id, &sprint_key, &format!("Sprint {}", sprint_key), "running", Some(&sprint_started_at), None).await?;
    save_sprint_features(state, &run, &sprint_id, None).await?;
    append_sprint_event(state, &sprint_id, "sprint_started", &sprint_started_at, None, "sprint started", json!({ "sprint_key": sprint_key })).await?;
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
    patches::create_baseline(&workspace.integration)?;
    let workflow_template_id = context_uuid(&run.context, "workflow_template_id");
    let integration_template_id = match run.strategy {
        SupervisorExecutionStrategy::Parallel => context_uuid(&run.context, "integration_template_id"),
        SupervisorExecutionStrategy::Series => None,
    };

    for item in &scheduled_items {
        let shard = repo_snapshot::shard_path(&workspace, &item.id);
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
    let Some(supervisor_id) = supervisor_context_uuid(&supervisor_context, "supervisor_run_id") else {
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

    if run.integration_run_id == Some(workflow_run_id) {
        match status {
            RunStatus::Success => {
                let integration_path = run.integration_path.clone().filter(|value| !value.trim().is_empty()).ok_or_else(|| anyhow!("integration path missing"))?;
                let workspace = repo_snapshot::workspace_for(&run.root_repo_path, run.id)?;
                let final_patch = workspace.patches.join("integration-final.patch");
                patches::generate_patch(&PathBuf::from(integration_path), &final_patch)?;
                run.final_patch_path = Some(final_patch.to_string_lossy().to_string());
                run.status = SupervisorStatus::ReadyToApply;
                if let Some(sprint_id) = run.context.get("current_sprint_id").and_then(Value::as_str) {
                    sqlx::query("UPDATE sprints SET status = ?, integration_completed_at = COALESCE(integration_completed_at, ?), updated_at = ? WHERE id = ?")
                        .bind("ready_to_apply")
                        .bind(&now)
                        .bind(&now)
                        .bind(sprint_id)
                        .execute(&state.db)
                        .await?;
                    append_sprint_event(state, sprint_id, "integration_completed", &now, None, "integration workflow completed", json!({ "workflow_run_id": workflow_run_id })).await?;
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

pub async fn start_supervisor_child_workflow(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let feature_id = payload_feature_id(&payload)?;
    let sprint_id = current_sprint_id(&run).await?;

    refresh_supervisor_child_run_statuses(state, &mut run).await?;
    let (feature_id, workflow_run_id, _development_state) = supervisor_child_workflow_row(state, &sprint_id, &feature_id).await?;
    let child_run = engine::load_run(state, workflow_run_id).await?;

    if matches!(child_run.status, RunStatus::Queued | RunStatus::Running | RunStatus::Waiting) {
        publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "feature workflow already running").await?;
        return Ok(json!({
            "ok": true,
            "started": false,
            "reason": "already_running",
            "workflow_run_id": workflow_run_id,
            "supervisor_run": run
        }));
    }

    sqlx::query(
        "UPDATE sprint_features
         SET status = 'scheduled',
             development_state = 'scheduled',
             last_error = NULL,
             updated_at = ?
         WHERE sprint_id = ?
           AND feature_id = ?
           AND development_state NOT IN ('development_succeeded', 'integrated', 'applied')",
    )
    .bind(Utc::now().to_rfc3339())
    .bind(&sprint_id)
    .bind(&feature_id)
    .execute(&state.db)
    .await?;

    run.status = SupervisorStatus::RunningChildren;
    start_next_series_child(state, &mut run).await?;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "feature workflow start requested").await?;

    Ok(json!({
        "ok": true,
        "workflow_run_id": workflow_run_id,
        "supervisor_run": run
    }))
}

pub async fn pause_supervisor_child_workflow(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let run = load_supervisor_run(state, id).await?;
    let feature_id = payload_feature_id(&payload)?;
    let sprint_id = current_sprint_id(&run).await?;
    let (feature_id, workflow_run_id, _development_state) = supervisor_child_workflow_row(state, &sprint_id, &feature_id).await?;

    let result = engine::pause_run(state, workflow_run_id).await?;
    update_sprint_feature_workflow_state(
        state,
        &sprint_id,
        &feature_id,
        Some(workflow_run_id),
        "scheduled",
        "scheduled",
        None,
        None,
    )
    .await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "feature workflow pause requested").await?;

    Ok(json!({
        "ok": true,
        "workflow_run_id": workflow_run_id,
        "pause_result": result,
        "supervisor_run": run
    }))
}

pub async fn remove_supervisor_child_workflow(state: &AppState, id: Uuid, payload: Value) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let feature_id = payload
        .get("feature_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("feature_id is required"))?;
    let sprint_id = current_sprint_id(&run).await?;

    let row = sqlx::query("SELECT feature_id, current_workflow_run_id, shard_path FROM sprint_features WHERE sprint_id = ? AND feature_id = ?")
        .bind(&sprint_id)
        .bind(&feature_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| anyhow!("feature workflow is missing"))?;

    let execution_item_id: String = row.get("feature_id");
    let workflow_run_id_text = row
        .try_get::<Option<String>, _>("current_workflow_run_id")
        .ok()
        .flatten();
    if let Some(workflow_run_id_text) = workflow_run_id_text {
        if let Ok(workflow_run_id) = Uuid::parse_str(&workflow_run_id_text) {
            delete_supervisor_workflow_run_records(state, workflow_run_id).await?;
        }
    }

    let old_shard_path = row
        .try_get::<Option<String>, _>("shard_path")
        .ok()
        .flatten();
    if let Some(old_shard_path) = old_shard_path {
        let path = PathBuf::from(old_shard_path);
        if path.exists() {
            if path.is_dir() {
                fs::remove_dir_all(&path).with_context(|| format!("failed to delete stale shard {}", path.display()))?;
            } else {
                fs::remove_file(&path).with_context(|| format!("failed to delete stale shard file {}", path.display()))?;
            }
        }
    }

    if let Some(integration_run_id) = run.integration_run_id {
        delete_supervisor_workflow_run_records(state, integration_run_id).await?;
    }
    run.integration_run_id = None;
    run.final_patch_path = None;
    run.merge_report = json!({});
    run.validation_report = json!({});

    let workspace = repo_snapshot::workspace_for(&run.root_repo_path, run.id)?;
    let feature = run
        .feature_plan_items
        .iter()
        .find(|item| item.id == execution_item_id)
        .cloned()
        .ok_or_else(|| anyhow!("feature plan item {} is missing", execution_item_id))?;
    let template_id = run
        .execution_plan_items
        .iter()
        .find(|item| item.feature_plan_item_id == execution_item_id)
        .and_then(|item| item.workflow_template_id)
        .or_else(|| context_uuid(&run.context, "workflow_template_id"));
    let shard = repo_snapshot::refresh_shard_from_worktree(&run.root_repo_path, run.id, &execution_item_id)?;
    patches::create_baseline(&shard)?;
    let shard_path = shard.to_string_lossy().to_string();

    let new_workflow_run_id = workflow_spawn::spawn_feature_plan_item_workflow(
        state,
        &feature,
        &shard_path,
        template_id,
        supervisor_context(&run, &workspace),
    ).await?;

    sqlx::query("UPDATE sprint_features SET current_workflow_run_id = ?, shard_path = ?, status = 'scheduled', development_state = 'scheduled', current_patch_id = NULL, last_error = NULL, development_started_at = NULL, development_completed_at = NULL, completed_at = NULL, updated_at = ? WHERE sprint_id = ? AND feature_id = ?")
        .bind(new_workflow_run_id.to_string())
        .bind(&shard_path)
        .bind(Utc::now().to_rfc3339())
        .bind(&sprint_id)
        .bind(&execution_item_id)
        .execute(&state.db)
        .await?;

    if let Some(feature_item) = run.feature_plan_items.iter_mut().find(|item| item.id == execution_item_id) {
        if !matches!(feature_item.status, FeaturePlanItemStatus::Applied) {
            feature_item.status = FeaturePlanItemStatus::Scheduled;
        }
    }

    run.status = SupervisorStatus::RunningChildren;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;

    start_next_series_child(state, &mut run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "feature workflow reset").await?;

    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "feature workflow reset").await?;
    return Ok(json!({
        "ok": true,
        "supervisor_run": run,
        "workflow_run_id": new_workflow_run_id,
        "invalidated_integration": true
    }));

}

pub async fn advance_supervisor_run(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    match run.status {
        SupervisorStatus::RunningChildren | SupervisorStatus::DevelopmentComplete => reconcile_development_runtime(state, &mut run).await?,
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
    if matches!(run.status, SupervisorStatus::RunningIntegration | SupervisorStatus::Validating | SupervisorStatus::ReadyToApply) {
        tick_integration(state, &mut run).await?;
        run.updated_at = Utc::now();
        update_supervisor_run(state, &run).await?;
    }
    let integration_run_id = run.integration_run_id.ok_or_else(|| anyhow!("integration workflow must complete successfully before applying sprint"))?;
    let integration_run = engine::load_run(state, integration_run_id).await?;
    if !matches!(integration_run.status, RunStatus::Success) || !matches!(run.status, SupervisorStatus::ReadyToApply) {
        return Err(anyhow!("current integration workflow must complete successfully before applying sprint"));
    }
    let final_patch = run.final_patch_path.clone().filter(|value| !value.trim().is_empty()).ok_or_else(|| anyhow!("current integration workflow completed without a final patch"))?;
    patches::apply_final_patch_to_root(&PathBuf::from(&run.root_repo_path), &PathBuf::from(final_patch))?;
    let completed_at = Utc::now();
    let completed_at_text = completed_at.to_rfc3339();
    let scheduled_feature_ids = run
        .execution_plan_items
        .iter()
        .map(|item| item.feature_plan_item_id.clone())
        .collect::<Vec<_>>();
    let sprint_id = run
        .context
        .get("current_sprint_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let sprint_key = run
        .context
        .get("current_sprint_key")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| sprint_key_for(&run.root_repo_path, &sprint_id));
    let sprint_title = format!("Sprint {} completed {}", sprint_key, completed_at_text);

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
        "title": sprint_title,
        "status": "applied",
        "applied_at": completed_at_text,
        "root_repo_path": run.root_repo_path,
        "snapshot_path": run.snapshot_path,
        "integration_path": run.integration_path,
        "integration_run_id": run.integration_run_id,
        "final_patch_path": run.final_patch_path,
        "features": completed_features,
        "execution_source": "sprint_features"
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
    sqlx::query("UPDATE sprint_features SET status = 'applied', development_state = 'applied', completed_at = COALESCE(completed_at, ?), updated_at = ? WHERE sprint_id = ?")
        .bind(&completed_at_text)
        .bind(&completed_at_text)
        .bind(&sprint_id)
        .execute(&state.db)
        .await?;
    append_sprint_event(state, &sprint_id, "sprint_completed", &completed_at_text, None, "sprint completed", json!({ "sprint_key": sprint_key, "features": completed_features })).await?;

    if !run.context.is_object() {
        run.context = json!({});
    }
    if let Some(obj) = run.context.as_object_mut() {
        obj.insert("sprint_completed_at".to_string(), Value::String(completed_at_text.clone()));
        obj.insert("completed_features".to_string(), sprint_record.get("features").cloned().unwrap_or_else(|| json!([])));
        let history = obj.entry("sprint_history".to_string()).or_insert_with(|| json!([]));
        if let Some(items) = history.as_array_mut() {
            items.push(sprint_record);
        }
        obj.remove("current_sprint_id");
        obj.remove("current_sprint_key");
        obj.remove("current_sprint_started_at");
    }
    run.status = SupervisorStatus::Applied;
    run.updated_at = completed_at;
    update_supervisor_run(state, &run).await?;
    Ok(json!({ "ok": true, "status": "applied", "sprint_completed_at": completed_at_text }))
}

fn supervisor_patch_paths(run: &SupervisorRun) -> Vec<Value> {
    let Ok(workspace) = repo_snapshot::workspace_for(&run.root_repo_path, run.id) else {
        return Vec::new();
    };
    run.execution_plan_items.iter().filter_map(|item| {
        let feature = run.feature_plan_items.iter().find(|feature| feature.id == item.feature_plan_item_id)?;
        let shard_path = repo_snapshot::shard_path(&workspace, &feature.id).to_string_lossy().to_string();
        Some(json!({
            "execution_item_id": feature.id,
            "title": feature.title,
            "patch_path": null,
            "patch_id": null,
            "shard_path": shard_path,
            "workflow_run_id": null
        }))
    }).collect::<Vec<_>>()
}

async fn spawn_live_integration_workflow(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    let workspace = repo_snapshot::refresh_integration_from_worktree(&run.root_repo_path, run.id)?;
    patches::create_baseline(&workspace.integration)?;
    let integration_path = workspace.integration.to_string_lossy().to_string();
    let patch_paths = supervisor_patch_paths(run);
    let integration_run_id = workflow_spawn::spawn_integration_workflow(
        state,
        &format!("{} merge integration", run.title),
        &integration_path,
        patch_paths,
        context_uuid(&run.context, "integration_template_id"),
        json!({
            "supervisor_run_id": run.id,
            "sprint_id": run.context.get("current_sprint_id").cloned().unwrap_or(Value::Null),
            "strategy": run.strategy,
            "root_repo_path": run.root_repo_path,
            "snapshot_path": run.snapshot_path,
            "integration_path": integration_path,
            "live_worktree": false,
            "integration_source": "current_worktree_copy"
        }),
    ).await?;
    let _ = Box::pin(engine::start_run(state, integration_run_id, None)).await?;
    run.integration_path = Some(integration_path);
    run.integration_run_id = Some(integration_run_id);
    run.final_patch_path = None;
    run.status = SupervisorStatus::RunningIntegration;
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

pub async fn restart_supervisor_integration_workflow(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    let sprint_id = current_sprint_id(&run).await?;
    let (total, succeeded, failed) = sprint_development_terminal_counts(state, &sprint_id).await?;
    if failed > 0 || total == 0 || succeeded < total {
        return Err(anyhow!("development must complete successfully before integration can start"));
    }
    if let Some(integration_run_id) = run.integration_run_id {
        delete_supervisor_workflow_run_records(state, integration_run_id).await?;
    }
    run.integration_run_id = None;
    run.final_patch_path = None;
    run.merge_report = json!({});
    run.validation_report = json!({});
    run.status = SupervisorStatus::DevelopmentComplete;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;

    spawn_live_integration_workflow(state, &mut run).await?;
    run.final_patch_path = None;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "integration workflow restarted").await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

pub async fn start_supervisor_integration_workflow(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    if matches!(run.status, SupervisorStatus::RunningChildren | SupervisorStatus::DevelopmentComplete | SupervisorStatus::Failed) {
        reconcile_development_runtime(state, &mut run).await?;
        run.updated_at = Utc::now();
        update_supervisor_run(state, &run).await?;
        run = load_supervisor_run(state, id).await?;
    }
    if !matches!(run.status, SupervisorStatus::DevelopmentComplete | SupervisorStatus::RunningIntegration | SupervisorStatus::ReadyToApply | SupervisorStatus::Failed) {
        return Err(anyhow!("development must complete before integration can start"));
    }
    let sprint_id = current_sprint_id(&run).await?;
    let (total, succeeded, failed) = sprint_development_terminal_counts(state, &sprint_id).await?;
    if failed > 0 || total == 0 || succeeded < total {
        return Err(anyhow!("development must complete successfully before integration can start"));
    }
    if matches!(run.status, SupervisorStatus::RunningIntegration | SupervisorStatus::ReadyToApply) && run.integration_run_id.is_some() {
        return Ok(json!({ "ok": true, "idempotent": true, "supervisor_run": run }));
    }
    spawn_live_integration_workflow(state, &mut run).await?;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    publish_supervisor_snapshot(state, &run, "supervisor_snapshot", "integration workflow started").await?;
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
        delete_supervisor_workflow_run_records(state, integration_run_id).await?;
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

async fn sprint_development_active_count(state: &AppState, sprint_id: &str) -> Result<usize> {
    let row = sqlx::query("SELECT COUNT(*) AS count FROM sprint_features WHERE sprint_id = ? AND development_state IN ('development_queued', 'development_running')")
        .bind(sprint_id)
        .fetch_one(&state.db)
        .await?;
    Ok(row.get::<i64, _>("count").max(0) as usize)
}

async fn sprint_development_terminal_counts(state: &AppState, sprint_id: &str) -> Result<(usize, usize, usize)> {
    let rows = sqlx::query("SELECT development_state, COUNT(*) AS count FROM sprint_features WHERE sprint_id = ? AND status != 'unscheduled' GROUP BY development_state")
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

        let shard = repo_snapshot::create_shard_from_snapshot(&workspace, &feature.id)?;
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

    let rows = sqlx::query("SELECT feature_id, current_workflow_run_id FROM sprint_features WHERE sprint_id = ? AND development_state = 'scheduled' AND TRIM(COALESCE(current_workflow_run_id, '')) != '' ORDER BY sort_order ASC, created_at ASC")
        .bind(&sprint_id)
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
                match engine::start_run(&state_for_task, child_run_id, None).await {
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
            let integration_path = run.integration_path.clone().filter(|value| !value.trim().is_empty()).ok_or_else(|| anyhow!("integration path missing"))?;
            let workspace = repo_snapshot::workspace_for(&run.root_repo_path, run.id)?;
            let final_patch = workspace.patches.join("integration-final.patch");
            patches::generate_patch(&PathBuf::from(integration_path), &final_patch)?;
            run.final_patch_path = Some(final_patch.to_string_lossy().to_string());
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
    let stored_plan = json!({
        "feature_plan_items": run.feature_plan_items,
        "execution_plan_items": run.execution_plan_items
    });
    sqlx::query("INSERT INTO supervisor_runs (id, mode, status, title, root_repo_path, snapshot_path, integration_path, features_json, child_runs_json, integration_run_id, final_patch_path, merge_report_json, validation_report_json, context_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(run.id.to_string())
        .bind(strategy_str(&run.strategy))
        .bind(status_supervisor_str(&run.status))
        .bind(&run.title)
        .bind(&run.root_repo_path)
        .bind(&run.snapshot_path)
        .bind(&run.integration_path)
        .bind(serde_json::to_string(&stored_plan)?)
        .bind("[]")
        .bind(run.integration_run_id.map(|id| id.to_string()))
        .bind(&run.final_patch_path)
        .bind(serde_json::to_string(&run.merge_report)?)
        .bind(serde_json::to_string(&run.validation_report)?)
        .bind(serde_json::to_string(&run.context)?)
        .bind(run.created_at.to_rfc3339())
        .bind(run.updated_at.to_rfc3339())
        .execute(&state.db)
        .await?;
    Ok(())
}

pub(crate) async fn update_supervisor_run(state: &AppState, run: &SupervisorRun) -> Result<()> {
    let stored_plan = json!({
        "feature_plan_items": run.feature_plan_items,
        "execution_plan_items": run.execution_plan_items
    });
    sqlx::query("UPDATE supervisor_runs SET mode = ?, status = ?, title = ?, root_repo_path = ?, snapshot_path = ?, integration_path = ?, features_json = ?, child_runs_json = ?, integration_run_id = ?, final_patch_path = ?, merge_report_json = ?, validation_report_json = ?, context_json = ?, updated_at = ? WHERE id = ?")
        .bind(strategy_str(&run.strategy))
        .bind(status_supervisor_str(&run.status))
        .bind(&run.title)
        .bind(&run.root_repo_path)
        .bind(&run.snapshot_path)
        .bind(&run.integration_path)
        .bind(serde_json::to_string(&stored_plan)?)
        .bind("[]")
        .bind(run.integration_run_id.map(|id| id.to_string()))
        .bind(&run.final_patch_path)
        .bind(serde_json::to_string(&run.merge_report)?)
        .bind(serde_json::to_string(&run.validation_report)?)
        .bind(serde_json::to_string(&run.context)?)
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
    let Some(sprint_id) = run.context.get("current_sprint_id").and_then(Value::as_str).filter(|value| !value.trim().is_empty()).map(str::to_string) else {
        return Ok(());
    };

    let rows = sqlx::query("SELECT sf.feature_id, COALESCE(pf.title, sf.feature_id) AS title, sf.shard_path, sf.current_workflow_run_id, sf.status, sf.development_state, sf.current_step_id, sf.current_patch_id, sf.last_error FROM sprint_features sf LEFT JOIN planner_features pf ON pf.id = sf.feature_id WHERE sf.sprint_id = ? AND sf.status != 'unscheduled' ORDER BY sf.sort_order ASC, sf.created_at ASC")
        .bind(&sprint_id)
        .fetch_all(&state.db)
        .await?;

    run.feature_workflows = rows
        .into_iter()
        .map(|row| {
            let workflow_run_id = row
                .try_get::<Option<String>, _>("current_workflow_run_id")
                .ok()
                .flatten()
                .and_then(|value| Uuid::parse_str(value.as_str()).ok());
            SupervisorFeatureWorkflow {
                feature_id: row.get("feature_id"),
                title: row.get("title"),
                shard_path: row.try_get("shard_path").ok(),
                workflow_run_id,
                status: row.try_get::<String, _>("status").unwrap_or_else(|_| "scheduled".to_string()),
                development_state: row.try_get::<String, _>("development_state").unwrap_or_else(|_| "scheduled".to_string()),
                current_step_id: row.try_get("current_step_id").ok(),
                current_patch_id: row.try_get("current_patch_id").ok(),
                last_error: row.try_get("last_error").ok(),
            }
        })
        .collect();
    Ok(())
}

fn row_to_supervisor_run(row: sqlx::sqlite::SqliteRow) -> Result<SupervisorRun> {
    let features_json = row.get::<String, _>("features_json");
    let stored_plan: Value = serde_json::from_str(features_json.as_str())?;
    let feature_plan_items: Vec<FeaturePlanItem> = if stored_plan.get("feature_plan_items").is_some() {
        serde_json::from_value(stored_plan.get("feature_plan_items").cloned().unwrap_or_else(|| json!([])))?
    } else {
        serde_json::from_value(stored_plan.clone())?
    };
    let execution_plan_items: Vec<ExecutionPlanItem> = if stored_plan.get("execution_plan_items").is_some() {
        serde_json::from_value(stored_plan.get("execution_plan_items").cloned().unwrap_or_else(|| json!([])))?
    } else {
        feature_plan_items.iter().enumerate().map(|(index, item): (usize, &FeaturePlanItem)| ExecutionPlanItem {
            feature_plan_item_id: item.id.clone(),
            workflow_template_id: None,
            order_index: Some(index as i64),
        }).collect()
    };
    Ok(SupervisorRun {
        id: Uuid::parse_str(row.get::<String, _>("id").as_str())?,
        strategy: parse_strategy(row.get::<String, _>("mode").as_str()),
        status: parse_status(row.get::<String, _>("status").as_str()),
        title: row.get("title"),
        root_repo_path: row.get("root_repo_path"),
        snapshot_path: row.get("snapshot_path"),
        integration_path: row.get("integration_path"),
        feature_plan_items,
        execution_plan_items,
        feature_workflows: Vec::new(),
        integration_run_id: row.get::<Option<String>, _>("integration_run_id").map(|value| Uuid::parse_str(value.as_str())).transpose()?,
        final_patch_path: row.get("final_patch_path"),
        merge_report: serde_json::from_str(row.get::<String, _>("merge_report_json").as_str())?,
        validation_report: serde_json::from_str(row.get::<String, _>("validation_report_json").as_str())?,
        context: serde_json::from_str(row.get::<String, _>("context_json").as_str())?,
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
