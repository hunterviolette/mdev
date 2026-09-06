use std::{future::Future, pin::Pin};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::models::WorkflowStepDefinition;

use super::{
    configured_execution_plan,
    user_input_node,
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

    fn descriptor(&self) -> crate::models::WorkflowStageDescriptor {
        crate::routes::qa_descriptor()
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
            context.step,
            context.automatic_execution,
        ))
    }

    fn lifecycle_hook(&self) -> Box<dyn StageLifecycleHook> {
        Box::new(QaStageLifecycleHook)
    }
}

pub struct QaStageLifecycleHook;

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
    step: &WorkflowStepDefinition,
    automatic_execution: bool,
) -> Vec<StageExecutionNode> {
    let mut plan = configured_execution_plan(step);
    plan.retain(|node| {
        node.kind != StageExecutionNodeKind::Capability
            || node.key != "operator_checkpoint"
    });

    if automatic_execution {
        plan.insert(
            0,
            user_input_node(
                "QA is ready to start. Continue, select another stage, or pause.",
                vec![],
            ),
        );
    }

    let run_after = plan
        .iter()
        .filter(|node| node.enabled && node.kind == StageExecutionNodeKind::Capability)
        .map(|node| node.key.clone())
        .collect::<Vec<_>>();

    plan.push(user_input_node(
        "QA is running and ready for testing. Continue, select another stage, or pause.",
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

    let execution_logic = state
        .entry("execution_logic".to_string())
        .or_insert_with(|| step.execution_logic.clone());

    if !execution_logic.is_object() {
        *execution_logic = json!({});
    }

    let execution_logic = execution_logic
        .as_object_mut()
        .ok_or_else(|| anyhow!("QA execution logic must be an object"))?;

    execution_logic.insert(
        "on_success".to_string(),
        json!({
            "status": "success",
            "transition": "move_next",
            "message": "QA validation completed successfully."
        }),
    );

    execution_logic
        .entry("on_error".to_string())
        .or_insert_with(|| json!({
            "status": "error",
            "transition": "stay",
            "message": "QA environment failed to start or become ready."
        }));

    Ok(local_state)
}
