pub mod registry;
pub mod automation;
pub mod context_export;
pub mod repo_sync;
pub mod changeset;
pub mod compile_commands;
pub mod shared_dependencies;
pub mod terminal_runtime;
pub mod filesystem;
pub mod git;
pub mod git_patch_payload;
pub mod review_validation;
pub mod inference;
pub mod operator_checkpoint;
pub mod qa_environment;
pub mod planner;
pub mod sap;

pub fn capability_enabled(
    global_state: &serde_json::Value,
    capability: &str,
    default_enabled: bool,
) -> bool {
    match capability {
        "repo_context" => global_state
            .get("capabilities")
            .and_then(|value| value.get("inference"))
            .and_then(|value| value.get("repo_context_armed"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default_enabled),
        "changeset_schema" => global_state
            .get("capabilities")
            .and_then(|value| value.get("inference"))
            .and_then(|value| value.get("changeset_schema_armed"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default_enabled),
        "planner_fragment" => global_state
            .get("capabilities")
            .and_then(|value| value.get("planner"))
            .and_then(|value| value.get("fragment_armed"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default_enabled),
        "planner_schema" => global_state
            .get("capabilities")
            .and_then(|value| value.get("planner"))
            .and_then(|value| value.get("schema_armed"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default_enabled),
        "planner_apply" => global_state
            .get("capabilities")
            .and_then(|value| value.get("planner"))
            .and_then(|value| value.get("auto_apply_armed"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default_enabled),
        key => global_state
            .get("capabilities")
            .and_then(|value| value.get(key))
            .and_then(|value| value.get("enabled"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default_enabled),
    }
}

pub use registry::{
    CapabilityContext,
    CapabilityInvocation,
    execute_capability_invocations,
};
