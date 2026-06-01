pub mod decisions;
pub mod evaluate;
pub mod policies;
pub mod scopes;

pub use decisions::{CapabilityInjection, GovernanceDecision};
pub use evaluate::{
    after_capability,
    after_stage,
    before_capability,
    before_stage,
    injected_capabilities,
    pause_message,
};
pub use scopes::GovernanceScope;

use serde_json::{json, Value};

use crate::models::WorkflowRun;

pub fn ensure_governance_slots(run: &mut WorkflowRun) {
    let root = crate::engine::ensure_engine_root(&mut run.context);
    ensure_object_slot(root, "global_state");
    let global_state = root
        .entry("global_state".to_string())
        .or_insert_with(|| json!({}));
    if !global_state.is_object() {
        *global_state = json!({});
    }
    if let Some(obj) = global_state.as_object_mut() {
        obj.entry("governance".to_string()).or_insert_with(|| json!({}));
    }
}

pub fn apply_context_mutations(
    run: &mut WorkflowRun,
    decisions: &[GovernanceDecision],
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
        if let GovernanceDecision::MutateContext { mutation } = decision {
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
