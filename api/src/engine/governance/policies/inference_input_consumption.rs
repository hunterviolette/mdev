use anyhow::Result;
use serde_json::{json, Map, Value};

use crate::{
    engine::{capabilities::binding_specs, stages},
    models::{WorkflowRun, WorkflowStepDefinition},
};

use super::super::{
    decisions::{ContextMutation, GovernanceDecision},
    scopes::GovernanceScope,
};

pub fn after_stage(
    run: &WorkflowRun,
    step: &WorkflowStepDefinition,
    _stage_execution_id: &str,
    capability_results: &[Value],
) -> Result<Vec<GovernanceDecision>> {
    let contract = stages::capability_contract_for_stage(step);
    if !contract.contains("inference") {
        return Ok(Vec::new());
    }

    let Some(inference_result) = capability_results
        .iter()
        .find(|item| item.get("key").and_then(Value::as_str) == Some("inference"))
    else {
        return Ok(Vec::new());
    };

    let consumed = inference_result
        .get("consumed_capabilities")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| {
            inference_result
                .get("result")
                .and_then(|v| v.get("consumed_capabilities"))
                .and_then(Value::as_array)
                .cloned()
        })
        .unwrap_or_default();

    let workflow_engine = run
        .context
        .get("workflow_engine")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let global_state = workflow_engine
        .get("global_state")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let consumed_contains = |key: &str| consumed.iter().any(|item| item.as_str() == Some(key));

    let mut inference_patch = Map::new();
    let mut context_export_patch = Map::new();
    let mut planner_patch = Map::new();

    if consumed_contains("repo_context")
        && binding_specs::shared_capability_enabled(&global_state, "repo_context", false)
    {
        inference_patch.insert("repo_context_armed".to_string(), Value::Bool(false));
        context_export_patch.insert("single_use_override".to_string(), Value::Null);
    }

    if consumed_contains("changeset_schema")
        && binding_specs::shared_capability_enabled(&global_state, "changeset_schema", false)
    {
        inference_patch.insert("changeset_schema_armed".to_string(), Value::Bool(false));
    }

    if consumed_contains("planner_fragment")
        && binding_specs::shared_capability_enabled(&global_state, "planner_fragment", false)
    {
        planner_patch.insert("fragment_armed".to_string(), Value::Bool(false));
    }

    if consumed_contains("prompt_fragments") {
        inference_patch.insert("active_prompt_fragments".to_string(), Value::Array(Vec::new()));
        inference_patch.insert("next_prompt_fragments".to_string(), Value::Array(Vec::new()));
    }

    if inference_patch.is_empty() && context_export_patch.is_empty() && planner_patch.is_empty() {
        return Ok(Vec::new());
    }

    let mut capabilities_patch = Map::new();
    if !inference_patch.is_empty() {
        capabilities_patch.insert("inference".to_string(), Value::Object(inference_patch));
    }
    if !context_export_patch.is_empty() {
        capabilities_patch.insert("context_export".to_string(), Value::Object(context_export_patch));
    }
    if !planner_patch.is_empty() {
        capabilities_patch.insert("planner".to_string(), Value::Object(planner_patch));
    }

    Ok(vec![GovernanceDecision::MutateContext {
        mutation: ContextMutation {
            scope: GovernanceScope::Global,
            patch: json!({
                "capabilities": Value::Object(capabilities_patch)
            }),
        },
    }])
}
