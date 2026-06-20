use serde_json::{json, Value};

use crate::{
    engine::capabilities::{context_export, planner},
    models::WorkflowStepDefinition,
};

pub fn supervisor_enabled(step: &WorkflowStepDefinition, local_state: &Value) -> bool {
    local_state
        .get("execution_logic")
        .and_then(|v| v.get("supervisor"))
        .and_then(|v| v.get("enabled"))
        .and_then(Value::as_bool)
        .or_else(|| {
            step.execution_logic
                .get("supervisor")
                .and_then(|v| v.get("enabled"))
                .and_then(Value::as_bool)
        })
        .unwrap_or(false)
}

pub fn planner_feature_selected(global_state: &Value) -> bool {
    !planner::build_planning_fragment(global_state).trim().is_empty()
}

pub fn planner_fragment_armed(global_state: &Value) -> bool {
    global_state
        .get("capabilities")
        .and_then(|v| v.get("planner"))
        .and_then(|v| v.get("fragment_armed"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub fn review_ai_enabled(step: &WorkflowStepDefinition, local_state: &Value) -> bool {
    local_state
        .get("execution_logic")
        .and_then(|v| v.get("ai_review"))
        .and_then(|v| v.get("enabled"))
        .and_then(Value::as_bool)
        .or_else(|| {
            step.execution_logic
                .get("ai_review")
                .and_then(|v| v.get("enabled"))
                .and_then(Value::as_bool)
        })
        .unwrap_or(false)
}

pub fn review_should_run_supervisor_feature_validation(
    global_state: &Value,
    step: &WorkflowStepDefinition,
    local_state: &Value,
) -> bool {
    step.step_type == "review"
        && review_ai_enabled(step, local_state)
        && planner_feature_selected(global_state)
}

pub fn supervisor_review_prompt(global_state: &Value) -> String {
    let planner_fragment = planner::build_planning_fragment(global_state);
    format!(
        "You are running supervisor v2 review for the selected planner feature. Review the repository context and unstaged diff against the selected feature. Return only a JSON object with this exact shape: {{\"status\":\"pass\"|\"fail\",\"reason\":\"string\"}}. Use status=pass only when the coding adjustments are aligned with the selected feature. Use status=fail when work is missing, misaligned, unsafe, incomplete, or unverifiable.\n\nSelected planner feature:\n{}",
        planner_fragment.trim()
    )
}

pub fn one_time_review_repo_context(repo_ref: &str, global_state: &Value) -> Value {
    let baseline = global_state
        .get("capabilities")
        .and_then(|v| v.get("context_export"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let repo_resource = global_state
        .get("resources")
        .and_then(|v| v.get("repo"))
        .cloned();

    let mut payload = context_export::normalize_context_export_payload(
        baseline,
        repo_resource,
        repo_ref,
    );

    let obj = payload
        .as_object_mut()
        .expect("context export payload must be object");

    obj.insert("git_ref".to_string(), Value::String("WORKTREE".to_string()));
    obj.insert("include_unstaged_diff".to_string(), Value::Bool(true));
    obj.insert("include_staged_diff".to_string(), Value::Bool(false));

    payload
}

pub fn force_review_repo_context_for_inference(global_state: &mut Value, repo_ref: &str) {
    if !global_state.is_object() {
        *global_state = json!({});
    }

    let review_context = one_time_review_repo_context(repo_ref, global_state);
    let global_obj = global_state
        .as_object_mut()
        .expect("global_state must be object");

    let capabilities = global_obj
        .entry("capabilities".to_string())
        .or_insert_with(|| json!({}));
    if !capabilities.is_object() {
        *capabilities = json!({});
    }
    let capabilities_obj = capabilities
        .as_object_mut()
        .expect("capabilities must be object");

    let inference = capabilities_obj
        .entry("inference".to_string())
        .or_insert_with(|| json!({}));
    if !inference.is_object() {
        *inference = json!({});
    }
    inference
        .as_object_mut()
        .expect("inference must be object")
        .insert("repo_context_armed".to_string(), Value::Bool(true));

    let context_export = capabilities_obj
        .entry("context_export".to_string())
        .or_insert_with(|| json!({}));
    if !context_export.is_object() {
        *context_export = json!({});
    }
    context_export
        .as_object_mut()
        .expect("context_export must be object")
        .insert("single_use_override".to_string(), review_context);
}

pub fn enable_continue_or_pause_checkpoint(execution_logic: &mut Value) {
    if !execution_logic.is_object() {
        *execution_logic = json!({});
    }

    let exec_obj = execution_logic
        .as_object_mut()
        .expect("execution_logic must be object");

    let automation = exec_obj
        .entry("automation".to_string())
        .or_insert_with(|| json!({}));
    if !automation.is_object() {
        *automation = json!({});
    }

    let automation_obj = automation
        .as_object_mut()
        .expect("automation must be object");

    let user_checkpoint = automation_obj
        .entry("user_checkpoint".to_string())
        .or_insert_with(|| json!({}));
    if !user_checkpoint.is_object() {
        *user_checkpoint = json!({});
    }

    let user_checkpoint_obj = user_checkpoint
        .as_object_mut()
        .expect("user_checkpoint must be object");
    user_checkpoint_obj.insert("enabled".to_string(), Value::Bool(true));
    user_checkpoint_obj.insert("kind".to_string(), Value::String("continue_or_pause".to_string()));
    user_checkpoint_obj.insert("available_dispositions".to_string(), json!(["move_next", "pause"]));

    let disposition_review = automation_obj
        .entry("disposition_review".to_string())
        .or_insert_with(|| json!({}));
    if !disposition_review.is_object() {
        *disposition_review = json!({});
    }

    let disposition_obj = disposition_review
        .as_object_mut()
        .expect("disposition_review must be object");
    disposition_obj.insert("enabled".to_string(), Value::Bool(true));
    disposition_obj.insert("available_dispositions".to_string(), json!(["move_next", "pause"]));
}
