mod code_stage;
mod compile_stage;
mod design_stage;
mod review_stage;

use std::time::Instant;

use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    models::{StageExecutionNode, StageExecutionNodeKind, WorkflowRun, WorkflowStepDefinition},
};

use super::capabilities::{execute_capability_invocations, CapabilityContext, CapabilityInvocation};
use super::{append_engine_event, ensure_engine_root, event_meta, persist_context};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum StageDisposition {
    Success,
    Error,
    ErrorCode(String),
    Paused,
    RetryStage,
    MoveToStep(String),
    MoveBack,
    Outcome(String),
    Stay,
}

#[derive(Debug, Clone)]
pub struct StageOutcome {
    pub ok: bool,
    pub disposition: StageDisposition,
    pub message: String,
    pub capability_results: Vec<Value>,
    pub local_state: Value,
}

pub async fn execute_stage(
    state: &AppState,
    run_id: Uuid,
    run: &mut WorkflowRun,
    step: &WorkflowStepDefinition,
) -> Result<StageOutcome> {
    let global_inference = run.context.get("model_inference").cloned();
    let root = ensure_engine_root(&mut run.context);
    let global_repo_context = root
        .get("global_state")
        .and_then(Value::as_object)
        .and_then(|m| m.get("repo_context"))
        .cloned();

    let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
    let stage_state_obj = stage_state
        .as_object_mut()
        .ok_or_else(|| anyhow!("stage_state must be object"))?;
    let local_state = stage_state_obj
        .entry(step.id.clone())
        .or_insert_with(|| json!({}))
        .clone();

    let effective_local_state = merge_stage_with_global_state(local_state, global_repo_context, global_inference);
    let mut hydrated_local_state = hydrate_stage_local_state(&run.repo_ref, step, effective_local_state);
    let stage_execution_id = Uuid::new_v4().to_string();
    if let Some(obj) = hydrated_local_state.as_object_mut() {
        obj.insert("_stage_execution_id".to_string(), Value::String(stage_execution_id.clone()));
    }
    stage_state_obj.insert(step.id.clone(), hydrated_local_state.clone());
    persist_context(state, run_id, &run.context).await?;

    append_engine_event(
        state,
        run_id,
        Some(step.id.as_str()),
        "info",
        "stage_execution_started",
        "Stage execution started",
        json!({
            "step_id": step.id,
            "step_type": step.step_type,
            "event_meta": event_meta(Some(stage_execution_id.as_str()), None, None, true)
        }),
    )
    .await?;

    let stage_started_at = Instant::now();

    let plan = resolve_effective_execution_plan(&hydrated_local_state, step);

    tracing::info!(
        run_id = %run_id,
        step_id = %step.id,
        step_type = %step.step_type,
        stage_execution_id = %stage_execution_id,
        plan_len = plan.len(),
        "stage execution plan resolved"
    );

    let policy_kind = step
        .execution_logic
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("default_stage_policy");

    let outcome = match policy_kind {
        "code_stage_policy" => {
            code_stage::execute_code_stage(state, run_id, &run.repo_ref, step, &hydrated_local_state, &plan).await?
        }
        "compile_stage_policy" => {
            compile_stage::execute(state, run_id, &run.repo_ref, step, hydrated_local_state.clone(), &plan).await?
        }
        "review_stage_policy" => {
            review_stage::execute(state, run_id, &run.repo_ref, step, hydrated_local_state.clone(), &plan).await?
        }
        _ => {
            design_stage::execute(state, run_id, &run.repo_ref, step, hydrated_local_state.clone(), &plan).await?
        }
    };

    let stage_execution_id = outcome.local_state
        .get("_stage_execution_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    tracing::info!(
        run_id = %run_id,
        step_id = %step.id,
        step_type = %step.step_type,
        stage_execution_id = %stage_execution_id,
        ok = outcome.ok,
        disposition = ?outcome.disposition,
        capability_result_count = outcome.capability_results.len(),
        "stage execution outcome resolved"
    );

    append_engine_event(
        state,
        run_id,
        Some(step.id.as_str()),
        if outcome.ok { "info" } else { "error" },
        "stage_execution_completed",
        "Stage executed through backend workflow engine",
        json!({
            "step_id": step.id,
            "step_type": step.step_type,
            "ok": outcome.ok,
            "message": outcome.message,
            "duration_ms": i64::try_from(stage_started_at.elapsed().as_millis()).unwrap_or(i64::MAX),
            "capability_results": outcome.capability_results,
            "event_meta": event_meta(Some(stage_execution_id.as_str()), None, None, true)
        }),
    )
    .await?;

    Ok(outcome)
}

pub(crate) async fn run_capability_plan(
    state: &AppState,
    run_id: Uuid,
    repo_ref: &str,
    step: &WorkflowStepDefinition,
    local_state: &Value,
    plan: &[StageExecutionNode],
) -> Result<Vec<Value>> {
    let ctx = CapabilityContext {
        state,
        run_id,
        repo_ref,
        step,
        local_state,
    };

    let queue = capability_invocations_from_plan(step, plan);
    let results = execute_capability_invocations(ctx, queue).await?;

    Ok(results
        .into_iter()
        .map(|item| {
            json!({
                "key": item.capability,
                "ok": item.ok,
                "result": item.payload,
            })
        })
        .collect())
}

fn resolve_effective_execution_plan(local_state: &Value, step: &WorkflowStepDefinition) -> Vec<StageExecutionNode> {
    let override_plan = local_state
        .get("execution_plan_override")
        .cloned()
        .and_then(|value| serde_json::from_value::<Vec<StageExecutionNode>>(value).ok())
        .unwrap_or_default();

    if !override_plan.is_empty() {
        return override_plan;
    }

    if !step.execution_plan.is_empty() {
        return step.execution_plan.clone();
    }

    synthesize_execution_plan(step)
}

fn capability_invocations_from_plan(
    step: &WorkflowStepDefinition,
    plan: &[StageExecutionNode],
) -> Vec<CapabilityInvocation> {
    let mut queue: Vec<CapabilityInvocation> = plan
        .iter()
        .filter(|node| node.enabled && node.kind == StageExecutionNodeKind::Capability)
        .map(|node| CapabilityInvocation {
            capability: node.key.clone(),
            config: node.config.clone(),
        })
        .collect();

    if queue.is_empty() {
        let fallback = if !step.execution_plan.is_empty() {
            step.execution_plan.clone()
        } else {
            synthesize_execution_plan(step)
        };
        queue = fallback
            .into_iter()
            .filter(|node| node.enabled && node.kind == StageExecutionNodeKind::Capability)
            .map(|node| CapabilityInvocation {
                capability: node.key,
                config: node.config,
            })
            .collect();
    }

    queue
}

fn merge_stage_with_global_state(
    local_state: Value,
    global_repo_context: Option<Value>,
    global_inference: Option<Value>,
) -> Value {
    let mut state = match local_state {
        Value::Object(map) => map,
        _ => Map::new(),
    };

    if !state.contains_key("repo_context") {
        if let Some(repo_context) = global_repo_context {
            state.insert("repo_context".to_string(), repo_context);
        }
    }

    if let Some(inference) = global_inference {
        state.insert("inference".to_string(), inference);
    }

    Value::Object(state)
}

fn hydrate_stage_local_state(repo_ref: &str, step: &WorkflowStepDefinition, local_state: Value) -> Value {
    let mut state = match local_state {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    };

    {
        let obj = state.as_object_mut().expect("stage local state must be object");
        let enabled = obj
            .entry("prompt_fragment_enabled".to_string())
            .or_insert_with(|| json!({}));
        if !enabled.is_object() {
            *enabled = json!({});
        }
    }

    {
        let obj = state.as_object_mut().expect("stage local state must be object");
        let fragments = obj
            .entry("prompt_fragments".to_string())
            .or_insert_with(|| json!({}));
        if !fragments.is_object() {
            *fragments = json!({});
        }
    }

    let include_changeset_schema = step.id == "code" && step.prompt.include_changeset_schema;
    if include_changeset_schema {
        let schema_enabled = state
            .get("prompt_fragment_enabled")
            .and_then(Value::as_object)
            .and_then(|m| m.get("changeset_schema"))
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let schema_empty = state
            .get("prompt_fragments")
            .and_then(Value::as_object)
            .and_then(|m| m.get("changeset_schema"))
            .and_then(Value::as_str)
            .map(|s| s.trim().is_empty())
            .unwrap_or(true);

        if schema_enabled && schema_empty {
            let obj = state.as_object_mut().expect("stage local state must be object");
            let fragments = obj
                .get_mut("prompt_fragments")
                .and_then(Value::as_object_mut)
                .expect("prompt_fragments must be object");
            fragments.insert(
                "changeset_schema".to_string(),
                Value::String(crate::engine::capabilities::changeset_schema::CHANGESET_SCHEMA_EXAMPLE.to_string()),
            );
        }
    }

    let include_repo_context = state
        .get("prompt_fragment_enabled")
        .and_then(Value::as_object)
        .and_then(|m| m.get("repo_context"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if include_repo_context {
        let repo_context = normalize_repo_context_payload(repo_ref, state.get("repo_context").cloned());
        let obj = state.as_object_mut().expect("stage local state must be object");
        obj.insert("repo_context".to_string(), repo_context);
        let fragments = obj
            .get_mut("prompt_fragments")
            .and_then(Value::as_object_mut)
            .expect("prompt_fragments must be object");
        fragments.remove("repo_context");
    }

    let prompt = compose_prompt_from_state(
        state.get("prompt_fragment_enabled").unwrap_or(&Value::Null),
        state.get("prompt_fragments").unwrap_or(&Value::Null),
    );

    let obj = state.as_object_mut().expect("stage local state must be object");
    obj.insert("composed_prompt".to_string(), Value::String(prompt));
    state
}

pub(crate) fn compose_prompt_from_state(enabled: &Value, fragments: &Value) -> String {
    let enabled_obj = enabled.as_object().cloned().unwrap_or_default();
    let fragments_obj = fragments.as_object().cloned().unwrap_or_default();

    let order = [
        "repo_context",
        "user_input",
        "changeset_schema",
        "apply_error",
        "compile_error",
    ];

    let mut parts = Vec::new();
    for key in order {
        let is_enabled = enabled_obj.get(key).and_then(Value::as_bool).unwrap_or(false);
        if !is_enabled {
            continue;
        }
        let value = fragments_obj.get(key).and_then(Value::as_str).unwrap_or("").trim();
        if !value.is_empty() {
            parts.push(value.to_string());
        }
    }
    parts.join("\n\n")
}

pub(crate) fn normalize_repo_context_payload(repo_ref: &str, repo_context: Option<Value>) -> Value {
    let mut value = repo_context.unwrap_or_else(|| json!({}));
    if !value.is_object() {
        value = json!({});
    }
    let obj = value.as_object_mut().expect("repo_context must be object");
    obj.entry("repo_ref".to_string())
        .or_insert(Value::String(repo_ref.to_string()));
    obj.entry("git_ref".to_string())
        .or_insert(Value::String("WORKTREE".to_string()));
    obj.entry("save_path".to_string())
        .or_insert(Value::String("/tmp/repo_context.txt".to_string()));
    Value::Object(obj.clone())
}

fn synthesize_execution_plan(step: &WorkflowStepDefinition) -> Vec<StageExecutionNode> {
    let mut plan = Vec::new();
    for binding in step.capabilities.iter().filter(|b| b.enabled) {
        plan.push(StageExecutionNode {
            kind: StageExecutionNodeKind::Capability,
            key: binding.capability.clone(),
            enabled: true,
            config: binding.config.clone(),
            input_mapping: binding.input_mapping.clone(),
            output_mapping: binding.output_mapping.clone(),
            run_after: Vec::new(),
            condition: Value::Null,
        });
    }
    plan
}

pub(crate) fn pause_on_enter(step: &WorkflowStepDefinition) -> bool {
    step.config
        .get("pause_policy")
        .and_then(|v| v.get("pause_on_enter"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}
