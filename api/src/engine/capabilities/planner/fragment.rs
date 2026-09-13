use serde_json::Value;

use crate::models::WorkflowStepDefinition;

use super::PlannerCapabilityState;

fn planner_state(global_state: &Value) -> Option<PlannerCapabilityState> {
    PlannerCapabilityState::from_global_state(global_state).ok()
}

fn planner_binding_present(global_state: &Value) -> bool {
    planner_state(global_state)
        .and_then(|state| state.binding_if_present().ok().flatten())
        .is_some()
}

pub fn planner_fragment_enabled(global_state: &Value, _step: &WorkflowStepDefinition) -> bool {
    planner_state(global_state)
        .map(|state| state.fragment_armed)
        .unwrap_or(false)
        && planner_binding_present(global_state)
}

pub fn planner_schema_enabled(global_state: &Value, _step: &WorkflowStepDefinition) -> bool {
    planner_state(global_state)
        .map(|state| state.schema_armed)
        .unwrap_or(false)
        && planner_binding_present(global_state)
}

pub fn build_planning_fragment(global_state: &Value) -> String {
    global_state
        .get("capabilities")
        .and_then(|value| value.get("inference"))
        .and_then(|value| value.get("prompt_fragments"))
        .and_then(|value| value.get("planning_fragment"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}
