mod code_stage;
mod compile_stage;
mod design_stage;
mod review_stage;
mod merge_patches_stage;
mod qa_stage;
mod sap_export_stage;
mod sap_import_stage;
mod sap_syntax_stage;
mod stage_utility;

use std::{future::Future, pin::Pin, time::Instant};

use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use uuid::Uuid;
use tokio_util::sync::CancellationToken;

use crate::{
    engine::normalize_inference_arm_state,
    app_state::AppState,
    models::{StageExecutionNode, StageExecutionNodeKind, WorkflowRun, WorkflowStepDefinition},
};

use super::capabilities::{
    execute_capability_invocations,
    planner,
    CapabilityContext,
    CapabilityInvocation,
};
use super::governance;
use super::{append_engine_event, ensure_engine_root, event_meta, merge_json_values, persist_context};

pub struct StageRegistration {
    implementation: &'static dyn Stage,
}

impl StageRegistration {
    pub const fn new(implementation: &'static dyn Stage) -> Self {
        Self { implementation }
    }
}

inventory::collect!(StageRegistration);

fn stage_registry() -> &'static std::collections::HashMap<&'static str, &'static dyn Stage> {
    static REGISTRY: std::sync::OnceLock<
        std::collections::HashMap<&'static str, &'static dyn Stage>,
    > = std::sync::OnceLock::new();

    REGISTRY.get_or_init(|| {
        let mut stages = std::collections::HashMap::new();

        for registration in inventory::iter::<StageRegistration> {
            let stage = registration.implementation;
            let stage_type = stage.stage_type();

            if stages.insert(stage_type, stage).is_some() {
                panic!("duplicate stage type registered: {stage_type}");
            }
        }

        stages
    })
}

pub struct StagePrepareContext<'a> {
    pub repo_ref: &'a str,
    pub global_state: &'a Value,
    pub step: &'a WorkflowStepDefinition,
}

pub struct StagePlanContext<'a> {
    pub run: &'a mut WorkflowRun,
    pub automatic_execution: bool,
    pub global_state: &'a Value,
    pub repo_ref: &'a str,
    pub step: &'a WorkflowStepDefinition,
    pub local_state: &'a Value,
}

pub trait Stage: Send + Sync {
    fn stage_type(&self) -> &'static str;

    fn capabilities(&self) -> StageCapabilities;

    fn prepare_state(
        &self,
        context: StagePrepareContext<'_>,
        local_state: Value,
    ) -> Result<Value>;

    fn build_execution_plan(
        &self,
        context: StagePlanContext<'_>,
    ) -> Result<Vec<StageExecutionNode>>;

    fn lifecycle_hook(&self) -> Box<dyn StageLifecycleHook> {
        Box::new(NoopStageLifecycleHook)
    }
}

fn stage_for_step(step: &WorkflowStepDefinition) -> &'static dyn Stage {
    let registry = stage_registry();

    registry
        .get(step.step_type.as_str())
        .copied()
        .or_else(|| registry.get("design").copied())
        .expect("design stage must be registered")
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum StageDisposition {
    Success,
    Error,
    ErrorCode(String),
    Paused,
    RetryStage,
    MoveNext,
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

#[derive(Debug, Clone)]
pub struct StageCapabilities {
    keys: Vec<&'static str>,
}

impl StageCapabilities {
    pub fn new<const N: usize>(keys: [&'static str; N]) -> Self {
        Self {
            keys: keys.into_iter().collect(),
        }
    }

    pub fn empty() -> Self {
        Self { keys: Vec::new() }
    }

    pub fn contains(&self, key: &str) -> bool {
        self.keys.iter().any(|item| *item == key)
    }

    pub fn keys(&self) -> &[&'static str] {
        &self.keys
    }
}

pub fn configured_execution_plan(step: &WorkflowStepDefinition) -> Vec<StageExecutionNode> {
    if !step.execution_plan.is_empty() {
        return step.execution_plan.clone();
    }

    step.capabilities
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

pub fn capability_contract_for_stage(step: &WorkflowStepDefinition) -> StageCapabilities {
    stage_for_step(step).capabilities()
}

pub fn stage_supports_capability(
    step: &WorkflowStepDefinition,
    capability: &str,
) -> bool {
    capability_contract_for_stage(step).contains(capability)
}

fn ensure_value_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().expect("value must be object")
}

pub fn rearm_session_scoped_inference_inputs(
    run: &mut WorkflowRun,
    _step: &WorkflowStepDefinition,
) {
    normalize_inference_arm_state(run);
    crate::engine::capabilities::automation::apply_trigger(
        run,
        crate::engine::capabilities::automation::AutomationTrigger::NewInferenceSession,
    );
}

fn reset_session_scoped_inference_state(state: &AppState, run: &mut WorkflowRun) -> bool {
    let root = ensure_engine_root(&mut run.context);
    let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
    let global_state_obj = ensure_value_object(global_state);
    let capabilities = global_state_obj
        .entry("capabilities".to_string())
        .or_insert_with(|| json!({}));
    let capabilities_obj = ensure_value_object(capabilities);
    let inference = capabilities_obj
        .entry("inference".to_string())
        .or_insert_with(|| json!({}));
    let inference_obj = ensure_value_object(inference);

    inference_obj.remove("next_prompt_fragments");
    inference_obj.remove("active_prompt_fragments");

    if let Some(enabled) = inference_obj
        .get_mut("prompt_fragment_enabled")
        .and_then(Value::as_object_mut)
    {
        enabled.remove("apply_error");
        enabled.remove("compile_error");
        if enabled.is_empty() {
            inference_obj.remove("prompt_fragment_enabled");
        }
    }

    if let Some(fragments) = inference_obj
        .get_mut("prompt_fragments")
        .and_then(Value::as_object_mut)
    {
        fragments.remove("apply_error");
        fragments.remove("compile_error");
        if fragments.is_empty() {
            inference_obj.remove("prompt_fragments");
        }
    }

    let connection_runtime = inference_obj
        .entry("connection_runtime".to_string())
        .or_insert_with(|| json!({}));
    let connection_runtime_obj = ensure_value_object(connection_runtime);

    let current_process_session_id = state.process_session_id().to_string();
    let persisted_process_session_id = connection_runtime_obj
        .get("process_session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if persisted_process_session_id == current_process_session_id {
        return false;
    }

    connection_runtime_obj.clear();
    connection_runtime_obj.insert(
        "process_session_id".to_string(),
        Value::String(current_process_session_id),
    );

    true
}

pub(crate) async fn clear_auto_prompt_fragments(state: &AppState, run_id: Uuid) -> Result<()> {
    state.orchestration_inputs.clear_run(run_id);
    Ok(())
}

fn sanitize_stage_execution_prefix(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string();

    if sanitized.is_empty() {
        "stage".to_string()
    } else {
        sanitized
    }
}

fn record_stage_execution_id(context: &mut Value, stage_execution_id: &str) {
    let root = ensure_engine_root(context);
    let stage_executions = root
        .entry("stage_executions".to_string())
        .or_insert_with(|| json!([]));
    if !stage_executions.is_array() {
        *stage_executions = json!([]);
    }
    let stage_executions = stage_executions
        .as_array_mut()
        .expect("stage_executions must be array");

    if !stage_executions.iter().any(|item| item.as_str() == Some(stage_execution_id)) {
        stage_executions.push(Value::String(stage_execution_id.to_string()));
    }
}

fn capability_user_input_result(capability_results: &[Value]) -> Option<Value> {
    capability_results.iter().find_map(|item| {
        let capability = item
            .get("key")
            .or_else(|| item.get("capability"))
            .and_then(Value::as_str)?;
        let mut result = item
            .get("result")
            .or_else(|| item.get("payload"))?
            .clone();

        if result.get("needs_user_response").and_then(Value::as_bool) != Some(true) {
            return None;
        }

        if let Some(result_obj) = result.as_object_mut() {
            result_obj
                .entry("capability".to_string())
                .or_insert_with(|| Value::String(capability.to_string()));
        }

        Some(result)
    })
}

pub struct StageExitContext<'a> {
    pub state: &'a AppState,
    pub run_id: Uuid,
    pub step: &'a WorkflowStepDefinition,
    pub next_step_id: Option<&'a str>,
}

pub trait StageLifecycleHook: Send + Sync {
    fn on_restart<'a>(
        &'a self,
        _context: StageExitContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn on_exit<'a>(
        &'a self,
        context: StageExitContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
}

struct NoopStageLifecycleHook;

impl StageLifecycleHook for NoopStageLifecycleHook {
    fn on_exit<'a>(
        &'a self,
        _context: StageExitContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

pub fn lifecycle_hook_for_step(step: &WorkflowStepDefinition) -> Box<dyn StageLifecycleHook> {
    stage_for_step(step).lifecycle_hook()
}

pub fn record_stage_checkpoint_continue(
    run: &mut WorkflowRun,
    step: &WorkflowStepDefinition,
    phase: &str,
) -> Result<()> {
    if phase != "before_stage" {
        return Ok(());
    }

    let root = ensure_engine_root(&mut run.context);
    let run_state = root
        .entry("run_state".to_string())
        .or_insert_with(|| json!({}));
    let run_state = run_state
        .as_object_mut()
        .ok_or_else(|| anyhow!("run_state must be object"))?;
    run_state.insert(
        "stage_checkpoint_approval".to_string(),
        json!({
            "step_id": step.id,
            "phase": phase
        }),
    );

    Ok(())
}

pub async fn invoke_stage_restart_hook(
    state: &AppState,
    run_id: Uuid,
    step: &WorkflowStepDefinition,
) -> Result<()> {
    lifecycle_hook_for_step(step)
        .on_restart(StageExitContext {
            state,
            run_id,
            step,
            next_step_id: Some(step.id.as_str()),
        })
        .await
}

pub async fn invoke_stage_exit_hook(
    state: &AppState,
    run_id: Uuid,
    step: &WorkflowStepDefinition,
    next_step_id: Option<&str>,
) -> Result<()> {
    lifecycle_hook_for_step(step)
        .on_exit(StageExitContext {
            state,
            run_id,
            step,
            next_step_id,
        })
        .await
}

pub async fn execute_stage(
    state: &AppState,
    run_id: Uuid,
    run: &mut WorkflowRun,
    step: &WorkflowStepDefinition,
    automatic_execution: bool,
    cancellation: CancellationToken,
) -> Result<StageOutcome> {
    if cancellation.is_cancelled() {
        return Err(anyhow!("workflow execution was cancelled"));
    }
    let stage_execution_id = format!("{}-{}", sanitize_stage_execution_prefix(&step.step_type), Uuid::new_v4());
    let stage_started_at = Instant::now();

    reset_session_scoped_inference_state(state, run);
    crate::engine::clear_prepared_inference_step(run);

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

    let supervisor_context = run.context.get("supervisor").cloned();
    let root = ensure_engine_root(&mut run.context);
    let mut global_state = root.get("global_state").cloned().unwrap_or_else(|| json!({}));
    if let Some(supervisor_context) = supervisor_context {
        if !global_state.is_object() {
            global_state = json!({});
        }
        if let Some(global_obj) = global_state.as_object_mut() {
            global_obj.insert("supervisor".to_string(), supervisor_context);
        }
    }
    let existing_local_state = root
        .get("stage_overrides")
        .and_then(Value::as_object)
        .and_then(|obj| obj.get(step.id.as_str()))
        .cloned()
        .or_else(|| {
            root
                .get("stage_state")
                .and_then(Value::as_object)
                .and_then(|obj| obj.get(step.id.as_str()))
                .cloned()
        })
        .unwrap_or_else(|| json!({}));

    let repo_ref = global_state
        .get("resources")
        .and_then(|v| v.get("repo"))
        .and_then(|v| v.get("repo_ref"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(run.repo_ref.as_str())
        .to_string();

    planner::apply_repo_planner_capability(&state.db, &mut global_state, repo_ref.as_str()).await?;
    root.insert("global_state".to_string(), global_state.clone());

    let mut execution_global_state = global_state.clone();
    planner::hydrate_repo_planner_prompt_fragment(
        &state.db,
        &mut execution_global_state,
    )
    .await?;

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
    let execution_state = local_state_obj
        .entry("execution".to_string())
        .or_insert_with(|| json!({}));
    let execution_state_obj = ensure_value_object(execution_state);
    execution_state_obj.insert(
        "mode".to_string(),
        Value::String(if automatic_execution {
            "automatic".to_string()
        } else {
            "manual".to_string()
        }),
    );

    let prepared_local_state = prepare_stage_local_state(
        repo_ref.as_str(),
        &execution_global_state,
        step,
        local_state,
    )?;
    if stage_for_step(step).stage_type() == "merge_patches" {
        return merge_patches_stage::execute_stage(
            state,
            run_id,
            run,
            step,
            repo_ref.as_str(),
            prepared_local_state,
        )
        .await;
    }
    let plan = resolve_effective_execution_plan(
        run,
        automatic_execution,
        &execution_global_state,
        repo_ref.as_str(),
        step,
        &prepared_local_state,
    )?;
    persist_context(state, run_id, &run.context).await?;
    let prepared_local_state_obj = prepared_local_state
        .as_object()
        .ok_or_else(|| anyhow!("prepared stage local state must be object"))?;

    let execution_local_state = materialize_capability_runtime_state(prepared_local_state.clone(), &global_state, repo_ref.as_str());
    let capability_results = match run_capability_plan(
        state,
        run_id,
        repo_ref.as_str(),
        step,
        &execution_local_state,
        &plan,
        cancellation.clone(),
    )
    .await
    {
        Ok(results) => results,
        Err(error) => {
            let cancelled = cancellation.is_cancelled();
            append_engine_event(
                state,
                run_id,
                Some(step.id.as_str()),
                "error",
                "stage_execution_failed",
                if cancelled {
                    "Stage execution was cancelled"
                } else {
                    "Stage execution failed"
                },
                json!({
                    "step_id": step.id,
                    "step_type": step.step_type,
                    "ok": false,
                    "cancelled": cancelled,
                    "execution_state": "failed",
                    "message": error.to_string(),
                    "duration_ms": i64::try_from(stage_started_at.elapsed().as_millis()).unwrap_or(i64::MAX),
                    "event_meta": event_meta(Some(stage_execution_id.as_str()), None, None, true)
                }),
            )
            .await?;
            return Err(error);
        }
    };

    if cancellation.is_cancelled() {
        append_engine_event(
            state,
            run_id,
            Some(step.id.as_str()),
            "error",
            "stage_execution_failed",
            "Stage execution was cancelled",
            json!({
                "step_id": step.id,
                "step_type": step.step_type,
                "ok": false,
                "cancelled": true,
                "execution_state": "failed",
                "message": "workflow execution was cancelled",
                "duration_ms": i64::try_from(stage_started_at.elapsed().as_millis()).unwrap_or(i64::MAX),
                "event_meta": event_meta(Some(stage_execution_id.as_str()), None, None, true)
            }),
        )
        .await?;
        return Err(anyhow!("workflow execution was cancelled"));
    }
    let capability_failed = capability_results
        .iter()
        .any(|item| item.get("ok").and_then(Value::as_bool) == Some(false));

    let after_decisions = governance::after_stage(
        state,
        run_id,
        run,
        step,
        stage_execution_id.as_str(),
        &capability_results,
    )
    .await?;

    let latest_persisted_run = crate::engine::load_run(state, run_id)
        .await
        .unwrap_or_else(|_| run.clone());
    run.context = latest_persisted_run.context;

    governance::apply_context_mutations(run, &after_decisions, Some(step.id.as_str()), None)?;

    let branch = resolve_stage_branch(step, &prepared_local_state, capability_failed, &capability_results);

    if let Some(message) = governance::pause_message(&after_decisions) {
        persist_context(state, run_id, &run.context).await?;
        return Ok(StageOutcome {
            ok: false,
            disposition: StageDisposition::Paused,
            message,
            capability_results,
            local_state: Value::Object(prepared_local_state_obj.clone()),
        });
    }

    {
        let root = ensure_engine_root(&mut run.context);

        if let Some(patch) = branch.patch.clone() {
            if let Some(global_patch) = patch.get("global_state") {
                let global_state_slot = root
                    .entry("global_state".to_string())
                    .or_insert_with(|| json!({}));
                merge_json_values(global_state_slot, global_patch);
            }
        }

        if let Some(stage_overrides) = root.get_mut("stage_overrides").and_then(Value::as_object_mut) {
            stage_overrides.remove(step.id.as_str());
            if stage_overrides.is_empty() {
                root.remove("stage_overrides");
            }
        }

        if let Some(stage_state) = root.get_mut("stage_state").and_then(Value::as_object_mut) {
            stage_state.remove(step.id.as_str());
            if stage_state.is_empty() {
                root.remove("stage_state");
            }
        }

        record_stage_execution_id(&mut run.context, stage_execution_id.as_str());
    }

    persist_context(state, run_id, &run.context).await?;

    let outcome = StageOutcome {
        ok: !capability_failed,
        disposition: branch.disposition.clone(),
        message: branch.message.clone(),
        capability_results: capability_results.clone(),
        local_state: prepared_local_state,
    };

    let pending_capability_user_input = outcome.ok
        && capability_user_input_result(&outcome.capability_results).is_some();
    let user_input_payload = capability_user_input_result(&outcome.capability_results)
        .unwrap_or_else(|| json!({}));

    append_engine_event(
        state,
        run_id,
        Some(step.id.as_str()),
        if outcome.ok { "info" } else { "error" },
        if pending_capability_user_input {
            "stage_execution_state_changed"
        } else {
            "stage_execution_completed"
        },
        if pending_capability_user_input {
            "Stage execution is awaiting capability user input."
        } else {
            "Stage executed through backend workflow engine"
        },
        json!({
            "step_id": step.id,
            "step_type": step.step_type,
            "ok": outcome.ok,
            "execution_state": if pending_capability_user_input { "awaiting_user_input" } else { "completed" },
            "user_input": user_input_payload,
            "message": outcome.message,
            "disposition": format_disposition(&outcome.disposition),
            "duration_ms": if pending_capability_user_input {
                Value::Null
            } else {
                json!(i64::try_from(stage_started_at.elapsed().as_millis()).unwrap_or(i64::MAX))
            },
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
    stage_for_step(step).prepare_state(
        StagePrepareContext {
            repo_ref,
            global_state,
            step,
        },
        local_state,
    )
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

    let patch = build_branch_patch(step, &branch, capability_results);
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
        ("code", "changeset", "apply_error_to_code_prompt") => None,
        ("review", "review_validation", "review_failure_to_code_prompt") => {
            Some(review_stage::build_review_failure_patch(capability_results))
        }
        ("sap_syntax", "sap/export", "sap_syntax_success_state") => {
            Some(sap_syntax_stage::build_sap_syntax_success_patch(capability_results))
        }
        ("sap_syntax", "sap/export", "sap_syntax_error_to_code_prompt") => {
            Some(sap_syntax_stage::build_sap_syntax_error_patch(capability_results))
        }
        ("sap_export", "sap/export", "sap_execution_state") => {
            Some(sap_export_stage::build_sap_execution_patch(capability_results))
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
        "move_next" => StageDisposition::MoveNext,
        "move_back" => StageDisposition::MoveBack,
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
        StageDisposition::MoveNext => format!("{} stage requested move next.", step.name),
        StageDisposition::MoveBack => format!("{} stage requested move back.", step.name),
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
    cancellation: CancellationToken,
) -> Result<Vec<Value>> {
    if cancellation.is_cancelled() {
        return Err(anyhow!("workflow execution was cancelled"));
    }
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
        cancellation,
    };

    let results = execute_capability_invocations(ctx, queue).await?;
    Ok(results
        .into_iter()
        .map(|item| {
            let consumed_capabilities = item
                .payload
                .get("consumed_capabilities")
                .cloned()
                .unwrap_or_else(|| json!([]));

            json!({
                "key": item.capability,
                "ok": item.ok,
                "result": item.payload,
                "consumed_capabilities": consumed_capabilities
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
    run: &mut WorkflowRun,
    automatic_execution: bool,
    global_state: &Value,
    repo_ref: &str,
    step: &WorkflowStepDefinition,
    local_state: &Value,
) -> Result<Vec<StageExecutionNode>> {
    stage_for_step(step).build_execution_plan(StagePlanContext {
        run,
        automatic_execution,
        global_state,
        repo_ref,
        step,
        local_state,
    })
}

pub(crate) fn compose_prompt_from_state(
    enabled: &Value,
    fragments: &Value,
) -> String {
    let enabled_obj = enabled.as_object().cloned().unwrap_or_default();
    let fragments_obj = fragments.as_object().cloned().unwrap_or_default();
    let order = ["user_input", "review_failure", "planning_fragment", "repo_context", "changeset_schema", "planner_schema"];

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

fn format_disposition(disposition: &StageDisposition) -> String {
    match disposition {
        StageDisposition::Success => "success".to_string(),
        StageDisposition::Error => "error".to_string(),
        StageDisposition::ErrorCode(code) => format!("error_code:{}", code),
        StageDisposition::Paused => "paused".to_string(),
        StageDisposition::RetryStage => "retry_stage".to_string(),
        StageDisposition::MoveNext => "move_next".to_string(),
        StageDisposition::MoveBack => "move_back".to_string(),
        StageDisposition::Outcome(name) => format!("outcome:{}", name),
        StageDisposition::Stay => "stay".to_string(),
    }
}
