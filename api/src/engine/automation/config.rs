use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{
    engine::{ensure_engine_root, WorkflowRun},
    models::WorkflowAutomationControlDescriptor,
};

use crate::engine::capabilities::planner::PlannerCapabilityState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationProfile {
    #[serde(default = "default_new_session_capabilities")]
    pub new_session: Vec<String>,
    #[serde(default = "default_selected_automations")]
    pub selected: Vec<String>,
    #[serde(default = "default_inject_file_context_after_changeset_failures")]
    pub inject_file_context_after_changeset_failures: u64,
    #[serde(default = "default_inject_broad_context_after_changeset_failures")]
    pub inject_broad_context_after_changeset_failures: u64,
    #[serde(default = "default_pause_after_changeset_failures")]
    pub pause_after_changeset_failures: u64,
    #[serde(default = "default_inject_changeset_schema_after_errors")]
    pub inject_changeset_schema_after_errors: u64,
    #[serde(default = "default_pause_after_compile_errors")]
    pub pause_after_compile_errors: u64,
}

impl AutomationProfile {
    pub fn is_selected(&self, key: &str) -> bool {
        self.selected.iter().any(|item| item == key)
    }
}

impl Default for AutomationProfile {
    fn default() -> Self {
        Self {
            new_session: default_new_session_capabilities(),
            selected: default_selected_automations(),
            inject_file_context_after_changeset_failures: default_inject_file_context_after_changeset_failures(),
            inject_broad_context_after_changeset_failures: default_inject_broad_context_after_changeset_failures(),
            pause_after_changeset_failures: default_pause_after_changeset_failures(),
            inject_changeset_schema_after_errors: default_inject_changeset_schema_after_errors(),
            pause_after_compile_errors: default_pause_after_compile_errors(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationTrigger {
    NewInferenceSession,
}

fn default_new_session_capabilities() -> Vec<String> {
    vec![
        "repo_context".to_string(),
        "changeset_schema".to_string(),
        "planner_fragment".to_string(),
    ]
}

fn default_selected_automations() -> Vec<String> {
    vec![
        "inject_file_context_after_changeset_failures".to_string(),
        "inject_broad_context_after_changeset_failures".to_string(),
        "pause_after_changeset_failures".to_string(),
        "inject_changeset_schema_after_errors".to_string(),
        "pause_after_compile_errors".to_string(),
    ]
}

fn default_inject_file_context_after_changeset_failures() -> u64 {
    2
}

fn default_inject_broad_context_after_changeset_failures() -> u64 {
    4
}

fn default_pause_after_changeset_failures() -> u64 {
    6
}

fn default_inject_changeset_schema_after_errors() -> u64 {
    3
}

fn default_pause_after_compile_errors() -> u64 {
    4
}

pub fn control_descriptors() -> Vec<WorkflowAutomationControlDescriptor> {
    vec![
        WorkflowAutomationControlDescriptor {
            key: "inject_file_context_after_changeset_failures".to_string(),
            label: "Inject targeted file context".to_string(),
            description: "Inject targeted context for a file after consecutive changeset failures.".to_string(),
            section: "Changeset".to_string(),
            field_type: "integer".to_string(),
            default: json!(2),
            required_capabilities: vec!["changeset".to_string()],
        },
        WorkflowAutomationControlDescriptor {
            key: "inject_broad_context_after_changeset_failures".to_string(),
            label: "Inject broad context".to_string(),
            description: "Escalate to broad context after consecutive changeset failures for the same file.".to_string(),
            section: "Changeset".to_string(),
            field_type: "integer".to_string(),
            default: json!(4),
            required_capabilities: vec!["changeset".to_string()],
        },
        WorkflowAutomationControlDescriptor {
            key: "pause_after_changeset_failures".to_string(),
            label: "Pause workflow".to_string(),
            description: "Pause the workflow after consecutive changeset failures for the same file.".to_string(),
            section: "Changeset".to_string(),
            field_type: "integer".to_string(),
            default: json!(6),
            required_capabilities: vec!["changeset".to_string()],
        },
        WorkflowAutomationControlDescriptor {
            key: "inject_changeset_schema_after_errors".to_string(),
            label: "Re-arm changeset schema".to_string(),
            description: "Re-arm the changeset schema after consecutive invalid changeset payload errors.".to_string(),
            section: "Changeset".to_string(),
            field_type: "integer".to_string(),
            default: json!(3),
            required_capabilities: vec!["changeset".to_string()],
        },
        WorkflowAutomationControlDescriptor {
            key: "pause_after_compile_errors".to_string(),
            label: "Pause workflow".to_string(),
            description: "Pause the workflow after consecutive compile failures.".to_string(),
            section: "Compile".to_string(),
            field_type: "integer".to_string(),
            default: json!(4),
            required_capabilities: vec!["compile_commands".to_string()],
        },
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
        .get("automation")
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


