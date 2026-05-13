use serde_json::Value;

use crate::models::WorkflowStepDefinition;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedCapabilityLifecycle {
    Sticky,
    SingleUseGlobal,
}

pub fn shared_capability_lifecycle(primitive_key: &str) -> SharedCapabilityLifecycle {
    match primitive_key {
        "repo_context" | "changeset_schema" | "planner_fragment" => SharedCapabilityLifecycle::SingleUseGlobal,
        _ => SharedCapabilityLifecycle::Sticky,
    }
}

pub fn shared_capability_storage_key<'a>(primitive_key: &'a str) -> &'a str {
    match primitive_key {
        "repo_context" => "context_export",
        "changeset_schema" => "changeset_schema",
        "planner_fragment" => "planner",
        _ => primitive_key,
    }
}

pub fn stage_supports_shared_capability(step: &WorkflowStepDefinition, primitive_key: &str) -> bool {
    match primitive_key {
        "repo_context" => {
            step.step_type == "design"
                || step.step_type == "code"
                || step.prompt.include_repo_context
                || step
                    .execution_logic
                    .get("connections")
                    .and_then(|v| v.get("inference"))
                    .and_then(|v| v.get("repo_context"))
                    .is_some()
        }
        "changeset_schema" => {
            step.step_type == "code"
                || step.prompt.include_changeset_schema
                || step
                    .execution_logic
                    .get("connections")
                    .and_then(|v| v.get("inference"))
                    .and_then(|v| v.get("changeset_schema"))
                    .is_some()
        }
        "planner_fragment" => {
            step.step_type == "design"
                || step
                    .execution_logic
                    .get("connections")
                    .and_then(|v| v.get("inference"))
                    .and_then(|v| v.get("planning_fragment").or_else(|| v.get("planner_fragment")).or_else(|| v.get("planner")))
                    .is_some()
        }
        _ => false,
    }
}

pub fn shared_capability_enabled(global_state: &Value, primitive_key: &str, default_enabled: bool) -> bool {
    match primitive_key {
        "repo_context" => global_state
            .get("capabilities")
            .and_then(|v| v.get("inference"))
            .and_then(|v| v.get("repo_context_armed"))
            .and_then(Value::as_bool)
            .unwrap_or(default_enabled),
        "changeset_schema" => global_state
            .get("capabilities")
            .and_then(|v| v.get("inference"))
            .and_then(|v| v.get("changeset_schema_armed"))
            .and_then(Value::as_bool)
            .unwrap_or(default_enabled),
        "planner_fragment" => global_state
            .get("capabilities")
            .and_then(|v| v.get("planner"))
            .and_then(|v| v.get("fragment_armed"))
            .and_then(Value::as_bool)
            .unwrap_or(default_enabled),
        _ => {
            let storage_key = shared_capability_storage_key(primitive_key);
            global_state
                .get("capabilities")
                .and_then(|v| v.get(storage_key))
                .and_then(|v| v.get("enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(default_enabled)
        }
    }
}
