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
    models::{RunStatus, WorkflowRun},
};
use models::{CreateSupervisorRunRequest, EnsureSupervisorPlannerRequest, EnsureSupervisorPlannerResponse, SupervisorActionRequest, SupervisorChildRun, SupervisorExecutionStrategy, SupervisorRun, SupervisorStatus};

pub async fn list_supervisor_runs(state: &AppState) -> Result<Vec<SupervisorRun>> {
    let rows = sqlx::query("SELECT * FROM supervisor_runs ORDER BY updated_at DESC")
        .fetch_all(&state.db)
        .await?;
    rows.into_iter().map(row_to_supervisor_run).collect()
}

pub async fn load_supervisor_run(state: &AppState, id: Uuid) -> Result<SupervisorRun> {
    let row = sqlx::query("SELECT * FROM supervisor_runs WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(&state.db)
        .await?;
    row_to_supervisor_run(row)
}

pub async fn load_supervisor_run_reconciled(state: &AppState, id: Uuid) -> Result<SupervisorRun> {
    let run = load_supervisor_run(state, id).await?;
    if matches!(run.status, SupervisorStatus::RunningChildren | SupervisorStatus::RunningIntegration | SupervisorStatus::Validating) {
        let _ = tick_supervisor_run(state, id).await?;
        load_supervisor_run(state, id).await
    } else {
        Ok(run)
    }
}

pub async fn list_supervisor_runs_reconciled(state: &AppState) -> Result<Vec<SupervisorRun>> {
    let runs = list_supervisor_runs(state).await?;
    for run in &runs {
        if matches!(run.status, SupervisorStatus::RunningChildren | SupervisorStatus::RunningIntegration | SupervisorStatus::Validating) {
            let _ = tick_supervisor_run(state, run.id).await;
        }
    }
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
        child_runs: Vec::new(),
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
        .unwrap_or_else(|| run.child_runs.is_empty() && run.integration_run_id.is_none() && run.final_patch_path.is_none())
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
    sqlx::query("DELETE FROM planner_features WHERE repo_id = ?")
        .bind(&repo_id)
        .execute(&state.db)
        .await?;
    for (index, item) in items.iter().enumerate() {
        sqlx::query("INSERT INTO planner_features (id, repo_id, title, status, sort_order, payload_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
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
            "child_runs": run.child_runs
        }))?)
        .execute(&state.db)
        .await?;
    Ok(())
}

async fn save_sprint_features(state: &AppState, run: &SupervisorRun, sprint_id: &str, completed_at: Option<&str>) -> Result<()> {
    save_repo_feature_plan_items(state, &run.root_repo_path, &run.feature_plan_items).await?;
    let now = Utc::now().to_rfc3339();
    sqlx::query("DELETE FROM sprint_features WHERE sprint_id = ?")
        .bind(sprint_id)
        .execute(&state.db)
        .await?;
    for (index, execution_item) in run.execution_plan_items.iter().enumerate() {
        let status = run
            .feature_plan_items
            .iter()
            .find(|item| item.id == execution_item.feature_plan_item_id)
            .map(|item| serde_json::to_value(&item.status).ok().and_then(|value| value.as_str().map(str::to_string)).unwrap_or_else(|| "planned".to_string()))
            .unwrap_or_else(|| "planned".to_string());
        sqlx::query("INSERT INTO sprint_features (id, sprint_id, feature_id, status, completed_at, sort_order, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(Uuid::new_v4().to_string())
            .bind(sprint_id)
            .bind(&execution_item.feature_plan_item_id)
            .bind(status)
            .bind(completed_at)
            .bind(index as i64)
            .bind(&now)
            .bind(&now)
            .execute(&state.db)
            .await?;
    }
    Ok(())
}

async fn append_sprint_event(state: &AppState, sprint_id: &str, event_type: &str, event_time: &str, feature_id: Option<&str>, message: &str, payload: Value) -> Result<()> {
    let sequence_no = sqlx::query("SELECT COALESCE(MAX(sequence_no), 0) + 1 AS next_sequence_no FROM sprint_events WHERE sprint_id = ?")
        .bind(sprint_id)
        .fetch_one(&state.db)
        .await?
        .get::<i64, _>("next_sequence_no");
    sqlx::query("INSERT INTO sprint_events (id, sprint_id, sequence_no, event_type, event_time, feature_id, actor, message, payload_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(Uuid::new_v4().to_string())
        .bind(sprint_id)
        .bind(sequence_no)
        .bind(event_type)
        .bind(event_time)
        .bind(feature_id)
        .bind("system")
        .bind(message)
        .bind(serde_json::to_string(&payload)?)
        .bind(Utc::now().to_rfc3339())
        .execute(&state.db)
        .await?;
    Ok(())
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

    let mut workflow_run_ids = run.child_runs
        .iter()
        .filter_map(|child| child.workflow_run_id)
        .collect::<Vec<_>>();
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

    run.child_runs.clear();
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
    run.child_runs.clear();
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

pub async fn start_supervisor_run(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
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
    let workspace = repo_snapshot::create_workspace(&run.root_repo_path, run.id, &scheduled_items)?;
    patches::create_baseline(&workspace.snapshot)?;
    patches::create_baseline(&workspace.integration)?;
    let workflow_template_id = context_uuid(&run.context, "workflow_template_id");
    let integration_template_id = match run.strategy {
        SupervisorExecutionStrategy::Parallel => context_uuid(&run.context, "integration_template_id"),
        SupervisorExecutionStrategy::Series => None,
    };

    let mut children = Vec::new();
    for item in &scheduled_items {
        let shard = repo_snapshot::shard_path(&workspace, &item.id);
        patches::create_baseline(&shard)?;
        let shard_path = shard.to_string_lossy().to_string();
        let template_id = run.execution_plan_items
            .iter()
            .find(|execution_item| execution_item.feature_plan_item_id == item.id)
            .and_then(|execution_item| execution_item.workflow_template_id)
            .or(workflow_template_id);
        let child_run_id = workflow_spawn::spawn_feature_plan_item_workflow(
            state,
            item,
            &shard_path,
            template_id,
            supervisor_context(&run, &workspace),
        ).await?;
        let should_start_child = matches!(run.strategy, SupervisorExecutionStrategy::Parallel) || children.is_empty();
        let child_status = if should_start_child {
            engine::start_run(state, child_run_id, None)
                .await?
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("waiting")
                .to_string()
        } else {
            "draft".to_string()
        };
        children.push(SupervisorChildRun {
            execution_item_id: item.id.clone(),
            title: item.title.clone(),
            shard_path,
            workflow_run_id: Some(child_run_id),
            status: child_status,
            patch_path: None,
        });
    }
    run.child_runs = children;
    run.status = SupervisorStatus::RunningChildren;

    if matches!(run.strategy, SupervisorExecutionStrategy::Parallel) {
        if let Some(template_id) = integration_template_id {
            if let Some(obj) = run.context.as_object_mut() {
                obj.insert("integration_template_id".to_string(), Value::String(template_id.to_string()));
            }
        }
    }

    run.snapshot_path = Some(workspace.snapshot.to_string_lossy().to_string());
    run.integration_path = Some(workspace.integration.to_string_lossy().to_string());
    save_repo_feature_plan_items(state, &run.root_repo_path, &run.feature_plan_items).await?;
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

fn supervisor_child_success(child: &SupervisorChildRun) -> bool {
    child.status == "success"
}

fn supervisor_child_terminal_failure(child: &SupervisorChildRun) -> bool {
    child.status == "error" || child.status == "cancelled"
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
        let workspace = repo_snapshot::workspace_for(&run.root_repo_path, run.id)?;

        if let Some(child) = run.child_runs.iter_mut().find(|child| child.workflow_run_id == Some(workflow_run_id)) {
            child.status = status_str(&status).to_string();
            if matches!(status, RunStatus::Success) && child.patch_path.is_none() {
                let patch_path = patches::patch_path(&workspace.patches, &child.execution_item_id);
                patches::generate_patch(&PathBuf::from(&child.shard_path), &patch_path)?;
                child.patch_path = Some(patch_path.to_string_lossy().to_string());
            }
        }

        if matches!(status, RunStatus::Success) {
            if let Some(feature_id) = feature_id.as_deref() {
                if let Some(item) = run.feature_plan_items.iter_mut().find(|item| item.id == feature_id) {
                    item.status = FeaturePlanItemStatus::Completed;
                }
                if let Some(sprint_id) = sprint_id.as_deref() {
                    sqlx::query("UPDATE sprint_features SET status = ?, completed_at = ?, updated_at = ? WHERE sprint_id = ? AND feature_id = ?")
                        .bind("completed")
                        .bind(&now)
                        .bind(&now)
                        .bind(sprint_id)
                        .bind(feature_id)
                        .execute(&state.db)
                        .await?;
                }
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

        if run.child_runs.iter().any(supervisor_child_terminal_failure) {
            run.status = SupervisorStatus::Failed;
        } else if !run.child_runs.is_empty() && run.child_runs.iter().all(supervisor_child_success) {
            run.status = SupervisorStatus::DevelopmentComplete;
            if let Some(sprint_id) = sprint_id.as_deref() {
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
                        "child_runs": run.child_runs
                    }),
                ).await?;
            }
        } else {
            start_next_series_child(state, &mut run).await?;
        }

        run.updated_at = Utc::now();
        update_supervisor_run(state, &run).await?;
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
    }

    Ok(())
}

pub async fn tick_supervisor_run(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    match run.status {
        SupervisorStatus::RunningChildren => tick_children(state, &mut run).await?,
        SupervisorStatus::RunningIntegration | SupervisorStatus::Validating => tick_integration(state, &mut run).await?,
        _ => {}
    }
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

pub async fn apply_supervisor_final_patch(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    if matches!(run.status, SupervisorStatus::RunningIntegration) {
        tick_integration(state, &mut run).await?;
    }
    if !matches!(run.status, SupervisorStatus::ReadyToApply) {
        return Err(anyhow!("integration workflow must complete successfully before applying sprint"));
    }
    let final_patch = run.final_patch_path.clone().filter(|value| !value.trim().is_empty()).ok_or_else(|| anyhow!("integration completed without a final patch"))?;
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
        "child_runs": run.child_runs
    });

    save_repo_feature_plan_items(state, &run.root_repo_path, &run.feature_plan_items).await?;
    upsert_sprint_record(state, &run, &sprint_id, &sprint_key, &sprint_title, "applied", None, Some(&completed_at_text)).await?;
    save_sprint_features(state, &run, &sprint_id, Some(&completed_at_text)).await?;
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
    run.child_runs.iter().filter_map(|child| {
        child.patch_path.as_ref().map(|patch_path| json!({
            "execution_item_id": child.execution_item_id,
            "title": child.title,
            "patch_path": patch_path,
            "workflow_run_id": child.workflow_run_id
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
            "strategy": run.strategy,
            "root_repo_path": run.root_repo_path,
            "snapshot_path": run.snapshot_path,
            "integration_path": integration_path,
            "live_worktree": false,
            "integration_source": "current_worktree_copy"
        }),
    ).await?;
    let _ = engine::start_run(state, integration_run_id, None).await?;
    run.integration_path = Some(integration_path);
    run.integration_run_id = Some(integration_run_id);
    run.final_patch_path = None;
    run.status = SupervisorStatus::RunningIntegration;
    Ok(())
}

pub async fn reopen_supervisor_development(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    run.integration_run_id = None;
    run.final_patch_path = None;
    run.merge_report = json!({});
    run.validation_report = json!({});
    run.status = SupervisorStatus::DevelopmentComplete;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

pub async fn restart_supervisor_integration_workflow(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    if run.child_runs.is_empty() {
        return Err(anyhow!("development has not produced any feature workflow runs"));
    }
    run.integration_run_id = None;
    run.final_patch_path = None;
    run.merge_report = json!({});
    run.validation_report = json!({});
    spawn_live_integration_workflow(state, &mut run).await?;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

pub async fn start_supervisor_integration_workflow(state: &AppState, id: Uuid) -> Result<Value> {
    let mut run = load_supervisor_run(state, id).await?;
    if !matches!(run.status, SupervisorStatus::DevelopmentComplete | SupervisorStatus::RunningIntegration | SupervisorStatus::ReadyToApply | SupervisorStatus::Failed) {
        return Err(anyhow!("development must complete before integration can start"));
    }
    if run.child_runs.is_empty() {
        return Err(anyhow!("development has not produced any feature workflow runs"));
    }
    spawn_live_integration_workflow(state, &mut run).await?;
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
}

fn supervisor_child_active(status: &str) -> bool {
    matches!(status, "queued" | "running" | "waiting" | "paused")
}

fn supervisor_child_pending(status: &str) -> bool {
    matches!(status, "draft" | "pending")
}

async fn start_next_series_child(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    if !matches!(run.strategy, SupervisorExecutionStrategy::Series) {
        return Ok(());
    }
    if run.child_runs.iter().any(|child| supervisor_child_active(child.status.as_str())) {
        return Ok(());
    }
    let Some(child) = run.child_runs.iter_mut().find(|child| supervisor_child_pending(child.status.as_str())) else {
        return Ok(());
    };
    let Some(child_run_id) = child.workflow_run_id else {
        return Ok(());
    };
    let start_result = Box::pin(engine::start_run(state, child_run_id, None)).await?;
    child.status = start_result
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("waiting")
        .to_string();
    Ok(())
}

async fn tick_children(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    let workspace = repo_snapshot::workspace_for(&run.root_repo_path, run.id)?;
    let mut all_done = true;
    for child in &mut run.child_runs {
        let Some(child_run_id) = child.workflow_run_id else {
            all_done = false;
            continue;
        };
        let child_run = crate::engine::load_run(state, child_run_id).await?;
        child.status = status_str(&child_run.status).to_string();
        match child_run.status {
            RunStatus::Success => {
                if child.patch_path.is_none() {
                    let patch_path = patches::patch_path(&workspace.patches, &child.execution_item_id);
                    patches::generate_patch(&PathBuf::from(&child.shard_path), &patch_path)?;
                    child.patch_path = Some(patch_path.to_string_lossy().to_string());
                }
            }
            RunStatus::Error | RunStatus::Cancelled => {
                run.status = SupervisorStatus::Failed;
                all_done = false;
            }
            _ => all_done = false,
        }
    }

    if all_done && !matches!(run.status, SupervisorStatus::Failed) {
        run.status = SupervisorStatus::DevelopmentComplete;
    } else if !matches!(run.status, SupervisorStatus::Failed) {
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
        .bind(serde_json::to_string(&run.child_runs)?)
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
        .bind(serde_json::to_string(&run.child_runs)?)
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

async fn update_status(state: &AppState, id: Uuid, status: SupervisorStatus) -> Result<()> {
    sqlx::query("UPDATE supervisor_runs SET status = ?, updated_at = ? WHERE id = ?")
        .bind(status_supervisor_str(&status))
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .execute(&state.db)
        .await?;
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
        child_runs: serde_json::from_str(row.get::<String, _>("child_runs_json").as_str())?,
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
