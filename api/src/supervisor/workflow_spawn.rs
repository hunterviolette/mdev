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
    models::{AutomationMode, RunStatus, WorkflowGlobalConfig, WorkflowRun, WorkflowStepDefinition, WorkflowStepExecutionConfig, WorkflowStepPromptConfig, WorkflowStepAdvancementConfig, WorkflowTemplateDefinition},
};

pub async fn spawn_series_workflow_on_integration(
    state: &AppState,
    title: &str,
    integration_path: &str,
    items: &[FeaturePlanItem],
    template_id: Option<Uuid>,
    supervisor_context: Value,
) -> Result<Uuid> {
    let definition = match template_id {
        Some(template_id) => load_template_definition(state, template_id).await?,
        None => return Err(anyhow!("workflow_template_id is required for supervisor series runs")),
    };
    let planner_id = supervisor_context
        .get("planner_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let selected_feature_id = items.first().map(|item| item.id.clone());
    let planner = match (planner_id, selected_feature_id) {
        (Some(planner_id), Some(feature_id)) => json!({
            "fragment_armed": true,
            "schema_armed": false,
            "auto_apply_armed": false,
            "planner_id": planner_id,
            "feature_id": feature_id
        }),
        _ => json!({
            "fragment_armed": false,
            "schema_armed": false,
            "auto_apply_armed": false,
            "planner_id": "",
            "feature_id": ""
        }),
    };

    insert_and_start_run(state, title, integration_path, template_id, definition, json!({
        "supervisor": supervisor_context,
        "input_source": "feature_plan_items",
        "workflow_engine": {
            "global_state": {
                "capabilities": {
                    "planner": planner
                }
            }
        }
    })).await
}

fn is_sprint_feature_context(supervisor_context: &Value) -> bool {
    supervisor_context
        .get("input_source")
        .and_then(Value::as_str)
        == Some("supervisor_sprint_feature")
}

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
    let input_source = supervisor_context
        .get("input_source")
        .and_then(Value::as_str);
    let is_supervisor_sprint_feature = is_sprint_feature_context(&supervisor_context);
    let is_supervisor_manual_shard = input_source == Some("supervisor_manual_shard");
    let is_planner_refinement = input_source == Some("supervisor_planner_feature");
    let explicit_planner_id = supervisor_context
        .get("planner_id")
        .or_else(|| supervisor_context.get("selected_planner_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let planner_id = if explicit_planner_id.is_some() || is_supervisor_manual_shard {
        explicit_planner_id
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT planner_id FROM planner_features WHERE id = ? AND TRIM(COALESCE(planner_id, '')) != '' AND COALESCE(status, '') != 'deleted' LIMIT 1",
        )
        .bind(&item.id)
        .fetch_optional(&state.db)
        .await?
    };

    if is_planner_refinement && planner_id.is_none() {
        return Err(anyhow!("planner_id is required for planner refinement workflows"));
    }

    let mut supervisor_context = supervisor_context;
    if let Some(supervisor_obj) = supervisor_context.as_object_mut() {
        if is_supervisor_manual_shard {
            supervisor_obj.insert("manual_shard_id".to_string(), Value::String(item.id.clone()));
            supervisor_obj.remove("feature_id");
            supervisor_obj.remove("planner_id");
        } else {
            supervisor_obj.insert("feature_id".to_string(), Value::String(item.id.clone()));
            if let Some(planner_id) = planner_id.as_ref() {
                supervisor_obj.insert("planner_id".to_string(), Value::String(planner_id.clone()));
            }
        }
    }

    let planner = if is_supervisor_manual_shard {
        json!({
            "planner_id": "",
            "feature_id": "",
            "fragment_armed": false,
            "schema_armed": false,
            "auto_apply_armed": false
        })
    } else if let Some(planner_id) = planner_id {
        json!({
            "planner_id": planner_id,
            "feature_id": item.id,
            "fragment_armed": true,
            "schema_armed": is_planner_refinement,
            "auto_apply_armed": is_planner_refinement
        })
    } else {
        json!({
            "planner_id": "",
            "feature_id": "",
            "fragment_armed": false,
            "schema_armed": false,
            "auto_apply_armed": false
        })
    };

    let context = json!({
        "supervisor": supervisor_context,
        "workflow_engine": {
            "run_state": {
                "pause_requested": false
            },
            "global_state": {
                "capabilities": {
                    "planner": planner,
                    "context_export": {
                        "enabled": is_supervisor_sprint_feature
                    }
                }
            }
        }
    });

    insert_and_start_run(state, &item.title, shard_path, template_id, definition, context).await
}

pub async fn spawn_integration_workflow(
    state: &AppState,
    title: &str,
    integration_path: &str,
    _patch_paths: Vec<Value>,
    template_id: Option<Uuid>,
    supervisor_context: Value,
) -> Result<Uuid> {
    let mut definition = match template_id {
        Some(template_id) => load_template_definition(state, template_id).await?,
        None => integration_definition(),
    };

    for step in definition.steps.iter_mut() {
        if step.step_type == "merge_patches" {
            if !step.config.is_object() {
                step.config = json!({});
            }
            if let Some(obj) = step.config.as_object_mut() {
                obj.remove("patches");
                obj.remove("supervisor_run_id");
            }
        }
    }

    insert_and_start_run(state, title, integration_path, template_id, definition, json!({
        "supervisor": {
            "supervisor_id": supervisor_context
                .get("supervisor_id")
                .or_else(|| supervisor_context.get("supervisor_run_id"))
                .cloned()
                .unwrap_or(Value::Null),
            "pool_type": supervisor_context.get("pool_type").cloned().unwrap_or_else(|| Value::String("integration".to_string()))
        }
    })).await
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

fn seed_workflow_input_into_start_step(context: &mut Value, step_id: Option<&str>, include_workflow_input: bool) -> Result<()> {
    let Some(step_id) = step_id else {
        return Ok(());
    };

    let root = engine::ensure_engine_root(context);
    let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
    if !stage_state.is_object() {
        *stage_state = json!({});
    }
    let stage_state_obj = stage_state.as_object_mut().ok_or_else(|| anyhow!("stage_state must be object"))?;
    let stage = stage_state_obj.entry(step_id.to_string()).or_insert_with(|| json!({}));
    if !stage.is_object() {
        *stage = json!({});
    }
    let stage_obj = stage.as_object_mut().ok_or_else(|| anyhow!("stage state must be object"))?;
    let prompt = stage_obj.entry("prompt".to_string()).or_insert_with(|| json!({}));
    if !prompt.is_object() {
        *prompt = json!({});
    }
    let prompt_obj = prompt.as_object_mut().ok_or_else(|| anyhow!("stage prompt must be object"))?;
    prompt_obj.insert("include_workflow_input".to_string(), Value::Bool(include_workflow_input));
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

fn integration_definition() -> WorkflowTemplateDefinition {
    WorkflowTemplateDefinition {
        version: 1,
        globals: WorkflowGlobalConfig::default(),
        governance: json!({}),
        steps: vec![
            WorkflowStepDefinition {
                id: "merge_patches".to_string(),
                name: "Merge patches".to_string(),
                step_type: "merge_patches".to_string(),
                automation_mode: AutomationMode::Automatic,
                execution: WorkflowStepExecutionConfig::default(),
                prompt: WorkflowStepPromptConfig {
                    include_repo_context: false,
                    include_changeset_schema: false,
                    include_user_context: true,
                },
                config: json!({}),
                capabilities: Vec::new(),
                execution_logic: json!({
                    "kind": "merge_patches_stage_policy",
                    "automation": {
                        "apply_patches": true
                    }
                }),
                execution_plan: Vec::new(),
                transitions: Vec::new(),
                advancement: WorkflowStepAdvancementConfig {
                    mode: Some("automatic".to_string()),
                    auto_run_on_enter: true,
                    auto_advance_on_success: true,
                    auto_advance_on_error: false,
                    auto_advance_on_paused: false,
                },
            },
            WorkflowStepDefinition {
                id: "review".to_string(),
                name: "Review".to_string(),
                step_type: "review".to_string(),
                automation_mode: AutomationMode::Manual,
                execution: WorkflowStepExecutionConfig::default(),
                prompt: WorkflowStepPromptConfig {
                    include_repo_context: false,
                    include_changeset_schema: false,
                    include_user_context: true,
                },
                config: json!({}),
                capabilities: Vec::new(),
                execution_logic: json!({
                    "kind": "review_stage_policy",
                    "require_manual_approval": true,
                    "ai_review": {
                        "enabled": false
                    },
                    "automation": {
                        "disposition_review": {
                            "enabled": true,
                            "available_dispositions": ["move_next", "pause"]
                        }
                    }
                }),
                execution_plan: Vec::new(),
                transitions: Vec::new(),
                advancement: WorkflowStepAdvancementConfig {
                    mode: Some("manual".to_string()),
                    auto_run_on_enter: false,
                    auto_advance_on_success: false,
                    auto_advance_on_error: false,
                    auto_advance_on_paused: false,
                },
            }
        ],
    }
}
