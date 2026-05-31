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
    binding_specs::shared_capability_enabled(
        global_state,
        primitive_key,
        inferred_default_enabled(step, primitive_key),
    )
}

fn set_shared_capability_enabled(capabilities_obj: &mut Map<String, Value>, primitive_key: &str, enabled: Option<bool>) {
    if primitive_key == "planner_fragment" {
        let planner = capabilities_obj
            .entry("planner".to_string())
            .or_insert_with(|| json!({}));
        let planner_obj = ensure_object(planner);
        match enabled {
            Some(value) => {
                planner_obj.insert("fragment_armed".to_string(), Value::Bool(value));
            }
            None => {
                planner_obj.remove("fragment_armed");
            }
        }
        return;
    }

    let inference = capabilities_obj
        .entry("inference".to_string())
        .or_insert_with(|| json!({}));
    let inference_obj = ensure_object(inference);
    let key = match primitive_key {
        "repo_context" => "repo_context_armed",
        "changeset_schema" => "changeset_schema_armed",
        _ => "enabled",
    };
    match enabled {
        Some(value) => {
            inference_obj.insert(key.to_string(), Value::Bool(value));
        }
        None => {
            inference_obj.remove(key);
        }
    }
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
            "changeset_schema_armed": changeset_schema_armed,
            "planner_fragment_armed": effective_enabled(&global_snapshot, selected_step, "planner_fragment")
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

    for primitive_key in ["repo_context", "changeset_schema", "planner_fragment"] {
        if !binding_specs::stage_supports_shared_capability(step, primitive_key) {
            continue;
        }
        if !matches!(
            binding_specs::shared_capability_lifecycle(primitive_key),
            binding_specs::SharedCapabilityLifecycle::SingleUseGlobal
        ) {
            continue;
        }

        set_shared_capability_enabled(capabilities_obj, primitive_key, Some(false));
    }

    if let Some(inference_obj) = capabilities_obj
        .get_mut("inference")
        .and_then(Value::as_object_mut)
    {
        inference_obj.remove("shared_inference_state");
    }
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

        set_shared_capability_enabled(capabilities_obj, primitive_key, None);
    }

    set_shared_capability_enabled(capabilities_obj, "planner_fragment", Some(false));
}
