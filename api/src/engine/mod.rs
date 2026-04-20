pub(crate) mod capabilities;
pub(crate) mod governance;
mod runtime;
mod stages;
mod transitions;

use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Map, Value};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    models::{RunStatus, WorkflowEventStreamItem, WorkflowRun, WorkflowStepDefinition, WorkflowTemplateDefinition},
};

pub use runtime::{pause_run, resume_run, run_step, start_run};
pub use transitions::{next_step_id, previous_step_id};

pub async fn load_run(state: &AppState, run_id: Uuid) -> Result<WorkflowRun> {
    let row = sqlx::query(
        "SELECT id, template_id, definition_json, status, current_step_id, title, repo_ref, context_json, created_at, updated_at FROM workflow_runs WHERE id = ?"
    )
    .bind(run_id.to_string())
    .fetch_one(&state.db)
    .await?;

    let mut run = WorkflowRun {
        id: Uuid::parse_str(row.get::<String, _>("id").as_str())?,
        template_id: row.get::<Option<String>, _>("template_id").map(|v| Uuid::parse_str(v.as_str())).transpose()?,
        definition: serde_json::from_str(row.get::<String, _>("definition_json").as_str())?,
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
    };

    if rearm_session_scoped_behavior_on_load(state, &mut run).await? {
        run.updated_at = Utc::now();
    }

    Ok(run)
}

pub async fn load_template_definition(_state: &AppState, run: &WorkflowRun) -> Result<Option<WorkflowTemplateDefinition>> {
    Ok(Some(run.definition.clone()))
}

async fn rearm_session_scoped_behavior_on_load(state: &AppState, run: &mut WorkflowRun) -> Result<bool> {
    let root = ensure_engine_root(&mut run.context);
    let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
    let global_state_obj = global_state
        .as_object_mut()
        .ok_or_else(|| anyhow!("global_state must be object"))?;
    let capabilities = global_state_obj
        .entry("capabilities".to_string())
        .or_insert_with(|| json!({}));
    let capabilities_obj = capabilities
        .as_object_mut()
        .ok_or_else(|| anyhow!("capabilities must be object"))?;
    let inference = capabilities_obj
        .entry("inference".to_string())
        .or_insert_with(|| json!({}));
    let inference_obj = inference
        .as_object_mut()
        .ok_or_else(|| anyhow!("inference must be object"))?;
    let connection_runtime = inference_obj
        .entry("connection_runtime".to_string())
        .or_insert_with(|| json!({}));
    let connection_runtime_obj = connection_runtime
        .as_object_mut()
        .ok_or_else(|| anyhow!("connection_runtime must be object"))?;

    let current_process_session_id = state.process_session_id().to_string();
    let persisted_process_session_id = connection_runtime_obj
        .get("process_session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if persisted_process_session_id == current_process_session_id {
        return Ok(false);
    }

    let previous_connection_runtime = connection_runtime_obj.clone();
    let had_prior_session_state = !previous_connection_runtime.is_empty();
    let mut changed = had_prior_session_state;

    connection_runtime_obj.clear();
    connection_runtime_obj.insert(
        "process_session_id".to_string(),
        Value::String(current_process_session_id),
    );

    if inference_obj.remove("next_prompt_fragments").is_some() {
        changed = true;
    }
    if inference_obj.remove("active_prompt_fragments").is_some() {
        changed = true;
    }

    if had_prior_session_state {
        let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
        let stage_state_obj = stage_state
            .as_object_mut()
            .ok_or_else(|| anyhow!("stage_state must be object"))?;

        for stage_value in stage_state_obj.values_mut() {
            let stage_obj = match stage_value.as_object_mut() {
                Some(obj) => obj,
                None => continue,
            };
            let execution_logic = stage_obj
                .entry("execution_logic".to_string())
                .or_insert_with(|| json!({}));
            let execution_logic_obj = execution_logic
                .as_object_mut()
                .ok_or_else(|| anyhow!("execution_logic must be object"))?;
            let connections = execution_logic_obj
                .entry("connections".to_string())
                .or_insert_with(|| json!({}));
            let connections_obj = connections
                .as_object_mut()
                .ok_or_else(|| anyhow!("connections must be object"))?;
            let inference_connections = connections_obj
                .entry("inference".to_string())
                .or_insert_with(|| json!({}));
            let inference_connections_obj = inference_connections
                .as_object_mut()
                .ok_or_else(|| anyhow!("inference connections must be object"))?;

            if let Some(repo_context) = inference_connections_obj.get_mut("repo_context") {
                let repo_context_obj = match repo_context.as_object_mut() {
                    Some(obj) => obj,
                    None => continue,
                };
                if repo_context_obj.get("enabled").and_then(Value::as_bool) != Some(true) {
                    repo_context_obj.insert("enabled".to_string(), Value::Bool(true));
                    changed = true;
                }
            }
        }
    }

    if changed {
        persist_context(state, run.id, &run.context).await?;
    }

    Ok(changed)
}

fn strip_inference_enabled_fields_from_stage_patch(payload: &mut Map<String, Value>) {
    let Some(execution_logic) = payload.get_mut("execution_logic") else {
        return;
    };
    let Some(execution_logic_obj) = execution_logic.as_object_mut() else {
        return;
    };
    let Some(connections) = execution_logic_obj.get_mut("connections") else {
        return;
    };
    let Some(connections_obj) = connections.as_object_mut() else {
        return;
    };
    let Some(inference) = connections_obj.get_mut("inference") else {
        return;
    };
    let Some(inference_obj) = inference.as_object_mut() else {
        return;
    };

    for key in ["repo_context", "changeset_schema"] {
        if let Some(fragment) = inference_obj.get_mut(key) {
            if let Some(fragment_obj) = fragment.as_object_mut() {
                fragment_obj.remove("enabled");
            }
        }
    }
}

pub(crate) fn activate_next_prompt_fragments_for_stage(run: &mut WorkflowRun) {
    let root = ensure_engine_root(&mut run.context);
    let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
    let global_state_obj = ensure_value_object(global_state);
    let capabilities = global_state_obj
        .entry("capabilities".to_string())
        .or_insert_with(|| json!({}));
    let capabilities_obj = ensure_value_object(capabilities);
    let inference = capabilities_obj
        .entry("inference".to_string())
        .or_insert_with(|| json!({}));
    let inference_obj = ensure_value_object(inference);

    let next = inference_obj
        .remove("next_prompt_fragments")
        .unwrap_or_else(|| json!([]));

    if next.as_array().map(|items| !items.is_empty()).unwrap_or(false) {
        inference_obj.insert("active_prompt_fragments".to_string(), next);
    } else {
        inference_obj.remove("active_prompt_fragments");
    }
}

pub(crate) fn clear_active_prompt_fragments_for_stage(run: &mut WorkflowRun) {
    let root = ensure_engine_root(&mut run.context);
    let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
    let global_state_obj = ensure_value_object(global_state);
    let capabilities = global_state_obj
        .entry("capabilities".to_string())
        .or_insert_with(|| json!({}));
    let capabilities_obj = ensure_value_object(capabilities);
    let inference = capabilities_obj
        .entry("inference".to_string())
        .or_insert_with(|| json!({}));
    let inference_obj = ensure_value_object(inference);
    inference_obj.remove("active_prompt_fragments");
}

fn ensure_value_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().expect("value must be object")
}

pub async fn select_step(state: &AppState, run_id: Uuid, step_id: &str) -> Result<Value> {
    let mut run = load_run(state, run_id).await?;
    let definition = load_template_definition(state, &run)
        .await?
        .ok_or_else(|| anyhow!("run has no template definition"))?;
    let step = definition
        .steps
        .iter()
        .find(|item| item.id == step_id)
        .ok_or_else(|| anyhow!("unknown step_id {}", step_id))?;

    run.current_step_id = Some(step.id.clone());
    run.status = RunStatus::Paused;

    let decisions = governance::before_stage(state, run_id, &mut run, step).await?;
    governance::apply_context_mutations(&mut run, &decisions, Some(step.id.as_str()), None)?;

    update_run_context(&state.db, run_id, &run.context).await?;
    update_run_status(&state.db, run_id, RunStatus::Paused, Some(step.id.as_str())).await?;

    Ok(json!({ "ok": true, "current_step_id": step.id }))
}

pub async fn patch_global_state(state: &AppState, run_id: Uuid, payload: Value) -> Result<Value> {
    let mut run = load_run(state, run_id).await?;

    let global_payload = match payload {
        Value::Object(map) => Value::Object(map),
        _ => return Err(anyhow!("global payload must be object")),
    };

    let global_state_snapshot = {
        let root = ensure_engine_root(&mut run.context);
        let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
        merge_json_values(global_state, &global_payload);
        global_state.clone()
    };

    update_run_context(&state.db, run_id, &run.context).await?;
    Ok(json!({ "ok": true, "global_state": global_state_snapshot }))
}

fn strip_governance_owned_inference_enabled_flags(payload: &mut Map<String, Value>) {
    let Some(execution_logic) = payload.get_mut("execution_logic") else {
        return;
    };
    let Some(execution_logic_obj) = execution_logic.as_object_mut() else {
        return;
    };
    let Some(connections) = execution_logic_obj.get_mut("connections") else {
        return;
    };
    let Some(connections_obj) = connections.as_object_mut() else {
        return;
    };
    let Some(inference) = connections_obj.get_mut("inference") else {
        return;
    };
    let Some(inference_obj) = inference.as_object_mut() else {
        return;
    };

    for key in ["repo_context", "changeset_schema"] {
        let Some(fragment) = inference_obj.get_mut(key) else {
            continue;
        };
        let Some(fragment_obj) = fragment.as_object_mut() else {
            continue;
        };
        fragment_obj.remove("enabled");
    }
}

pub async fn patch_stage_state(state: &AppState, run_id: Uuid, step_id: &str, payload: Value) -> Result<Value> {
    let mut run = load_run(state, run_id).await?;

    let stage_missing = run
        .context
        .get("workflow_engine")
        .and_then(|v| v.get("stage_state"))
        .and_then(|v| v.get(step_id))
        .is_none();

    if stage_missing {
        let definition = load_template_definition(state, &run)
            .await?
            .ok_or_else(|| anyhow!("run has no template definition"))?;
        let step = definition
            .steps
            .iter()
            .find(|item| item.id == step_id)
            .ok_or_else(|| anyhow!("unknown step_id {}", step_id))?;

        let decisions = governance::before_stage(state, run_id, &mut run, step).await?;
        governance::apply_context_mutations(&mut run, &decisions, Some(step.id.as_str()), None)?;
    }

    let root = ensure_engine_root(&mut run.context);

    let mut stage_payload = match payload {
        Value::Object(map) => map,
        _ => return Err(anyhow!("stage payload must be object")),
    };
    strip_governance_owned_inference_enabled_flags(&mut stage_payload);

    {
        let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
        let global_state_obj = global_state.as_object_mut().ok_or_else(|| anyhow!("global_state must be object"))?;
        if let Some(global_patch) = stage_payload.remove("global_state") {
            let mut merged = Value::Object(global_state_obj.clone());
            merge_json_values(&mut merged, &global_patch);
            *global_state_obj = merged.as_object().cloned().unwrap_or_default();
        }
    }

    let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
    let stage_state_obj = stage_state.as_object_mut().ok_or_else(|| anyhow!("stage_state must be object"))?;
    let existing = stage_state_obj.entry(step_id.to_string()).or_insert_with(|| json!({}));
    let mut merged = existing.clone();
    merge_json_values(&mut merged, &Value::Object(stage_payload.clone()));
    *existing = merged.clone();

    update_run_context(&state.db, run_id, &run.context).await?;
    Ok(json!({ "ok": true, "step_id": step_id, "stage_state": merged }))
}

pub(crate) async fn clear_auto_prompt_fragments(state: &AppState, run_id: Uuid) -> Result<()> {
    let mut run = crate::engine::load_run(state, run_id).await?;
    clear_active_prompt_fragments_for_stage(&mut run);

    let root = ensure_engine_root(&mut run.context);
    let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
    let global_state_obj = ensure_value_object(global_state);
    let capabilities = global_state_obj
        .entry("capabilities".to_string())
        .or_insert_with(|| json!({}));
    let capabilities_obj = ensure_value_object(capabilities);
    let inference = capabilities_obj
        .entry("inference".to_string())
        .or_insert_with(|| json!({}));
    let inference_obj = ensure_value_object(inference);
    inference_obj.remove("next_prompt_fragments");

    persist_context(state, run_id, &run.context).await?;
    Ok(())
}

pub(crate) async fn append_event(
    db: &SqlitePool,
    run_id: Uuid,
    step_id: Option<&str>,
    level: &str,
    kind: &str,
    message: &str,
    payload: Value,
) -> Result<WorkflowEventStreamItem> {
    let stage_execution_id = payload.get("event_meta")
        .and_then(|v| v.get("stage_execution_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let capability_invocation_id = payload.get("event_meta")
        .and_then(|v| v.get("capability_invocation_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let parent_invocation_id = payload.get("event_meta")
        .and_then(|v| v.get("parent_invocation_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let is_header_event = payload.get("event_meta")
        .and_then(|v| v.get("is_header_event"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let sequence_no: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence_no), 0) + 1 FROM workflow_events WHERE run_id = ?")
        .bind(run_id.to_string())
        .fetch_one(db)
        .await?;
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    let run_id_str = run_id.to_string();
    let payload_json = payload.to_string();

    sqlx::query(
        r#"
        INSERT INTO workflow_events (
            id,
            run_id,
            step_id,
            stage_execution_id,
            capability_invocation_id,
            parent_invocation_id,
            sequence_no,
            is_header_event,
            level,
            kind,
            message,
            payload_json,
            created_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&id)
    .bind(&run_id_str)
    .bind(step_id)
    .bind(&stage_execution_id)
    .bind(&capability_invocation_id)
    .bind(&parent_invocation_id)
    .bind(sequence_no)
    .bind(if is_header_event { 1 } else { 0 })
    .bind(level)
    .bind(kind)
    .bind(message)
    .bind(&payload_json)
    .bind(&now)
    .execute(db)
    .await?;

    sqlx::query("UPDATE workflow_runs SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&run_id_str)
        .execute(db)
        .await?;

    Ok(WorkflowEventStreamItem {
        id,
        run_id: run_id_str,
        step_id: step_id.map(ToString::to_string),
        stage_execution_id,
        capability_invocation_id,
        parent_invocation_id,
        sequence_no,
        level: level.to_string(),
        kind: kind.to_string(),
        message: message.to_string(),
        payload,
        created_at: now,
    })
}

pub(crate) async fn update_run_status(
    db: &SqlitePool,
    run_id: Uuid,
    status: RunStatus,
    current_step_id: Option<&str>,
) -> Result<()> {
    let status_str = serde_json::to_string(&status)?;
    let status_str = status_str.trim_matches('"').to_string();

    sqlx::query(
        "UPDATE workflow_runs SET status = ?, current_step_id = ?, updated_at = ? WHERE id = ?",
    )
    .bind(status_str)
    .bind(current_step_id)
    .bind(Utc::now().to_rfc3339())
    .bind(run_id.to_string())
    .execute(db)
    .await?;

    Ok(())
}

pub(crate) fn merge_json_values(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base_obj), Value::Object(patch_obj)) => {
            for (key, patch_value) in patch_obj {
                if patch_value.is_null() {
                    base_obj.remove(key);
                    continue;
                }
                match base_obj.get_mut(key) {
                    Some(base_value) => merge_json_values(base_value, patch_value),
                    None => {
                        base_obj.insert(key.clone(), patch_value.clone());
                    }
                }
            }
        }
        (base_slot, patch_value) => {
            *base_slot = patch_value.clone();
        }
    }
}

async fn update_run_context(
    db: &SqlitePool,
    run_id: Uuid,
    context: &Value,
) -> Result<()> {
    sqlx::query(
        "UPDATE workflow_runs SET context_json = ?, updated_at = ? WHERE id = ?",
    )
    .bind(serde_json::to_string_pretty(context)?)
    .bind(Utc::now().to_rfc3339())
    .bind(run_id.to_string())
    .execute(db)
    .await?;

    Ok(())
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
    let event = append_event(&state.db, run_id, step_id, level, kind, message, payload).await?;
    state.publish_workflow_event(event);
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
