use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    db::new_workflow_key,
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
    let selected_feature_id = items.first().map(|item| item.id.clone());
    let planner = selected_feature_id
        .as_ref()
        .map(|id| {
            let selected_feature = items
                .iter()
                .find(|item| item.id == *id)
                .or_else(|| items.first());
            let mut planner = json!({
                "fragment_armed": true,
                "schema_armed": false,
                "auto_apply_armed": false,
                "selected_feature_id": id,
                "supervisor_run_id": supervisor_context.get("supervisor_run_id").cloned().unwrap_or(Value::Null),
                "schema_id": "supervisor_feature_plan_item_v1",
                "preserve_rough_definition": true
            });
            if let Some(selected_feature) = selected_feature {
                if let Some(obj) = planner.as_object_mut() {
                    obj.insert("selected_feature".to_string(), serde_json::to_value(selected_feature).unwrap_or_else(|_| json!({})));
                }
            }
            planner
        })
        .unwrap_or_else(|| json!({
            "fragment_armed": false,
            "schema_armed": false,
            "auto_apply_armed": false,
            "schema_id": "supervisor_feature_plan_item_v1",
            "preserve_rough_definition": true
        }));

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

fn template_structured_output(definition: &WorkflowTemplateDefinition) -> Value {
    definition
        .steps
        .iter()
        .find_map(|step| step.execution_logic.get("structured_output"))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

fn apply_supervisor_planner_template_controls(supervisor_context: &mut Value, definition: &WorkflowTemplateDefinition) {
    let template_structured_output = template_structured_output(definition);
    let context_structured_output = supervisor_context
        .get("structured_output")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let mut structured_output = template_structured_output;
    if let (Some(target), Some(source)) = (structured_output.as_object_mut(), context_structured_output.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    } else if !context_structured_output.is_null() {
        structured_output = context_structured_output;
    }

    if let Some(supervisor_obj) = supervisor_context.as_object_mut() {
        supervisor_obj.insert("structured_output".to_string(), structured_output);
    }
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
    let definition = match template_id {
        Some(template_id) => load_template_definition(state, template_id).await?,
        None => return Err(anyhow!("workflow_template_id is required for supervisor parallel runs")),
    };
    let is_supervisor_planner_feature = supervisor_context
        .get("input_source")
        .and_then(Value::as_str)
        == Some("supervisor_planner_feature");
    let is_supervisor_sprint_feature = is_sprint_feature_context(&supervisor_context);

    let mut supervisor_context = supervisor_context;
    if is_supervisor_planner_feature {
        apply_supervisor_planner_template_controls(&mut supervisor_context, &definition);
    }
    if let Some(supervisor_obj) = supervisor_context.as_object_mut() {
        supervisor_obj.insert("feature_id".to_string(), Value::String(item.id.clone()));
    }

    let structured_output = supervisor_context
        .get("structured_output")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let planner = json!({
        "fragment_armed": structured_output
            .get("fragment_armed")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        "schema_armed": structured_output
            .get("schema_armed")
            .or_else(|| structured_output.get("fine_feature_format_armed"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "auto_apply_armed": structured_output
            .get("auto_apply_armed")
            .or_else(|| structured_output.get("auto_normalize_and_apply_to_planner"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "selected_feature_id": item.id,
        "selected_feature": item,
        "supervisor_run_id": supervisor_context.get("supervisor_run_id").cloned().unwrap_or(Value::Null),
        "schema_id": structured_output
            .get("schema_id")
            .and_then(Value::as_str)
            .unwrap_or("supervisor_feature_plan_item_v1"),
        "preserve_rough_definition": structured_output
            .get("preserve_rough_definition")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    });

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
    patch_paths: Vec<Value>,
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
                obj.insert("patches".to_string(), Value::Array(patch_paths.clone()));
                obj.insert("supervisor_run_id".to_string(), supervisor_context.get("supervisor_run_id").cloned().unwrap_or(Value::Null));
            }
        }
    }

    insert_and_start_run(state, title, integration_path, template_id, definition, json!({
        "supervisor": supervisor_context,
        "workflow_engine": {
            "global_state": {
                "supervisor": {
                    "patches": patch_paths,
                    "supervisor_run_id": supervisor_context.get("supervisor_run_id").cloned().unwrap_or(Value::Null)
                }
            }
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

fn feature_plan_items_prompt_fragment(items: &[FeaturePlanItem]) -> String {
    items.iter().map(feature_plan_item_prompt_fragment).collect::<Vec<_>>().join("\n\n---\n\n")
}

fn feature_plan_item_prompt_fragment(item: &FeaturePlanItem) -> String {
    serde_json::to_string_pretty(item).unwrap_or_else(|_| format!("{}\n\n{}", item.title, item.summary))
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
    let id = Uuid::new_v4();
    let key = new_workflow_key(repo_path);
    let now = Utc::now();
    let requested_start_step_id = context
        .get("supervisor")
        .and_then(|value| value.get("workflow_start_step_id"))
        .and_then(Value::as_str)
        .or_else(|| context.get("workflow_start_step_id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let is_supervisor_sprint_feature = context
        .get("supervisor")
        .map(is_sprint_feature_context)
        .unwrap_or(false);
    let current_step_id = requested_start_step_id
        .and_then(|step_id| definition.steps.iter().find(|step| step.id == step_id).map(|step| step.id.clone()))
        .or_else(|| definition.steps.first().map(|step| step.id.clone()));
    if let Some(obj) = definition.globals.resources.as_object_mut() {
        let repo = obj.entry("repo").or_insert_with(|| json!({}));
        if let Some(repo_obj) = repo.as_object_mut() {
            repo_obj.insert("repo_ref".to_string(), Value::String(repo_path.to_string()));
            repo_obj.insert("git_ref".to_string(), Value::String("WORKTREE".to_string()));
        }
    }
    seed_template_globals_into_context(&mut context, &definition, repo_path)?;
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
        repo_ref: repo_path.to_string(),
        workflow_key: key.clone(),
        context,
        created_at: now,
        updated_at: now,
    };
    if let Some(step) = initial_step {
        let decisions = engine::governance::before_stage(state, id, &mut seeded_run, step).await?;
        engine::governance::apply_context_mutations(&mut seeded_run, &decisions, Some(step.id.as_str()), None)?;
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
        .bind(repo_path)
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
                    },
                    "on_success": {
                        "disposition": "move_next",
                        "message": "Patches merged successfully."
                    },
                    "on_error": {
                        "disposition": "stay",
                        "message": "Patch merge failed."
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
