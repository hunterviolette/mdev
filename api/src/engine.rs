use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    executor::{append_event, update_run_context, update_run_status, CHANGESET_SCHEMA_EXAMPLE},
    models::{RunStatus, StageExecutionNode, StageExecutionNodeKind, WorkflowCapabilityBinding, WorkflowRun, WorkflowStepDefinition, WorkflowTemplateDefinition},
};

pub async fn load_run(state: &AppState, run_id: Uuid) -> Result<WorkflowRun> {
    let row = sqlx::query(
        "SELECT id, template_id, status, current_step_id, title, repo_ref, context_json, created_at, updated_at FROM workflow_runs WHERE id = ?"
    )
    .bind(run_id.to_string())
    .fetch_one(&state.db)
    .await?;

    Ok(WorkflowRun {
        id: Uuid::parse_str(row.get::<String, _>("id").as_str())?,
        template_id: row.get::<Option<String>, _>("template_id").map(|v| Uuid::parse_str(v.as_str())).transpose()?,
        status: match row.get::<String, _>("status").as_str() {
            "draft" => RunStatus::Draft,
            "running" => RunStatus::Running,
            "paused" => RunStatus::Paused,
            "success" => RunStatus::Success,
            _ => RunStatus::Error,
        },
        current_step_id: row.get("current_step_id"),
        title: row.get("title"),
        repo_ref: row.get("repo_ref"),
        context: serde_json::from_str(row.get::<String, _>("context_json").as_str())?,
        created_at: chrono::DateTime::parse_from_rfc3339(row.get::<String, _>("created_at").as_str())?.with_timezone(&chrono::Utc),
        updated_at: chrono::DateTime::parse_from_rfc3339(row.get::<String, _>("updated_at").as_str())?.with_timezone(&chrono::Utc),
    })
}

pub async fn load_template_definition(state: &AppState, run: &WorkflowRun) -> Result<Option<WorkflowTemplateDefinition>> {
    let Some(template_id) = run.template_id else {
        return Ok(None);
    };

    let row = sqlx::query("SELECT definition_json FROM workflow_templates WHERE id = ?")
        .bind(template_id.to_string())
        .fetch_optional(&state.db)
        .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let definition = serde_json::from_str::<WorkflowTemplateDefinition>(row.get::<String, _>("definition_json").as_str())?;
    Ok(Some(definition))
}

pub fn next_step_id(definition: &WorkflowTemplateDefinition, current_step_id: Option<&str>) -> Option<String> {
    let current_id = current_step_id.or_else(|| definition.steps.first().map(|s| s.id.as_str()))?;
    let index = definition.steps.iter().position(|step| step.id == current_id)?;
    definition.steps.get(index + 1).map(|step| step.id.clone())
}

pub fn previous_step_id(definition: &WorkflowTemplateDefinition, current_step_id: Option<&str>) -> Option<String> {
    let current_id = current_step_id.or_else(|| definition.steps.first().map(|s| s.id.as_str()))?;
    let index = definition.steps.iter().position(|step| step.id == current_id)?;
    index.checked_sub(1).and_then(|idx| definition.steps.get(idx)).map(|step| step.id.clone())
}

pub async fn select_step(state: &AppState, run_id: Uuid, step_id: &str) -> Result<Value> {
    update_run_status(&state.db, run_id, RunStatus::Paused, Some(step_id)).await?;
    Ok(json!({ "ok": true, "current_step_id": step_id }))
}

pub async fn patch_stage_state(state: &AppState, run_id: Uuid, step_id: &str, payload: Value) -> Result<Value> {
    let mut run = load_run(state, run_id).await?;
    let root = ensure_engine_root(&mut run.context);

    let mut stage_payload = match payload {
        Value::Object(map) => map,
        _ => return Err(anyhow!("stage payload must be object")),
    };

    if let Some(repo_context) = stage_payload.remove("repo_context") {
        let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
        let global_state_obj = global_state.as_object_mut().ok_or_else(|| anyhow!("global_state must be object"))?;
        global_state_obj.insert("repo_context".to_string(), repo_context);
    }

    let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
    let stage_state_obj = stage_state.as_object_mut().ok_or_else(|| anyhow!("stage_state must be object"))?;
    stage_state_obj.insert(step_id.to_string(), Value::Object(stage_payload.clone()));
    update_run_context(&state.db, run_id, &run.context).await?;
    Ok(json!({ "ok": true, "step_id": step_id, "state": Value::Object(stage_payload) }))
}

pub async fn run_step(state: &AppState, run_id: Uuid, requested_step_id: Option<&str>) -> Result<Value> {
    let mut run = load_run(state, run_id).await?;
    let definition = load_template_definition(state, &run).await?
        .ok_or_else(|| anyhow!("run has no template definition"))?;

    let step_id = requested_step_id
        .map(|s| s.to_string())
        .or_else(|| run.current_step_id.clone())
        .or_else(|| definition.steps.first().map(|s| s.id.clone()))
        .ok_or_else(|| anyhow!("template has no steps"))?;

    let step = definition.steps.iter().find(|s| s.id == step_id)
        .ok_or_else(|| anyhow!("unknown step_id {}", step_id))?
        .clone();

    update_run_status(&state.db, run_id, RunStatus::Running, Some(step.id.as_str())).await?;

    let result = execute_stage(state, run_id, &mut run, &step).await?;
    let status = match result.get("status").and_then(Value::as_str) {
        Some("paused") => RunStatus::Paused,
        Some("success") => RunStatus::Success,
        Some("running") => RunStatus::Running,
        _ => RunStatus::Error,
    };

    update_run_status(&state.db, run_id, status, Some(step.id.as_str())).await?;

    append_event(
        &state.db,
        run_id,
        Some(step.id.as_str()),
        "info",
        "stage_executed",
        "Stage executed through backend workflow engine",
        result.clone(),
    ).await?;

    Ok(result)
}

async fn execute_stage(
    state: &AppState,
    run_id: Uuid,
    run: &mut WorkflowRun,
    step: &WorkflowStepDefinition,
) -> Result<Value> {
    let root = ensure_engine_root(&mut run.context);
    let global_repo_context = root
        .get("global_state")
        .and_then(Value::as_object)
        .and_then(|m| m.get("repo_context"))
        .cloned();

    let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
    let stage_state_obj = stage_state.as_object_mut().ok_or_else(|| anyhow!("stage_state must be object"))?;
    let local_state = stage_state_obj.entry(step.id.clone()).or_insert_with(|| json!({})).clone();
    let effective_local_state = merge_stage_with_global_state(local_state, global_repo_context);
    let hydrated_local_state = hydrate_stage_local_state(step, effective_local_state);
    stage_state_obj.insert(step.id.clone(), hydrated_local_state.clone());
    update_run_context(&state.db, run_id, &run.context).await?;

    let plan = if !step.execution_plan.is_empty() {
        step.execution_plan.clone()
    } else {
        synthesize_execution_plan(step.capabilities.clone(), step.execution_logic.clone())
    };

    let policy_kind = step.execution_logic
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("default_stage_policy");

    let result = match policy_kind {
        "code_stage_policy" => execute_code_stage(state, run_id, &run.repo_ref, step, &hydrated_local_state, &plan).await?,
        _ => execute_default_stage(state, run_id, &run.repo_ref, step, &hydrated_local_state, &plan).await?,
    };

    Ok(json!({
        "ok": result.get("ok").and_then(Value::as_bool).unwrap_or(true),
        "status": result.get("status").cloned().unwrap_or_else(|| Value::String("success".to_string())),
        "step_id": step.id,
        "step_type": step.step_type,
        "local_state": hydrated_local_state,
        "execution_plan": plan,
        "capability_results": result.get("capability_results").cloned().unwrap_or_else(|| json!([])),
        "message": result.get("message").cloned().unwrap_or_else(|| Value::String("Stage executed by backend workflow engine.".to_string()))
    }))
}

async fn execute_default_stage(
    state: &AppState,
    run_id: Uuid,
    repo_ref: &str,
    step: &WorkflowStepDefinition,
    local_state: &Value,
    plan: &[StageExecutionNode],
) -> Result<Value> {
    let mut capability_results = Vec::new();
    let prompt = local_state.get("composed_prompt").and_then(Value::as_str).unwrap_or("").to_string();
    let include_repo_context = local_state
        .get("prompt_fragment_enabled")
        .and_then(|v| v.get("repo_context"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let repo_context = normalize_repo_context_payload(repo_ref, local_state.get("repo_context").cloned());

    for node in plan.iter().filter(|n| n.enabled) {
        if node.kind == StageExecutionNodeKind::Capability && node.key == "context_export" && include_repo_context {
            capability_results.push(
                crate::executor::execute_context_export(
                    state,
                    run_id,
                    Some(step.id.clone()),
                    repo_context.clone(),
                )
                .await?,
            );
        }

        if node.kind == StageExecutionNodeKind::Capability && node.key == "model_inference" && !prompt.trim().is_empty() {
            capability_results.push(
                crate::executor::execute_model_inference_send_prompt(
                    state,
                    run_id,
                    Some(step.id.clone()),
                    crate::executor::ModelInferenceExecutionRequest {
                        prompt: prompt.clone(),
                        include_repo_context,
                        repo_context: if include_repo_context { Some(repo_context.clone()) } else { None },
                    },
                )
                .await?,
            );
        }
    }

    Ok(json!({
        "ok": true,
        "status": if pause_on_enter(step) { "paused" } else { "success" },
        "capability_results": capability_results,
        "message": "Stage executed by backend workflow engine."
    }))
}

async fn execute_code_stage(
    state: &AppState,
    run_id: Uuid,
    repo_ref: &str,
    step: &WorkflowStepDefinition,
    local_state: &Value,
    plan: &[StageExecutionNode],
) -> Result<Value> {
    let max_attempts = step.execution_logic
        .get("max_consecutive_apply_failures")
        .and_then(Value::as_u64)
        .unwrap_or(3) as usize;

    let mut enabled = local_state.get("prompt_fragment_enabled").cloned().unwrap_or_else(|| json!({}));
    let mut fragments = local_state.get("prompt_fragments").cloned().unwrap_or_else(|| json!({}));
    let include_repo_context = enabled.get("repo_context").and_then(Value::as_bool).unwrap_or(false);
    let repo_context = normalize_repo_context_payload(repo_ref, local_state.get("repo_context").cloned());
    let compile_commands: Vec<String> = plan.iter()
        .find(|n| n.kind == StageExecutionNodeKind::Capability && n.key == "compile_checks")
        .and_then(|n| n.config.get("commands"))
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(ToString::to_string).collect())
        .unwrap_or_default();

    for attempt in 1..=max_attempts {
        let prompt = compose_prompt_from_state(&enabled, &fragments);
        let mut capability_results = Vec::new();

        if include_repo_context {
            capability_results.push(
                crate::executor::execute_context_export(
                    state,
                    run_id,
                    Some(step.id.clone()),
                    repo_context.clone(),
                )
                .await?,
            );
        }

        let inference_json = crate::executor::execute_model_inference_send_prompt(
            state,
            run_id,
            Some(step.id.clone()),
            crate::executor::ModelInferenceExecutionRequest {
                prompt: prompt.clone(),
                include_repo_context,
                repo_context: if include_repo_context { Some(repo_context.clone()) } else { None },
            },
        ).await?;
        capability_results.push(inference_json.clone());

        let payload_text = inference_json
            .get("result")
            .and_then(|v| v.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        if payload_text.trim().is_empty() {
            return Ok(json!({
                "ok": false,
                "status": "error",
                "capability_results": capability_results,
                "message": "Inference returned an empty ChangeSet payload."
            }));
        }

        let apply_json = crate::executor::execute_payload_gateway(
            state,
            run_id,
            Some(step.id.clone()),
            json!({
                "repo_ref": repo_ref,
                "git_ref": repo_context.get("git_ref").cloned().unwrap_or_else(|| Value::String("WORKTREE".to_string())),
                "mode": "changeset_apply",
                "payload_text": payload_text
            }),
        ).await?;
        capability_results.push(apply_json.clone());

        if !apply_json.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            let lines = apply_json.get("lines")
                .and_then(Value::as_array)
                .map(|items| items.iter().map(|v| v.as_str().unwrap_or("")).collect::<Vec<_>>().join("\n"))
                .unwrap_or_else(|| apply_json.get("summary").and_then(Value::as_str).unwrap_or("ChangeSet apply failed.").to_string());
            fragments["apply_error"] = Value::String(format!("ChangeSet apply failed.\n\n{}\n\nPlease provide a NEW ChangeSet JSON (version 1) that fixes the apply errors.", lines));
            enabled["apply_error"] = Value::Bool(true);
            persist_stage_retry_state(state, run_id, &step.id, &enabled, &fragments).await?;
            if attempt < max_attempts {
                continue;
            }
            return Ok(json!({
                "ok": false,
                "status": "error",
                "capability_results": capability_results,
                "message": "Code stage exhausted retry attempts on ChangeSet apply."
            }));
        }

        if !compile_commands.is_empty() {
            let terminal_json = crate::executor::execute_terminal_command(
                state,
                run_id,
                Some(step.id.clone()),
                json!({
                    "repo_ref": repo_ref,
                    "commands": compile_commands
                }),
            ).await?;
            capability_results.push(terminal_json.clone());

            if !terminal_json.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                let outputs = terminal_json.get("outputs")
                    .and_then(Value::as_array)
                    .map(|rows| rows.iter().map(|row| {
                        let obj = row.as_object().cloned().unwrap_or_default();
                        format!("COMMAND: {}\n{}", obj.get("command").and_then(Value::as_str).unwrap_or(""), obj.get("output").and_then(Value::as_str).unwrap_or(""))
                    }).collect::<Vec<_>>().join("\n\n"))
                    .unwrap_or_else(|| "Compile checks failed.".to_string());
                fragments["compile_error"] = Value::String(format!("Postprocess command failed after applying the previous ChangeSet.\n\nPOSTPROCESS OUTPUT:\n{}\n\nPlease provide a NEW ChangeSet JSON (version 1) that fixes the errors.", outputs));
                enabled["compile_error"] = Value::Bool(true);
                persist_stage_retry_state(state, run_id, &step.id, &enabled, &fragments).await?;
                if attempt < max_attempts {
                    continue;
                }
                return Ok(json!({
                    "ok": false,
                    "status": "error",
                    "capability_results": capability_results,
                    "message": "Code stage exhausted retry attempts on compile checks."
                }));
            }
        }

        return Ok(json!({
            "ok": true,
            "status": "success",
            "capability_results": capability_results,
            "message": "Code stage completed successfully through backend workflow engine."
        }));
    }

    Ok(json!({
        "ok": false,
        "status": "error",
        "capability_results": [],
        "message": "Code stage exhausted retry attempts."
    }))
}

async fn persist_stage_retry_state(
    state: &AppState,
    run_id: Uuid,
    step_id: &str,
    enabled: &Value,
    fragments: &Value,
) -> Result<()> {
    let mut run = load_run(state, run_id).await?;
    let root = ensure_engine_root(&mut run.context);
    let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
    let stage_state_obj = stage_state.as_object_mut().ok_or_else(|| anyhow!("stage_state must be object"))?;
    let existing = stage_state_obj.entry(step_id.to_string()).or_insert_with(|| json!({}));
    if let Some(obj) = existing.as_object_mut() {
        obj.insert("prompt_fragment_enabled".to_string(), enabled.clone());
        obj.insert("prompt_fragments".to_string(), fragments.clone());
        let prompt = compose_prompt_from_state(enabled, fragments);
        obj.insert("composed_prompt".to_string(), Value::String(prompt));
    }
    update_run_context(&state.db, run_id, &run.context).await?;
    Ok(())
}

fn merge_stage_with_global_state(local_state: Value, global_repo_context: Option<Value>) -> Value {
    let mut state = match local_state {
        Value::Object(map) => map,
        _ => Map::new(),
    };

    if !state.contains_key("repo_context") {
        if let Some(repo_context) = global_repo_context {
            state.insert("repo_context".to_string(), repo_context);
        }
    }

    Value::Object(state)
}

fn hydrate_stage_local_state(step: &WorkflowStepDefinition, local_state: Value) -> Value {
    let mut state = match local_state {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    };

    let include_changeset_schema = step.id == "code"
        && step.prompt.include_changeset_schema;

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
                Value::String(CHANGESET_SCHEMA_EXAMPLE.to_string()),
            );
        }
    }

    let prompt = compose_prompt_from_state(
        state.get("prompt_fragment_enabled").unwrap_or(&Value::Null),
        state.get("prompt_fragments").unwrap_or(&Value::Null),
    );

    let obj = state.as_object_mut().expect("stage local state must be object");
    obj.insert("composed_prompt".to_string(), Value::String(prompt));
    state
}

fn normalize_repo_context_payload(repo_ref: &str, repo_context: Option<Value>) -> Value {
    let mut payload = match repo_context {
        Some(Value::Object(map)) => Value::Object(map),
        _ => json!({}),
    };

    let obj = payload.as_object_mut().expect("repo context payload must be object");
    obj.insert("repo_ref".to_string(), Value::String(repo_ref.to_string()));
    obj.entry("exclude_regex".to_string()).or_insert_with(|| json!([]));
    payload
}

fn compose_prompt_from_state(enabled: &Value, fragments: &Value) -> String {
    let ordered = [
        ("repo_context", "REPO CONTEXT"),
        ("changeset_schema", "CHANGESET SCHEMA"),
        ("apply_error", "APPLY ERROR"),
        ("compile_error", "COMPILE ERROR"),
        ("user_input", "USER INPUT"),
    ];

    let mut out = Vec::new();
    for (key, label) in ordered {
        let is_enabled = enabled.get(key).and_then(Value::as_bool).unwrap_or(false);
        let text = fragments.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
        if is_enabled && !text.is_empty() {
            out.push(format!("### {}\n{}", label, text));
        }
    }
    out.join("\n\n")
}

fn synthesize_execution_plan(
    capabilities: Vec<WorkflowCapabilityBinding>,
    execution_logic: Value,
) -> Vec<StageExecutionNode> {
    let mut plan = Vec::new();
    for binding in capabilities {
        if binding.enabled {
            plan.push(StageExecutionNode {
                kind: StageExecutionNodeKind::Capability,
                key: binding.capability,
                enabled: true,
                config: binding.config,
                input_mapping: binding.input_mapping,
                output_mapping: binding.output_mapping,
                run_after: Vec::new(),
                condition: Value::Null,
            });
        }
    }

    if !execution_logic.is_null() {
        plan.push(StageExecutionNode {
            kind: StageExecutionNodeKind::StageLogic,
            key: "stage_logic".to_string(),
            enabled: true,
            config: execution_logic,
            input_mapping: Value::Null,
            output_mapping: Value::Null,
            run_after: Vec::new(),
            condition: Value::Null,
        });
    }

    plan
}

fn pause_on_enter(step: &WorkflowStepDefinition) -> bool {
    step.config
        .get("pause_policy")
        .and_then(|v| v.get("pause_on_enter"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn ensure_engine_root(context: &mut Value) -> &mut Map<String, Value> {
    let root = context.as_object_mut().expect("run context must be object");
    if !root.contains_key("workflow_engine") {
        root.insert("workflow_engine".to_string(), json!({}));
    }
    root.get_mut("workflow_engine").and_then(Value::as_object_mut).expect("workflow_engine must be object")
}
