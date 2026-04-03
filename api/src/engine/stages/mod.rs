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
    engine::capabilities::inference::stage_support::{
        build_inference_execution_plan,
        InferenceStageSettings,
    },
    models::{StageExecutionNode, StageExecutionNodeKind, WorkflowCapabilityBinding, WorkflowRun, WorkflowStepDefinition},
};

use super::capabilities::{execute_capability_invocations, CapabilityContext, CapabilityInvocation};
use super::{append_engine_event, ensure_engine_root, event_meta, merge_json_values, persist_context};

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

fn ensure_value_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().expect("value must be object")
}

fn sanitize_persisted_stage_state(local_state: Value) -> Value {
    let mut sanitized = json!({});
    let src = match local_state {
        Value::Object(map) => map,
        _ => return sanitized,
    };

    let dst = sanitized.as_object_mut().expect("sanitized stage state must be object");
    for key in ["_stage_execution_id", "review", "composed_prompt", "execution_logic", "repo_context", "prompt_fragment_enabled"] {
        if let Some(value) = src.get(key).cloned() {
            dst.insert(key.to_string(), value);
        }
    }

    sanitized
}

pub(crate) async fn clear_auto_prompt_fragments(state: &AppState, run_id: Uuid) -> Result<()> {
    let mut run = crate::engine::load_run(state, run_id).await?;
    let root = ensure_engine_root(&mut run.context);
    let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
    let global_state_obj = global_state
        .as_object_mut()
        .ok_or_else(|| anyhow!("global_state must be object"))?;
    let capabilities = global_state_obj
        .entry("capabilities".to_string())
        .or_insert_with(|| json!({}));
    let capabilities_obj = ensure_value_object(capabilities);
    let inference = capabilities_obj
        .entry("inference".to_string())
        .or_insert_with(|| json!({}));
    let inference_obj = ensure_value_object(inference);

    {
        let enabled = inference_obj
            .entry("prompt_fragment_enabled".to_string())
            .or_insert_with(|| json!({}));
        let enabled_obj = ensure_value_object(enabled);
        enabled_obj.insert("apply_error".to_string(), Value::Bool(false));
        enabled_obj.insert("compile_error".to_string(), Value::Bool(false));
    }

    {
        let fragments = inference_obj
            .entry("prompt_fragments".to_string())
            .or_insert_with(|| json!({}));
        let fragments_obj = ensure_value_object(fragments);
        fragments_obj.remove("apply_error");
        fragments_obj.remove("compile_error");
    }

    persist_context(state, run_id, &run.context).await?;
    Ok(())
}

pub async fn execute_stage(
    state: &AppState,
    run_id: Uuid,
    run: &mut WorkflowRun,
    step: &WorkflowStepDefinition,
) -> Result<StageOutcome> {
    let stage_execution_id = Uuid::new_v4().to_string();
    let stage_started_at = Instant::now();

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

    let root = ensure_engine_root(&mut run.context);
    let global_state = root.get("global_state").cloned().unwrap_or_else(|| json!({}));
    let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
    let stage_state_obj = stage_state
        .as_object_mut()
        .ok_or_else(|| anyhow!("stage_state must be object"))?;
    let existing_local_state = stage_state_obj
        .get(step.id.as_str())
        .cloned()
        .unwrap_or_else(|| json!({}));

    let repo_ref = global_state
        .get("resources")
        .and_then(|v| v.get("repo"))
        .and_then(|v| v.get("repo_ref"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(run.repo_ref.as_str())
        .to_string();

    let mut local_state = match existing_local_state {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    };
    let local_state_obj = local_state
        .as_object_mut()
        .ok_or_else(|| anyhow!("stage local state must be object"))?;
    local_state_obj.insert(
        "_stage_execution_id".to_string(),
        Value::String(stage_execution_id.clone()),
    );

    let prepared_local_state = prepare_stage_local_state(repo_ref.as_str(), &global_state, step, local_state)?;
    let plan = resolve_effective_execution_plan(&global_state, repo_ref.as_str(), step, &prepared_local_state)?;
    let execution_local_state = materialize_capability_runtime_state(prepared_local_state.clone(), &global_state, repo_ref.as_str());
    let capability_results = run_capability_plan(state, run_id, repo_ref.as_str(), step, &execution_local_state, &plan).await?;
    let capability_failed = capability_results
        .iter()
        .any(|item| item.get("ok").and_then(Value::as_bool) == Some(false));

    let branch = resolve_stage_branch(step, &prepared_local_state, capability_failed, &capability_results);

    {
        let root = ensure_engine_root(&mut run.context);

        let mut merged_local_state = sanitize_persisted_stage_state(prepared_local_state.clone());
        if let Some(patch) = branch.patch.clone() {
            if let Some(global_patch) = patch.get("global_state") {
                let global_state_slot = root
                    .entry("global_state".to_string())
                    .or_insert_with(|| json!({}));
                merge_json_values(global_state_slot, global_patch);
            }
            if let Some(local_patch) = patch.get("local_state") {
                merge_json_values(&mut merged_local_state, local_patch);
            }
        }

        let stage_state_slot = root
            .entry("stage_state".to_string())
            .or_insert_with(|| json!({}));
        let stage_state_obj = stage_state_slot
            .as_object_mut()
            .ok_or_else(|| anyhow!("stage_state must be object"))?;
        stage_state_obj.insert(step.id.clone(), merged_local_state);
    }

    persist_context(state, run_id, &run.context).await?;

    let outcome = StageOutcome {
        ok: !capability_failed,
        disposition: branch.disposition.clone(),
        message: branch.message.clone(),
        capability_results: capability_results.clone(),
        local_state: prepared_local_state,
    };

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
            "disposition": format_disposition(&outcome.disposition),
            "duration_ms": i64::try_from(stage_started_at.elapsed().as_millis()).unwrap_or(i64::MAX),
            "capability_results": outcome.capability_results,
            "event_meta": event_meta(Some(stage_execution_id.as_str()), None, None, true)
        }),
    )
    .await?;

    Ok(outcome)
}

fn prepare_stage_local_state(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    local_state: Value,
) -> Result<Value> {
    match step.step_type.as_str() {
        "code" => code_stage::prepare_stage_state(repo_ref, global_state, step, local_state),
        "compile" => compile_stage::prepare_stage_state(step, local_state),
        "review" => review_stage::prepare_stage_state(step, local_state),
        _ => design_stage::prepare_stage_state(repo_ref, global_state, step, local_state),
    }
}

#[derive(Debug, Clone)]
struct StageBranch {
    disposition: StageDisposition,
    message: String,
    patch: Option<Value>,
}

fn resolve_stage_branch(
    step: &WorkflowStepDefinition,
    local_state: &Value,
    capability_failed: bool,
    capability_results: &[Value],
) -> StageBranch {
    let runtime_logic = local_state
        .get("execution_logic")
        .cloned()
        .unwrap_or_else(|| step.execution_logic.clone());

    let branch_key = if capability_failed { "on_error" } else { "on_success" };
    let branch = runtime_logic
        .get(branch_key)
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));

    let patch = if capability_failed {
        build_branch_patch(step, &branch, capability_results)
    } else {
        branch.get("patch").cloned()
    };

    let disposition = parse_stage_disposition(step, branch_key, &branch, capability_failed);

    StageBranch {
        disposition: disposition.clone(),
        message: branch
            .get("message")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| default_branch_message(step, capability_failed, &disposition)),
        patch,
    }
}

fn build_branch_patch(step: &WorkflowStepDefinition, branch: &Value, capability_results: &[Value]) -> Option<Value> {
    if let Some(patch) = branch.get("patch") {
        return Some(patch.clone());
    }

    let descriptor = branch.get("patch_from_capability")?;
    let capability = descriptor.get("capability").and_then(Value::as_str)?;
    let mode = descriptor.get("mode").and_then(Value::as_str).unwrap_or("");

    match (step.step_type.as_str(), capability, mode) {
        ("compile", "compile_commands", "compile_error_to_code_prompt") => {
            Some(compile_stage::build_compile_error_patch(capability_results))
        }
        ("code", "gateway_model/changeset", "apply_error_to_code_prompt") => {
            Some(code_stage::build_apply_error_patch(capability_results))
        }
        _ => None,
    }
}

fn parse_stage_disposition(
    step: &WorkflowStepDefinition,
    branch_key: &str,
    branch: &Value,
    capability_failed: bool,
) -> StageDisposition {
    let disposition = branch
        .get("disposition")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if capability_failed {
                "error"
            } else if pause_on_enter(step) && branch_key == "on_success" {
                "paused"
            } else {
                "success"
            }
        });

    match disposition {
        "success" => StageDisposition::Success,
        "error" => StageDisposition::Error,
        "paused" => StageDisposition::Paused,
        "retry_stage" => StageDisposition::RetryStage,
        "stay" => StageDisposition::Stay,
        "move_back" => StageDisposition::MoveBack,
        "move_to_step" => branch
            .get("target_step_id")
            .and_then(Value::as_str)
            .map(|value| StageDisposition::MoveToStep(value.to_string()))
            .unwrap_or(StageDisposition::Stay),
        "outcome" => branch
            .get("name")
            .and_then(Value::as_str)
            .map(|value| StageDisposition::Outcome(value.to_string()))
            .unwrap_or(StageDisposition::Stay),
        "error_code" => branch
            .get("code")
            .and_then(Value::as_str)
            .map(|value| StageDisposition::ErrorCode(value.to_string()))
            .unwrap_or(StageDisposition::Error),
        _ => {
            if capability_failed {
                StageDisposition::Error
            } else {
                StageDisposition::Success
            }
        }
    }
}

fn default_branch_message(
    step: &WorkflowStepDefinition,
    capability_failed: bool,
    disposition: &StageDisposition,
) -> String {
    match disposition {
        StageDisposition::Paused => format!("{} stage completed and is paused.", step.name),
        StageDisposition::RetryStage => format!("{} stage requires a retry.", step.name),
        StageDisposition::MoveBack => format!("{} stage requested move back.", step.name),
        StageDisposition::MoveToStep(target) => format!("{} stage requested move to step '{}'.", step.name, target),
        StageDisposition::Outcome(name) => format!("{} stage completed with outcome '{}'.", step.name, name),
        StageDisposition::Stay => format!("{} stage completed and remains active.", step.name),
        StageDisposition::ErrorCode(code) => format!("{} stage failed with code '{}'.", step.name, code),
        StageDisposition::Error => format!("{} stage failed during backend workflow execution.", step.name),
        StageDisposition::Success => {
            if capability_failed {
                format!("{} stage failed during backend workflow execution.", step.name)
            } else {
                format!("{} stage completed successfully through backend workflow engine.", step.name)
            }
        }
    }
}

async fn run_capability_plan(
    state: &AppState,
    run_id: Uuid,
    repo_ref: &str,
    step: &WorkflowStepDefinition,
    local_state: &Value,
    plan: &[StageExecutionNode],
) -> Result<Vec<Value>> {
    let queue = plan
        .iter()
        .filter(|node| node.enabled && node.kind == StageExecutionNodeKind::Capability)
        .map(|node| CapabilityInvocation {
            capability: node.key.clone(),
            config: node.config.clone(),
        })
        .collect::<Vec<_>>();

    if queue.is_empty() {
        return Ok(Vec::new());
    }

    let ctx = CapabilityContext {
        state,
        run_id,
        repo_ref,
        step,
        local_state,
    };

    let results = execute_capability_invocations(ctx, queue).await?;
    Ok(results
        .into_iter()
        .map(|item| {
            json!({
                "key": item.capability,
                "ok": item.ok,
                "result": item.payload
            })
        })
        .collect())
}

fn materialize_capability_runtime_state(stage_state: Value, global_state: &Value, repo_ref: &str) -> Value {
    let mut local_state = match stage_state {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    };

    let obj = local_state
        .as_object_mut()
        .expect("stage local state must be object");

    let mut resources = global_state
        .get("resources")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if resources
        .get("repo")
        .and_then(|v| v.get("repo_ref"))
        .and_then(Value::as_str)
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
    {
        let resources_obj = ensure_value_object(&mut resources);
        resources_obj.insert(
            "repo".to_string(),
            json!({
                "repo_ref": repo_ref,
                "git_ref": "WORKTREE"
            }),
        );
    }

    obj.insert("resources".to_string(), resources);
    obj.insert(
        "capabilities".to_string(),
        global_state
            .get("capabilities")
            .cloned()
            .unwrap_or_else(|| json!({})),
    );

    if !obj.contains_key("execution") {
        obj.insert("execution".to_string(), json!({}));
    }

    local_state
}

fn resolve_effective_execution_plan(
    global_state: &Value,
    repo_ref: &str,
    step: &WorkflowStepDefinition,
    local_state: &Value,
) -> Result<Vec<StageExecutionNode>> {
    match step.step_type.as_str() {
        "code" => build_inference_execution_plan(
            repo_ref,
            global_state,
            step,
            local_state,
            InferenceStageSettings {
                include_changeset_schema: step.prompt.include_changeset_schema,
            },
        ),
        "design" => build_inference_execution_plan(
            repo_ref,
            global_state,
            step,
            local_state,
            InferenceStageSettings {
                include_changeset_schema: false,
            },
        ),
        "compile" => Ok(vec![StageExecutionNode {
            kind: StageExecutionNodeKind::Capability,
            key: "compile_commands".to_string(),
            enabled: true,
            config: json!({}),
            input_mapping: json!({}),
            output_mapping: json!({}),
            run_after: vec![],
            condition: Value::Null,
        }]),
        _ => {
            if !step.execution_plan.is_empty() {
                Ok(step.execution_plan.clone())
            } else {
                Ok(synthesize_execution_plan(&step.capabilities))
            }
        }
    }
}

fn synthesize_execution_plan(bindings: &[WorkflowCapabilityBinding]) -> Vec<StageExecutionNode> {
    bindings
        .iter()
        .filter(|binding| binding.enabled)
        .map(|binding| StageExecutionNode {
            kind: StageExecutionNodeKind::Capability,
            key: binding.capability.clone(),
            enabled: true,
            config: binding.config.clone(),
            input_mapping: binding.input_mapping.clone(),
            output_mapping: binding.output_mapping.clone(),
            run_after: Vec::new(),
            condition: Value::Null,
        })
        .collect()
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

pub(crate) fn pause_on_enter(step: &WorkflowStepDefinition) -> bool {
    step.config
        .get("pause_policy")
        .and_then(|v| v.get("pause_on_enter"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn format_disposition(disposition: &StageDisposition) -> String {
    match disposition {
        StageDisposition::Success => "success".to_string(),
        StageDisposition::Error => "error".to_string(),
        StageDisposition::ErrorCode(code) => format!("error_code:{}", code),
        StageDisposition::Paused => "paused".to_string(),
        StageDisposition::RetryStage => "retry_stage".to_string(),
        StageDisposition::MoveToStep(step_id) => format!("move_to_step:{}", step_id),
        StageDisposition::MoveBack => "move_back".to_string(),
        StageDisposition::Outcome(name) => format!("outcome:{}", name),
        StageDisposition::Stay => "stay".to_string(),
    }
}
