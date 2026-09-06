pub mod config;
pub mod decisions;
pub mod evaluate;
pub mod policies;
pub mod scopes;

pub use config::{
    apply_trigger,
    arm_capabilities,
    can_arm,
    control_descriptors,
    profile,
    profile_from_global_state,
    AutomationProfile,
    AutomationTrigger,
};
pub use decisions::{AutomationDecision, CapabilityInjection};
pub use evaluate::{
    after_capability,
    after_stage,
    before_capability,
    before_stage,
    injected_capabilities,
    pause_message,
};
pub use scopes::AutomationScope;

use serde_json::{json, Value};

use crate::models::WorkflowRun;

pub fn ensure_automation_slots(run: &mut WorkflowRun) {
    let root = crate::engine::ensure_engine_root(&mut run.context);
    ensure_object_slot(root, "global_state");
    let global_state = root
        .entry("global_state".to_string())
        .or_insert_with(|| json!({}));
    if !global_state.is_object() {
        *global_state = json!({});
    }
    if let Some(obj) = global_state.as_object_mut() {
        obj.entry("automation".to_string()).or_insert_with(|| json!({}));
    }
}

pub fn apply_context_mutations(
    run: &mut WorkflowRun,
    decisions: &[AutomationDecision],
    _step_id: Option<&str>,
    _capability: Option<&str>,
) -> anyhow::Result<()> {
    let root = crate::engine::ensure_engine_root(&mut run.context);
    ensure_object_slot(root, "global_state");
    let global_state = root
        .entry("global_state".to_string())
        .or_insert_with(|| json!({}));
    if !global_state.is_object() {
        *global_state = json!({});
    }

    for decision in decisions {
        if let AutomationDecision::MutateContext { mutation } = decision {
            merge_json_values(global_state, &mutation.patch);
        }
    }

    Ok(())
}

fn ensure_object_slot(root: &mut serde_json::Map<String, Value>, key: &str) {
    let slot = root.entry(key.to_string()).or_insert_with(|| json!({}));
    if !slot.is_object() {
        *slot = json!({});
    }
}

fn merge_json_values(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                merge_json_values(target.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (target, patch) => {
            *target = patch.clone();
        }
    }
}
