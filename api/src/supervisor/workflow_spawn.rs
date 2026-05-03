use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    db::new_workflow_key,
    engine,
    models::{AutomationMode, WorkflowGlobalConfig, WorkflowStepDefinition, WorkflowStepExecutionConfig, WorkflowStepPromptConfig, WorkflowStepAdvancementConfig, WorkflowTemplateDefinition},
    supervisor::models::FeaturePlanItem,
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
    insert_and_start_run(state, title, integration_path, template_id, definition, json!({
        "supervisor": supervisor_context,
        "input_source": "feature_plan_items",
        "feature_plan_items": items,
        "workflow_input": render_feature_plan_items(items)
    })).await
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
    insert_and_start_run(state, &item.title, shard_path, template_id, definition, json!({
        "supervisor": supervisor_context,
        "input_source": "feature_plan_item",
        "feature_plan_item": item,
        "workflow_input": render_feature_plan_item(item)
    })).await
}

pub async fn spawn_integration_workflow(
    state: &AppState,
    title: &str,
    integration_path: &str,
    patch_paths: Vec<Value>,
    template_id: Option<Uuid>,
    supervisor_context: Value,
) -> Result<Uuid> {
    let definition = match template_id {
        Some(template_id) => load_template_definition(state, template_id).await?,
        None => integration_definition(),
    };
    insert_and_start_run(state, title, integration_path, template_id, definition, json!({
        "supervisor": supervisor_context,
        "patches": patch_paths
    })).await
}

fn render_feature_plan_items(items: &[FeaturePlanItem]) -> String {
    items.iter().map(render_feature_plan_item).collect::<Vec<_>>().join("\n\n---\n\n")
}

fn render_feature_plan_item(item: &FeaturePlanItem) -> String {
    let mut out = Vec::new();
    out.push(format!("Feature: {}", item.title));
    if !item.summary.trim().is_empty() {
        out.push(format!("Summary:\n{}", item.summary.trim()));
    }
    push_list(&mut out, "Requirements", &item.requirements);
    push_list(&mut out, "Acceptance criteria", &item.acceptance_criteria);
    push_list(&mut out, "Implementation notes", &item.implementation_notes);
    push_list(&mut out, "Review expectations", &item.review_expectations);
    push_list(&mut out, "Target files or areas", &item.target_files_or_areas);
    push_list(&mut out, "Dependencies", &item.dependencies);
    out.join("\n\n")
}

fn push_list(out: &mut Vec<String>, label: &str, values: &[String]) {
    let values = values.iter().map(|value| value.trim()).filter(|value| !value.is_empty()).collect::<Vec<_>>();
    if values.is_empty() {
        return;
    }
    out.push(format!("{}:\n{}", label, values.iter().map(|value| format!("- {}", value)).collect::<Vec<_>>().join("\n")));
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
    let current_step_id = definition.steps.first().map(|step| step.id.clone());
    if let Some(obj) = definition.globals.resources.as_object_mut() {
        let repo = obj.entry("repo").or_insert_with(|| json!({}));
        if let Some(repo_obj) = repo.as_object_mut() {
            repo_obj.insert("repo_ref".to_string(), Value::String(repo_path.to_string()));
            repo_obj.insert("git_ref".to_string(), Value::String("WORKTREE".to_string()));
        }
    }
    if let Some(workflow_input) = context
        .get("workflow_input")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
    {
        let context_obj = context.as_object_mut().ok_or_else(|| anyhow!("workflow context must be an object"))?;
        let workflow_engine = context_obj.entry("workflow_engine".to_string()).or_insert_with(|| json!({}));
        let workflow_engine_obj = workflow_engine.as_object_mut().ok_or_else(|| anyhow!("workflow_engine context must be an object"))?;
        let global_state = workflow_engine_obj.entry("global_state".to_string()).or_insert_with(|| json!({}));
        let global_state_obj = global_state.as_object_mut().ok_or_else(|| anyhow!("workflow_engine.global_state must be an object"))?;
        let capabilities = global_state_obj.entry("capabilities".to_string()).or_insert_with(|| json!({}));
        let capabilities_obj = capabilities.as_object_mut().ok_or_else(|| anyhow!("workflow_engine.global_state.capabilities must be an object"))?;
        let inference = capabilities_obj.entry("inference".to_string()).or_insert_with(|| json!({}));
        let inference_obj = inference.as_object_mut().ok_or_else(|| anyhow!("workflow_engine.global_state.capabilities.inference must be an object"))?;
        {
            let prompt_fragments = inference_obj.entry("prompt_fragments".to_string()).or_insert_with(|| json!({}));
            let prompt_fragments_obj = prompt_fragments.as_object_mut().ok_or_else(|| anyhow!("inference.prompt_fragments must be an object"))?;
            prompt_fragments_obj.insert("user_input".to_string(), Value::String(workflow_input.clone()));
            prompt_fragments_obj.insert("planner_schema".to_string(), Value::String(String::new()));
        }
        {
            let prompt_fragment_enabled = inference_obj.entry("prompt_fragment_enabled".to_string()).or_insert_with(|| json!({}));
            let prompt_fragment_enabled_obj = prompt_fragment_enabled.as_object_mut().ok_or_else(|| anyhow!("inference.prompt_fragment_enabled must be an object"))?;
            prompt_fragment_enabled_obj.insert("user_input".to_string(), Value::Bool(true));
            prompt_fragment_enabled_obj.insert("planner_schema".to_string(), Value::Bool(false));
        }
    }
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
        steps: vec![WorkflowStepDefinition {
            id: "merge".to_string(),
            name: "Merge patches".to_string(),
            step_type: "code".to_string(),
            automation_mode: AutomationMode::Automatic,
            execution: WorkflowStepExecutionConfig::default(),
            prompt: WorkflowStepPromptConfig {
                include_repo_context: true,
                include_changeset_schema: true,
                include_user_context: true,
            },
            config: json!({}),
            capabilities: Vec::new(),
            execution_logic: json!({}),
            execution_plan: Vec::new(),
            transitions: Vec::new(),
            advancement: WorkflowStepAdvancementConfig {
                mode: Some("auto".to_string()),
                auto_run_on_enter: true,
                auto_advance_on_success: false,
                auto_advance_on_error: false,
                auto_advance_on_paused: false,
            },
        }],
    }
}
