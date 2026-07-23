use std::{future::Future, pin::Pin};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::{
    engine::stages::capability_contract::StageCapabilities,
    models::WorkflowStepDefinition,
};

use super::{
    StageExecutionNode,
    StageExecutionNodeKind,
    StageExitContext,
    StageLifecycleHook,
};

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

fn set_qa_entry_approval(run: &mut crate::models::WorkflowRun, step_id: &str) {
    let engine = run
        .context
        .as_object_mut()
        .expect("workflow context must be object")
        .entry("workflow_engine".to_string())
        .or_insert_with(|| json!({}));
    let engine = engine.as_object_mut().expect("workflow_engine must be object");
    let run_state = engine
        .entry("run_state".to_string())
        .or_insert_with(|| json!({}));
    let run_state = run_state.as_object_mut().expect("run_state must be object");
    run_state.insert(
        "stage_checkpoint_approval".to_string(),
        json!({
            "step_id": step_id,
            "phase": "before_stage"
        }),
    );
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
    fn prepare_plan(
        &self,
        run: &mut crate::models::WorkflowRun,
        step: &WorkflowStepDefinition,
        automatic_execution: bool,
        _local_state: &Value,
        mut plan: Vec<StageExecutionNode>,
    ) -> Vec<StageExecutionNode> {
        plan.retain(|node| {
            node.kind != StageExecutionNodeKind::Capability
                || node.key != "operator_checkpoint"
        });

        if automatic_execution && !consume_qa_entry_approval(run, step.id.as_str()) {
            plan.insert(
                0,
                checkpoint_node(
                    "before_stage",
                    "QA is ready to start. Continue to launch the QA environment.",
                    vec![],
                ),
            );
            return plan;
        }

        let run_after = plan
            .iter()
            .filter(|node| {
                node.enabled
                    && node.kind == StageExecutionNodeKind::Capability
            })
            .map(|node| node.key.clone())
            .collect::<Vec<_>>();

        plan.push(checkpoint_node(
            "after_stage",
            "QA is running and ready for testing. Continue when QA validation is complete.",
            run_after,
        ));

        plan
    }

    fn on_checkpoint_continue(
        &self,
        run: &mut crate::models::WorkflowRun,
        step: &WorkflowStepDefinition,
        phase: &str,
    ) -> Result<()> {
        if phase == "before_stage" {
            set_qa_entry_approval(run, step.id.as_str());
        }

        Ok(())
    }

    fn on_restart<'a>(
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
                terminated_processes = results.len().saturating_sub(failures),
                termination_failures = failures,
                "terminated QA processes before stage restart"
            );

            Ok(())
        })
    }

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

pub fn capabilities() -> StageCapabilities {
    StageCapabilities::new(["shared_dependencies", "qa_environment"])
}

pub fn prepare_stage_state(
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
