use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::{governance, refresh_inference_arm_state},
    models::RunStatus,
};

use super::{
    activate_next_prompt_fragments_for_stage,
    append_engine_event,
    clear_active_prompt_fragments_for_stage,
    event_meta,
    current_step,
    ensure_engine_root,
    load_run,
    load_template_definition,
    persist_context,
    set_run_status,
};
use super::stages::{execute_stage, StageDisposition};
use super::transitions::{next_step_id, resolve_next_target, should_auto_advance};

pub async fn start_run(state: &AppState, run_id: Uuid, requested_step_id: Option<&str>) -> Result<serde_json::Value> {
    let run = load_run(state, run_id).await?;
    let current_step_id = requested_step_id.or(run.current_step_id.as_deref());
    set_run_status(state, run_id, RunStatus::Queued, current_step_id).await?;
    start_or_resume_automatic_run(state, run_id, requested_step_id).await
}

pub async fn resume_run(state: &AppState, run_id: Uuid) -> Result<serde_json::Value> {
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
            == Some("disposition_review");
    let autonomous_pause_eligible = if matches!(run.status, RunStatus::Waiting) {
        load_template_definition(state, &run)
            .await?
            .and_then(|definition| current_step(&definition, &run, None).ok().map(step_is_auto_runnable))
            .unwrap_or(false)
    } else {
        false
    };

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
    let root = ensure_engine_root(&mut run.context);
    if let Some(run_state) = root.get_mut("run_state").and_then(|v| v.as_object_mut()) {
        run_state.remove("pause_requested");
    }
    persist_context(state, run_id, &run.context).await?;

    set_run_status(state, run_id, RunStatus::Waiting, run.current_step_id.as_deref()).await?;
    append_engine_event(
        state,
        run_id,
        run.current_step_id.as_deref(),
        "warn",
        "run_force_unlocked",
        "Workflow run was force-unlocked by operator and returned to waiting state.",
        json!({
            "previous_status": format!("{:?}", run.status).to_lowercase(),
            "current_step_id": run.current_step_id
        }),
    ).await?;
    Ok(json!({ "ok": true, "status": "waiting", "current_step_id": run.current_step_id }))
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

pub async fn resolve_disposition_review(state: &AppState, run_id: Uuid, disposition: &str) -> Result<serde_json::Value> {
    let mut run = load_run(state, run_id).await?;
    let blocked_on = run
        .context
        .get("workflow_engine")
        .and_then(|v| v.get("run_state"))
        .and_then(|v| v.get("blocked_on"))
        .cloned()
        .ok_or_else(|| anyhow!("workflow is not waiting on disposition review"))?;

    let blocked_kind = blocked_on
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if blocked_kind != "disposition_review" {
        return Err(anyhow!("workflow is blocked on {}, not disposition_review", blocked_kind));
    }

    let stage_id = blocked_on
        .get("stage_id")
        .and_then(Value::as_str)
        .or(run.current_step_id.as_deref())
        .ok_or_else(|| anyhow!("disposition review is missing stage_id"))?
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
    let stage_execution_id = match blocked_on
        .get("stage_execution_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
    {
        Some(value) => Some(value),
        None => latest_stage_execution_id_for_step(state, run_id, stage_id.as_str()).await?,
    };


    if !matches!(disposition, "move_next" | "pause" | "paused") {
        return Err(anyhow!("unsupported disposition {}", disposition));
    }

    {
        let root = ensure_engine_root(&mut run.context);
        let run_state = root.entry("run_state".to_string()).or_insert_with(|| json!({}));
        let run_state_obj = run_state.as_object_mut().ok_or_else(|| anyhow!("run_state must be object"))?;
        run_state_obj.remove("blocked_on");
    }
    persist_context(state, run_id, &run.context).await?;

    match disposition {
        "pause" | "paused" => {
            append_disposition_stage_completion_event(
                state,
                run_id,
                stage_id.as_str(),
                stage_execution_id.as_deref(),
                false,
                "paused",
                "Stage paused by operator at disposition review.",
                None,
            ).await?;

            set_run_status(state, run_id, RunStatus::Paused, Some(stage_id.as_str())).await?;
            append_engine_event(
                state,
                run_id,
                Some(stage_id.as_str()),
                "info",
                "disposition_review_resolved",
                "Operator paused workflow at disposition review.",
                json!({ "disposition": "pause", "stage_id": stage_id }),
            ).await?;
            Ok(json!({
                "ok": true,
                "status": "paused",
                "disposition": "pause",
                "current_step_id": stage_id,
                "followup_action": "pause"
            }))
        }
        "move_next" => {
            let definition = load_template_definition(state, &run).await?
                .ok_or_else(|| anyhow!("run has no template definition"))?;
            let target = next_target
                .or_else(|| next_step_id(&definition, Some(stage_id.as_str())));

            if target.is_none() {
                append_engine_event(
                    state,
                    run_id,
                    Some(stage_id.as_str()),
                    "info",
                    "disposition_review_resolved",
                    "Operator approved terminal review stage; workflow completed successfully.",
                    json!({ "disposition": "move_next", "stage_id": stage_id }),
                ).await?;

                append_disposition_stage_completion_event(
                    state,
                    run_id,
                    stage_id.as_str(),
                    stage_execution_id.as_deref(),
                    true,
                    "success",
                    "Terminal review approved. Workflow completed successfully.",
                    None,
                ).await?;

                set_run_status(state, run_id, RunStatus::Success, Some(stage_id.as_str())).await?;
                return Ok(json!({
                    "ok": true,
                    "status": "success",
                    "disposition": "move_next",
                    "current_step_id": stage_id,
                    "followup_action": "complete_workflow"
                }));
            }

            let target = target.expect("target checked above");
            let target_step = current_step(&definition, &run, Some(target.as_str()))?.clone();
            append_engine_event(
                state,
                run_id,
                Some(stage_id.as_str()),
                "info",
                "disposition_review_resolved",
                "Operator approved moving to the next stage after disposition review.",
                json!({ "disposition": "move_next", "stage_id": stage_id, "next_step_id": target }),
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
                    "disposition": "pause",
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
                "move_next",
                "Stage completed after operator disposition review.",
                Some(target.as_str()),
            ).await?;

            set_current_step_waiting(state, run_id, target.as_str()).await?;

            append_engine_event(
                state,
                run_id,
                Some(target.as_str()),
                "info",
                "disposition_transition_committed",
                "Disposition review transition committed before backend continuation.",
                json!({
                    "disposition": "move_next",
                    "from_step_id": stage_id,
                    "current_step_id": target,
                    "resume_mode": resume_mode,
                    "auto_runnable": step_is_auto_runnable(&target_step),
                }),
            ).await?;

            continue_from_disposition_transition(
                state,
                run_id,
                target.as_str(),
                resume_mode.as_str(),
                &target_step,
            ).await
        }
        other => Err(anyhow!("unsupported disposition {}", other)),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DispositionFollowupAction {
    StartAutonomous,
    RunStage,
    Pause,
    None,
}

fn resolve_disposition_followup_action(
    resume_mode: &str,
    target_step: &super::WorkflowStepDefinition,
) -> DispositionFollowupAction {
    match resume_mode {
        "autonomous" => DispositionFollowupAction::StartAutonomous,
        "pause" | "paused" => DispositionFollowupAction::Pause,
        "none" | "wait" | "waiting" => DispositionFollowupAction::None,
        _ if step_is_auto_runnable(target_step) => DispositionFollowupAction::StartAutonomous,
        _ => DispositionFollowupAction::RunStage,
    }
}

fn format_disposition_followup_action(action: DispositionFollowupAction) -> &'static str {
    match action {
        DispositionFollowupAction::StartAutonomous => "start_run",
        DispositionFollowupAction::RunStage => "run_step",
        DispositionFollowupAction::Pause => "pause",
        DispositionFollowupAction::None => "none",
    }
}

async fn continue_from_disposition_transition(
    state: &AppState,
    run_id: Uuid,
    target_step_id: &str,
    resume_mode: &str,
    target_step: &super::WorkflowStepDefinition,
) -> Result<serde_json::Value> {
    let followup_action = resolve_disposition_followup_action(resume_mode, target_step);

    append_engine_event(
        state,
        run_id,
        Some(target_step_id),
        "info",
        "disposition_transition_committed",
        "Disposition review transition committed; backend continuation policy selected follow-up action.",
        json!({
            "step_id": target_step_id,
            "resume_mode": resume_mode,
            "auto_runnable": step_is_auto_runnable(target_step),
            "followup_action": format_disposition_followup_action(followup_action),
        }),
    ).await?;

    match followup_action {
        DispositionFollowupAction::StartAutonomous => {
            start_run(state, run_id, Some(target_step_id)).await
        }
        DispositionFollowupAction::RunStage => {
            run_step(state, run_id, Some(target_step_id)).await
        }
        DispositionFollowupAction::Pause => {
            set_run_status(state, run_id, RunStatus::Paused, Some(target_step_id)).await?;
            Ok(json!({
                "ok": true,
                "status": "paused",
                "disposition": "move_next",
                "current_step_id": target_step_id,
                "followup_action": "pause"
            }))
        }
        DispositionFollowupAction::None => {
            set_run_status(state, run_id, RunStatus::Waiting, Some(target_step_id)).await?;
            Ok(json!({
                "ok": true,
                "status": "waiting",
                "disposition": "move_next",
                "current_step_id": target_step_id,
                "followup_action": "none"
            }))
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Manual,
    Autonomous,
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
    let step = current_step(&definition, &run, requested_step_id)?.clone();

    run.current_step_id = Some(step.id.clone());

    let decisions = governance::before_stage(state, run_id, &mut run, &step).await?;
    governance::apply_context_mutations(&mut run, &decisions, Some(step.id.as_str()), None)?;
    refresh_inference_arm_state(&mut run, Some(&step));

    let prepared_inference_snapshot = run
        .context
        .get("workflow_engine")
        .and_then(|value| value.get("global_state"))
        .and_then(|value| value.get("capabilities"))
        .and_then(|value| value.get("inference"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    {
        let root = ensure_engine_root(&mut run.context);
        let run_state = root.entry("run_state".to_string()).or_insert_with(|| json!({}));
        let run_state_obj = run_state.as_object_mut().ok_or_else(|| anyhow!("run_state must be object"))?;
        run_state_obj.insert("last_prepared_stage".to_string(), json!({
            "step_id": step.id,
            "step_type": step.step_type,
            "inference": prepared_inference_snapshot
        }));
    }

    let pause_message = governance::pause_message(&decisions);
    let prepared_status = if pause_message.is_some() {
        RunStatus::Waiting
    } else {
        RunStatus::Running
    };

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
            "status": if pause_message.is_some() { "waiting" } else { "running" },
            "prepared_status": if pause_message.is_some() { "waiting" } else { "running" },
            "run_mode": match mode {
                RunMode::Manual => "manual",
                RunMode::Autonomous => "autonomous",
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
        RunMode::Autonomous,
    ).await?;

    Ok(json!({
        "ok": prepared.pause_message.is_none(),
        "status": if prepared.pause_message.is_some() { "waiting" } else { "running" },
        "current_step_id": prepared.step.id,
        "step_id": prepared.step.id,
        "step_type": prepared.step.step_type,
        "message": prepared.pause_message,
        "run": prepared.run
    }))
}

pub async fn run_step(state: &AppState, run_id: Uuid, requested_step_id: Option<&str>) -> Result<serde_json::Value> {
    run_stages(state, run_id, requested_step_id, RunMode::Manual).await
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

    run_stages(state, run_id, requested_step_id, RunMode::Autonomous).await
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

fn consume_single_use_inference_arm_state(run: &mut super::WorkflowRun, step: &super::WorkflowStepDefinition) {
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

fn stage_disposition_review_enabled(
    step: &super::WorkflowStepDefinition,
    outcome: &super::stages::StageOutcome,
) -> bool {
    let step_enabled = step
        .execution_logic
        .get("automation")
        .and_then(|v| v.get("disposition_review"))
        .and_then(|v| v.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let outcome_enabled = outcome
        .local_state
        .get("execution_logic")
        .and_then(|v| v.get("automation"))
        .and_then(|v| v.get("disposition_review"))
        .and_then(|v| v.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    step_enabled || outcome_enabled
}

fn disposition_review_options(
    step: &super::WorkflowStepDefinition,
    outcome: &super::stages::StageOutcome,
) -> Value {
    let configured = outcome
        .local_state
        .get("execution_logic")
        .and_then(|v| v.get("automation"))
        .and_then(|v| v.get("disposition_review"))
        .and_then(|v| v.get("available_dispositions"))
        .cloned()
        .or_else(|| {
            step.execution_logic
                .get("automation")
                .and_then(|v| v.get("disposition_review"))
                .and_then(|v| v.get("available_dispositions"))
                .cloned()
        });

    normalize_disposition_review_options(configured)
}

fn normalize_disposition_review_options(configured: Option<Value>) -> Value {
    let options = configured
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| match value.as_str() {
            Some("move_next") => Some(json!("move_next")),
            Some("pause") => Some(json!("pause")),
            _ => None,
        })
        .collect::<Vec<Value>>();

    if options.is_empty() {
        json!(["move_next", "pause"])
    } else {
        Value::Array(options)
    }
}

fn clear_pending_disposition_review(run: &mut super::WorkflowRun) {
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

fn set_pending_disposition_review(
    run: &mut super::WorkflowRun,
    step: &super::WorkflowStepDefinition,
    outcome: &super::stages::StageOutcome,
    next_target: Option<String>,
    resume_mode: &str,
) {
    let root = ensure_engine_root(&mut run.context);
    let run_state = root.entry("run_state".to_string()).or_insert_with(|| json!({}));
    let run_state_obj = run_state.as_object_mut().expect("run_state must be object");
    let stage_execution_id = outcome
        .local_state
        .get("_stage_execution_id")
        .and_then(Value::as_str)
        .unwrap_or("");

    run_state_obj.insert("blocked_on".to_string(), json!({
        "kind": "disposition_review",
        "stage_id": step.id,
        "stage_type": step.step_type,
        "stage_execution_id": stage_execution_id,
        "recommended_disposition": format_disposition(&outcome.disposition),
        "available_dispositions": disposition_review_options(step, outcome),
        "next_step_id": next_target,
        "message": outcome.message,
        "resume_mode": resume_mode
    }));
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

        activate_next_prompt_fragments_for_stage(&mut run);
        let outcome = execute_stage(state, run_id, &mut run, &step, automatic).await?;
        clear_active_prompt_fragments_for_stage(&mut run);

        if matches!(
            outcome.disposition,
            StageDisposition::Success
                | StageDisposition::MoveNext
                | StageDisposition::MoveBack
                | StageDisposition::Outcome(_)
                | StageDisposition::Stay
        ) {
            consume_single_use_inference_arm_state(&mut run, &step);
        }

        let pending_disposition_review = outcome.ok && stage_disposition_review_enabled(&step, &outcome);

        if pending_disposition_review {
            clear_pending_disposition_review(&mut run);
            let next_target = resolve_next_target(&definition, &step, &outcome);
            let disposition_resume_mode = if automatic { "autonomous" } else { "manual" };
            set_pending_disposition_review(&mut run, &step, &outcome, next_target.clone(), disposition_resume_mode);
            persist_context(state, run_id, &run.context).await?;
            set_run_status(state, run_id, RunStatus::Waiting, Some(step.id.as_str())).await?;
            append_engine_event(
                state,
                run_id,
                Some(step.id.as_str()),
                "info",
                "workflow_waiting_for_disposition_review",
                "Workflow is waiting for operator disposition review.",
                json!({
                    "stage_id": step.id,
                    "stage_type": step.step_type,
                    "recommended_disposition": format_disposition(&outcome.disposition),
                    "available_dispositions": disposition_review_options(&step, &outcome),
                    "next_step_id": next_target.clone(),
                    "resume_mode": disposition_resume_mode,
                }),
            ).await?;
            return Ok(json!({
                "ok": true,
                "status": "waiting",
                "blocked_on": "disposition_review",
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

        clear_pending_disposition_review(&mut run);
        let next_target = resolve_next_target(&definition, &step, &outcome);
        let auto_advance = automatic && should_auto_advance(&step, &outcome);
        let latest_run = load_run(state, run_id).await?;
        if run_pause_requested(&latest_run) {
            clear_run_pause_requested(&mut run);
            persist_context(state, run_id, &run.context).await?;
            set_run_status(state, run_id, RunStatus::Paused, Some(step.id.as_str())).await?;
            append_engine_event(
                state,
                run_id,
                Some(step.id.as_str()),
                "info",
                "run_paused_after_stage",
                "Workflow run paused after the current stage completed.",
                json!({}),
            ).await?;
            return Ok(json!({
                "ok": true,
                "status": "paused",
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
            set_run_status(state, run_id, status.clone(), current_step_id).await?;

            return Ok(json!({
                "ok": outcome.ok,
                "status": match status {
                    RunStatus::Draft => "waiting",
                    RunStatus::Queued => "queued",
                    RunStatus::Running => "running",
                    RunStatus::Waiting => "waiting",
                    RunStatus::Paused => "paused",
                    RunStatus::Success => "success",
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
                set_run_status(state, run_id, RunStatus::Waiting, Some(target.as_str())).await?;
                return Ok(json!({
                    "ok": outcome.ok,
                    "status": "waiting",
                    "step_id": step.id,
                    "next_step_id": target,
                    "message": outcome.message,
                    "disposition": "retry_stage"
                }));
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
                return Ok(json!({
                    "ok": outcome.ok,
                    "status": "success",
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
                return Ok(json!({
                    "ok": true,
                    "status": "success",
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
