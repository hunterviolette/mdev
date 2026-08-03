use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::engine::{ensure_engine_root, WorkflowRun};

use super::planner::PlannerCapabilityState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationProfile {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_new_session_capabilities")]
    pub new_session: Vec<String>,
}

impl Default for AutomationProfile {
    fn default() -> Self {
        Self {
            enabled: true,
            new_session: default_new_session_capabilities(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationTrigger {
    NewInferenceSession,
}

fn default_enabled() -> bool {
    true
}

fn default_new_session_capabilities() -> Vec<String> {
    vec![
        "repo_context".to_string(),
        "changeset_schema".to_string(),
        "planner_fragment".to_string(),
    ]
}

fn ensure_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }

    value.as_object_mut().expect("value must be object")
}

fn run_global_state(run: &WorkflowRun) -> Option<&Value> {
    run.context
        .get("workflow_engine")
        .and_then(|value| value.get("global_state"))
}

pub fn profile_from_global_state(global_state: &Value) -> AutomationProfile {
    global_state
        .get("capabilities")
        .and_then(|value| value.get("automation"))
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub fn profile(run: &WorkflowRun) -> AutomationProfile {
    run_global_state(run)
        .map(profile_from_global_state)
        .unwrap_or_default()
}

fn planner_feature_selected(run: &WorkflowRun) -> bool {
    let Some(global_state) = run_global_state(run) else {
        return false;
    };

    PlannerCapabilityState::from_global_state(global_state)
        .ok()
        .and_then(|state| state.binding_if_present().ok().flatten())
        .is_some()
}

pub fn can_arm(run: &WorkflowRun, capability: &str) -> bool {
    match capability {
        "planner_fragment" | "planner_schema" | "planner_apply" => {
            planner_feature_selected(run)
        }
        _ => true,
    }
}

fn set_arm_state(run: &mut WorkflowRun, capability: &str, armed: bool) {
    let root = ensure_engine_root(&mut run.context);
    let global_state = root
        .entry("global_state".to_string())
        .or_insert_with(|| json!({}));
    let global_state = ensure_object(global_state);
    let capabilities = global_state
        .entry("capabilities".to_string())
        .or_insert_with(|| json!({}));
    let capabilities = ensure_object(capabilities);

    match capability {
        "repo_context" => {
            let inference = capabilities
                .entry("inference".to_string())
                .or_insert_with(|| json!({}));
            ensure_object(inference)
                .insert("repo_context_armed".to_string(), Value::Bool(armed));
        }
        "changeset_schema" => {
            let inference = capabilities
                .entry("inference".to_string())
                .or_insert_with(|| json!({}));
            ensure_object(inference)
                .insert("changeset_schema_armed".to_string(), Value::Bool(armed));
        }
        "planner_fragment" => {
            let planner = capabilities
                .entry("planner".to_string())
                .or_insert_with(|| json!({}));
            ensure_object(planner)
                .insert("fragment_armed".to_string(), Value::Bool(armed));
        }
        "planner_schema" => {
            let planner = capabilities
                .entry("planner".to_string())
                .or_insert_with(|| json!({}));
            ensure_object(planner)
                .insert("schema_armed".to_string(), Value::Bool(armed));
        }
        "planner_apply" => {
            let planner = capabilities
                .entry("planner".to_string())
                .or_insert_with(|| json!({}));
            ensure_object(planner)
                .insert("auto_apply_armed".to_string(), Value::Bool(armed));
        }
        _ => {}
    }
}

pub fn apply_trigger(run: &mut WorkflowRun, trigger: AutomationTrigger) {
    let profile = profile(run);
    if !profile.enabled {
        return;
    }

    let capabilities = match trigger {
        AutomationTrigger::NewInferenceSession => &profile.new_session,
    };

    for capability in capabilities {
        if can_arm(run, capability) {
            set_arm_state(run, capability, true);
        }
    }
}

pub fn arm_capabilities(run: &mut WorkflowRun, capabilities: &[&str]) {
    for capability in capabilities {
        if can_arm(run, capability) {
            set_arm_state(run, capability, true);
        }
    }
}


