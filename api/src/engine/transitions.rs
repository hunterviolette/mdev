use anyhow::{anyhow, Result};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    models::{WorkflowRun, WorkflowStepDefinition, WorkflowTemplateDefinition},
};

use super::stages::{StageOutcome, StageStatus, StageTransition};

pub async fn transition_to_step(
    state: &AppState,
    run_id: Uuid,
    run: &mut WorkflowRun,
    definition: &WorkflowTemplateDefinition,
    next_step_id: &str,
) -> Result<WorkflowStepDefinition> {
    let next_step = definition
        .steps
        .iter()
        .find(|step| step.id == next_step_id)
        .cloned()
        .ok_or_else(|| anyhow!("unknown step_id {}", next_step_id))?;

    let previous_step_id = run.current_step_id.clone();
    if previous_step_id.as_deref() == Some(next_step.id.as_str()) {
        return Ok(next_step);
    }

    if let Some(previous_step) = previous_step_id
        .as_deref()
        .and_then(|step_id| definition.steps.iter().find(|step| step.id == step_id))
    {
        super::stages::invoke_stage_exit_hook(
            state,
            run_id,
            previous_step,
            Some(next_step.id.as_str()),
        )
        .await?;
    }

    run.current_step_id = Some(next_step.id.clone());
    Ok(next_step)
}

pub fn next_step_id(definition: &WorkflowTemplateDefinition, current_step_id: Option<&str>) -> Option<String> {
    let current_id = current_step_id.or_else(|| definition.steps.first().map(|s| s.id.as_str()))?;
    let index = definition.steps.iter().position(|step| step.id == current_id)?;
    definition.steps.get(index + 1).map(|step| step.id.clone())
}

pub fn previous_step_id(definition: &WorkflowTemplateDefinition, current_step_id: Option<&str>) -> Option<String> {
    let current_id = current_step_id.or_else(|| definition.steps.first().map(|s| s.id.as_str()))?;
    let index = definition.steps.iter().position(|step| step.id == current_id)?;
    index.checked_sub(1).and_then(|idx| definition.steps.get(idx)).map(|step| step.id.clone())
}

pub fn resolve_next_target(
    definition: &WorkflowTemplateDefinition,
    step: &WorkflowStepDefinition,
    outcome: &StageOutcome,
) -> Option<String> {
    match &outcome.transition {
        StageTransition::MoveNext => next_step_id(definition, Some(step.id.as_str())),
        StageTransition::MoveBack => previous_step_id(definition, Some(step.id.as_str())),
        StageTransition::RetryStage | StageTransition::Stay => Some(step.id.clone()),
        StageTransition::Stop => None,
        StageTransition::Target(target) => Some(target.clone()),
    }
}

pub fn should_auto_advance(_step: &WorkflowStepDefinition, outcome: &StageOutcome) -> bool {
    match (&outcome.status, &outcome.transition) {
        (_, StageTransition::MoveNext | StageTransition::MoveBack | StageTransition::RetryStage | StageTransition::Target(_)) => true,
        (StageStatus::Success | StageStatus::Error | StageStatus::ErrorCode(_) | StageStatus::Paused | StageStatus::Outcome(_) | StageStatus::Stay, StageTransition::Stay | StageTransition::Stop) => false,
    }
}
