use anyhow::Result;
use serde_json::{json, Value};

use crate::{
    engine::{
        capabilities::{binding_specs, changeset::schema as changeset_schema, context_export, planner},
        stages::compose_prompt_from_state,
    },
    models::{StageExecutionNode, StageExecutionNodeKind, WorkflowStepDefinition},
};

pub struct InferenceStageSettings {
    pub include_changeset_schema: bool,
}

pub fn prepare_inference_stage_state(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    local_state: Value,
    settings: InferenceStageSettings,
) -> Result<Value> {
    let mut state = ensure_object(local_state);

    let inference_state = global_state
        .get("capabilities")
        .and_then(|v| v.get("inference"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let mut fragments = ensure_object(
        inference_state
            .get("prompt_fragments")
            .cloned()
            .unwrap_or_else(|| json!({})),
    );

    let include_repo_context = shared_inference_primitive_enabled(
        global_state,
        step,
        "repo_context",
        step.prompt.include_repo_context,
    );

    let include_changeset_schema = shared_inference_primitive_enabled(
        global_state,
        step,
        "changeset_schema",
        settings.include_changeset_schema,
    );

    let user_input = state
        .get("prompt")
        .and_then(|v| v.get("user_input"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    if let Some(user_input_fragment) = user_input.clone() {
        fragments
            .as_object_mut()
            .expect("prompt fragments must be object")
            .insert("user_input".to_string(), Value::String(user_input_fragment));
    } else {
        fragments
            .as_object_mut()
            .expect("prompt fragments must be object")
            .remove("user_input");
    }

    let include_planning_fragment = shared_inference_primitive_enabled(
        global_state,
        step,
        "planner_fragment",
        false,
    ) && planner::planner_fragment_enabled(global_state, step);
    let planning_fragment = if include_planning_fragment {
        planner::build_planning_fragment(global_state)
    } else {
        String::new()
    };

    if include_planning_fragment && !planning_fragment.trim().is_empty() {
        fragments
            .as_object_mut()
            .expect("prompt fragments must be object")
            .insert("planning_fragment".to_string(), Value::String(planning_fragment));
    } else {
        fragments
            .as_object_mut()
            .expect("prompt fragments must be object")
            .remove("planning_fragment");
    }

    let repo_context = if include_repo_context {
        let repo_context = context_export::normalize_context_export_payload(
            resolve_context_export_state(global_state),
            global_state.get("resources").and_then(|v| v.get("repo")).cloned(),
            repo_ref,
        );
        let repo_context_fragment = build_repo_context_prompt_fragment(&repo_context);

        fragments
            .as_object_mut()
            .expect("prompt fragments must be object")
            .insert("repo_context".to_string(), Value::String(repo_context_fragment));

        Some(repo_context)
    } else {
        fragments
            .as_object_mut()
            .expect("prompt fragments must be object")
            .remove("repo_context");
        None
    };

    let changeset_schema_fragment = if include_changeset_schema {
        global_state
            .get("capabilities")
            .and_then(|v| v.get("changeset_schema"))
            .and_then(|v| v.get("schema"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| changeset_schema::CHANGESET_SCHEMA_EXAMPLE.to_string())
    } else {
        String::new()
    };

    if include_changeset_schema {
        fragments
            .as_object_mut()
            .expect("prompt fragments must be object")
            .insert(
                "changeset_schema".to_string(),
                Value::String(changeset_schema_fragment),
            );
    } else {
        fragments
            .as_object_mut()
            .expect("prompt fragments must be object")
            .remove("changeset_schema");
    }

    let include_planner_schema = planner::planner_schema_enabled(global_state, step);

    let planner_schema_fragment = if include_planner_schema {
        planner::schema::PLANNER_SCHEMA_PROMPT_FRAGMENT.to_string()
    } else {
        String::new()
    };

    if include_planner_schema {
        fragments
            .as_object_mut()
            .expect("prompt fragments must be object")
            .insert(
                "planner_schema".to_string(),
                Value::String(planner_schema_fragment),
            );
    } else {
        fragments
            .as_object_mut()
            .expect("prompt fragments must be object")
            .remove("planner_schema");
    }

    let transient_prompt_fragments = collect_active_transient_prompt_fragments(global_state);

    let mut effective_enabled = json!({});
    let enabled_obj = effective_enabled
        .as_object_mut()
        .expect("prompt fragment enabled must be object");
    enabled_obj.insert("repo_context".to_string(), Value::Bool(include_repo_context));
    enabled_obj.insert(
        "user_input".to_string(),
        Value::Bool(user_input.is_some()),
    );
    enabled_obj.insert("planning_fragment".to_string(), Value::Bool(include_planning_fragment && fragments.get("planning_fragment").and_then(Value::as_str).map(|value| !value.trim().is_empty()).unwrap_or(false)));
    enabled_obj.insert("changeset_schema".to_string(), Value::Bool(include_changeset_schema));
    enabled_obj.insert("planner_schema".to_string(), Value::Bool(include_planner_schema));

    let prompt = compose_prompt_from_state(&effective_enabled, &fragments, &transient_prompt_fragments);

    let obj = state.as_object_mut().expect("stage state must be object");
    obj.insert("composed_prompt".to_string(), Value::String(prompt));
    obj.insert("prompt_fragment_enabled".to_string(), effective_enabled);
    obj.insert(
        "transient_prompt_fragments".to_string(),
        Value::Array(
            transient_prompt_fragments
                .iter()
                .map(|item| Value::String(item.clone()))
                .collect(),
        ),
    );

    if let Some(repo_context) = repo_context {
        obj.insert("repo_context".to_string(), repo_context);
    } else {
        obj.remove("repo_context");
    }

    Ok(state)
}

pub fn auto_apply_enabled(step: &WorkflowStepDefinition, state: &Value) -> bool {
    state
        .get("execution_logic")
        .and_then(|v| v.get("automation"))
        .and_then(|v| v.get("auto_apply_changeset"))
        .and_then(Value::as_bool)
        .or_else(|| {
            state
                .get("execution")
                .and_then(|v| v.get("changeset_apply"))
                .and_then(|v| v.get("auto_apply"))
                .and_then(Value::as_bool)
        })
        .or_else(|| {
            step.execution_logic
                .get("automation")
                .and_then(|v| v.get("auto_apply_changeset"))
                .and_then(Value::as_bool)
        })
        .or_else(|| {
            step.execution
                .changeset_apply
                .get("auto_apply")
                .and_then(Value::as_bool)
        })
        .unwrap_or(false)
}

fn collect_active_transient_prompt_fragments(global_state: &Value) -> Vec<String> {
    global_state
        .get("capabilities")
        .and_then(|v| v.get("inference"))
        .and_then(|v| v.get("active_prompt_fragments"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn resolve_context_export_state(global_state: &Value) -> Value {
    let baseline = global_state
        .get("capabilities")
        .and_then(|v| v.get("context_export"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    baseline
        .get("single_use_override")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or(baseline)
}

pub fn build_repo_context_prompt_fragment(repo_context: &Value) -> String {
    let save_path = repo_context
        .get("save_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("/tmp/repo_context.txt");
    let inline = repo_context
        .get("inline_repo_context_in_prompt")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if inline {
        "Repo context is included inline under ### REPO CONTEXT in this prompt. Use it as repository context.".to_string()
    } else {
        format!(
            "Repo context is attached as a file generated from backend export ({save_path}). Use the uploaded attachment as repository context."
        )
    }
}

fn shared_inference_primitive_enabled(
    global_state: &Value,
    step: &WorkflowStepDefinition,
    key: &str,
    _default_enabled: bool,
) -> bool {
    if !binding_specs::stage_supports_shared_capability(step, key) {
        return false;
    }

    binding_specs::shared_capability_enabled(global_state, key, false)
}

pub fn build_inference_execution_plan(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    local_state: &Value,
    settings: InferenceStageSettings,
) -> Result<Vec<StageExecutionNode>> {
    let include_repo_context = shared_inference_primitive_enabled(
        global_state,
        step,
        "repo_context",
        step.prompt.include_repo_context,
    );

    let include_changeset_schema = shared_inference_primitive_enabled(
        global_state,
        step,
        "changeset_schema",
        settings.include_changeset_schema,
    );

    let auto_apply_changeset = auto_apply_enabled(step, local_state);

    let repo_context = if include_repo_context {
        Some(context_export::normalize_context_export_payload(
            resolve_context_export_state(global_state),
            global_state.get("resources").and_then(|v| v.get("repo")).cloned(),
            repo_ref,
        ))
    } else {
        None
    };

    build_execution_plan(
        include_repo_context,
        include_changeset_schema,
        auto_apply_changeset,
        repo_context,
    )
}

fn build_execution_plan(
    include_repo_context: bool,
    include_changeset_schema: bool,
    auto_apply_changeset: bool,
    repo_context: Option<Value>,
) -> Result<Vec<StageExecutionNode>> {
    let mut nodes = Vec::new();

    if include_repo_context {
        nodes.push(StageExecutionNode {
            kind: StageExecutionNodeKind::Capability,
            key: "context_export".to_string(),
            enabled: true,
            config: repo_context.unwrap_or_else(|| json!({})),
            input_mapping: json!({}),
            output_mapping: json!({}),
            run_after: vec![],
            condition: Value::Null,
        });
    }

    let mut inference_after = Vec::new();
    if include_repo_context {
        inference_after.push("context_export".to_string());
    }

    nodes.push(StageExecutionNode {
        kind: StageExecutionNodeKind::Capability,
        key: "inference".to_string(),
        enabled: true,
        config: json!({}),
        input_mapping: json!({}),
        output_mapping: json!({}),
        run_after: inference_after,
        condition: Value::Null,
    });

    if auto_apply_changeset {
        nodes.push(StageExecutionNode {
            kind: StageExecutionNodeKind::Capability,
            key: "changeset".to_string(),
            enabled: true,
            config: json!({}),
            input_mapping: json!({
                "changeset": {
                    "from": "inference.payload"
                }
            }),
            output_mapping: json!({}),
            run_after: vec!["inference".to_string()],
            condition: Value::Null,
        });
    }

    Ok(nodes)
}

fn ensure_object(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    }
}
