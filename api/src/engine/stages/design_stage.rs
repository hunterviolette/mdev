use anyhow::Result;
use serde_json::{json, Value};

use crate::{
    engine::{
        capabilities::{
            capability_enabled,
            inference::stage_support::{
                build_inference_execution_plan,
                prepare_inference_stage_state_with_hooks,
                InferenceStageHooks,
                InferenceStageSettings,
            },
            planner,
        },
        stages::{
            Stage,
            StageCapabilities,
            StageExecutionNode,
            StageExecutionNodeKind,
            StagePlanContext,
            StagePrepareContext,
        },
    },
    models::WorkflowStepDefinition,
};

pub struct DesignStage;

pub static STAGE: DesignStage = DesignStage;

inventory::submit! {
    super::StageRegistration::new(&STAGE)
}

impl Stage for DesignStage {
    fn stage_type(&self) -> &'static str {
        "design"
    }

    fn capabilities(&self) -> StageCapabilities {
        StageCapabilities::new([
            "inference",
            "repo_context",
            "planner_fragment",
            "planner_schema",
            "planner_apply",
        ])
    }

    fn prepare_state(
        &self,
        context: StagePrepareContext<'_>,
        local_state: Value,
    ) -> Result<Value> {
        prepare_design_state(
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
        build_design_execution_plan(
            context.repo_ref,
            context.global_state,
            context.step,
            context.local_state,
            context.automatic_execution,
        )
    }
}

fn build_design_execution_plan(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    local_state: &Value,
    automatic_execution: bool,
) -> Result<Vec<StageExecutionNode>> {
    let mut plan = build_inference_execution_plan(
        repo_ref,
        global_state,
        step,
        local_state,
        InferenceStageSettings {
            include_changeset_schema: false,
        },
    )?;

    if automatic_execution {
        plan.push(StageExecutionNode {
            kind: StageExecutionNodeKind::Capability,
            key: "operator_checkpoint".to_string(),
            enabled: true,
            config: json!({
                "phase": "after_stage",
                "message": "Design is complete. Continue automatically or pause to realign.",
                "recommended_disposition": "continue_auto",
                "available_dispositions": ["continue_auto", "pause_error"]
            }),
            input_mapping: json!({}),
            output_mapping: json!({}),
            run_after: vec!["inference".to_string()],
            condition: Value::Null,
        });
    }

    Ok(plan)
}

fn prepare_design_state(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    local_state: Value,
) -> Result<Value> {
    let planner_fragment_enabled = super::stage_supports_capability(step, "planner_fragment")
        && capability_enabled(global_state, "planner_fragment", false)
        && planner::planner_fragment_enabled(global_state, step);

    let empty_user_input_default = if planner_fragment_enabled {
        Some("are you aligned with the feature implementation?".to_string())
    } else {
        None
    };

    let mut state = prepare_inference_stage_state_with_hooks(
        repo_ref,
        global_state,
        step,
        local_state,
        InferenceStageSettings {
            include_changeset_schema: false,
        },
        InferenceStageHooks {
            empty_user_input_default,
        },
    )?;

    let obj = state.as_object_mut().expect("stage state must be object");
    let execution_logic = obj
        .entry("execution_logic".to_string())
        .or_insert_with(|| step.execution_logic.clone());
    if !execution_logic.is_object() {
        *execution_logic = json!({});
    }
    let exec_obj = execution_logic.as_object_mut().expect("execution_logic must be object");

    if !exec_obj.contains_key("on_success") {
        exec_obj.insert(
            "on_success".to_string(),
            json!({
                "disposition": "move_next",
                "message": "Design stage completed successfully through backend workflow engine."
            }),
        );
    }

    if !exec_obj.contains_key("on_error") {
        exec_obj.insert(
            "on_error".to_string(),
            json!({
                "disposition": "stay",
                "message": "Design stage failed during backend workflow execution."
            }),
        );
    }

    Ok(state)
}
