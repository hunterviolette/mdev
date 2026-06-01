use anyhow::Result;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::capabilities::registry::{CapabilityInvocation, CapabilityResult},
    models::{WorkflowRun, WorkflowStepDefinition},
};

use super::decisions::GovernanceDecision;

pub mod changeset_file_failures;
pub mod compile_failures;
pub mod inference_input_consumption;
pub mod pause;

pub async fn before_stage(
    _state: &AppState,
    _run_id: Uuid,
    _run: &mut WorkflowRun,
    _step: &WorkflowStepDefinition,
) -> Result<Vec<GovernanceDecision>> {
    Ok(Vec::new())
}

pub async fn after_stage(
    state: &AppState,
    run_id: Uuid,
    run: &mut WorkflowRun,
    step: &WorkflowStepDefinition,
    stage_execution_id: &str,
    capability_results: &[Value],
) -> Result<Vec<GovernanceDecision>> {
    let latest_run = crate::engine::load_run(state, run_id)
        .await
        .unwrap_or_else(|_| run.clone());

    let mut decisions = Vec::new();
    decisions.extend(inference_input_consumption::after_stage(
        &latest_run,
        step,
        stage_execution_id,
        capability_results,
    )?);
    decisions.extend(pause::after_stage(&latest_run)?);

    Ok(decisions)
}

pub async fn before_capability(
    _state: &AppState,
    _run_id: Uuid,
    _run: &WorkflowRun,
    _step: &WorkflowStepDefinition,
    _stage_execution_id: Option<&str>,
    _invocation: &CapabilityInvocation,
    _prior_results: &[CapabilityResult],
) -> Result<Vec<GovernanceDecision>> {
    Ok(Vec::new())
}

pub async fn after_capability(
    _state: &AppState,
    _run_id: Uuid,
    run: &WorkflowRun,
    step: &WorkflowStepDefinition,
    _stage_execution_id: Option<&str>,
    result: &CapabilityResult,
    prior_results: &[CapabilityResult],
) -> Result<Vec<GovernanceDecision>> {
    let mut decisions = Vec::new();
    decisions.extend(changeset_file_failures::after_capability(run, step, result, prior_results)?);
    decisions.extend(compile_failures::after_capability(run, step, result, prior_results)?);
    Ok(decisions)
}
