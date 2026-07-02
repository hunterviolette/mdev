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

    planner
        .get("fragment_armed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && selected_feature_id(planner).is_some()
}

pub fn planner_schema_enabled(global_state: &Value, _step: &WorkflowStepDefinition) -> bool {
    let Some(planner) = planner_state(global_state) else {
        return false;
    };

    planner
        .get("schema_armed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && selected_feature_id(planner).is_some()
}

pub fn build_planning_fragment(global_state: &Value) -> String {
    let Some(planner) = planner_state(global_state) else {
        return String::new();
    };

    if !planner
        .get("fragment_armed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return String::new();
    }

    if selected_feature_id(planner).is_none() {
        return String::new();
    }

    let selected_feature = selected_feature_payload(planner)
        .cloned()
        .unwrap_or_else(|| json!({
            "id": selected_feature_id(planner).unwrap_or_default(),
            "supervisor_run_id": planner
                .get("supervisor_run_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
        }));

    let payload = json!({
        "feature": selected_feature,
        "schema_id": planner
            .get("schema_id")
            .and_then(Value::as_str)
            .unwrap_or("supervisor_feature_plan_item_v1"),
        "preserve_rough_definition": planner
            .get("preserve_rough_definition")
            .and_then(Value::as_bool)
            .unwrap_or(true)
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
