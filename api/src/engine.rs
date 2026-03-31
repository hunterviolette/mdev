pub(crate) mod capabilities;
mod runtime;
mod stages;
mod transitions;

use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    executor::{append_event, update_run_context, update_run_status},
    models::{RunStatus, WorkflowRun, WorkflowStepDefinition, WorkflowTemplateDefinition},
};

pub use runtime::{pause_run, resume_run, run_step, start_run};
pub use transitions::{next_step_id, previous_step_id};

pub async fn load_run(state: &AppState, run_id: Uuid) -> Result<WorkflowRun> {
    let row = sqlx::query(
        "SELECT id, template_id, status, current_step_id, title, repo_ref, context_json, created_at, updated_at FROM workflow_runs WHERE id = ?"
    )
    .bind(run_id.to_string())
    .fetch_one(&state.db)
    .await?;

    Ok(WorkflowRun {
        id: Uuid::parse_str(row.get::<String, _>("id").as_str())?,
        template_id: row.get::<Option<String>, _>("template_id").map(|v| Uuid::parse_str(v.as_str())).transpose()?,
        status: match row.get::<String, _>("status").as_str() {
            "draft" => RunStatus::Draft,
            "queued" => RunStatus::Queued,
            "running" => RunStatus::Running,
            "waiting" => RunStatus::Waiting,
            "paused" => RunStatus::Paused,
            "success" => RunStatus::Success,
            "cancelled" => RunStatus::Cancelled,
            _ => RunStatus::Error,
        },
        current_step_id: row.get("current_step_id"),
        title: row.get("title"),
        repo_ref: row.get("repo_ref"),
        context: serde_json::from_str(row.get::<String, _>("context_json").as_str())?,
        created_at: chrono::DateTime::parse_from_rfc3339(row.get::<String, _>("created_at").as_str())?.with_timezone(&chrono::Utc),
        updated_at: chrono::DateTime::parse_from_rfc3339(row.get::<String, _>("updated_at").as_str())?.with_timezone(&chrono::Utc),
    })
}

pub async fn load_template_definition(state: &AppState, run: &WorkflowRun) -> Result<Option<WorkflowTemplateDefinition>> {
    let Some(template_id) = run.template_id else {
        return Ok(None);
    };

    let row = sqlx::query("SELECT definition_json FROM workflow_templates WHERE id = ?")
        .bind(template_id.to_string())
        .fetch_optional(&state.db)
        .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let definition = serde_json::from_str::<WorkflowTemplateDefinition>(row.get::<String, _>("definition_json").as_str())?;
    Ok(Some(definition))
}

pub async fn select_step(state: &AppState, run_id: Uuid, step_id: &str) -> Result<Value> {
    update_run_status(&state.db, run_id, RunStatus::Paused, Some(step_id)).await?;
    Ok(json!({ "ok": true, "current_step_id": step_id }))
}

pub async fn patch_stage_state(state: &AppState, run_id: Uuid, step_id: &str, payload: Value) -> Result<Value> {
    let mut run = load_run(state, run_id).await?;
    let root = ensure_engine_root(&mut run.context);

    let mut stage_payload = match payload {
        Value::Object(map) => map,
        _ => return Err(anyhow!("stage payload must be object")),
    };

    if let Some(repo_context) = stage_payload.remove("repo_context") {
        let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
        let global_state_obj = global_state.as_object_mut().ok_or_else(|| anyhow!("global_state must be object"))?;
        global_state_obj.insert("repo_context".to_string(), repo_context);
    }

    let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
    let stage_state_obj = stage_state.as_object_mut().ok_or_else(|| anyhow!("stage_state must be object"))?;
    stage_state_obj.insert(step_id.to_string(), Value::Object(stage_payload.clone()));
    update_run_context(&state.db, run_id, &run.context).await?;
    Ok(json!({ "ok": true, "step_id": step_id, "state": Value::Object(stage_payload) }))
}

pub(crate) async fn append_engine_event(
    state: &AppState,
    run_id: Uuid,
    step_id: Option<&str>,
    level: &str,
    kind: &str,
    message: &str,
    payload: Value,
) -> Result<()> {
    append_event(&state.db, run_id, step_id, level, kind, message, payload).await?;
    Ok(())
}

pub(crate) fn event_meta(
    stage_execution_id: Option<&str>,
    capability_invocation_id: Option<&str>,
    parent_invocation_id: Option<&str>,
    is_header_event: bool,
) -> Value {
    json!({
        "stage_execution_id": stage_execution_id,
        "capability_invocation_id": capability_invocation_id,
        "parent_invocation_id": parent_invocation_id,
        "is_header_event": is_header_event,
    })
}

pub(crate) async fn set_run_status(
    state: &AppState,
    run_id: Uuid,
    status: RunStatus,
    current_step_id: Option<&str>,
) -> Result<()> {
    update_run_status(&state.db, run_id, status, current_step_id).await?;
    Ok(())
}

pub(crate) async fn persist_context(state: &AppState, run_id: Uuid, context: &Value) -> Result<()> {
    update_run_context(&state.db, run_id, context).await?;
    Ok(())
}

pub(crate) fn ensure_engine_root(context: &mut Value) -> &mut Map<String, Value> {
    let root = context.as_object_mut().expect("run context must be object");
    if !root.contains_key("workflow_engine") {
        root.insert("workflow_engine".to_string(), json!({}));
    }
    root.get_mut("workflow_engine")
        .and_then(Value::as_object_mut)
        .expect("workflow_engine must be object")
}

pub(crate) fn current_step<'a>(definition: &'a WorkflowTemplateDefinition, run: &WorkflowRun, requested_step_id: Option<&str>) -> Result<&'a WorkflowStepDefinition> {
    let step_id = requested_step_id
        .map(|s| s.to_string())
        .or_else(|| run.current_step_id.clone())
        .or_else(|| definition.steps.first().map(|s| s.id.clone()))
        .ok_or_else(|| anyhow!("template has no steps"))?;

    definition.steps.iter().find(|s| s.id == step_id).ok_or_else(|| anyhow!("unknown step_id {}", step_id))
}
