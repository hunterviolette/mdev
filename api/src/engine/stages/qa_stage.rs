use std::{future::Future, pin::Pin};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::models::WorkflowStepDefinition;

use super::{
    configured_execution_plan,
    Stage,
    StageCapabilities,
    StageExecutionNode,
    StageExecutionNodeKind,
    StageExitContext,
    StageLifecycleHook,
    StagePlanContext,
    StagePrepareContext,
};

pub struct QaStage;

pub static STAGE: QaStage = QaStage;

inventory::submit! {
    super::StageRegistration::new(&STAGE)
}

impl Stage for QaStage {
    fn stage_type(&self) -> &'static str {
        "qa"
    }

    fn capabilities(&self) -> StageCapabilities {
        StageCapabilities::new(["shared_dependencies", "qa_environment"])
    }

    fn prepare_state(
        &self,
        context: StagePrepareContext<'_>,
        local_state: Value,
    ) -> Result<Value> {
        prepare_qa_state(
            context.repo_ref,
            context.global_state,
            context.step,
            local_state,
        )
    }

    fn build_execution_plan(
        &self,
        context: StagePlanContext<'_>,
    ) -> Result<Vec<StageExecutionNode>> {
        Ok(build_qa_execution_plan(
            context.run,
            context.step,
            context.automatic_execution,
        ))
    }

    fn lifecycle_hook(&self) -> Box<dyn StageLifecycleHook> {
        Box::new(QaStageLifecycleHook)
    }
}

pub struct QaStageLifecycleHook;

fn qa_entry_approved(run: &crate::models::WorkflowRun, step_id: &str) -> bool {
    run.context
        .get("workflow_engine")
        .and_then(|value| value.get("run_state"))
        .and_then(|value| value.get("stage_checkpoint_approval"))
        .and_then(|value| value.get("step_id"))
        .and_then(Value::as_str)
        == Some(step_id)
}

fn consume_qa_entry_approval(run: &mut crate::models::WorkflowRun, step_id: &str) -> bool {
    if !qa_entry_approved(run, step_id) {
        return false;
    }

    if let Some(run_state) = run
        .context
        .get_mut("workflow_engine")
        .and_then(|value| value.get_mut("run_state"))
        .and_then(Value::as_object_mut)
    {
        run_state.remove("stage_checkpoint_approval");
    }

    true
}

fn checkpoint_node(phase: &str, message: &str, run_after: Vec<String>) -> StageExecutionNode {
    StageExecutionNode {
        kind: StageExecutionNodeKind::Capability,
        key: "operator_checkpoint".to_string(),
        enabled: true,
        config: json!({
            "phase": phase,
            "message": message,
            "recommended_disposition": "continue_auto",
            "available_dispositions": ["continue_auto", "pause_error"]
        }),
        input_mapping: json!({}),
        output_mapping: json!({}),
        run_after,
        condition: Value::Null,
    }
}

impl StageLifecycleHook for QaStageLifecycleHook {
    fn on_exit<'a>(
        &'a self,
        context: StageExitContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let run_id = context.run_id.to_string();
            let results = context
                .state
                .process_registry
                .terminate_deployment(
                    run_id.as_str(),
                    context.step.id.as_str(),
                    true,
                )
                .await;

            let failures = results.iter().filter(|result| result.is_err()).count();

            tracing::info!(
                run_id = %context.run_id,
                step_id = %context.step.id,
                next_step_id = ?context.next_step_id,
                terminated_processes = results.len().saturating_sub(failures),
                termination_failures = failures,
                "terminated QA processes on stage exit"
            );

            Ok(())
        })
    }
}

fn build_qa_execution_plan(
    run: &mut crate::models::WorkflowRun,
    step: &WorkflowStepDefinition,
    automatic_execution: bool,
) -> Vec<StageExecutionNode> {
    let mut plan = configured_execution_plan(step);
    plan.retain(|node| {
        node.kind != StageExecutionNodeKind::Capability
            || node.key != "operator_checkpoint"
    });

    if automatic_execution && !consume_qa_entry_approval(run, step.id.as_str()) {
        return vec![checkpoint_node(
            "before_stage",
            "QA is ready to start. Continue to launch the QA environment.",
            vec![],
        )];
    }

    let run_after = plan
        .iter()
        .filter(|node| node.enabled && node.kind == StageExecutionNodeKind::Capability)
        .map(|node| node.key.clone())
        .collect::<Vec<_>>();

    plan.push(checkpoint_node(
        "after_stage",
        "QA is running and ready for testing. Continue when QA validation is complete.",
        run_after,
    ));

    plan
}

fn prepare_qa_state(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    mut local_state: Value,
) -> Result<Value> {
    let qa = step
        .execution
        .qa
        .as_ref()
        .ok_or_else(|| anyhow!("QA stage '{}' is missing execution.qa configuration", step.id))?;

    if qa.environment.services.is_empty() {
        return Err(anyhow!("QA stage '{}' must define at least one service", step.id));
    }

    let state = local_state
        .as_object_mut()
        .ok_or_else(|| anyhow!("QA stage local state must be an object"))?;

    state.insert("resources".to_string(), json!({
        "repo": {
            "repo_ref": repo_ref,
            "git_ref": "WORKTREE"
        }
    }));
    state.insert("global_state".to_string(), global_state.clone());
    state.insert("qa".to_string(), serde_json::to_value(qa)?);
    state.insert("execution".to_string(), json!({
        "qa": qa
    }));

    let execution_logic = state
        .entry("execution_logic".to_string())
        .or_insert_with(|| step.execution_logic.clone());

    if !execution_logic.is_object() {
        *execution_logic = json!({});
    }

    let execution_logic = execution_logic
        .as_object_mut()
        .ok_or_else(|| anyhow!("QA execution logic must be an object"))?;

    execution_logic
        .entry("on_success".to_string())
        .or_insert_with(|| json!({
            "disposition": "paused",
            "message": "QA environment is running and requires operator approval."
        }));

    execution_logic
        .entry("on_error".to_string())
        .or_insert_with(|| json!({
            "disposition": "stay",
            "message": "QA environment failed to start or become ready."
        }));

    Ok(local_state)
}
