use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::{automation, refresh_inference_arm_state},
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
use super::stages::{execute_stage, StageStatus, StageTransition};
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
    run
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
            "status": "running",
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
    let disposition_review_waiting = run_is_waiting_on_operator_checkpoint(&run);
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

    request_run_pause_after_stage(&mut run)?;
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

fn action_allows_completed_workflow(action: &str) -> bool {
    matches!(
        action,
        "select_step"
            | "patch_transient_stage_user_input"
            | "patch_stage_state"
            | "patch_global_state"
            | "prepare_stage"
            | "prepare_current_stage"
            | "start_run"
            | "restart_stage"
            | "restart_current_stage"
            | "run_step"
            | "run_current_step"
    )
}

fn action_allows_running_workflow(action: &str) -> bool {
    matches!(
        action,
        "pause_run"
            | "cancel_run"
            | "force_wait_run"
            | "force_unlock_run"
            | "force_complete_stage"
            | "resolve_operator_checkpoint"
    )
}

fn action_requires_operator_checkpoint(action: &str) -> bool {
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

    if matches!(run.status, RunStatus::Cancelled)
        || matches!(run.status, RunStatus::Success) && !action_allows_completed_workflow(action)
    {
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

    if action_requires_operator_checkpoint(action) && !run_is_waiting_on_operator_checkpoint(&run) {
        return Err(anyhow!(
            "stale workflow action: {} requires an active operator checkpoint",
            action
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

pub async fn resolve_operator_checkpoint(
    state: &AppState,
    run_id: Uuid,
    disposition: &str,
    selected_step_id: Option<&str>,
) -> Result<serde_json::Value> {
    let run = load_run(state, run_id).await?;
    let blocked_on = run
        .context
        .get("workflow_engine")
        .and_then(|value| value.get("run_state"))
        .and_then(|value| value.get("blocked_on"))
        .ok_or_else(|| anyhow!("workflow is not waiting on operator input"))?;

    if blocked_on.get("kind").and_then(Value::as_str) != Some("operator_checkpoint") {
        return Err(anyhow!("workflow is not waiting on an operator checkpoint"));
    }

    let stage_id = blocked_on
        .get("stage_id")
        .and_then(Value::as_str)
        .or(run.current_step_id.as_deref())
        .ok_or_else(|| anyhow!("operator checkpoint is missing stage_id"))?
        .to_string();

    let normalized_disposition = match disposition {
        "continue_auto" | "auto" | "autonomous" | "move_next" | "continue" => "continue_auto",
        "select_stage" | "select" | "continue_manual" | "manual" => "select_stage",
        "pause_error" | "pause" | "paused" | "complete" => "pause_error",
        other => return Err(anyhow!("unsupported operator checkpoint disposition {}", other)),
    };

    let selected_step_id = if normalized_disposition == "select_stage" {
        Some(
            selected_step_id
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| anyhow!("selected_step_id is required when selecting a workflow stage"))?
                .to_string(),
        )
    } else {
        None
    };

    if !state.operator_inputs.resolve(
        run_id,
        crate::engine::capabilities::operator_checkpoint::OperatorInputResponse {
            disposition: normalized_disposition.to_string(),
            selected_step_id: selected_step_id.clone(),
        },
    ) {
        return Err(anyhow!("operator checkpoint runtime is not active; restart the stage"));
    }

    Ok(json!({
        "ok": true,
        "accepted": true,
        "status": "running",
        "disposition": normalized_disposition,
        "current_step_id": stage_id,
        "next_step_id": selected_step_id,
        "followup_action": "continue_existing_runtime"
    }))
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

    let decisions = automation::before_stage(state, run_id, &mut run, &step).await?;
    automation::apply_context_mutations(
        &mut run,
        &decisions,
        Some(step.id.as_str()),
        None,
    )?;

    let pause_message = automation::pause_message(&decisions);
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
        "Automation prepared stage before execution.",
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
            "paused_by_automation": pause_message.is_some(),
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

    run_stages(
        state,
        run_id,
        requested_step_id,
        RunMode::Manual,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
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

    if run_is_waiting_on_operator_checkpoint(&run) {
        return Ok(json!({
            "ok": false,
            "status": "waiting",
            "blocked_on": "operator_checkpoint",
            "current_step_id": run.current_step_id,
            "message": "Resolve or cancel the active operator checkpoint before restarting the stage."
        }));
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
        tokio_util::sync::CancellationToken::new(),
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

    Box::pin(run_stages(
        state,
        run_id,
        requested_step_id,
        RunMode::Autonomous,
        tokio_util::sync::CancellationToken::new(),
    ))
    .await
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

pub(crate) fn request_run_pause_after_stage(run: &mut super::WorkflowRun) -> Result<()> {
    let root = ensure_engine_root(&mut run.context);
    let run_state = root.entry("run_state".to_string()).or_insert_with(|| json!({}));
    let run_state_obj = run_state.as_object_mut().ok_or_else(|| anyhow!("run_state must be object"))?;
    run_state_obj.insert("pause_requested".to_string(), json!(true));
    Ok(())
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
                            paused = true;
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
                            resolve_operator_checkpoint(
                                state,
                                run_id,
                                disposition.as_str(),
                                selected_step_id.as_deref(),
                            )
                            .await?;
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
        let execution = run_stages(
            state,
            run_id,
            execution_step_id.as_deref(),
            mode,
            cancellation.clone(),
        );
        tokio::pin!(execution);

        let result = loop {
            tokio::select! {
                command = command_rx.recv() => {
                    let Some(command) = command else {
                        fail_runtime_workflow(state, run_id, "runtime_channel_closed", "Workflow runtime command channel closed.").await?;
                        return Ok(());
                    };

                    match command {
                        WorkflowRuntimeCommand::Pause => {
                            pause_run(state, run_id).await?;
                        }
                        WorkflowRuntimeCommand::Cancel { .. } => {
                            cancellation.cancel();
                        }
                        WorkflowRuntimeCommand::ResolveCheckpoint { disposition, selected_step_id } => {
                            resolve_operator_checkpoint(
                                state,
                                run_id,
                                disposition.as_str(),
                                selected_step_id.as_deref(),
                            )
                            .await?;
                        }
                        WorkflowRuntimeCommand::MoveTo { step_id } => {
                            select_step(state, run_id, step_id.as_str()).await?;
                            requested_step_id = Some(step_id);
                            paused = true;
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

        let run = load_run(state, run_id).await?;
        if run_is_waiting_on_operator_checkpoint(&run) {
            paused = true;
            continue;
        }

        let status = result.get("status").and_then(Value::as_str).unwrap_or("waiting");
        match status {
            "complete" | "success" | "error" | "cancelled" => return Ok(()),
            "paused" => paused = true,
            "waiting" => {
                if execution_mode == WorkflowExecutionMode::SingleStage {
                    active_mode = None;
                } else {
                    requested_step_id = run.current_step_id;
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

async fn run_stages(
    state: &AppState,
    run_id: Uuid,
    requested_step_id: Option<&str>,
    mode: RunMode,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<serde_json::Value> {
    let mut last_payload = json!({ "ok": true, "status": "waiting" });
    let mut requested = requested_step_id.map(|s| s.to_string());
    let mut hops = 0usize;

    loop {
        let automatic = matches!(mode, RunMode::Autonomous);
        if cancellation.is_cancelled() {
            return Ok(json!({
                "ok": false,
                "status": "cancelled",
                "cancelled": true
            }));
        }
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
            state.operator_inputs.clear(run_id);
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

        if run_is_waiting_on_operator_checkpoint(&latest_run) {
            return Ok(json!({
                "ok": true,
                "status": "running",
                "blocked_on": "operator_checkpoint",
                "current_step_id": latest_run.current_step_id
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

        let outcome = execute_stage(
            state,
            run_id,
            &mut run,
            &step,
            automatic,
            cancellation.clone(),
        )
        .await?;
        let next_target = resolve_next_target(&definition, &step, &outcome);
        let latest_run = load_run(state, run_id).await?;
        if run_cancel_requested(&latest_run) {
            let mut cancelled_run = latest_run;
            clear_run_cancel_requested(&mut cancelled_run);
            state.operator_inputs.clear(run_id);
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

        append_engine_event(
            state,
            run_id,
            Some(step.id.as_str()),
            if outcome.ok { "info" } else { "error" },
            "stage_executed",
            &outcome.message,
            json!({
                "stage_status": format_stage_status(&outcome.status),
            "transition": format_stage_transition(&outcome.transition),
                "capability_results": outcome.capability_results,
                "local_state": outcome.local_state,
                "final_context": run.context.clone(),
            }),
        ).await?;

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
            let status = match &outcome.transition {
                StageTransition::Stop => terminal_run_status(&outcome.status),
                StageTransition::Stay
                | StageTransition::RetryStage
                | StageTransition::MoveNext
                | StageTransition::MoveBack
                | StageTransition::Target(_) => RunStatus::Waiting,
            };
            let current_step_id = match &outcome.transition {
                StageTransition::Stop | StageTransition::Stay => Some(step.id.as_str()),
                StageTransition::RetryStage
                | StageTransition::MoveNext
                | StageTransition::MoveBack
                | StageTransition::Target(_) => next_target.as_deref().or(Some(step.id.as_str())),
            };
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
                "Workflow run updated after stage completion.",
                json!({
                    "status": format_run_status(&status),
                    "stage_status": format_stage_status(&outcome.status),
                    "transition": format_stage_transition(&outcome.transition),
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
                "status": format_run_status(&status),
                "stage_status": format_stage_status(&outcome.status),
                "transition": format_stage_transition(&outcome.transition),
                "step_id": step.id,
                "next_step_id": next_target,
                "message": outcome.message,
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

        match (&outcome.transition, next_target.clone(), auto_advance) {
            (StageTransition::MoveNext, Some(target), true)
            | (StageTransition::MoveBack, Some(target), true)
            | (StageTransition::RetryStage, Some(target), true)
            | (StageTransition::Target(_), Some(target), true) => {
                set_run_status(state, run_id, RunStatus::Running, Some(target.as_str())).await?;
                last_payload = json!({
                    "ok": outcome.ok,
                    "status": "running",
                    "stage_status": format_stage_status(&outcome.status),
                    "transition": format_stage_transition(&outcome.transition),
                    "step_id": step.id,
                    "next_step_id": target,
                    "message": outcome.message,
                });
                requested = Some(target);
                continue;
            }
            (StageTransition::MoveNext, Some(target), false)
            | (StageTransition::MoveBack, Some(target), false)
            | (StageTransition::RetryStage, Some(target), false)
            | (StageTransition::Target(_), Some(target), false) => {
                set_run_status(state, run_id, RunStatus::Waiting, Some(target.as_str())).await?;
                return Ok(json!({
                    "ok": outcome.ok,
                    "status": "waiting",
                    "stage_status": format_stage_status(&outcome.status),
                    "transition": format_stage_transition(&outcome.transition),
                    "step_id": step.id,
                    "next_step_id": target,
                    "message": outcome.message,
                }));
            }
            (StageTransition::MoveNext, None, _)
            | (StageTransition::Target(_), None, _) => {
                let status = terminal_run_status(&outcome.status);
                set_run_status(state, run_id, status.clone(), Some(step.id.as_str())).await?;
                if matches!(status, RunStatus::Success | RunStatus::Error) {
                    crate::supervisor::handle_workflow_terminal_event(state, run_id, status.clone(), Some(step.id.as_str())).await?;
                }
                return Ok(json!({
                    "ok": outcome.ok,
                    "status": format_run_status(&status),
                    "stage_status": format_stage_status(&outcome.status),
                    "transition": format_stage_transition(&outcome.transition),
                    "step_id": step.id,
                    "message": outcome.message,
                }));
            }
            (StageTransition::MoveBack, None, _)
            | (StageTransition::RetryStage, None, _) => {
                set_run_status(state, run_id, RunStatus::Waiting, Some(step.id.as_str())).await?;
                return Ok(json!({
                    "ok": outcome.ok,
                    "status": "waiting",
                    "stage_status": format_stage_status(&outcome.status),
                    "transition": format_stage_transition(&outcome.transition),
                    "step_id": step.id,
                    "message": outcome.message,
                }));
            }
            (StageTransition::Stay, _, _) => {
                let status = if matches!(outcome.status, StageStatus::Paused) {
                    RunStatus::Paused
                } else {
                    RunStatus::Waiting
                };
                set_run_status(state, run_id, status.clone(), Some(step.id.as_str())).await?;
                return Ok(json!({
                    "ok": outcome.ok,
                    "status": format_run_status(&status),
                    "stage_status": format_stage_status(&outcome.status),
                    "transition": format_stage_transition(&outcome.transition),
                    "step_id": step.id,
                    "message": outcome.message,
                }));
            }
            (StageTransition::Stop, _, _) => {
                let status = terminal_run_status(&outcome.status);
                set_run_status(state, run_id, status.clone(), Some(step.id.as_str())).await?;
                if matches!(status, RunStatus::Success | RunStatus::Error) {
                    crate::supervisor::handle_workflow_terminal_event(state, run_id, status.clone(), Some(step.id.as_str())).await?;
                }
                return Ok(json!({
                    "ok": outcome.ok,
                    "status": format_run_status(&status),
                    "stage_status": format_stage_status(&outcome.status),
                    "transition": format_stage_transition(&outcome.transition),
                    "step_id": step.id,
                    "message": outcome.message,
                }));
            }
        }
    }
}

fn format_stage_status(status: &StageStatus) -> String {
    match status {
        StageStatus::Success => "success".to_string(),
        StageStatus::Error => "error".to_string(),
        StageStatus::ErrorCode(code) => format!("error_code:{}", code),
        StageStatus::Paused => "paused".to_string(),
        StageStatus::Outcome(name) => format!("outcome:{}", name),
        StageStatus::Stay => "stay".to_string(),
    }
}

fn format_stage_transition(transition: &StageTransition) -> String {
    match transition {
        StageTransition::MoveNext => "move_next".to_string(),
        StageTransition::MoveBack => "move_back".to_string(),
        StageTransition::RetryStage => "retry_stage".to_string(),
        StageTransition::Stay => "stay".to_string(),
        StageTransition::Stop => "stop".to_string(),
        StageTransition::Target(target) => format!("target:{}", target),
    }
}

fn terminal_run_status(status: &StageStatus) -> RunStatus {
    match status {
        StageStatus::Success | StageStatus::Outcome(_) => RunStatus::Success,
        StageStatus::Error | StageStatus::ErrorCode(_) => RunStatus::Error,
        StageStatus::Paused => RunStatus::Paused,
        StageStatus::Stay => RunStatus::Waiting,
    }
}

fn format_run_status(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Draft => "waiting",
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::Waiting => "waiting",
        RunStatus::Paused => "paused",
        RunStatus::Success => "complete",
        RunStatus::Error => "error",
        RunStatus::Cancelled => "cancelled",
    }
}
