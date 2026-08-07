pub(crate) mod capabilities;
pub(crate) mod governance;
pub(crate) mod orchestration_inputs;
mod runtime;
pub(crate) mod runtime_tools;

pub(crate) mod stages;
mod transitions;
pub mod workflow_lifecycle;

use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Map, Value};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    models::{RunStatus, WorkflowEventStreamItem, WorkflowRun, WorkflowStepDefinition, WorkflowTemplateDefinition},
};

pub use runtime::{get_transient_stage_user_input, patch_transient_stage_user_input, prepare_run_stage_for_execution, validate_workflow_action_request, WorkflowActionPreconditions};
pub(crate) use runtime::{fail_runtime_workflow, run_workflow_runtime};
pub use transitions::{next_step_id, previous_step_id};

async fn fail_open_capability_invocations_for_process_stop(
    state: &AppState,
    run_ids: &[Uuid],
    reason: &str,
) -> Result<usize> {
    let rows = sqlx::query(
        r#"
        SELECT
            started.run_id,
            started.step_id,
            started.stage_execution_id,
            started.capability_invocation_id,
            started.parent_invocation_id,
            started.kind,
            json_extract(started.payload_json, '$.capability') AS capability
        FROM workflow_events started
        WHERE started.kind LIKE '%_started'
          AND started.capability_invocation_id IS NOT NULL
          AND TRIM(started.capability_invocation_id) != ''
          AND NOT EXISTS (
              SELECT 1
              FROM workflow_events terminal
              WHERE terminal.run_id = started.run_id
                AND terminal.capability_invocation_id = started.capability_invocation_id
                AND (
                    terminal.kind LIKE '%_completed'
                    OR terminal.kind LIKE '%_failed'
                )
          )
        ORDER BY started.run_id ASC, started.sequence_no ASC
        "#,
    )
    .fetch_all(&state.db)
    .await?;

    let mut failed_count = 0usize;

    for row in rows {
        let run_id = Uuid::parse_str(row.get::<String, _>("run_id").as_str())?;
        let step_id = row.get::<Option<String>, _>("step_id");
        let stage_execution_id = row.get::<Option<String>, _>("stage_execution_id");
        let capability_invocation_id = row.get::<String, _>("capability_invocation_id");
        if !run_ids.contains(&run_id) {
            continue;
        }
        if !run_ids.contains(&run_id) {
            continue;
        }
        let parent_invocation_id = row.get::<Option<String>, _>("parent_invocation_id");
        let started_kind = row.get::<String, _>("kind");
        let capability = row
            .get::<Option<String>, _>("capability")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                started_kind
                    .strip_suffix("_started")
                    .unwrap_or(started_kind.as_str())
                    .to_string()
            });

        append_engine_event(
            state,
            run_id,
            step_id.as_deref(),
            "error",
            format!("{}_failed", capability).as_str(),
            reason,
            json!({
                "capability": capability,
                "ok": false,
                "error": reason,
                "interrupted": true,
                "disposition": "process_stopped",
                "process_session_id": state.process_session_id(),
                "event_meta": event_meta(
                    stage_execution_id.as_deref(),
                    Some(capability_invocation_id.as_str()),
                    parent_invocation_id.as_deref(),
                    false
                )
            }),
        )
        .await?;

        failed_count += 1;
    }

    Ok(failed_count)
}

async fn fail_open_stage_executions_for_process_stop(
    state: &AppState,
    run_ids: &[Uuid],
    reason: &str,
) -> Result<usize> {
    let rows = sqlx::query(
        r#"
        SELECT
            started.run_id,
            started.step_id,
            started.stage_execution_id
        FROM workflow_events started
        WHERE started.kind = 'stage_execution_started'
          AND started.stage_execution_id IS NOT NULL
          AND TRIM(started.stage_execution_id) != ''
          AND NOT EXISTS (
              SELECT 1
              FROM workflow_events terminal
              WHERE terminal.run_id = started.run_id
                AND terminal.stage_execution_id = started.stage_execution_id
                AND terminal.kind = 'stage_execution_completed'
          )
        ORDER BY started.run_id ASC, started.sequence_no ASC
        "#,
    )
    .fetch_all(&state.db)
    .await?;

    let mut failed_count = 0usize;

    for row in rows {
        let run_id = Uuid::parse_str(row.get::<String, _>("run_id").as_str())?;
        let step_id = row.get::<Option<String>, _>("step_id");
        let stage_execution_id = row.get::<String, _>("stage_execution_id");
        if !run_ids.contains(&run_id) {
            continue;
        }
        if !run_ids.contains(&run_id) {
            continue;
        }

        append_engine_event(
            state,
            run_id,
            step_id.as_deref(),
            "error",
            "stage_execution_completed",
            reason,
            json!({
                "step_id": step_id,
                "ok": false,
                "message": reason,
                "disposition": "process_stopped",
                "interrupted": true,
                "process_session_id": state.process_session_id(),
                "event_meta": event_meta(
                    Some(stage_execution_id.as_str()),
                    None,
                    None,
                    true
                )
            }),
        )
        .await?;

        failed_count += 1;
    }

    Ok(failed_count)
}

pub async fn fail_active_runs_for_process_stop(
    state: &AppState,
    run_ids: &[Uuid],
    reason: &str,
) -> Result<usize> {
    if run_ids.is_empty() {
        return Ok(0);
    }

    let failed_capability_invocations =
        fail_open_capability_invocations_for_process_stop(state, run_ids, reason).await?;
    let failed_stage_executions =
        fail_open_stage_executions_for_process_stop(state, run_ids, reason).await?;

    if failed_capability_invocations > 0 {
        tracing::warn!(
            failed_capability_invocations,
            "marked interrupted capability invocations as failed"
        );
    }

    if failed_stage_executions > 0 {
        tracing::warn!(
            failed_stage_executions,
            "marked interrupted stage executions as failed"
        );
    }

    let rows = sqlx::query(
        r#"
        SELECT id, current_step_id, context_json
        FROM workflow_runs
        ORDER BY created_at ASC
        "#,
    )
    .fetch_all(&state.db)
    .await?;

    let mut failed_count = 0usize;

    for row in rows {
        let run_id = Uuid::parse_str(row.get::<String, _>("id").as_str())?;
        if !run_ids.contains(&run_id) {
            continue;
        }
        let current_step_id = row.get::<Option<String>, _>("current_step_id");
        let context_json = row.get::<String, _>("context_json");
        let mut context = serde_json::from_str::<Value>(&context_json)
            .unwrap_or_else(|_| json!({}));

        let workflow_engine = context
            .as_object_mut()
            .expect("workflow context must be an object")
            .entry("workflow_engine".to_string())
            .or_insert_with(|| json!({}));

        let workflow_engine = workflow_engine
            .as_object_mut()
            .expect("workflow_engine must be an object");

        let run_state = workflow_engine
            .entry("run_state".to_string())
            .or_insert_with(|| json!({}));

        let run_state = run_state
            .as_object_mut()
            .expect("run_state must be an object");

        let interrupted_checkpoint = run_state.remove("blocked_on");

        run_state.insert(
            "terminal_error".to_string(),
            json!({
                "kind": "process_stopped",
                "message": reason,
                "process_session_id": state.process_session_id(),
                "interrupted_checkpoint": interrupted_checkpoint,
                "occurred_at": Utc::now().to_rfc3339()
            }),
        );

        if let Some(step_id) = current_step_id.as_deref() {
            let local_state = workflow_engine
                .entry("local_state".to_string())
                .or_insert_with(|| json!({}));

            if let Some(local_state) = local_state.as_object_mut() {
                let stages = local_state
                    .entry("stages".to_string())
                    .or_insert_with(|| json!({}));

                if let Some(stages) = stages.as_object_mut() {
                    let stage = stages
                        .entry(step_id.to_string())
                        .or_insert_with(|| json!({}));

                    if let Some(stage) = stage.as_object_mut() {
                        stage.insert("status".to_string(), Value::String("error".to_string()));
                        stage.insert("error".to_string(), Value::String(reason.to_string()));
                        stage.insert(
                            "completed_at".to_string(),
                            Value::String(Utc::now().to_rfc3339()),
                        );
                    }
                }
            }
        }

        sqlx::query(
            r#"
            UPDATE workflow_runs
            SET status = 'error',
                context_json = ?,
                updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(serde_json::to_string_pretty(&context)?)
        .bind(Utc::now().to_rfc3339())
        .bind(run_id.to_string())
        .execute(&state.db)
        .await?;

        append_engine_event(
            state,
            run_id,
            current_step_id.as_deref(),
            "error",
            "workflow_process_stopped",
            reason,
            json!({
                "reason": reason,
                "terminal": true,
                "process_session_id": state.process_session_id(),
                "event_meta": {
                    "is_header_event": true
                }
            }),
        )
        .await?;

        failed_count += 1;
    }

    Ok(failed_count)
}

pub async fn fail_stale_running_runs_on_startup(state: &AppState) -> Result<usize> {
    let rows = sqlx::query(
        r#"
        SELECT id
        FROM workflow_runs
        WHERE status = 'running'
        ORDER BY created_at ASC
        "#,
    )
    .fetch_all(&state.db)
    .await?;

    let run_ids = rows
        .into_iter()
        .map(|row| Uuid::parse_str(row.get::<String, _>("id").as_str()))
        .collect::<std::result::Result<Vec<_>, _>>()?;

    if run_ids.is_empty() {
        return Ok(0);
    }

    fail_active_runs_for_process_stop(
        state,
        &run_ids,
        "The previous API process stopped before the stage execution completed.",
    )
    .await
}

pub async fn load_run(state: &AppState, run_id: Uuid) -> Result<WorkflowRun> {
    let row = sqlx::query(
        "SELECT id, template_id, definition_json, status, current_step_id, title, repo_ref, workflow_key, context_json, created_at, updated_at FROM workflow_runs WHERE id = ?"
    )
    .bind(run_id.to_string())
    .fetch_one(&state.db)
    .await?;

    let mut run = WorkflowRun {
        id: Uuid::parse_str(row.get::<String, _>("id").as_str())?,
        template_id: row.get::<Option<String>, _>("template_id").map(|v| Uuid::parse_str(v.as_str())).transpose()?,
        definition: parse_definition_json_for_load(row.get::<String, _>("id").as_str(), row.get::<String, _>("definition_json").as_str()),
        status: match row.get::<String, _>("status").as_str() {
            "draft" => RunStatus::Draft,
            "queued" => RunStatus::Queued,
            "running" => RunStatus::Running,
            "waiting" => RunStatus::Waiting,
            "paused" => RunStatus::Paused,
            "complete" | "success" => RunStatus::Success,
            "cancelled" => RunStatus::Cancelled,
            _ => RunStatus::Error,
        },
        current_step_id: row.get("current_step_id"),
        title: row.get("title"),
        repo_ref: row.get("repo_ref"),
        workflow_key: row.get("workflow_key"),
        context: parse_context_json_for_load(row.get::<String, _>("id").as_str(), row.get::<String, _>("context_json").as_str()),
        created_at: chrono::DateTime::parse_from_rfc3339(row.get::<String, _>("created_at").as_str())?.with_timezone(&chrono::Utc),
        updated_at: chrono::DateTime::parse_from_rfc3339(row.get::<String, _>("updated_at").as_str())?.with_timezone(&chrono::Utc),
    };

    normalize_inference_arm_state(&mut run);

    Ok(run)
}

pub async fn load_template_definition(_state: &AppState, run: &WorkflowRun) -> Result<Option<WorkflowTemplateDefinition>> {
    Ok(Some(run.definition.clone()))
}

fn json_preview_for_load_log(raw: &str) -> String {
    raw.chars().take(512).collect::<String>()
}

fn parse_definition_json_for_load(run_id: &str, raw: &str) -> WorkflowTemplateDefinition {
    let trimmed = raw.trim();
    match serde_json::from_str::<WorkflowTemplateDefinition>(trimmed) {
        Ok(definition) => definition,
        Err(err) => {
            tracing::error!(
                run_id = %run_id,
                definition_json_bytes = raw.len(),
                definition_json_trimmed_bytes = trimmed.len(),
                definition_json_preview = %json_preview_for_load_log(trimmed),
                error = %err,
                "workflow run definition_json is malformed; using empty readable definition"
            );
            WorkflowTemplateDefinition {
                version: 1,
                globals: Default::default(),
                governance: json!({}),
                steps: Vec::new(),
            }
        }
    }
}

fn parse_context_json_for_load(run_id: &str, raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        tracing::warn!(run_id = %run_id, "workflow run context_json is empty; using empty context");
        return json!({});
    }

    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(
                run_id = %run_id,
                context_json_bytes = raw.len(),
                context_json_trimmed_bytes = trimmed.len(),
                context_json_preview = %json_preview_for_load_log(trimmed),
                error = %err,
                "workflow run context_json is malformed; using empty context"
            );
            json!({})
        }
    }
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

fn ensure_value_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().expect("value must be object")
}

pub fn refresh_inference_arm_state(run: &mut WorkflowRun, _selected_step: Option<&WorkflowStepDefinition>) {
    normalize_inference_arm_state(run);
}

pub fn rearm_inference_inputs_for_stage(
    run: &mut WorkflowRun,
    _step: &WorkflowStepDefinition,
) {
    normalize_inference_arm_state(run);
}

fn normalize_inference_arm_state(run: &mut WorkflowRun) {
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

    inference_obj.remove("shared_inference_state");
}

fn prepared_inference_step_id(run: &WorkflowRun) -> Option<&str> {
    run.context
        .get("workflow_engine")
        .and_then(|value| value.get("run_state"))
        .and_then(|value| value.get("prepared_inference_step_id"))
        .and_then(Value::as_str)
}

fn set_prepared_inference_step_id(run: &mut WorkflowRun, step_id: &str) {
    let root = ensure_engine_root(&mut run.context);
    let run_state = root
        .entry("run_state".to_string())
        .or_insert_with(|| json!({}));
    let run_state = ensure_value_object(run_state);
    run_state.insert(
        "prepared_inference_step_id".to_string(),
        Value::String(step_id.to_string()),
    );
}

pub fn clear_prepared_inference_step(run: &mut WorkflowRun) {
    let Some(run_state) = run
        .context
        .get_mut("workflow_engine")
        .and_then(Value::as_object_mut)
        .and_then(|root| root.get_mut("run_state"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };

    run_state.remove("prepared_inference_step_id");
}

pub async fn reconcile_inference_session(
    state: &AppState,
    run: &mut WorkflowRun,
) -> Result<bool> {
    let step = run
        .current_step_id
        .as_deref()
        .and_then(|step_id| run.definition.steps.iter().find(|step| step.id == step_id))
        .cloned();

    let Some(step) = step else {
        return Ok(false);
    };

    let changed = capabilities::inference::browser::reconcile_browser_session_rearm(
        state,
        run,
        &step,
    )
    .await?;

    if changed {
        persist_context(state, run.id, &run.context).await?;
    }

    Ok(changed)
}

pub async fn preprocess_run_for_current_stage(
    state: &AppState,
    run: &mut WorkflowRun,
    previous_step_id: Option<&str>,
) -> Result<bool> {
    let step = run
        .current_step_id
        .as_deref()
        .and_then(|step_id| run.definition.steps.iter().find(|step| step.id == step_id))
        .cloned();

    let Some(step) = step else {
        return Ok(false);
    };

    let stage_changed = previous_step_id.is_some()
        && previous_step_id != Some(step.id.as_str());
    let stage_prepared = prepared_inference_step_id(run) == Some(step.id.as_str());

    if stage_changed || !stage_prepared {
        rearm_inference_inputs_for_stage(run, &step);
        let changed = reconcile_inference_session(state, run).await?;
        set_prepared_inference_step_id(run, step.id.as_str());
        persist_context(state, run.id, &run.context).await?;
        return Ok(changed);
    }

    refresh_inference_arm_state(run, Some(&step));
    persist_context(state, run.id, &run.context).await?;

    Ok(false)
}

pub async fn select_step(state: &AppState, run_id: Uuid, step_id: &str) -> Result<Value> {
    let mut run = load_run(state, run_id).await?;
    let definition = load_template_definition(state, &run)
        .await?
        .ok_or_else(|| anyhow!("run has no template definition"))?;
    let previous_step_id = run.current_step_id.clone();

    let step = transitions::transition_to_step(
        state,
        run_id,
        &mut run,
        &definition,
        step_id,
    )
    .await?;

    preprocess_run_for_current_stage(
        state,
        &mut run,
        previous_step_id.as_deref(),
    )
    .await?;

    run.status = RunStatus::Waiting;
    update_run_context(&state.db, run_id, &run.context).await?;
    set_run_status(
        state,
        run_id,
        RunStatus::Waiting,
        Some(step.id.as_str()),
    )
    .await?;

    Ok(json!({
        "ok": true,
        "run_id": run_id,
        "current_step_id": step.id,
        "status": "waiting",
        "run": run
    }))
}

pub async fn patch_global_state(state: &AppState, run_id: Uuid, payload: Value) -> Result<Value> {
    let mut run = load_run(state, run_id).await?;

    let global_payload = match payload {
        Value::Object(map) => Value::Object(map),
        _ => return Err(anyhow!("global payload must be object")),
    };

    let selected_step = run
        .current_step_id
        .as_deref()
        .and_then(|step_id| run.definition.steps.iter().find(|step| step.id == step_id))
        .cloned();

    {
        let root = ensure_engine_root(&mut run.context);
        let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
        merge_json_values(global_state, &global_payload);
        normalize_runtime_planner(global_state);
    }

    refresh_inference_arm_state(&mut run, selected_step.as_ref());

    let global_state_snapshot = run
        .context
        .get("workflow_engine")
        .and_then(|v| v.get("global_state"))
        .cloned()
        .unwrap_or_else(|| json!({}));

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
        .and_then(|v| v.get("stage_overrides"))
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

    let selected_step = run
        .definition
        .steps
        .iter()
        .find(|item| item.id == step_id)
        .cloned();

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

    let stage_overrides = root.entry("stage_overrides".to_string()).or_insert_with(|| json!({}));
    let stage_overrides_obj = stage_overrides.as_object_mut().ok_or_else(|| anyhow!("stage_overrides must be object"))?;
    let existing = stage_overrides_obj.entry(step_id.to_string()).or_insert_with(|| json!({}));
    let mut merged = existing.clone();
    merge_json_values(&mut merged, &Value::Object(stage_payload.clone()));
    *existing = merged.clone();

    refresh_inference_arm_state(&mut run, selected_step.as_ref());
    update_run_context(&state.db, run_id, &run.context).await?;
    Ok(json!({ "ok": true, "step_id": step_id, "stage_state": merged }))
}

pub(crate) async fn clear_auto_prompt_fragments(state: &AppState, run_id: Uuid) -> Result<()> {
    state.orchestration_inputs.clear_run(run_id);
    Ok(())
}

async fn persist_workflow_event(
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
    let global_sequence_no: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(global_sequence_no), 0) + 1 FROM workflow_events",
    )
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
            global_sequence_no,
            is_header_event,
            level,
            kind,
            message,
            payload_json,
            created_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&id)
    .bind(&run_id_str)
    .bind(step_id)
    .bind(&stage_execution_id)
    .bind(&capability_invocation_id)
    .bind(&parent_invocation_id)
    .bind(sequence_no)
    .bind(global_sequence_no)
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
        global_sequence_no,
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

fn normalize_runtime_planner(global_state: &mut Value) {
    let Some(capabilities) = global_state
        .get_mut("capabilities")
        .and_then(Value::as_object_mut)
    else {
        return;
    };

    capabilities.remove("planner_fragment");

    if let Some(inference) = capabilities
        .get_mut("inference")
        .and_then(Value::as_object_mut)
    {
        inference.remove("planner");
    }

    let Some(planner) = capabilities
        .get_mut("planner")
        .and_then(Value::as_object_mut)
    else {
        return;
    };

    planner.remove("feature_plan_items");
    planner.remove("selected_feature_ids");
    planner.remove("enabled");
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
    let event = persist_workflow_event(
        &state.db,
        run_id,
        step_id,
        level,
        kind,
        message,
        payload,
    )
    .await?;
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
    let status_value = serde_json::to_value(status)?;
    update_run_status(&state.db, run_id, status, current_step_id).await?;
    append_engine_event(
        state,
        run_id,
        current_step_id,
        "info",
        "run_status_changed",
        "Workflow run status changed.",
        json!({
            "status": status_value,
            "current_step_id": current_step_id,
            "event_meta": {
                "is_header_event": true
            }
        }),
    )
    .await?;
    Ok(())
}

pub(crate) async fn persist_context(state: &AppState, run_id: Uuid, context: &Value) -> Result<()> {
    update_run_context(&state.db, run_id, context).await?;
    Ok(())
}

pub(crate) async fn on_browser_session_changed(
    state: &AppState,
    run_id: Uuid,
    previous_session_id: &str,
    next_session_id: &str,
) -> Result<()> {
    let mut run = load_run(state, run_id).await?;
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
    let connection_runtime = inference_obj
        .entry("connection_runtime".to_string())
        .or_insert_with(|| json!({}));
    let connection_runtime_obj = ensure_value_object(connection_runtime);

    connection_runtime_obj.insert(
        "session_rearm".to_string(),
        json!({
            "needed": true,
            "reason": "browser_session_changed",
            "previous_session_id": previous_session_id,
            "next_session_id": next_session_id
        }),
    );

    inference_obj.remove("shared_inference_state");
    inference_obj.remove("next_prompt_fragments");
    inference_obj.remove("active_prompt_fragments");

    clear_prepared_inference_step(&mut run);

    tracing::warn!(
        run_id = %run_id,
        previous_session_id = %previous_session_id,
        next_session_id = %next_session_id,
        "browser inference session changed; deferred inference fragment rearming to stage preprocessing"
    );

    persist_context(state, run_id, &run.context).await
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
