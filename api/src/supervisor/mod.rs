pub mod models;
pub mod patches;
pub mod repo_snapshot;
pub mod workflow_spawn;

use std::path::PathBuf;

use anyhow::{anyhow, Result};
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
use models::{CreateSupervisorRunRequest, SupervisorActionRequest, SupervisorChildRun, SupervisorExecutionStrategy, SupervisorRun, SupervisorStatus};

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
        if matches!(req.strategy, SupervisorExecutionStrategy::Parallel) {
            if let Some(template_id) = req.integration_template_id {
                obj.insert("integration_template_id".to_string(), Value::String(template_id.to_string()));
            }
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
        if matches!(run.strategy, SupervisorExecutionStrategy::Parallel) {
            if let Some(template_id) = payload.get("integration_template_id").and_then(Value::as_str).filter(|value| !value.is_empty()) {
                obj.insert("integration_template_id".to_string(), Value::String(template_id.to_string()));
            } else if payload.get("integration_template_id").is_some() {
                obj.remove("integration_template_id");
            }
        } else {
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
        if is_scheduled && !matches!(item.status, FeaturePlanItemStatus::Completed) {
            item.status = FeaturePlanItemStatus::Scheduled;
        } else if !is_scheduled && matches!(item.status, FeaturePlanItemStatus::Scheduled) {
            item.status = FeaturePlanItemStatus::Fine;
        }
    }
    run.feature_plan_items = planner_items;
    run.execution_plan_items = sprint_items;
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
    supervisor_run.updated_at = Utc::now();
    update_supervisor_run(state, &supervisor_run).await?;
    Ok(())
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
    let scheduled_items = scheduled_feature_plan_items(&run)?;
    let scheduled_feature_ids = scheduled_items.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
    for item in &mut run.feature_plan_items {
        if scheduled_feature_ids.iter().any(|id| id == &item.id) && !matches!(item.status, FeaturePlanItemStatus::Completed) {
            item.status = FeaturePlanItemStatus::Scheduled;
        }
    }
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
        let start_result = engine::start_run(state, child_run_id, None).await?;
        let child_status = start_result
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("waiting")
            .to_string();
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
    run.updated_at = Utc::now();
    update_supervisor_run(state, &run).await?;
    Ok(json!({ "ok": true, "supervisor_run": run }))
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
    let final_patch = run.final_patch_path.clone().ok_or_else(|| anyhow!("final patch is not available"))?;
    patches::apply_final_patch_to_root(&PathBuf::from(&run.root_repo_path), &PathBuf::from(final_patch))?;
    let completed_at = Utc::now();
    let completed_at_text = completed_at.to_rfc3339();
    let scheduled_feature_ids = run
        .execution_plan_items
        .iter()
        .map(|item| item.feature_plan_item_id.clone())
        .collect::<Vec<_>>();
    for item in &mut run.feature_plan_items {
        if scheduled_feature_ids.iter().any(|id| id == &item.id) {
            item.status = FeaturePlanItemStatus::Completed;
        }
    }
    let completed_features = run
        .feature_plan_items
        .iter()
        .filter(|item| scheduled_feature_ids.iter().any(|id| id == &item.id))
        .map(|item| json!({
            "id": item.id,
            "title": item.title,
            "completed_at": completed_at_text
        }))
        .collect::<Vec<_>>();
    if !run.context.is_object() {
        run.context = json!({});
    }
    if let Some(obj) = run.context.as_object_mut() {
        obj.insert("sprint_completed_at".to_string(), Value::String(completed_at_text.clone()));
        obj.insert("completed_features".to_string(), Value::Array(completed_features));
    }
    run.status = SupervisorStatus::Applied;
    run.updated_at = completed_at;
    update_supervisor_run(state, &run).await?;
    Ok(json!({ "ok": true, "status": "applied", "sprint_completed_at": completed_at_text }))
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
        let integration_path = run.integration_path.clone().ok_or_else(|| anyhow!("integration path missing"))?;
        let patch_paths = run.child_runs.iter().filter_map(|child| {
            child.patch_path.as_ref().map(|patch_path| json!({
                "execution_item_id": child.execution_item_id,
                "title": child.title,
                "patch_path": patch_path,
                "workflow_run_id": child.workflow_run_id
            }))
        }).collect::<Vec<_>>();
        let integration_run_id = workflow_spawn::spawn_integration_workflow(
            state,
            &format!("{} merge integration", run.title),
            &integration_path,
            patch_paths,
            context_uuid(&run.context, "integration_template_id"),
            json!({
                "supervisor_run_id": run.id,
                "strategy": run.strategy,
                "snapshot_path": run.snapshot_path,
                "integration_path": run.integration_path
            }),
        ).await?;
        run.integration_run_id = Some(integration_run_id);
        run.status = SupervisorStatus::RunningIntegration;
    }
    Ok(())
}

async fn tick_integration(state: &AppState, run: &mut SupervisorRun) -> Result<()> {
    let Some(integration_run_id) = run.integration_run_id else {
        return Ok(());
    };
    let integration_run = crate::engine::load_run(state, integration_run_id).await?;
    match integration_run.status {
        RunStatus::Success | RunStatus::Waiting | RunStatus::Paused => {
            if let Some(integration_path) = run.integration_path.clone() {
                let workspace = repo_snapshot::workspace_for(&run.root_repo_path, run.id)?;
                let final_patch = workspace.patches.join("final.patch");
                patches::generate_patch(&PathBuf::from(integration_path), &final_patch)?;
                run.final_patch_path = Some(final_patch.to_string_lossy().to_string());
                run.status = SupervisorStatus::ReadyToApply;
            }
        }
        RunStatus::Error | RunStatus::Cancelled => run.status = SupervisorStatus::Failed,
        _ => {}
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
