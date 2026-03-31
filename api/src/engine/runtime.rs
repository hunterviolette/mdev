use anyhow::{anyhow, Result};
use serde_json::json;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    models::RunStatus,
};

use super::{
    append_engine_event, current_step, load_run, load_template_definition, set_run_status,
};
use super::stages::{execute_stage, StageDisposition};
use super::transitions::{resolve_next_target, should_auto_advance};

pub async fn start_run(state: &AppState, run_id: Uuid) -> Result<serde_json::Value> {
    set_run_status(state, run_id, RunStatus::Queued, None).await?;
    continue_until_wait(state, run_id, None).await
}

pub async fn resume_run(state: &AppState, run_id: Uuid) -> Result<serde_json::Value> {
    continue_until_wait(state, run_id, None).await
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

async fn execute_single_step(state: &AppState, run_id: Uuid, requested_step_id: Option<&str>) -> Result<serde_json::Value> {
    let mut run = load_run(state, run_id).await?;
    let definition = load_template_definition(state, &run).await?
        .ok_or_else(|| anyhow!("run has no template definition"))?;

    let step = current_step(&definition, &run, requested_step_id)?.clone();

    set_run_status(state, run_id, RunStatus::Running, Some(step.id.as_str())).await?;

    let outcome = execute_stage(state, run_id, &mut run, &step).await?;

    let next_target = resolve_next_target(&definition, &step, &outcome);
    let status = match outcome.disposition {
        StageDisposition::Paused => RunStatus::Paused,
        StageDisposition::Success => RunStatus::Waiting,
        StageDisposition::RetryStage => RunStatus::Waiting,
        StageDisposition::MoveToStep(_) | StageDisposition::MoveBack | StageDisposition::Outcome(_) | StageDisposition::Stay => RunStatus::Waiting,
        StageDisposition::Error | StageDisposition::ErrorCode(_) => RunStatus::Error,
    };
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

        let outcome = execute_stage(state, run_id, &mut run, &step).await?;
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
            (StageDisposition::MoveToStep(_), Some(target), _) | (StageDisposition::MoveBack, Some(target), _) => {
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
        StageDisposition::MoveToStep(step_id) => format!("move_to_step:{}", step_id),
        StageDisposition::MoveBack => "move_back".to_string(),
        StageDisposition::Outcome(name) => format!("outcome:{}", name),
        StageDisposition::Stay => "stay".to_string(),
    }
}
