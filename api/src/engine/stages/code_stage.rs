use anyhow::Result;
use serde_json::{json, Value};

use crate::{
    engine::{
        capabilities::inference::stage_support::{
            auto_apply_enabled,
            build_inference_execution_plan,
            prepare_inference_stage_state_with_hooks,
            InferenceStageHooks,
            InferenceStageSettings,
        },
        stages::{
            Stage,
            StageCapabilities,
            StagePlanContext,
            StagePrepareContext,
        },
    },
    models::{StageExecutionNode, WorkflowStepDefinition},
};

pub struct CodeStage;

pub static STAGE: CodeStage = CodeStage;

inventory::submit! {
    super::StageRegistration::new(&STAGE)
}

impl Stage for CodeStage {
    fn stage_type(&self) -> &'static str {
        "code"
    }

    fn capabilities(&self) -> StageCapabilities {
        StageCapabilities::new(["inference", "changeset"])
    }

    fn prepare_state(
        &self,
        context: StagePrepareContext<'_>,
        local_state: Value,
    ) -> Result<Value> {
        prepare_code_state(
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
        build_code_execution_plan(
            context.repo_ref,
            context.global_state,
            context.step,
            context.local_state,
        )
    }
}

fn build_code_execution_plan(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    local_state: &Value,
) -> Result<Vec<StageExecutionNode>> {
    build_inference_execution_plan(
        repo_ref,
        global_state,
        step,
        local_state,
        InferenceStageSettings {
            include_changeset_schema: step.prompt.include_changeset_schema,
        },
    )
}

fn prepare_code_state(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    local_state: Value,
) -> Result<Value> {
    let auto_apply = auto_apply_enabled(step, &local_state);
    let mut state = prepare_inference_stage_state_with_hooks(
        repo_ref,
        global_state,
        step,
        local_state,
        InferenceStageSettings {
            include_changeset_schema: step.prompt.include_changeset_schema,
        },
        InferenceStageHooks {
            empty_user_input_default: Some(
                "please provide changeset in a codeblock with no comments to align coding".to_string(),
            ),
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
                "message": "Code stage completed successfully through backend workflow engine."
            }),
        );
    }

    if !exec_obj.contains_key("on_error") {
        exec_obj.insert(
            "on_error".to_string(),
            if auto_apply {
                json!({
                    "disposition": "retry_stage",
                    "message": "Code stage apply failed; retry the code stage with the apply error included in the prompt.",
                    "patch_from_capability": {
                        "capability": "changeset",
                        "mode": "apply_error_to_code_prompt"
                    }
                })
            } else {
                json!({
                    "disposition": "retry_stage",
                    "message": "Code stage failed during backend workflow execution."
                })
            },
        );
    }

    Ok(state)
}

pub fn build_apply_error_feedback(capability_results: &[Value]) -> String {
    let apply_result = capability_results
        .iter()
        .find(|item| item.get("key").and_then(Value::as_str) == Some("changeset"))
        .and_then(|item| item.get("result"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let summary = apply_result
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("ChangeSet apply failed.")
        .to_string();

    let lines = apply_result
        .get("lines")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let detail = if summary.contains("no JSON object found in changeset payload") || lines.is_empty() {
        summary
    } else {
        format!(
            "{}\n\n{}",
            summary,
            lines
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    format!(
        "{}\n\nPlease provide a NEW ChangeSet JSON (version 1) that fixes the apply errors.",
        detail
    )
}
