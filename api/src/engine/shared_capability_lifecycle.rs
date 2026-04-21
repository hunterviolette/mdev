use serde_json::{json, Map, Value};

use crate::models::{WorkflowRun, WorkflowStepDefinition};

use super::{capabilities::binding_specs, ensure_engine_root};

fn ensure_object<'a>(value: &'a mut Value) -> &'a mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().expect("value must be object")
}

fn inferred_default_enabled(step: Option<&WorkflowStepDefinition>, primitive_key: &str) -> bool {
    step.map(|item| binding_specs::stage_supports_shared_capability(item, primitive_key))
        .unwrap_or(false)
}

fn effective_enabled(global_state: &Value, step: Option<&WorkflowStepDefinition>, primitive_key: &str) -> bool {
    let storage_key = binding_specs::shared_capability_storage_key(primitive_key);
    global_state
        .get("capabilities")
        .and_then(|v| v.get(storage_key))
        .and_then(|v| v.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| inferred_default_enabled(step, primitive_key))
}

pub fn refresh_shared_capability_arm_state(
    run: &mut WorkflowRun,
    selected_step: Option<&WorkflowStepDefinition>,
) {
    let global_snapshot = {
        let root = ensure_engine_root(&mut run.context);
        let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
        let global_state_obj = ensure_object(global_state);
        Value::Object(global_state_obj.clone())
    };

    let repo_context_armed = effective_enabled(&global_snapshot, selected_step, "repo_context");
    let changeset_schema_armed = effective_enabled(&global_snapshot, selected_step, "changeset_schema");

    let root = ensure_engine_root(&mut run.context);
    let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
    let global_state_obj = ensure_object(global_state);
    let capabilities = global_state_obj
        .entry("capabilities".to_string())
        .or_insert_with(|| json!({}));
    let capabilities_obj = ensure_object(capabilities);
    let inference = capabilities_obj
        .entry("inference".to_string())
        .or_insert_with(|| json!({}));
    let inference_obj = ensure_object(inference);

    inference_obj.insert(
        "shared_inference_state".to_string(),
        json!({
            "repo_context_armed": repo_context_armed,
            "changeset_schema_armed": changeset_schema_armed
        }),
    );
}

pub fn consume_shared_capabilities_for_step(run: &mut WorkflowRun, step: &WorkflowStepDefinition) {
    let root = ensure_engine_root(&mut run.context);
    let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
    let global_state_obj = ensure_object(global_state);
    let capabilities = global_state_obj
        .entry("capabilities".to_string())
        .or_insert_with(|| json!({}));
    let capabilities_obj = ensure_object(capabilities);

    for primitive_key in ["repo_context", "changeset_schema"] {
        if !binding_specs::stage_supports_shared_capability(step, primitive_key) {
            continue;
        }
        if !matches!(
            binding_specs::shared_capability_lifecycle(primitive_key),
            binding_specs::SharedCapabilityLifecycle::SingleUseGlobal
        ) {
            continue;
        }

        let storage_key = binding_specs::shared_capability_storage_key(primitive_key).to_string();
        let capability_value = capabilities_obj
            .entry(storage_key)
            .or_insert_with(|| json!({}));
        let capability_obj = ensure_object(capability_value);
        capability_obj.insert("enabled".to_string(), Value::Bool(false));
    }

    refresh_shared_capability_arm_state(run, Some(step));
}

pub fn reset_session_scoped_shared_capability_lifecycle(run: &mut WorkflowRun) {
    let root = ensure_engine_root(&mut run.context);
    let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
    let global_state_obj = ensure_object(global_state);
    let capabilities = global_state_obj
        .entry("capabilities".to_string())
        .or_insert_with(|| json!({}));
    let capabilities_obj = ensure_object(capabilities);

    for primitive_key in ["repo_context", "changeset_schema"] {
        if !matches!(
            binding_specs::shared_capability_lifecycle(primitive_key),
            binding_specs::SharedCapabilityLifecycle::SingleUseGlobal
        ) {
            continue;
        }

        let storage_key = binding_specs::shared_capability_storage_key(primitive_key).to_string();
        let capability_value = capabilities_obj
            .entry(storage_key)
            .or_insert_with(|| json!({}));
        let capability_obj = ensure_object(capability_value);
        capability_obj.remove("enabled");
    }
}
