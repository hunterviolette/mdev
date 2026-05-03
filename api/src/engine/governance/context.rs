use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::{
    engine::capabilities::registry::{CapabilityInvocation, CapabilityResult},
    models::{WorkflowRun, WorkflowStepDefinition},
};

use super::{ContextMutation, GovernanceDecision, GovernanceScope};

pub struct GovernanceContext<'a> {
    pub run_id: Uuid,
    pub run: &'a WorkflowRun,
    pub step: Option<&'a WorkflowStepDefinition>,
    pub stage_execution_id: Option<&'a str>,
    pub capability: Option<&'a CapabilityInvocation>,
    pub capability_result: Option<&'a CapabilityResult>,
    pub prior_results: &'a [CapabilityResult],
    pub capability_results: &'a [Value],
}

impl<'a> GovernanceContext<'a> {
    pub fn new(
        run_id: Uuid,
        run: &'a WorkflowRun,
        step: Option<&'a WorkflowStepDefinition>,
        stage_execution_id: Option<&'a str>,
        capability: Option<&'a CapabilityInvocation>,
        capability_result: Option<&'a CapabilityResult>,
        prior_results: &'a [CapabilityResult],
        capability_results: &'a [Value],
    ) -> Self {
        Self {
            run_id,
            run,
            step,
            stage_execution_id,
            capability,
            capability_result,
            prior_results,
            capability_results,
        }
    }
}

pub fn ensure_governance_slots(run: &mut WorkflowRun) {
    let root = crate::engine::ensure_engine_root(&mut run.context);
    ensure_object_slot(root, "governance");
    ensure_object_slot(root, "global_state");
    ensure_object_slot(root, "run_state");
    ensure_object_slot(root, "stage_state");
    ensure_object_slot(root, "capability_state");
}

pub fn apply_context_mutations(
    run: &mut WorkflowRun,
    decisions: &[GovernanceDecision],
    step_id: Option<&str>,
    capability: Option<&str>,
) -> Result<()> {
    ensure_governance_slots(run);

    for decision in decisions {
        if let GovernanceDecision::MutateContext { mutation } = decision {
            apply_mutation(run, mutation, step_id, capability)?;
        }
    }

    Ok(())
}

fn apply_mutation(
    run: &mut WorkflowRun,
    mutation: &ContextMutation,
    step_id: Option<&str>,
    capability: Option<&str>,
) -> Result<()> {
    let root = crate::engine::ensure_engine_root(&mut run.context);

    match mutation.scope {
        GovernanceScope::Global => {
            let slot = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
            merge_json_values(slot, &mutation.patch);
        }
        GovernanceScope::Run => {
            let slot = root.entry("run_state".to_string()).or_insert_with(|| json!({}));
            merge_json_values(slot, &mutation.patch);
        }
        GovernanceScope::Governance => {
            let slot = root.entry("governance".to_string()).or_insert_with(|| json!({}));
            merge_json_values(slot, &mutation.patch);
        }
        GovernanceScope::Stage => {
            let step_id = step_id.ok_or_else(|| anyhow!("stage-scoped governance mutation requires step_id"))?;
            let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
            let stage_state_obj = ensure_value_object(stage_state);
            let slot = stage_state_obj.entry(step_id.to_string()).or_insert_with(|| json!({}));
            merge_json_values(slot, &mutation.patch);
        }
        GovernanceScope::Capability => {
            let step_id = step_id.ok_or_else(|| anyhow!("capability-scoped governance mutation requires step_id"))?;
            let capability = capability.ok_or_else(|| anyhow!("capability-scoped governance mutation requires capability name"))?;
            let capability_state = root.entry("capability_state".to_string()).or_insert_with(|| json!({}));
            let capability_state_obj = ensure_value_object(capability_state);
            let per_stage = capability_state_obj.entry(step_id.to_string()).or_insert_with(|| json!({}));
            let per_stage_obj = ensure_value_object(per_stage);
            let slot = per_stage_obj.entry(capability.to_string()).or_insert_with(|| json!({}));
            merge_json_values(slot, &mutation.patch);
        }
    }

    Ok(())
}

fn ensure_object_slot<'a>(root: &'a mut Map<String, Value>, key: &str) -> &'a mut Value {
    let slot = root.entry(key.to_string()).or_insert_with(|| json!({}));
    if !slot.is_object() {
        *slot = json!({});
    }
    slot
}

fn ensure_value_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().expect("value must be object")
}

fn merge_json_values(dst: &mut Value, src: &Value) {
    match (dst, src) {
        (Value::Object(dst_map), Value::Object(src_map)) => {
            for (key, src_value) in src_map {
                match dst_map.get_mut(key) {
                    Some(dst_value) => merge_json_values(dst_value, src_value),
                    None => {
                        dst_map.insert(key.clone(), src_value.clone());
                    }
                }
            }
        }
        (dst_value, src_value) => {
            *dst_value = src_value.clone();
        }
    }
}
