use serde_json::{json, Value};

use crate::models::WorkflowStepDefinition;

fn planner_state(global_state: &Value) -> Option<&Value> {
    global_state
        .get("capabilities")
        .and_then(|value| value.get("planner"))
}

pub fn planner_fragment_enabled(global_state: &Value, _step: &WorkflowStepDefinition) -> bool {
    let Some(planner) = planner_state(global_state) else {
        return false;
    };

    selected_feature_id(planner).is_some()
}

pub fn planner_schema_enabled(global_state: &Value, _step: &WorkflowStepDefinition) -> bool {
    let Some(planner) = planner_state(global_state) else {
        return false;
    };

    selected_feature_id(planner).is_some()
}

pub fn build_planning_fragment(global_state: &Value) -> String {
    let Some(planner) = planner_state(global_state) else {
        return String::new();
    };

    if selected_feature_id(planner).is_none() {
        return String::new();
    }

    let Some(selected_feature) = selected_feature_payload(planner).cloned() else {
        return String::new();
    };

    let payload = json!({
        "feature": selected_feature
    });

    serde_json::to_string_pretty(&payload).unwrap_or_default()
}

fn selected_feature_payload(planner: &Value) -> Option<&Value> {
    planner
        .get("selected_feature")
        .or_else(|| planner.get("feature"))
        .or_else(|| planner.get("feature_plan_item"))
        .filter(|value| value.is_object())
}

fn selected_feature_id(planner: &Value) -> Option<String> {
    planner
        .get("selected_feature_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}
