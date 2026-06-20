use anyhow::Result;
use serde_json::{json, Value};

use crate::{
    engine::capabilities::inference::stage_support::{
        build_inference_execution_plan,
        prepare_inference_stage_state,
        InferenceStageSettings,
    },
    models::{StageExecutionNode, StageExecutionNodeKind, WorkflowStepDefinition},
};

use super::{capability_contract::StageCapabilities, stage_utility};

pub fn capabilities() -> StageCapabilities {
    StageCapabilities::new(["context_export", "inference", "review_validation"])
}

pub fn prepare_stage_state(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    local_state: Value,
) -> Result<Value> {
    let mut seed_state = ensure_object(local_state);
    let should_run_ai_review = stage_utility::review_should_run_supervisor_feature_validation(
        global_state,
        step,
        &seed_state,
    );

    if should_run_ai_review {
        let seed_obj = seed_state.as_object_mut().expect("stage state must be object");
        let prompt = seed_obj.entry("prompt".to_string()).or_insert_with(|| json!({}));
        if !prompt.is_object() {
            *prompt = json!({});
        }
        prompt
            .as_object_mut()
            .expect("prompt must be object")
            .insert("user_input".to_string(), Value::String(stage_utility::supervisor_review_prompt(global_state)));
    }

    let mut inference_global_state = global_state.clone();
    if should_run_ai_review {
        stage_utility::force_review_repo_context_for_inference(&mut inference_global_state, repo_ref);
    }

    let mut state = prepare_inference_stage_state(
        repo_ref,
        &inference_global_state,
        step,
        seed_state,
        InferenceStageSettings {
            include_changeset_schema: false,
        },
    )?;

    let require_manual_approval = step
        .execution_logic
        .get("require_manual_approval")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let existing_review = state.get("review").cloned().unwrap_or_else(|| json!({}));
    let approved = existing_review
        .get("approved")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let obj = state.as_object_mut().expect("stage state must be object");
    let review = obj.entry("review".to_string()).or_insert_with(|| {
        json!({
            "approved": false,
            "rejected": false,
            "notes": "",
            "source_control": {
                "selected_scope": "unstaged",
                "selected_path": null,
                "diff_style": "unified",
                "only_changes": true,
                "context_lines": 10,
                "whole_file": false
            }
        })
    });
    if !review.is_object() {
        *review = json!({});
    }
    if should_run_ai_review {
        review
            .as_object_mut()
            .expect("review must be object")
            .insert("ai_review_required".to_string(), Value::Bool(true));
    }

    let execution_logic = obj
        .entry("execution_logic".to_string())
        .or_insert_with(|| step.execution_logic.clone());
    if !execution_logic.is_object() {
        *execution_logic = json!({});
    }

    if require_manual_approval || should_run_ai_review {
        stage_utility::enable_continue_or_pause_checkpoint(execution_logic);
    }

    let require_checkpoint = execution_logic
        .get("automation")
        .and_then(|v| v.get("user_checkpoint"))
        .and_then(|v| v.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let exec_obj = execution_logic.as_object_mut().expect("execution_logic must be object");

    if !exec_obj.contains_key("on_success") {
        exec_obj.insert(
            "on_success".to_string(),
            json!({
                "disposition": if require_checkpoint { "move_next" } else if require_manual_approval && !approved { "paused" } else { "success" },
                "message": if require_checkpoint && should_run_ai_review {
                    "AI review passed. Continue to the next stage or pause to realign."
                } else if require_checkpoint {
                    "Manual review required. Continue to the next stage or pause to realign."
                } else if require_manual_approval && !approved {
                    "Review passed and the workflow is waiting for human review."
                } else {
                    "Review stage completed successfully."
                }
            }),
        );
    }

    if !exec_obj.contains_key("on_error") {
        exec_obj.insert(
            "on_error".to_string(),
            json!({
                "disposition": "paused",
                "message": "Supervisor review failed. The workflow is paused so the loop can be realigned.",
                "patch_from_capability": {
                    "capability": "review_validation",
                    "mode": "review_failure_to_code_prompt"
                }
            }),
        );
    }

    Ok(state)
}

pub fn build_review_execution_plan(
    repo_ref: &str,
    global_state: &Value,
    step: &WorkflowStepDefinition,
    local_state: &Value,
) -> Result<Vec<StageExecutionNode>> {
    let supervisor_feature_review = stage_utility::review_should_run_supervisor_feature_validation(
        global_state,
        step,
        local_state,
    );

    if !supervisor_feature_review {
        return Ok(vec![StageExecutionNode {
            kind: StageExecutionNodeKind::Capability,
            key: "review_validation".to_string(),
            enabled: true,
            config: json!({}),
            input_mapping: json!({}),
            output_mapping: json!({}),
            run_after: vec![],
            condition: Value::Null,
        }]);
    }

    let mut inference_global_state = global_state.clone();
    stage_utility::force_review_repo_context_for_inference(&mut inference_global_state, repo_ref);

    let mut plan = build_inference_execution_plan(
        repo_ref,
        &inference_global_state,
        step,
        local_state,
        InferenceStageSettings {
            include_changeset_schema: false,
        },
    )?;

    for node in plan.iter_mut() {
        if node.key == "context_export" {
            node.config = stage_utility::one_time_review_repo_context(repo_ref, global_state);
        }
    }

    let inference_present = plan.iter().any(|node| node.key == "inference" && node.enabled);
    let review_run_after = if inference_present {
        vec!["inference".to_string()]
    } else {
        Vec::new()
    };

    plan.push(StageExecutionNode {
        kind: StageExecutionNodeKind::Capability,
        key: "review_validation".to_string(),
        enabled: true,
        config: json!({}),
        input_mapping: json!({}),
        output_mapping: json!({}),
        run_after: review_run_after,
        condition: Value::Null,
    });

    Ok(plan)
}

pub fn build_review_failure_patch(capability_results: &[Value]) -> Value {
    let review_result = capability_results
        .iter()
        .find(|item| item.get("key").and_then(Value::as_str) == Some("review_validation"))
        .and_then(|item| item.get("result"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let summary = review_result
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("Review failed.");
    let reason = review_result
        .get("reason")
        .and_then(Value::as_str)
        .or_else(|| review_result.get("explanation").and_then(Value::as_str))
        .unwrap_or("");

    json!({
        "global_state": {
            "capabilities": {
                "inference": {
                    "prompt_fragment_enabled": {
                        "review_failure": true
                    },
                    "prompt_fragments": {
                        "review_failure": format!("### REVIEW FAILURE\n{}\n\n{}", summary, reason)
                    },
                    "review_failure_armed": true
                }
            }
        }
    })
}

fn ensure_object(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    }
}
