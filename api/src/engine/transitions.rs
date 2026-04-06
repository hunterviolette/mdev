use crate::models::{WorkflowStepDefinition, WorkflowTemplateDefinition};

use super::stages::{StageDisposition, StageOutcome};

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
    match &outcome.disposition {
        StageDisposition::MoveNext => next_step_id(definition, Some(step.id.as_str())),
        StageDisposition::MoveBack => previous_step_id(definition, Some(step.id.as_str())),
        StageDisposition::RetryStage => Some(step.id.clone()),
        StageDisposition::Stay => Some(step.id.clone()),
        _ => None,
    }
}

pub fn should_auto_advance(step: &WorkflowStepDefinition, outcome: &StageOutcome) -> bool {
    match outcome.disposition {
        StageDisposition::Success => step.advancement.auto_advance_on_success,
        StageDisposition::Error | StageDisposition::ErrorCode(_) => step.advancement.auto_advance_on_error,
        StageDisposition::Paused => step.advancement.auto_advance_on_paused,
        StageDisposition::RetryStage => true,
        StageDisposition::MoveNext | StageDisposition::MoveBack => true,
        StageDisposition::Outcome(_) | StageDisposition::Stay => false,
    }
}
