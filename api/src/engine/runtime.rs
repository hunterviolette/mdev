use anyhow::{anyhow, Result};
use serde_json::json;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    models::RunStatus,
};

use super::{
    activate_next_prompt_fragments_for_stage,
    append_engine_event,
    clear_active_prompt_fragments_for_stage,
    current_step,
    load_run,
    load_template_definition,
    persist_context,
    refresh_inference_arm_state,
    set_run_status,
};
use super::stages::{execute_stage, StageDisposition};
use super::transitions::{resolve_next_target, should_auto_advance};

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
    let run = load_run(state, run_id).await?;
    set_run_status(state, run_id, RunStatus::Paused, run.current_step_id.as_deref()).await?;
    append_engine_event(
        state,
        run_id,
        run.current_step_id.as_deref(),
        "info",
        "run_paused",
        "Workflow run paused by operator.",
        json!({}),
    ).await?;
    Ok(json!({ "ok": true, "status": "paused", "current_step_id": run.current_step_id }))
}

pub async fn run_step(state: &AppState, run_id: Uuid, requested_step_id: Option<&str>) -> Result<serde_json::Value> {
    execute_single_step(state, run_id, requested_step_id).await
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

    continue_until_wait(state, run_id, requested_step_id).await
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
    let root = super::ensure_engine_root(&mut run.context);
    let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
    let global_state_obj = global_state.as_object_mut().expect("global_state must be object");
    let capabilities = global_state_obj
        .entry("capabilities".to_string())
        .or_insert_with(|| json!({}));
    let capabilities_obj = capabilities.as_object_mut().expect("capabilities must be object");
    let inference = capabilities_obj
        .entry("inference".to_string())
        .or_insert_with(|| json!({}));
    let inference_obj = inference.as_object_mut().expect("inference must be object");

    if crate::engine::capabilities::binding_specs::stage_supports_shared_capability(step, "repo_context") {
        inference_obj.insert("repo_context_armed".to_string(), json!(false));
    }
    if crate::engine::capabilities::binding_specs::stage_supports_shared_capability(step, "changeset_schema") {
        inference_obj.insert("changeset_schema_armed".to_string(), json!(false));
    }

    inference_obj.remove("shared_inference_state");
}

async fn execute_single_step(state: &AppState, run_id: Uuid, requested_step_id: Option<&str>) -> Result<serde_json::Value> {
    let mut run = load_run(state, run_id).await?;
    let definition = load_template_definition(state, &run).await?
        .ok_or_else(|| anyhow!("run has no template definition"))?;

    let step = current_step(&definition, &run, requested_step_id)?.clone();

    set_run_status(state, run_id, RunStatus::Running, Some(step.id.as_str())).await?;

    activate_next_prompt_fragments_for_stage(&mut run);
    let outcome = execute_stage(state, run_id, &mut run, &step, false).await?;
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

    let next_target = resolve_next_target(&definition, &step, &outcome);
    let status = match outcome.disposition {
        StageDisposition::Paused => RunStatus::Paused,
        StageDisposition::Success => RunStatus::Waiting,
        StageDisposition::RetryStage => RunStatus::Waiting,
        StageDisposition::MoveNext | StageDisposition::MoveBack | StageDisposition::Outcome(_) | StageDisposition::Stay => RunStatus::Waiting,
        StageDisposition::Error | StageDisposition::ErrorCode(_) => RunStatus::Error,
    };
    let next_step = next_target
        .as_deref()
        .and_then(|step_id| definition.steps.iter().find(|candidate| candidate.id == step_id));
    refresh_inference_arm_state(&mut run, next_step.or(Some(&step)));
    persist_context(state, run_id, &run.context).await?;
    let next_step = next_target
        .as_deref()
        .and_then(|step_id| definition.steps.iter().find(|candidate| candidate.id == step_id));
    refresh_inference_arm_state(&mut run, next_step.or(Some(&step)));
    persist_context(state, run_id, &run.context).await?;
    let current_step_id = next_target.as_deref().or(Some(step.id.as_str()));
    set_run_status(state, run_id, status.clone(), current_step_id).await?;

    Ok(json!({
        "ok": outcome.ok,
        "status": match status {
            RunStatus::Draft => "draft",
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
    }))
}

async fn continue_until_wait(state: &AppState, run_id: Uuid, requested_step_id: Option<&str>) -> Result<serde_json::Value> {
    let mut last_payload = json!({ "ok": true, "status": "waiting" });
    let mut requested = requested_step_id.map(|s| s.to_string());

    loop {
        let mut run = load_run(state, run_id).await?;
        let definition = load_template_definition(state, &run).await?
            .ok_or_else(|| anyhow!("run has no template definition"))?;

        let step = current_step(&definition, &run, requested.take().as_deref())?.clone();

        set_run_status(state, run_id, RunStatus::Running, Some(step.id.as_str())).await?;

        activate_next_prompt_fragments_for_stage(&mut run);
        let outcome = execute_stage(state, run_id, &mut run, &step, true).await?;
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
            }),
        ).await?;

        let next_target = resolve_next_target(&definition, &step, &outcome);
        let auto_advance = should_auto_advance(&step, &outcome);
        let next_step = next_target
            .as_deref()
            .and_then(|step_id| definition.steps.iter().find(|candidate| candidate.id == step_id));
        refresh_inference_arm_state(&mut run, next_step.or(Some(&step)));
        persist_context(state, run_id, &run.context).await?;
        let next_step = next_target
            .as_deref()
            .and_then(|step_id| definition.steps.iter().find(|candidate| candidate.id == step_id));
        refresh_inference_arm_state(&mut run, next_step.or(Some(&step)));
        persist_context(state, run_id, &run.context).await?;

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
                });
                requested = next_target;
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
            _ => {
                return Ok(last_payload);
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
