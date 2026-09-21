use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    db::{new_workflow_key, normalize_repo_ref},
    engine,
    engine::capabilities::planner::FeaturePlanItem,
    models::{RunStatus, WorkflowRun, WorkflowTemplateDefinition},
};

pub async fn spawn_feature_plan_item_workflow(
    state: &AppState,
    item: &FeaturePlanItem,
    shard_path: &str,
    template_id: Option<Uuid>,
    supervisor_context: Value,
) -> Result<Uuid> {
    spawn_feature_plan_item_workflow_with_definition(
        state,
        item,
        shard_path,
        template_id,
        None,
        supervisor_context,
    ).await
}

pub async fn spawn_feature_plan_item_workflow_with_definition(
    state: &AppState,
    item: &FeaturePlanItem,
    shard_path: &str,
    template_id: Option<Uuid>,
    fallback_definition: Option<WorkflowTemplateDefinition>,
    supervisor_context: Value,
) -> Result<Uuid> {
    let mut fallback_definition = fallback_definition;
    let definition = match template_id {
        Some(template_id) => match load_template_definition(state, template_id).await {
            Ok(definition) => definition,
            Err(err) => match fallback_definition.take() {
                Some(definition) => definition,
                None => return Err(err),
            },
        },
        None => match fallback_definition.take() {
            Some(definition) => definition,
            None => return Err(anyhow!("workflow_template_id is required for supervisor parallel runs")),
        },
    };
    let planner_id = supervisor_context
        .get("planner_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let feature_id = supervisor_context
        .get("feature_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let planner_repo_ref = supervisor_context
        .get("root_repo_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let mut context = json!({
        "supervisor": supervisor_context
    });

    if let (Some(planner_id), Some(feature_id)) = (planner_id, feature_id) {
        context["workflow_engine"]["global_state"]["capabilities"]["planner"]["planner_id"] = Value::String(planner_id);
        context["workflow_engine"]["global_state"]["capabilities"]["planner"]["feature_id"] = Value::String(feature_id);
        if let Some(repo_ref) = planner_repo_ref {
            context["workflow_engine"]["global_state"]["capabilities"]["planner"]["repo_ref"] = Value::String(repo_ref);
        }
    }

    insert_and_start_run(state, &item.title, shard_path, template_id, definition, context).await
}


async fn load_template_definition(state: &AppState, template_id: Uuid) -> Result<WorkflowTemplateDefinition> {
    let row = sqlx::query("SELECT definition_json FROM workflow_templates WHERE id = ?")
        .bind(template_id.to_string())
        .fetch_optional(&state.db)
        .await?;
    let Some(row) = row else {
        return Err(anyhow!("workflow template {} not found", template_id));
    };
    Ok(serde_json::from_str(row.get::<String, _>("definition_json").as_str())?)
}

fn seed_template_globals_into_context(context: &mut Value, definition: &WorkflowTemplateDefinition, repo_path: &str) -> Result<()> {
    let root = engine::ensure_engine_root(context);
    let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
    let runtime_global_state = global_state.clone();
    let mut seeded_global_state = serde_json::to_value(definition.globals.clone())?;

    if !seeded_global_state.is_object() {
        seeded_global_state = json!({});
    }

    engine::merge_json_values(&mut seeded_global_state, &runtime_global_state);
    *global_state = seeded_global_state;

    let global_obj = global_state.as_object_mut().ok_or_else(|| anyhow!("global_state must be object"))?;
    let resources = global_obj.entry("resources".to_string()).or_insert_with(|| json!({}));
    if !resources.is_object() {
        *resources = json!({});
    }
    let resources_obj = resources.as_object_mut().ok_or_else(|| anyhow!("resources must be object"))?;
    let repo = resources_obj.entry("repo".to_string()).or_insert_with(|| json!({}));
    if !repo.is_object() {
        *repo = json!({});
    }
    let repo_obj = repo.as_object_mut().ok_or_else(|| anyhow!("repo resource must be object"))?;
    repo_obj.insert("repo_ref".to_string(), json!(repo_path));
    repo_obj.insert("git_ref".to_string(), json!("WORKTREE"));
    Ok(())
}

async fn insert_and_start_run(
    state: &AppState,
    title: &str,
    repo_path: &str,
    template_id: Option<Uuid>,
    mut definition: WorkflowTemplateDefinition,
    mut context: Value,
) -> Result<Uuid> {
    let repo_path = normalize_repo_ref(repo_path);
    if repo_path.is_empty() {
        return Err(anyhow!("repo path is required"));
    }
    let id = Uuid::new_v4();
    let key = new_workflow_key(&repo_path);
    let now = Utc::now();
    crate::routes::normalize_shared_dependencies(&mut definition.globals);
    let requested_start_step_id = context
        .get("supervisor")
        .and_then(|value| value.get("workflow_start_step_id"))
        .and_then(Value::as_str)
        .or_else(|| context.get("workflow_start_step_id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let current_step_id = requested_start_step_id
        .and_then(|step_id| definition.steps.iter().find(|step| step.id == step_id).map(|step| step.id.clone()))
        .or_else(|| definition.steps.first().map(|step| step.id.clone()));
    if let Some(obj) = definition.globals.resources.as_object_mut() {
        let repo = obj.entry("repo").or_insert_with(|| json!({}));
        if let Some(repo_obj) = repo.as_object_mut() {
            repo_obj.insert("repo_ref".to_string(), Value::String(repo_path.clone()));
            repo_obj.insert("git_ref".to_string(), Value::String("WORKTREE".to_string()));
        }
    }
    seed_template_globals_into_context(&mut context, &definition, &repo_path)?;
    if let Some(context_obj) = context.as_object_mut() {
        context_obj.remove("workflow_input");
    }

    let initial_step = current_step_id
        .as_deref()
        .and_then(|step_id| definition.steps.iter().find(|step| step.id == step_id));
    let mut seeded_run = WorkflowRun {
        id,
        template_id,
        definition: definition.clone(),
        status: RunStatus::Draft,
        current_step_id: current_step_id.clone(),
        title: title.to_string(),
        repo_ref: repo_path.clone(),
        workflow_key: key.clone(),
        context,
        created_at: now,
        updated_at: now,
    };
    if let Some(step) = initial_step {
        let decisions = engine::automation::before_stage(state, id, &mut seeded_run, step).await?;
        engine::automation::apply_context_mutations(&mut seeded_run, &decisions, Some(step.id.as_str()), None)?;
        engine::refresh_inference_arm_state(&mut seeded_run, Some(step));
    }
    context = seeded_run.context;

    sqlx::query("INSERT INTO workflow_runs (id, template_id, definition_json, status, current_step_id, title, repo_ref, workflow_key, context_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(id.to_string())
        .bind(template_id.map(|value| value.to_string()))
        .bind(serde_json::to_string(&definition)?)
        .bind("draft")
        .bind(current_step_id)
        .bind(title)
        .bind(&repo_path)
        .bind(key)
        .bind(serde_json::to_string(&context)?)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&state.db)
        .await?;
    Ok(id)
}


