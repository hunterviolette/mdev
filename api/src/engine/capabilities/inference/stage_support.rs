use anyhow::Result;
use serde_json::{json, Value};

use crate::{
    engine::{
        capabilities::{changeset_schema, context_export},
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

    let enabled = inference_state
        .get("prompt_fragment_enabled")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut fragments = ensure_object(
        inference_state
            .get("prompt_fragments")
            .cloned()
            .unwrap_or_else(|| json!({})),
    );

    let automation = state
        .get("execution_logic")
        .and_then(|v| v.get("automation"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let include_repo_context = automation
        .get("inject_context")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            enabled
                .get("repo_context")
                .and_then(Value::as_bool)
                .unwrap_or(step.prompt.include_repo_context)
        });

    let include_changeset_schema = automation
        .get("inject_changeset_schema")
        .and_then(Value::as_bool)
        .unwrap_or(settings.include_changeset_schema);

    let repo_context = if include_repo_context {
        let context_export_state = global_state
            .get("capabilities")
            .and_then(|v| v.get("context_export"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let repo_context = context_export::normalize_context_export_payload(
            context_export_state,
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

    if include_changeset_schema {
        let schema_empty = fragments
            .get("changeset_schema")
            .and_then(Value::as_str)
            .map(|s| s.trim().is_empty())
            .unwrap_or(true);

        if schema_empty {
            fragments
                .as_object_mut()
                .expect("prompt fragments must be object")
                .insert(
                    "changeset_schema".to_string(),
                    Value::String(changeset_schema::CHANGESET_SCHEMA_EXAMPLE.to_string()),
                );
        }
    } else {
        fragments
            .as_object_mut()
            .expect("prompt fragments must be object")
            .remove("changeset_schema");
    }

    let mut effective_enabled = enabled;
    let enabled_obj = effective_enabled
        .as_object_mut()
        .expect("prompt fragment enabled must be object");
    enabled_obj.insert("repo_context".to_string(), Value::Bool(include_repo_context));
    enabled_obj.insert("changeset_schema".to_string(), Value::Bool(include_changeset_schema));

    let prompt = compose_prompt_from_state(&effective_enabled, &fragments);

    let obj = state.as_object_mut().expect("stage state must be object");
    obj.insert("composed_prompt".to_string(), Value::String(prompt));
    obj.insert("prompt_fragment_enabled".to_string(), effective_enabled);

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

pub fn build_repo_context_prompt_fragment(repo_context: &Value) -> String {
    let save_path = repo_context
        .get("save_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("/tmp/repo_context.txt");

    format!(
        "Repo context is attached as a file generated from backend export ({save_path}). Use the uploaded attachment as repository context."
    )
}

pub fn build_inference_execution_plan(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    local_state: &Value,
    settings: InferenceStageSettings,
) -> Result<Vec<StageExecutionNode>> {
    let automation = local_state
        .get("execution_logic")
        .and_then(|v| v.get("automation"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let inference_state = global_state
        .get("capabilities")
        .and_then(|v| v.get("inference"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let enabled = inference_state
        .get("prompt_fragment_enabled")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let include_repo_context = automation
        .get("inject_context")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            enabled
                .get("repo_context")
                .and_then(Value::as_bool)
                .unwrap_or(step.prompt.include_repo_context)
        });

    let include_changeset_schema = automation
        .get("inject_changeset_schema")
        .and_then(Value::as_bool)
        .unwrap_or(settings.include_changeset_schema);

    let auto_apply_changeset = automation
        .get("auto_apply_changeset")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let repo_context = if include_repo_context {
        let context_export_state = global_state
            .get("capabilities")
            .and_then(|v| v.get("context_export"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        Some(context_export::normalize_context_export_payload(
            context_export_state,
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

    if include_changeset_schema {
        nodes.push(StageExecutionNode {
            kind: StageExecutionNodeKind::Capability,
            key: "changeset_schema".to_string(),
            enabled: true,
            config: json!({}),
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
    if include_changeset_schema {
        inference_after.push("changeset_schema".to_string());
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
            key: "gateway_model/changeset".to_string(),
            enabled: true,
            config: json!({}),
            input_mapping: json!({}),
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
