use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::{governance, refresh_inference_arm_state},
    models::RunStatus,
};

use super::{
    append_engine_event,
    event_meta,
    current_step,
    ensure_engine_root,
    load_run,
    load_template_definition,
    persist_context,
    select_step,
    set_run_status,
};
use super::stages::{execute_stage, StageDisposition};
use super::transitions::{next_step_id, resolve_next_target, should_auto_advance, transition_to_step};

pub async fn patch_transient_stage_user_input(
    state: &AppState,
    run_id: Uuid,
    step_id: &str,
    payload: Value,
) -> Result<Value> {
    let text = payload
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("payload.text must be a string"))?
        .to_string();
    let client_id = payload
        .get("client_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    state
        .orchestration_inputs
        .set_user_instruction(run_id, step_id, text.clone());

    append_engine_event(
        state,
        run_id,
        Some(step_id),
        "info",
        "transient_stage_user_input_patched",
        "Transient stage user input updated",
        json!({
            "text": text,
            "client_id": client_id
        }),
    )
    .await?;

    Ok(json!({
        "ok": true,
        "run_id": run_id,
        "step_id": step_id,
        "text": text,
        "client_id": client_id
    }))
}

pub fn get_transient_stage_user_input(
    state: &AppState,
    run_id: Uuid,
    step_id: &str,
) -> Value {
    json!({
        "ok": true,
        "run_id": run_id,
        "step_id": step_id,
        "text": state.orchestration_inputs.user_instruction(run_id, step_id)
    })
}

fn run_status_label(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Draft => "draft",
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::Waiting => "waiting",
        RunStatus::Paused => "paused",
        RunStatus::Success => "complete",
        RunStatus::Error => "error",
        RunStatus::Cancelled => "cancelled",
    }
}

fn run_is_waiting_on_operator_checkpoint(run: &super::WorkflowRun) -> bool {
    matches!(run.status, RunStatus::Waiting)
        && run
            .context
            .get("workflow_engine")
            .and_then(|value| value.get("run_state"))
            .and_then(|value| value.get("blocked_on"))
            .and_then(|value| value.get("kind"))
            .and_then(Value::as_str)
            == Some("operator_checkpoint")
}

fn run_is_blocked_by_user_control(run: &super::WorkflowRun) -> bool {
    run
        .context
        .get("workflow_engine")
        .and_then(|value| value.get("run_state"))
        .and_then(|value| value.get("blocked_on"))
        .and_then(|value| value.get("kind"))
        .and_then(Value::as_str)
        == Some("pause_after_stage")
}

fn run_blocked_on_label(run: &super::WorkflowRun) -> &'static str {
    if run_is_waiting_on_operator_checkpoint(run) {
        "operator_checkpoint"
    } else if run_is_blocked_by_user_control(run) {
        "pause_after_stage"
    } else {
        ""
    }
}

fn clear_user_control_block(run: &mut super::WorkflowRun) {
    let root = ensure_engine_root(&mut run.context);
    if let Some(run_state) = root.get_mut("run_state").and_then(|value| value.as_object_mut()) {
        let should_clear = run_state
            .get("blocked_on")
            .and_then(|value| value.get("kind"))
            .and_then(Value::as_str)
            == Some("pause_after_stage");
        if should_clear {
            run_state.remove("blocked_on");
        }
    }
}

async fn run_stage_exit_hook_if_transitioning(
    state: &AppState,
    run_id: Uuid,
    definition: &crate::models::WorkflowTemplateDefinition,
    previous_step_id: Option<&str>,
    next_step_id: Option<&str>,
) -> Result<()> {
    if previous_step_id == next_step_id {
        return Ok(());
    }

    let Some(previous_step_id) = previous_step_id else {
        return Ok(());
    };

    let Some(previous_step) = definition
        .steps
        .iter()
        .find(|step| step.id == previous_step_id)
    else {
        return Ok(());
    };

    super::stages::invoke_stage_exit_hook(
        state,
        run_id,
        previous_step,
        next_step_id,
    )
    .await
}

pub async fn start_run(state: &AppState, run_id: Uuid, requested_step_id: Option<&str>) -> Result<serde_json::Value> {
    let mut run = load_run(state, run_id).await?;
    let definition = load_template_definition(state, &run)
        .await?
        .ok_or_else(|| anyhow!("run has no template definition"))?;
    let repaired_step_id = requested_step_id
        .map(str::to_string)
        .or_else(|| run.current_step_id.clone())
        .or_else(|| definition.steps.first().map(|step| step.id.clone()))
        .ok_or_else(|| anyhow!("workflow run {} has no workflow steps", run_id))?;

    if run.current_step_id.as_deref() != Some(repaired_step_id.as_str()) {
        run.current_step_id = Some(repaired_step_id.clone());
        set_run_status(state, run_id, run.status.clone(), Some(repaired_step_id.as_str())).await?;
    }

    if matches!(run.status, RunStatus::Queued | RunStatus::Running) || run_is_waiting_on_operator_checkpoint(&run) || run_is_blocked_by_user_control(&run) {
        let blocked_on = run_blocked_on_label(&run);

        tracing::warn!(
            run_id = %run_id,
            status = %run_status_label(&run.status),
            current_step_id = ?run.current_step_id,
            requested_step_id = ?requested_step_id,
            repaired_step_id = %repaired_step_id,
            blocked_on = %blocked_on,
            "workflow autonomous start rejected because the run is already active or blocked"
        );

        return Ok(json!({
            "ok": true,
            "idempotent": true,
            "started": false,
            "status": run_status_label(&run.status),
            "current_step_id": run.current_step_id,
            "blocked_on": blocked_on,
            "requires_user_release": blocked_on == "pause_after_stage"
        }));
    }

    tracing::info!(
        run_id = %run_id,
        previous_status = %run_status_label(&run.status),
        current_step_id = ?run.current_step_id,
        requested_step_id = ?requested_step_id,
        repaired_step_id = %repaired_step_id,
        "workflow autonomous start accepted"
    );

    set_run_status(state, run_id, RunStatus::Queued, Some(repaired_step_id.as_str())).await?;
    start_or_resume_automatic_run(state, run_id, Some(repaired_step_id.as_str())).await
}

pub async fn resume_run(state: &AppState, run_id: Uuid) -> Result<serde_json::Value> {
    let mut run = load_run(state, run_id).await?;

    let operator_checkpoint = run
        .context
        .get("workflow_engine")
        .and_then(|value| value.get("run_state"))
        .and_then(|value| value.get("blocked_on"))
        .filter(|blocked| {
            blocked
                .get("kind")
                .and_then(Value::as_str)
                == Some("operator_checkpoint")
        })
        .cloned();

    if let Some(blocked_on) = operator_checkpoint {
        return Ok(json!({
            "ok": false,
            "status": "waiting",
            "blocked_on": "operator_checkpoint",
            "current_step_id": run.current_step_id,
            "checkpoint": blocked_on,
            "message": "Resolve the operator checkpoint with a disposition instead of resuming the run."
        }));
    }

    if run_is_blocked_by_user_control(&run) {
        let current_step_id = run.current_step_id.clone();
        clear_user_control_block(&mut run);
        persist_context(state, run_id, &run.context).await?;
        tracing::info!(
            run_id = %run_id,
            current_step_id = ?current_step_id,
            "workflow user-control checkpoint released by explicit resume"
        );
        append_engine_event(
            state,
            run_id,
            current_step_id.as_deref(),
            "info",
            "user_control_released",
            "Workflow user-control checkpoint was released by explicit resume.",
            json!({ "blocked_on": "pause_after_stage" }),
        ).await?;
    }

    start_or_resume_automatic_run(state, run_id, None).await
}

pub async fn pause_run(state: &AppState, run_id: Uuid) -> Result<serde_json::Value> {
    let mut run = load_run(state, run_id).await?;
    let disposition_review_waiting = matches!(run.status, RunStatus::Waiting)
        && run
            .context
            .get("workflow_engine")
            .and_then(|v| v.get("run_state"))
            .and_then(|v| v.get("blocked_on"))
            .and_then(|v| v.get("kind"))
            .and_then(Value::as_str)
            == Some("operator_checkpoint");
    let autonomous_pause_eligible = if matches!(run.status, RunStatus::Waiting) {
        load_template_definition(state, &run)
            .await?
            .and_then(|definition| current_step(&definition, &run, None).ok().map(step_is_auto_runnable))
            .unwrap_or(false)
    } else {
        false
    };

    if disposition_review_waiting {
        return resolve_operator_checkpoint(state, run_id, "pause_error", None).await;
    }

    if !matches!(run.status, RunStatus::Queued | RunStatus::Running) && !disposition_review_waiting && !autonomous_pause_eligible {
        let status = format!("{:?}", run.status).to_lowercase();
        append_engine_event(
            state,
            run_id,
            run.current_step_id.as_deref(),
            "info",
            "run_pause_ignored",
            "Pause request ignored because the workflow run is not active.",
            json!({ "status": status }),
        ).await?;
        return Ok(json!({
            "ok": true,
            "status": status,
            "pause_requested": false,
            "current_step_id": run.current_step_id,
        }));
    }

    let root = ensure_engine_root(&mut run.context);
    let run_state = root.entry("run_state".to_string()).or_insert_with(|| json!({}));
    let run_state_obj = run_state.as_object_mut().ok_or_else(|| anyhow!("run_state must be object"))?;
    run_state_obj.insert("pause_requested".to_string(), json!(true));
    persist_context(state, run_id, &run.context).await?;

    append_engine_event(
        state,
        run_id,
        run.current_step_id.as_deref(),
        "info",
        "run_pause_requested",
        "Workflow run will pause after the current stage finishes.",
        json!({}),
    ).await?;
    Ok(json!({ "ok": true, "status": "pause_requested", "current_step_id": run.current_step_id }))
}

pub async fn force_wait_run(state: &AppState, run_id: Uuid) -> Result<serde_json::Value> {
    let mut run = load_run(state, run_id).await?;
    let previous_status = format!("{:?}", run.status).to_lowercase();
    let root = ensure_engine_root(&mut run.context);
    let run_state = root.entry("run_state".to_string()).or_insert_with(|| json!({}));
    let run_state_obj = run_state.as_object_mut().ok_or_else(|| anyhow!("run_state must be object"))?;
    run_state_obj.remove("pause_requested");
    run_state_obj.remove("blocked_on");
    run_state_obj.insert("cancel_requested".to_string(), json!(true));
    run_state_obj.insert("cancel_requested_at".to_string(), json!(Utc::now().to_rfc3339()));
    persist_context(state, run_id, &run.context).await?;

    set_run_status(state, run_id, RunStatus::Waiting, run.current_step_id.as_deref()).await?;
    append_engine_event(
        state,
        run_id,
        run.current_step_id.as_deref(),
        "warn",
        "run_cancel_requested",
        "Workflow run cancellation was requested by operator and control returned to waiting state.",
        json!({
            "previous_status": previous_status,
            "current_step_id": run.current_step_id
        }),
    ).await?;
    Ok(json!({ "ok": true, "status": "waiting", "cancel_requested": true, "current_step_id": run.current_step_id }))
}

async fn latest_stage_execution_id_for_step(
    state: &AppState,
    run_id: Uuid,
    stage_id: &str,
) -> Result<Option<String>> {
    let value = sqlx::query_scalar::<_, String>(
        r#"
        SELECT stage_execution_id
        FROM workflow_events
        WHERE run_id = ?
          AND step_id = ?
          AND stage_execution_id IS NOT NULL
          AND TRIM(stage_execution_id) != ''
        ORDER BY sequence_no DESC
        LIMIT 1
        "#,
    )
    .bind(run_id.to_string())
    .bind(stage_id)
    .fetch_optional(&state.db)
    .await?;

    Ok(value)
}

async fn append_disposition_stage_completion_event(
    state: &AppState,
    run_id: Uuid,
    stage_id: &str,
    stage_execution_id: Option<&str>,
    ok: bool,
    disposition: &str,
    message: &str,
    next_step_id: Option<&str>,
) -> Result<()> {
    let mut payload = json!({
        "step_id": stage_id,
        "ok": ok,
        "message": message,
        "disposition": disposition,
        "event_meta": event_meta(stage_execution_id, None, None, true)
    });

    if let Some(next_step_id) = next_step_id {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("next_step_id".to_string(), Value::String(next_step_id.to_string()));
        }
    }

    append_engine_event(
        state,
        run_id,
        Some(stage_id),
        if ok { "info" } else { "warn" },
        "stage_execution_completed",
        message,
        payload,
    )
    .await
}

#[derive(Debug, Clone, Default)]
pub struct WorkflowActionPreconditions<'a> {
    pub expected_status: Option<&'a str>,
    pub expected_step_id: Option<&'a str>,
    pub expected_stage_execution_id: Option<&'a str>,
    pub expected_capability_invocation_id: Option<&'a str>,
}

fn run_status_name(status: RunStatus) -> &'static str {
    match status {
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

fn action_is_read_only(action: &str) -> bool {
    matches!(action, "get_transient_stage_user_input")
}

fn action_allows_running_workflow(action: &str) -> bool {
    matches!(
        action,
        "pause_run"
            | "cancel_run"
            | "force_wait_run"
            | "force_unlock_run"
            | "force_complete_stage"
    )
}

fn action_requires_waiting_workflow(action: &str) -> bool {
    matches!(action, "resolve_operator_checkpoint")
}

fn action_requires_current_step(action: &str) -> bool {
    matches!(
        action,
        "prepare_stage"
            | "prepare_current_stage"
            | "start_run"
            | "restart_stage"
            | "restart_current_stage"
            | "run_step"
            | "run_current_step"
            | "patch_stage_state"
            | "patch_transient_stage_user_input"
    )
}

pub async fn validate_workflow_action_request(
    state: &AppState,
    run_id: Uuid,
    action: &str,
    requested_step_id: Option<&str>,
    preconditions: WorkflowActionPreconditions<'_>,
) -> Result<()> {
    let run = load_run(state, run_id).await?;
    let current_status = run_status_name(run.status);

    if let Some(expected_status) = preconditions
        .expected_status
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if current_status != expected_status {
            return Err(anyhow!(
                "stale workflow action: expected status {}, current status is {}",
                expected_status,
                current_status
            ));
        }
    }

    if action_is_read_only(action) {
        return Ok(());
    }

    if matches!(run.status, RunStatus::Cancelled | RunStatus::Success) {
        return Err(anyhow!(
            "workflow action {} is not allowed while workflow status is {}",
            action,
            current_status
        ));
    }

    if matches!(run.status, RunStatus::Running) && !action_allows_running_workflow(action) {
        return Err(anyhow!(
            "workflow action {} is not allowed while another stage execution is running",
            action
        ));
    }

    if action_requires_waiting_workflow(action) && !matches!(run.status, RunStatus::Waiting) {
        return Err(anyhow!(
            "stale workflow action: {} requires waiting status, current status is {}",
            action,
            current_status
        ));
    }

    let expected_step_id = preconditions
        .expected_step_id
        .or(requested_step_id)
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if action_requires_current_step(action) {
        if let Some(expected_step_id) = expected_step_id {
            if run.current_step_id.as_deref() != Some(expected_step_id) {
                return Err(anyhow!(
                    "stale workflow action: expected current step {}, current step is {}",
                    expected_step_id,
                    run.current_step_id.as_deref().unwrap_or("<none>")
                ));
            }
        }
    }

    let expected_stage_execution_id = preconditions
        .expected_stage_execution_id
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let expected_capability_invocation_id = preconditions
        .expected_capability_invocation_id
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if expected_stage_execution_id.is_none() && expected_capability_invocation_id.is_none() {
        return Ok(());
    }

    let blocked_on = run
        .context
        .get("workflow_engine")
        .and_then(|value| value.get("run_state"))
        .and_then(|value| value.get("blocked_on"))
        .ok_or_else(|| anyhow!("stale workflow action: workflow is no longer blocked"))?;

    if let Some(expected_stage_execution_id) = expected_stage_execution_id {
        let active_stage_execution_id = blocked_on
            .get("stage_execution_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow!("active workflow block is missing stage_execution_id"))?;

        if active_stage_execution_id != expected_stage_execution_id {
            return Err(anyhow!(
                "stale workflow action: expected stage execution {}, active stage execution is {}",
                expected_stage_execution_id,
                active_stage_execution_id
            ));
        }
    }

    if let Some(expected_capability_invocation_id) = expected_capability_invocation_id {
        let active_capability_invocation_id = blocked_on
            .get("capability_invocation_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow!("active workflow block is missing capability_invocation_id"))?;

        if active_capability_invocation_id != expected_capability_invocation_id {
            return Err(anyhow!(
                "stale workflow action: expected capability invocation {}, active capability invocation is {}",
                expected_capability_invocation_id,
                active_capability_invocation_id
            ));
        }
    }

    Ok(())
}

pub(crate) async fn fail_runtime_workflow(
    state: &AppState,
    run_id: Uuid,
    kind: &str,
    message: &str,
) -> Result<()> {
    let mut run = load_run(state, run_id).await?;
    let previous_status = run_status_label(&run.status).to_string();
    let root = ensure_engine_root(&mut run.context);
    let run_state = root.entry("run_state".to_string()).or_insert_with(|| json!({}));
    let run_state_obj = run_state.as_object_mut().ok_or_else(|| anyhow!("run_state must be object"))?;
    let interrupted_checkpoint = run_state_obj.remove("blocked_on");
    run_state_obj.remove("pause_requested");
    run_state_obj.remove("cancel_requested");
    run_state_obj.remove("cancel_requested_at");
    run_state_obj.insert(
        "terminal_error".to_string(),
        json!({
            "kind": kind,
            "message": message,
            "restartable": true,
            "previous_status": previous_status,
            "process_session_id": state.process_session_id(),
            "interrupted_checkpoint": interrupted_checkpoint,
            "occurred_at": Utc::now().to_rfc3339()
        }),
    );
    persist_context(state, run_id, &run.context).await?;
    set_run_status(state, run_id, RunStatus::Error, run.current_step_id.as_deref()).await?;
    append_engine_event(
        state,
        run_id,
        run.current_step_id.as_deref(),
        "error",
        "workflow_runtime_interrupted",
        message,
        json!({
            "kind": kind,
            "restartable": true,
            "previous_status": previous_status,
            "process_session_id": state.process_session_id()
        }),
    )
    .await?;
    Ok(())
}

pub async fn resolve_operator_checkpoint(state: &AppState, run_id: Uuid, disposition: &str, selected_step_id: Option<&str>) -> Result<serde_json::Value> {
    let mut run = load_run(state, run_id).await?;
    let blocked_on = run
        .context
        .get("workflow_engine")
        .and_then(|v| v.get("run_state"))
        .and_then(|v| v.get("blocked_on"))
        .cloned()
        .ok_or_else(|| anyhow!("workflow is not waiting on operator checkpoint"))?;

    let blocked_kind = blocked_on
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if blocked_kind != "capability_user_input" && blocked_kind != "operator_checkpoint" {
        return Err(anyhow!("workflow is blocked on {}, not capability user input", blocked_kind));
    }

    let blocked_capability = blocked_on
        .get("capability")
        .and_then(Value::as_str)
        .unwrap_or("operator_checkpoint")
        .to_string();

    let stage_id = blocked_on
        .get("stage_id")
        .and_then(Value::as_str)
        .or(run.current_step_id.as_deref())
        .ok_or_else(|| anyhow!("operator checkpoint is missing stage_id"))?
        .to_string();
    let next_target = blocked_on
        .get("next_step_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_string());
    let resume_mode = blocked_on
        .get("resume_mode")
        .and_then(Value::as_str)
        .unwrap_or("manual")
        .to_string();
    let checkpoint_phase = blocked_on
        .get("phase")
        .and_then(Value::as_str)
        .unwrap_or("after_stage")
        .to_string();
    let stage_execution_id = match blocked_on
        .get("stage_execution_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
    {
        Some(value) => Some(value),
        None => latest_stage_execution_id_for_step(state, run_id, stage_id.as_str()).await?,
    };

    let capability_invocation_id = blocked_on
        .get("capability_invocation_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);


    let normalized_disposition = match disposition {
        "continue_auto" | "auto" | "autonomous" | "move_next" | "continue" => "continue_auto",
        "select_stage" | "select" | "continue_manual" | "manual" => "select_stage",
        "pause_error" | "pause" | "paused" => "pause_error",
        other => return Err(anyhow!("unsupported operator checkpoint disposition {}", other)),
    };

    let resume_mode = match normalized_disposition {
        "continue_auto" => "autonomous".to_string(),
        "select_stage" => "manual".to_string(),
        "pause_error" => "none".to_string(),
        _ => resume_mode,
    };

    {
        let root = ensure_engine_root(&mut run.context);
        let run_state = root.entry("run_state".to_string()).or_insert_with(|| json!({}));
        let run_state_obj = run_state.as_object_mut().ok_or_else(|| anyhow!("run_state must be object"))?;
        run_state_obj.remove("blocked_on");
    }
    persist_context(state, run_id, &run.context).await?;

    let checkpoint_ok = normalized_disposition != "pause_error";
    let checkpoint_status = if checkpoint_ok { "success" } else { "paused" };
    append_engine_event(
        state,
        run_id,
        Some(stage_id.as_str()),
        if checkpoint_ok { "info" } else { "warn" },
        format!("{}_completed", blocked_capability).as_str(),
        if checkpoint_ok { "Capability user input resolved." } else { "Capability user input paused workflow." },
        json!({
            "capability": blocked_capability,
            "ok": checkpoint_ok,
            "execution_state": "completed",
            "disposition": normalized_disposition,
            "result": {
                "ok": checkpoint_ok,
                "mode": "operator_checkpoint",
                "status": checkpoint_status,
                "needs_user_response": false,
                "summary": if checkpoint_ok { "Operator checkpoint approved." } else { "Operator checkpoint paused workflow." },
                "message": if checkpoint_ok { "Operator checkpoint approved." } else { "Operator checkpoint paused workflow." },
                "stage_id": stage_id,
                "disposition": normalized_disposition
            },
            "event_meta": event_meta(stage_execution_id.as_deref(), capability_invocation_id.as_deref(), None, false)
        }),
    ).await?;

    match normalized_disposition {
        "pause_error" => {
            if checkpoint_phase != "before_stage" {
                let definition = load_template_definition(state, &run)
                    .await?
                    .ok_or_else(|| anyhow!("run has no template definition"))?;
                run_stage_exit_hook_if_transitioning(
                    state,
                    run_id,
                    &definition,
                    Some(stage_id.as_str()),
                    None,
                )
                .await?;
            }

            append_disposition_stage_completion_event(
                state,
                run_id,
                stage_id.as_str(),
                stage_execution_id.as_deref(),
                true,
                "paused",
                if checkpoint_phase == "before_stage" {
                    "Stage paused before execution by operator checkpoint."
                } else {
                    "Stage paused by operator checkpoint."
                },
                None,
            )
            .await?;

            set_run_status(state, run_id, RunStatus::Paused, Some(stage_id.as_str())).await?;
            append_engine_event(
                state,
                run_id,
                Some(stage_id.as_str()),
                "info",
                "operator_checkpoint_resolved",
                "Operator paused workflow at checkpoint.",
                json!({
                    "disposition": "pause_error",
                    "stage_id": stage_id,
                    "phase": checkpoint_phase,
                    "status": "paused"
                }),
            )
            .await?;

            Ok(json!({
                "ok": true,
                "status": "paused",
                "disposition": "pause_error",
                "current_step_id": stage_id,
                "followup_action": "pause",
                "continue_workflow": false
            }))
        }
        "continue_auto" | "select_stage" => {
            let definition = load_template_definition(state, &run).await?
                .ok_or_else(|| anyhow!("run has no template definition"))?;
            let target = if checkpoint_phase == "before_stage" {
                Some(stage_id.clone())
            } else if normalized_disposition == "select_stage" {
                selected_step_id
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| anyhow!("selected_step_id is required for select_stage checkpoint disposition"))?
                    .into()
            } else {
                next_target.or_else(|| next_step_id(&definition, Some(stage_id.as_str())))
            };

            if target.is_none() {
                append_engine_event(
                    state,
                    run_id,
                    Some(stage_id.as_str()),
                    "info",
                    "operator_checkpoint_resolved",
                    "Operator checkpoint approved the final workflow stage; workflow completed successfully.",
                    json!({ "disposition": normalized_disposition, "stage_id": stage_id, "next_step_id": Value::Null }),
                ).await?;

                append_disposition_stage_completion_event(
                    state,
                    run_id,
                    stage_id.as_str(),
                    stage_execution_id.as_deref(),
                    true,
                    "complete",
                    "Final workflow stage completed successfully after operator checkpoint.",
                    None,
                ).await?;

                set_run_status(state, run_id, RunStatus::Success, Some(stage_id.as_str())).await?;
                crate::supervisor::handle_workflow_terminal_event(state, run_id, RunStatus::Success, Some(stage_id.as_str())).await?;
                return Ok(json!({
                    "ok": true,
                    "status": "complete",
                    "disposition": normalized_disposition,
                    "current_step_id": stage_id,
                    "next_step_id": Value::Null,
                    "followup_action": "complete_workflow"
                }));
            }

            let target = target.expect("target checked above");
            let target_step = current_step(&definition, &run, Some(target.as_str()))?.clone();

            if checkpoint_phase == "before_stage" {
                crate::engine::stages::record_stage_checkpoint_continue(
                    &mut run,
                    &target_step,
                    checkpoint_phase.as_str(),
                )?;
                persist_context(state, run_id, &run.context).await?;

                append_disposition_stage_completion_event(
                    state,
                    run_id,
                    stage_id.as_str(),
                    stage_execution_id.as_deref(),
                    true,
                    "continue_auto",
                    "QA entry checkpoint approved.",
                    Some(stage_id.as_str()),
                )
                .await?;

                set_current_step_waiting(state, run_id, stage_id.as_str()).await?;
                append_engine_event(
                    state,
                    run_id,
                    Some(stage_id.as_str()),
                    "info",
                    "operator_checkpoint_resolved",
                    "Operator approved starting the QA stage.",
                    json!({
                        "disposition": normalized_disposition,
                        "stage_id": stage_id,
                        "next_step_id": stage_id,
                        "phase": "before_stage"
                    }),
                )
                .await?;

                return Ok(json!({
                "ok": true,
                "accepted": true,
                "status": "waiting",
                "disposition": normalized_disposition,
                "current_step_id": stage_id,
                "next_step_id": stage_id,
                "followup_action": "continue_existing_runtime"
            }));
            }

            let source_step = current_step(&definition, &run, Some(stage_id.as_str()))?.clone();
            run_stage_exit_hook_if_transitioning(
                state,
                run_id,
                &definition,
                Some(source_step.id.as_str()),
                Some(target.as_str()),
            )
            .await?;
            append_engine_event(
                state,
                run_id,
                Some(stage_id.as_str()),
                "info",
                "operator_checkpoint_resolved",
                "Operator approved moving to the next stage after checkpoint.",
                json!({ "disposition": normalized_disposition, "stage_id": stage_id, "next_step_id": target.as_str() }),
            ).await?;

            let latest_run = load_run(state, run_id).await?;
            if run_pause_requested(&latest_run) {
                let mut paused_run = latest_run;
                clear_run_pause_requested(&mut paused_run);
                persist_context(state, run_id, &paused_run.context).await?;
                set_run_status(state, run_id, RunStatus::Paused, Some(stage_id.as_str())).await?;
                append_engine_event(
                    state,
                    run_id,
                    Some(stage_id.as_str()),
                    "info",
                    "run_paused_after_stage",
                    "Workflow run paused after the current stage completed.",
                    json!({}),
                ).await?;
                return Ok(json!({
                    "ok": true,
                    "status": "paused",
                    "disposition": "pause_error",
                    "current_step_id": stage_id,
                    "next_step_id": target,
                }));
            }

            append_disposition_stage_completion_event(
                state,
                run_id,
                stage_id.as_str(),
                stage_execution_id.as_deref(),
                true,
                normalized_disposition,
                "Stage completed after operator checkpoint.",
                Some(target.as_str()),
            ).await?;

            set_current_step_waiting(state, run_id, target.as_str()).await?;

            append_engine_event(
                state,
                run_id,
                Some(target.as_str()),
                "info",
                "operator_checkpoint_transition_committed",
                "Operator checkpoint transition committed before backend continuation.",
                json!({
                    "disposition": normalized_disposition,
                    "from_step_id": stage_id,
                    "current_step_id": target,
                    "resume_mode": resume_mode,
                    "auto_runnable": step_is_auto_runnable(&target_step),
                }),
            ).await?;

            Ok(json!({
                "ok": true,
                "accepted": true,
                "status": "waiting",
                "disposition": normalized_disposition,
                "current_step_id": target,
                "next_step_id": target,
                "resume_mode": resume_mode,
                "followup_action": "continue_existing_runtime"
            }))
        }
        other => Err(anyhow!("unsupported disposition {}", other)),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Manual,
    Autonomous,
    PrepareOnly,
}

struct PreparedStage {
    run: super::WorkflowRun,
    step: super::WorkflowStepDefinition,
    pause_message: Option<String>,
}

async fn prepare_stage_for_execution(
    state: &AppState,
    run_id: Uuid,
    requested_step_id: Option<&str>,
    mode: RunMode,
) -> Result<PreparedStage> {
    let mut run = load_run(state, run_id).await?;
    let definition = load_template_definition(state, &run).await?
        .ok_or_else(|| anyhow!("run has no template definition"))?;
    let target_step_id = current_step(&definition, &run, requested_step_id)?.id.clone();
    let previous_step_id = run.current_step_id.clone();
    let step = transition_to_step(
        state,
        run_id,
        &mut run,
        &definition,
        target_step_id.as_str(),
    )
    .await?;

    crate::engine::preprocess_run_for_current_stage(
        state,
        &mut run,
        previous_step_id.as_deref(),
    )
    .await?;

    let decisions = governance::before_stage(state, run_id, &mut run, &step).await?;
    governance::apply_context_mutations(
        &mut run,
        &decisions,
        Some(step.id.as_str()),
        None,
    )?;

    let pause_message = governance::pause_message(&decisions);
    let prepared_status = match mode {
        RunMode::Manual | RunMode::Autonomous => RunStatus::Running,
        RunMode::PrepareOnly => RunStatus::Waiting,
    };
    let prepared_status_label = run_status_label(&prepared_status);

    persist_context(state, run_id, &run.context).await?;
    set_run_status(state, run_id, prepared_status, Some(step.id.as_str())).await?;

    let refreshed_run = load_run(state, run_id).await?;

    let prepared_global_state = refreshed_run
        .context
        .get("workflow_engine")
        .and_then(|value| value.get("global_state"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let prepared_stage_overrides = refreshed_run
        .context
        .get("workflow_engine")
        .and_then(|value| value.get("stage_overrides"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    append_engine_event(
        state,
        run_id,
        Some(step.id.as_str()),
        "info",
        "stage_prepared_for_execution",
        "Governance prepared stage before execution.",
        json!({
            "step_id": step.id,
            "step_type": step.step_type,
            "current_step_id": step.id,
            "status": prepared_status_label,
            "prepared_status": prepared_status_label,
            "run_mode": match mode {
                RunMode::Manual => "manual",
                RunMode::Autonomous => "autonomous",
                RunMode::PrepareOnly => "prepare_only",
            },
            "paused_by_governance": pause_message.is_some(),
            "prepared_context": refreshed_run.context.clone(),
            "prepared_global_state": prepared_global_state,
            "prepared_stage_overrides": prepared_stage_overrides
        }),
    ).await?;

    Ok(PreparedStage {
        run: refreshed_run,
        step,
        pause_message,
    })
}

pub async fn prepare_run_stage_for_execution(state: &AppState, run_id: Uuid, requested_step_id: Option<&str>) -> Result<serde_json::Value> {
    let prepared = prepare_stage_for_execution(
        state,
        run_id,
        requested_step_id,
        RunMode::PrepareOnly,
    ).await?;

    Ok(json!({
        "ok": prepared.pause_message.is_none(),
        "status": "waiting",
        "prepared": true,
        "current_step_id": prepared.step.id,
        "step_id": prepared.step.id,
        "step_type": prepared.step.step_type,
        "message": prepared.pause_message,
        "run": prepared.run
    }))
}

pub async fn run_step(state: &AppState, run_id: Uuid, requested_step_id: Option<&str>) -> Result<serde_json::Value> {
    let run = load_run(state, run_id).await?;

    if matches!(run.status, RunStatus::Running | RunStatus::Queued) {
        let requested_step_id = requested_step_id
            .map(str::to_string)
            .or_else(|| run.current_step_id.clone());

        append_engine_event(
            state,
            run_id,
            requested_step_id.as_deref(),
            "warn",
            "duplicate_stage_execution_rejected",
            "Stage execution request was rejected because the workflow is already active.",
            json!({
                "current_step_id": run.current_step_id,
                "requested_step_id": requested_step_id,
                "status": run_status_label(&run.status)
            }),
        )
        .await?;

        return Ok(json!({
            "ok": false,
            "status": run_status_label(&run.status),
            "already_running": true,
            "current_step_id": run.current_step_id,
            "requested_step_id": requested_step_id,
            "message": "The workflow is already executing a stage."
        }));
    }

    if run_is_waiting_on_operator_checkpoint(&run) {
        return Ok(json!({
            "ok": false,
            "status": "waiting",
            "blocked_on": "operator_checkpoint",
            "current_step_id": run.current_step_id,
            "message": "Resolve the active operator checkpoint before starting another stage execution."
        }));
    }

    run_stages(state, run_id, requested_step_id, RunMode::Manual).await
}

pub async fn restart_stage(
    state: &AppState,
    run_id: Uuid,
    requested_step_id: Option<&str>,
) -> Result<serde_json::Value> {
    let mut run = load_run(state, run_id).await?;
    let definition = load_template_definition(state, &run)
        .await?
        .ok_or_else(|| anyhow!("run has no template definition"))?;
    let step = current_step(&definition, &run, requested_step_id)?.clone();

    let blocked_checkpoint = run
        .context
        .get("workflow_engine")
        .and_then(|value| value.get("run_state"))
        .and_then(|value| value.get("blocked_on"))
        .filter(|blocked| {
            blocked.get("kind").and_then(Value::as_str)
                == Some("operator_checkpoint")
                && blocked.get("stage_id").and_then(Value::as_str)
                    == Some(step.id.as_str())
                && blocked.get("phase").and_then(Value::as_str)
                    == Some("after_stage")
        })
        .is_some();

    if run_is_waiting_on_operator_checkpoint(&run) && !blocked_checkpoint {
        return Ok(json!({
            "ok": false,
            "status": "waiting",
            "blocked_on": "operator_checkpoint",
            "current_step_id": run.current_step_id,
            "message": "The active operator checkpoint does not belong to this stage runtime."
        }));
    }

    if blocked_checkpoint {
        clear_pending_capability_user_input(&mut run);
        persist_context(state, run_id, &run.context).await?;
    }

    super::stages::invoke_stage_restart_hook(state, run_id, &step).await?;
    set_run_status(
        state,
        run_id,
        RunStatus::Waiting,
        Some(step.id.as_str()),
    )
    .await?;

    run_stages(
        state,
        run_id,
        Some(step.id.as_str()),
        RunMode::Manual,
    )
    .await
}

async fn start_or_resume_automatic_run(state: &AppState, run_id: Uuid, requested_step_id: Option<&str>) -> Result<serde_json::Value> {
    let run = load_run(state, run_id).await?;
    let definition = load_template_definition(state, &run).await?
        .ok_or_else(|| anyhow!("run has no template definition"))?;

    let step = current_step(&definition, &run, requested_step_id)?.clone();
    append_engine_event(
        state,
        run_id,
        Some(step.id.as_str()),
        "info",
        "automatic_run_started",
        "Autonomous run entered current stage runtime.",
        json!({
            "step_id": step.id,
            "step_type": step.step_type,
            "automation_mode": format_automation_mode(&step),
            "auto_runnable": step_is_auto_runnable(&step),
        }),
    ).await?;

    Box::pin(run_stages(state, run_id, requested_step_id, RunMode::Autonomous)).await
}

fn step_is_auto_runnable(step: &super::WorkflowStepDefinition) -> bool {
    step.advancement.auto_run_on_enter
        || matches!(step.automation_mode, crate::models::AutomationMode::Automatic)
}

fn format_automation_mode(step: &super::WorkflowStepDefinition) -> &'static str {
    match step.automation_mode {
        crate::models::AutomationMode::Manual => "manual",
        crate::models::AutomationMode::Assisted => "assisted",
        crate::models::AutomationMode::Automatic => "automatic",
    }
}

fn run_pause_requested(run: &super::WorkflowRun) -> bool {
    run.context
        .get("workflow_engine")
        .and_then(|v| v.get("run_state"))
        .and_then(|v| v.get("pause_requested"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn clear_run_pause_requested(run: &mut super::WorkflowRun) {
    let root = ensure_engine_root(&mut run.context);
    if let Some(run_state) = root.get_mut("run_state").and_then(|v| v.as_object_mut()) {
        run_state.remove("pause_requested");
    }
}

fn run_cancel_requested(run: &super::WorkflowRun) -> bool {
    run.context
        .get("workflow_engine")
        .and_then(|v| v.get("run_state"))
        .and_then(|v| v.get("cancel_requested"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn clear_run_cancel_requested(run: &mut super::WorkflowRun) {
    let root = ensure_engine_root(&mut run.context);
    if let Some(run_state) = root.get_mut("run_state").and_then(|v| v.as_object_mut()) {
        run_state.remove("cancel_requested");
        run_state.remove("cancel_requested_at");
    }
}

fn capability_user_input_result(outcome: &super::stages::StageOutcome) -> Option<Value> {
    outcome.capability_results.iter().find_map(|item| {
        let capability = item
            .get("key")
            .or_else(|| item.get("capability"))
            .and_then(Value::as_str)?;
        let mut result = item
            .get("result")
            .or_else(|| item.get("payload"))?
            .clone();

        if result.get("needs_user_response").and_then(Value::as_bool) != Some(true) {
            return None;
        }

        if let Some(result_obj) = result.as_object_mut() {
            result_obj
                .entry("capability".to_string())
                .or_insert_with(|| Value::String(capability.to_string()));
        }

        Some(result)
    })
}

fn capability_user_input_options(
    outcome: &super::stages::StageOutcome,
) -> Value {
    normalize_capability_user_input_options(
        capability_user_input_result(outcome)
            .and_then(|user_input| user_input.get("available_dispositions").cloned()),
    )
}

fn normalize_capability_user_input_options(configured: Option<Value>) -> Value {
    let options = configured
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| match value.as_str() {
            Some("continue_auto") | Some("auto") | Some("autonomous") | Some("move_next") | Some("continue") => Some(json!("continue_auto")),
            Some("select_stage") | Some("select") | Some("continue_manual") | Some("manual") => Some(json!("select_stage")),
            Some("pause_error") | Some("pause") | Some("paused") => Some(json!("pause_error")),
            _ => None,
        })
        .collect::<Vec<Value>>();

    if options.is_empty() {
        json!(["continue_auto", "pause_error", "select_stage"])
    } else {
        Value::Array(options)
    }
}

fn clear_pending_capability_user_input(run: &mut super::WorkflowRun) {
    let root = ensure_engine_root(&mut run.context);
    if let Some(run_state) = root.get_mut("run_state").and_then(|v| v.as_object_mut()) {
        run_state.remove("blocked_on");
    }
}

async fn set_current_step_waiting(state: &AppState, run_id: Uuid, step_id: &str) -> Result<()> {
    let mut run = load_run(state, run_id).await?;
    let definition = load_template_definition(state, &run).await?
        .ok_or_else(|| anyhow!("run has no template definition"))?;
    let step = current_step(&definition, &run, Some(step_id))?;

    run.current_step_id = Some(step.id.clone());
    run.status = RunStatus::Waiting;

    persist_context(state, run_id, &run.context).await?;
    set_run_status(state, run_id, RunStatus::Waiting, Some(step.id.as_str())).await?;

    Ok(())
}

fn set_pending_capability_user_input(
    run: &mut super::WorkflowRun,
    step: &super::WorkflowStepDefinition,
    outcome: &super::stages::StageOutcome,
    next_target: Option<String>,
    resume_mode: &str,
    process_session_id: &str,
) {
    let root = ensure_engine_root(&mut run.context);
    let run_state = root.entry("run_state".to_string()).or_insert_with(|| json!({}));
    let run_state_obj = run_state.as_object_mut().expect("run_state must be object");
    let stage_execution_id = outcome
        .local_state
        .get("_stage_execution_id")
        .and_then(Value::as_str)
        .unwrap_or("");

    let user_input = capability_user_input_result(outcome).unwrap_or_else(|| json!({}));
    run_state_obj.insert("blocked_on".to_string(), json!({
        "kind": "capability_user_input",
        "capability": user_input
            .get("capability")
            .and_then(Value::as_str)
            .unwrap_or("operator_checkpoint"),
        "process_session_id": process_session_id,
        "stage_id": step.id,
        "stage_type": step.step_type,
        "stage_execution_id": stage_execution_id,
        "capability_invocation_id": user_input.get("_capability_invocation_id").and_then(Value::as_str).unwrap_or(""),
        "phase": user_input.get("phase").and_then(Value::as_str).unwrap_or("after_stage"),
        "recommended_disposition": user_input
            .get("recommended_disposition")
            .and_then(Value::as_str)
            .unwrap_or("continue_auto"),
        "available_dispositions": capability_user_input_options(outcome),
        "next_step_id": next_target,
        "message": user_input
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or(outcome.message.as_str()),
        "resume_mode": resume_mode
    }));
}

pub(crate) async fn run_workflow_runtime(
    state: &AppState,
    run_id: Uuid,
    mut command_rx: tokio::sync::mpsc::Receiver<crate::engine::workflow_lifecycle::WorkflowRuntimeCommand>,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<()> {
    use crate::engine::workflow_lifecycle::{WorkflowExecutionMode, WorkflowRuntimeCommand};

    let mut active_mode = None::<WorkflowExecutionMode>;
    let mut requested_step_id = None::<String>;
    let mut paused = false;

    loop {
        if active_mode.is_none() || paused {
            tokio::select! {
                _ = cancellation.cancelled() => {
                    fail_runtime_workflow(state, run_id, "runtime_cancelled", "Workflow runtime was cancelled.").await?;
                    return Ok(());
                }
                command = command_rx.recv() => {
                    let Some(command) = command else {
                        fail_runtime_workflow(state, run_id, "runtime_channel_closed", "Workflow runtime command channel closed.").await?;
                        return Ok(());
                    };

                    match command {
                        WorkflowRuntimeCommand::Start { mode, step_id } => {
                            active_mode = Some(mode);
                            requested_step_id = step_id;
                            paused = false;
                        }
                        WorkflowRuntimeCommand::Resume => {
                            paused = false;
                            let run = load_run(state, run_id).await?;
                            if active_mode.is_none() {
                                active_mode = Some(WorkflowExecutionMode::MultiStage);
                                requested_step_id = run.current_step_id;
                            }
                            set_run_status(state, run_id, RunStatus::Waiting, requested_step_id.as_deref()).await?;
                        }
                        WorkflowRuntimeCommand::MoveTo { step_id } => {
                            select_step(state, run_id, step_id.as_str()).await?;
                            requested_step_id = Some(step_id);
                        }
                        WorkflowRuntimeCommand::Cancel { reason } => {
                            fail_runtime_workflow(state, run_id, reason.as_str(), "Workflow execution was cancelled.").await?;
                            return Ok(());
                        }
                        WorkflowRuntimeCommand::Archive => {
                            force_wait_run(state, run_id).await?;
                            return Ok(());
                        }
                        WorkflowRuntimeCommand::Pause => {
                            paused = true;
                            let run = load_run(state, run_id).await?;
                            set_run_status(state, run_id, RunStatus::Paused, run.current_step_id.as_deref()).await?;
                        }
                        WorkflowRuntimeCommand::ResolveCheckpoint { disposition, selected_step_id } => {
                            resolve_operator_checkpoint(state, run_id, disposition.as_str(), selected_step_id.as_deref()).await?;
                            paused = disposition == "pause_error" || disposition == "pause" || disposition == "paused";
                        }
                    }
                }
            }

            continue;
        }

        let execution_mode = active_mode.expect("active execution mode checked");
        let mode = match execution_mode {
            WorkflowExecutionMode::SingleStage => RunMode::Manual,
            WorkflowExecutionMode::MultiStage => RunMode::Autonomous,
        };

        let execution_step_id = requested_step_id.take();
        let execution = run_stages(state, run_id, execution_step_id.as_deref(), mode);
        tokio::pin!(execution);

        let result = loop {
            tokio::select! {
                _ = cancellation.cancelled() => {
                    fail_runtime_workflow(state, run_id, "runtime_cancelled", "Workflow runtime was cancelled.").await?;
                    return Ok(());
                }
                command = command_rx.recv() => {
                    let Some(command) = command else {
                        fail_runtime_workflow(state, run_id, "runtime_channel_closed", "Workflow runtime command channel closed.").await?;
                        return Ok(());
                    };

                    match command {
                        WorkflowRuntimeCommand::Pause => {
                            pause_run(state, run_id).await?;
                        }
                        WorkflowRuntimeCommand::Cancel { reason } => {
                            fail_runtime_workflow(state, run_id, reason.as_str(), "Workflow execution was cancelled.").await?;
                            return Ok(());
                        }
                        WorkflowRuntimeCommand::ResolveCheckpoint { disposition, selected_step_id } => {
                            resolve_operator_checkpoint(state, run_id, disposition.as_str(), selected_step_id.as_deref()).await?;
                        }
                        WorkflowRuntimeCommand::MoveTo { step_id } => {
                            select_step(state, run_id, step_id.as_str()).await?;
                            requested_step_id = Some(step_id);
                        }
                        WorkflowRuntimeCommand::Start { mode, step_id } => {
                            active_mode = Some(mode);
                            requested_step_id = step_id;
                        }
                        WorkflowRuntimeCommand::Resume => {}
                        WorkflowRuntimeCommand::Archive => {
                            force_wait_run(state, run_id).await?;
                            return Ok(());
                        }
                    }
                }
                result = &mut execution => break result,
            }
        }?;

        let status = result.get("status").and_then(Value::as_str).unwrap_or("waiting");
        match status {
            "complete" | "success" | "error" | "cancelled" => return Ok(()),
            "paused" => paused = true,
            "waiting" => {
                if execution_mode == WorkflowExecutionMode::SingleStage {
                    active_mode = None;
                } else {
                    let run = load_run(state, run_id).await?;
                    let blocked_on_checkpoint = run
                        .context
                        .get("workflow_engine")
                        .and_then(|value| value.get("run_state"))
                        .and_then(|value| value.get("blocked_on"))
                        .is_some();
                    if blocked_on_checkpoint {
                        paused = true;
                    } else {
                        requested_step_id = run.current_step_id;
                    }
                }
            }
            _ => {
                if execution_mode == WorkflowExecutionMode::SingleStage {
                    active_mode = None;
                }
            }
        }
    }
}

async fn run_stages(state: &AppState, run_id: Uuid, requested_step_id: Option<&str>, mode: RunMode) -> Result<serde_json::Value> {
    let mut last_payload = json!({ "ok": true, "status": "waiting" });
    let mut requested = requested_step_id.map(|s| s.to_string());
    let mut hops = 0usize;

    loop {
        let automatic = matches!(mode, RunMode::Autonomous);
        if automatic {
            hops += 1;
        }
        if automatic && hops > 50 {
            let run = load_run(state, run_id).await?;
            set_run_status(state, run_id, RunStatus::Waiting, run.current_step_id.as_deref()).await?;
            append_engine_event(
                state,
                run_id,
                run.current_step_id.as_deref(),
                "warn",
                "automatic_run_stopped",
                "Automatic run stopped after too many stage transitions.",
                json!({ "max_hops": 50 }),
            ).await?;
            return Ok(json!({
                "ok": false,
                "status": "waiting",
                "current_step_id": run.current_step_id,
                "message": "Automatic run stopped after too many stage transitions."
            }));
        }

        let latest_run = load_run(state, run_id).await?;
        if run_cancel_requested(&latest_run) {
            let mut cancelled_run = latest_run;
            clear_run_cancel_requested(&mut cancelled_run);
            clear_pending_capability_user_input(&mut cancelled_run);
            persist_context(state, run_id, &cancelled_run.context).await?;
            set_run_status(state, run_id, RunStatus::Waiting, cancelled_run.current_step_id.as_deref()).await?;
            append_engine_event(
                state,
                run_id,
                cancelled_run.current_step_id.as_deref(),
                "warn",
                "run_cancelled",
                "Workflow run stopped because operator cancellation was requested.",
                json!({ "current_step_id": cancelled_run.current_step_id }),
            ).await?;
            return Ok(json!({
                "ok": true,
                "status": "waiting",
                "cancelled": true,
                "current_step_id": cancelled_run.current_step_id
            }));
        }

        let prepared = prepare_stage_for_execution(
            state,
            run_id,
            requested.take().as_deref(),
            mode,
        ).await?;
        let mut run = prepared.run;
        let step = prepared.step;
        let definition = load_template_definition(state, &run).await?
            .ok_or_else(|| anyhow!("run has no template definition"))?;

        if let Some(message) = prepared.pause_message {
            return Ok(json!({
                "ok": false,
                "status": "waiting",
                "current_step_id": step.id,
                "message": message,
                "disposition": "paused"
            }));
        }

        let outcome = execute_stage(state, run_id, &mut run, &step, automatic).await?;
        let next_target = resolve_next_target(&definition, &step, &outcome);
        let latest_run = load_run(state, run_id).await?;
        if run_cancel_requested(&latest_run) {
            let mut cancelled_run = latest_run;
            clear_run_cancel_requested(&mut cancelled_run);
            clear_pending_capability_user_input(&mut cancelled_run);
            persist_context(state, run_id, &cancelled_run.context).await?;
            set_run_status(state, run_id, RunStatus::Waiting, cancelled_run.current_step_id.as_deref()).await?;
            append_engine_event(
                state,
                run_id,
                cancelled_run.current_step_id.as_deref(),
                "warn",
                "run_cancelled_after_stage",
                "Workflow run stopped after current stage because operator cancellation was requested.",
                json!({
                    "step_id": step.id,
                    "current_step_id": cancelled_run.current_step_id
                }),
            ).await?;
            return Ok(json!({
                "ok": true,
                "status": "waiting",
                "cancelled": true,
                "step_id": step.id,
                "current_step_id": cancelled_run.current_step_id
            }));
        }

        if matches!(
            outcome.disposition,
            StageDisposition::Success
                | StageDisposition::MoveNext
                | StageDisposition::MoveBack
                | StageDisposition::Outcome(_)
                | StageDisposition::Stay
        ) {
        }

        let pending_capability_user_input = capability_user_input_result(&outcome).is_some();

        if pending_capability_user_input {
            clear_pending_capability_user_input(&mut run);
            let disposition_resume_mode = if automatic { "autonomous" } else { "manual" };
            set_pending_capability_user_input(&mut run, &step, &outcome, next_target.clone(), disposition_resume_mode, state.process_session_id());
            persist_context(state, run_id, &run.context).await?;
            set_run_status(state, run_id, RunStatus::Waiting, Some(step.id.as_str())).await?;
            append_engine_event(
                state,
                run_id,
                Some(step.id.as_str()),
                "info",
                "workflow_execution_state_changed",
                "Workflow is awaiting capability user input.",
                json!({
                    "execution_state": "awaiting_user_input",
                    "stage_id": step.id,
                    "stage_type": step.step_type,
                    "recommended_disposition": format_disposition(&outcome.disposition),
                    "available_dispositions": capability_user_input_options(&outcome),
                    "next_step_id": next_target.clone(),
                    "resume_mode": disposition_resume_mode,
                }),
            ).await?;
            if !automatic {
                return Ok(json!({
                    "ok": outcome.ok,
                    "status": "waiting",
                    "blocked_on": "capability_user_input",
                    "step_id": step.id,
                    "next_step_id": next_target,
                    "message": outcome.message,
                    "disposition": format_disposition(&outcome.disposition),
                    "capability_results": outcome.capability_results,
                    "local_state": outcome.local_state,
                }));
            }

            return Ok(json!({
                "ok": outcome.ok,
                "status": "waiting",
                "blocked_on": "capability_user_input",
                "step_id": step.id,
                "next_step_id": next_target,
                "message": outcome.message,
                "disposition": format_disposition(&outcome.disposition),
                "capability_results": outcome.capability_results,
                "local_state": outcome.local_state,
            }));
        }

        append_engine_event(
            state,
            run_id,
            Some(step.id.as_str()),
            if outcome.ok { "info" } else { "error" },
            "stage_executed",
            &outcome.message,
            json!({
                "disposition": format_disposition(&outcome.disposition),
                "capability_results": outcome.capability_results,
                "local_state": outcome.local_state,
                "final_context": run.context.clone(),
            }),
        ).await?;

        clear_pending_capability_user_input(&mut run);
        let auto_advance = automatic && should_auto_advance(&step, &outcome);
        let latest_run = load_run(state, run_id).await?;
        if run_pause_requested(&latest_run) {
            clear_run_pause_requested(&mut run);
            {
                let root = ensure_engine_root(&mut run.context);
                let run_state = root.entry("run_state".to_string()).or_insert_with(|| json!({}));
                let run_state_obj = run_state.as_object_mut().ok_or_else(|| anyhow!("run_state must be object"))?;
                run_state_obj.insert("blocked_on".to_string(), json!({
                    "kind": "pause_after_stage",
                    "requires_user_release": true,
                    "stage_id": step.id,
                    "stage_type": step.step_type,
                    "next_step_id": next_target,
                    "message": "Workflow paused after stage and requires explicit user resume before autonomous progression can continue."
                }));
            }
            persist_context(state, run_id, &run.context).await?;
            set_run_status(state, run_id, RunStatus::Paused, Some(step.id.as_str())).await?;
            append_engine_event(
                state,
                run_id,
                Some(step.id.as_str()),
                "info",
                "run_paused_after_stage",
                "Workflow run paused after the current stage completed and now requires explicit user resume.",
                json!({
                    "blocked_on": "pause_after_stage",
                    "requires_user_release": true,
                    "next_step_id": next_target
                }),
            ).await?;
            return Ok(json!({
                "ok": true,
                "status": "paused",
                "blocked_on": "pause_after_stage",
                "requires_user_release": true,
                "step_id": step.id,
                "next_step_id": next_target,
                "message": outcome.message,
            }));
        }
        persist_context(state, run_id, &run.context).await?;

        if matches!(mode, RunMode::Manual) {
            let status = match outcome.disposition {
                StageDisposition::Paused => RunStatus::Paused,
                StageDisposition::Error | StageDisposition::ErrorCode(_) => RunStatus::Error,
                StageDisposition::Success
                | StageDisposition::RetryStage
                | StageDisposition::MoveNext
                | StageDisposition::MoveBack
                | StageDisposition::Outcome(_)
                | StageDisposition::Stay => RunStatus::Waiting,
            };
            let current_step_id = next_target.as_deref().or(Some(step.id.as_str()));
            run_stage_exit_hook_if_transitioning(
                state,
                run_id,
                &definition,
                Some(step.id.as_str()),
                current_step_id,
            )
            .await?;
            set_run_status(state, run_id, status.clone(), current_step_id).await?;
            append_engine_event(
                state,
                run_id,
                current_step_id,
                "info",
                "run_status_changed",
                "Workflow run advanced after stage completion.",
                json!({
                    "status": match status {
                        RunStatus::Draft => "waiting",
                        RunStatus::Queued => "queued",
                        RunStatus::Running => "running",
                        RunStatus::Waiting => "waiting",
                        RunStatus::Paused => "paused",
                        RunStatus::Success => "complete",
                        RunStatus::Error => "error",
                        RunStatus::Cancelled => "cancelled",
                    },
                    "completed_step_id": step.id,
                    "current_step_id": current_step_id,
                    "next_step_id": next_target,
                    "event_meta": {
                        "is_header_event": true
                    }
                }),
            )
            .await?;

            return Ok(json!({
                "ok": outcome.ok,
                "status": match status {
                    RunStatus::Draft => "waiting",
                    RunStatus::Queued => "queued",
                    RunStatus::Running => "running",
                    RunStatus::Waiting => "waiting",
                    RunStatus::Paused => "paused",
                    RunStatus::Success => "complete",
                    RunStatus::Error => "error",
                    RunStatus::Cancelled => "cancelled",
                },
                "step_id": step.id,
                "next_step_id": next_target,
                "message": outcome.message,
                "disposition": format_disposition(&outcome.disposition),
                "capability_results": outcome.capability_results,
                "local_state": outcome.local_state,
            }));
        }

        run_stage_exit_hook_if_transitioning(
            state,
            run_id,
            &definition,
            Some(step.id.as_str()),
            next_target.as_deref(),
        )
        .await?;

        match (&outcome.disposition, next_target.clone(), auto_advance) {
            (StageDisposition::Success, Some(target), true) => {
                set_run_status(state, run_id, RunStatus::Running, Some(target.as_str())).await?;
                last_payload = json!({
                    "ok": true,
                    "status": "running",
                    "step_id": step.id,
                    "next_step_id": target,
                    "message": outcome.message,
                });
                requested = next_target;
                continue;
            }
            (StageDisposition::RetryStage, Some(target), _) => {
                set_run_status(state, run_id, RunStatus::Running, Some(target.as_str())).await?;
                last_payload = json!({
                    "ok": outcome.ok,
                    "status": "running",
                    "step_id": step.id,
                    "next_step_id": target,
                    "message": outcome.message,
                    "disposition": "retry_stage"
                });
                requested = Some(target);
                continue;
            }
            (StageDisposition::MoveNext, Some(target), _) | (StageDisposition::MoveBack, Some(target), _) => {
                set_run_status(state, run_id, RunStatus::Running, Some(target.as_str())).await?;
                last_payload = json!({
                    "ok": outcome.ok,
                    "status": "running",
                    "step_id": step.id,
                    "next_step_id": target,
                    "message": outcome.message,
                });
                requested = next_target;
                continue;
            }
            (StageDisposition::MoveNext, None, _) => {
                set_run_status(state, run_id, RunStatus::Success, Some(step.id.as_str())).await?;
                crate::supervisor::handle_workflow_terminal_event(state, run_id, RunStatus::Success, Some(step.id.as_str())).await?;
                return Ok(json!({
                    "ok": outcome.ok,
                    "status": "complete",
                    "step_id": step.id,
                    "message": outcome.message,
                }));
            }
            (StageDisposition::Success, Some(target), false) => {
                set_run_status(state, run_id, RunStatus::Waiting, Some(target.as_str())).await?;
                return Ok(json!({
                    "ok": true,
                    "status": "waiting",
                    "step_id": step.id,
                    "next_step_id": target,
                    "message": outcome.message,
                }));
            }
            (StageDisposition::Success, None, _) => {
                set_run_status(state, run_id, RunStatus::Success, Some(step.id.as_str())).await?;
                crate::supervisor::handle_workflow_terminal_event(state, run_id, RunStatus::Success, Some(step.id.as_str())).await?;
                return Ok(json!({
                    "ok": true,
                    "status": "complete",
                    "step_id": step.id,
                    "message": outcome.message,
                }));
            }
            (StageDisposition::Paused, Some(target), false) => {
                set_run_status(state, run_id, RunStatus::Waiting, Some(target.as_str())).await?;
                return Ok(json!({
                    "ok": true,
                    "status": "waiting",
                    "step_id": step.id,
                    "next_step_id": target,
                    "message": outcome.message,
                }));
            }
            (StageDisposition::Paused, _, _) => {
                set_run_status(state, run_id, RunStatus::Waiting, Some(step.id.as_str())).await?;
                return Ok(json!({
                    "ok": true,
                    "status": "waiting",
                    "step_id": step.id,
                    "message": outcome.message,
                }));
            }
            (StageDisposition::Error, Some(target), true) | (StageDisposition::ErrorCode(_), Some(target), true) => {
                set_run_status(state, run_id, RunStatus::Running, Some(target.as_str())).await?;
                last_payload = json!({
                    "ok": false,
                    "status": "running",
                    "step_id": step.id,
                    "next_step_id": target,
                    "message": outcome.message,
                });
                requested = next_target;
                continue;
            }
            (StageDisposition::Error, _, _) | (StageDisposition::ErrorCode(_), _, _) => {
                set_run_status(state, run_id, RunStatus::Error, Some(step.id.as_str())).await?;
                crate::supervisor::handle_workflow_terminal_event(state, run_id, RunStatus::Error, Some(step.id.as_str())).await?;
                return Ok(json!({
                    "ok": false,
                    "status": "error",
                    "step_id": step.id,
                    "message": outcome.message,
                }));
            }
            (StageDisposition::Outcome(_), Some(target), false) => {
                set_run_status(state, run_id, RunStatus::Waiting, Some(target.as_str())).await?;
                return Ok(json!({
                    "ok": outcome.ok,
                    "status": "waiting",
                    "step_id": step.id,
                    "next_step_id": target,
                    "message": outcome.message,
                }));
            }
            (StageDisposition::Outcome(_), Some(target), true) => {
                set_run_status(state, run_id, RunStatus::Running, Some(target.as_str())).await?;
                requested = Some(target);
                continue;
            }
            (StageDisposition::Stay, _, _) => {
                set_run_status(state, run_id, RunStatus::Waiting, Some(step.id.as_str())).await?;
                return Ok(json!({
                    "ok": outcome.ok,
                    "status": "waiting",
                    "step_id": step.id,
                    "message": outcome.message,
                }));
            }
            _ => {
                set_run_status(state, run_id, RunStatus::Waiting, Some(step.id.as_str())).await?;
                return Ok(json!({
                    "ok": outcome.ok,
                    "status": "waiting",
                    "step_id": step.id,
                    "message": outcome.message,
                }));
            }
        }
    }
}

fn format_disposition(disposition: &StageDisposition) -> String {
    match disposition {
        StageDisposition::Success => "success".to_string(),
        StageDisposition::Error => "error".to_string(),
        StageDisposition::ErrorCode(code) => format!("error_code:{}", code),
        StageDisposition::Paused => "paused".to_string(),
        StageDisposition::RetryStage => "retry_stage".to_string(),
        StageDisposition::MoveNext => "move_next".to_string(),
        StageDisposition::MoveBack => "move_back".to_string(),
        StageDisposition::Outcome(name) => format!("outcome:{}", name),
        StageDisposition::Stay => "stay".to_string(),
    }
}
