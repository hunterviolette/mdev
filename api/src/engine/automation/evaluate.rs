use anyhow::Result;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::capabilities::registry::{CapabilityInvocation, CapabilityResult},
    models::{WorkflowRun, WorkflowStepDefinition},
};

use super::{ensure_automation_slots, AutomationDecision, CapabilityInjection};

pub async fn before_stage(
    state: &AppState,
    run_id: Uuid,
    run: &mut WorkflowRun,
    step: &WorkflowStepDefinition,
) -> Result<Vec<AutomationDecision>> {
    ensure_automation_slots(run);
    super::policies::before_stage(state, run_id, run, step).await
}

pub async fn after_stage(
    state: &AppState,
    run_id: Uuid,
    run: &mut WorkflowRun,
    step: &WorkflowStepDefinition,
    stage_execution_id: &str,
    capability_results: &[Value],
) -> Result<Vec<AutomationDecision>> {
    ensure_automation_slots(run);
    super::policies::after_stage(state, run_id, run, step, stage_execution_id, capability_results).await
}

pub async fn before_capability(
    state: &AppState,
    run_id: Uuid,
    run: &WorkflowRun,
    step: &WorkflowStepDefinition,
    stage_execution_id: Option<&str>,
    invocation: &CapabilityInvocation,
    prior_results: &[CapabilityResult],
) -> Result<Vec<AutomationDecision>> {
    super::policies::before_capability(
        state,
        run_id,
        run,
        step,
        stage_execution_id,
        invocation,
        prior_results,
    )
    .await
}

pub async fn after_capability(
    state: &AppState,
    run_id: Uuid,
    run: &WorkflowRun,
    step: &WorkflowStepDefinition,
    stage_execution_id: Option<&str>,
    result: &CapabilityResult,
    prior_results: &[CapabilityResult],
) -> Result<Vec<AutomationDecision>> {
    super::policies::after_capability(
        state,
        run_id,
        run,
        step,
        stage_execution_id,
        result,
        prior_results,
    )
    .await
}

pub fn pause_message(decisions: &[AutomationDecision]) -> Option<String> {
    for decision in decisions {
        match decision {
            AutomationDecision::Pause { reason } => return Some(reason.clone()),
            AutomationDecision::RequireApproval { reason } => return Some(reason.clone()),
            _ => {}
        }
    }
    None
}

pub fn injected_capabilities(decisions: &[AutomationDecision]) -> Vec<CapabilityInvocation> {
    decisions
        .iter()
        .filter_map(|decision| match decision {
            AutomationDecision::InjectCapability {
                capability:
                    CapabilityInjection {
                        capability,
                        config,
                    },
            } => Some(CapabilityInvocation {
                capability: capability.clone(),
                config: config.clone(),
            }),
            _ => None,
        })
        .collect()
}
