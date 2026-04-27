use axum::{extract::{Path, State}, routing::{get, post}, Json, Router};
use chrono::Utc;
use serde_json::{json, Map, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine,
    models::{CreateRunRequest, RunActionRequest, RunStatus, WorkflowEvent, WorkflowRun, WorkflowTemplateDefinition},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/workflow-runs", get(list_runs).post(create_run))
        .route("/api/workflow-runs/:run_id", get(get_run).delete(delete_run))
        .route("/api/workflow-runs/:run_id/open", post(open_run))
        .route("/api/workflow-runs/:run_id/events", get(list_run_events))
        .route("/api/workflow-runs/:run_id/actions", post(run_action))
}

async fn list_runs(State(state): State<AppState>) -> Result<Json<Vec<WorkflowRun>>, (axum::http::StatusCode, String)> {
    let rows = sqlx::query(
        "SELECT id, template_id, definition_json, status, current_step_id, title, repo_ref, context_json, created_at, updated_at FROM workflow_runs ORDER BY updated_at DESC"
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    let runs = rows.into_iter().map(row_to_run).collect::<Result<Vec<_>, _>>()?;
    Ok(Json(runs))
}

async fn get_run(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<WorkflowRun>, (axum::http::StatusCode, String)> {
    let row = sqlx::query(
        "SELECT id, template_id, definition_json, status, current_step_id, title, repo_ref, context_json, created_at, updated_at FROM workflow_runs WHERE id = ?"
    )
    .bind(run_id.to_string())
    .fetch_one(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(row_to_run(row)?))
}

async fn open_run(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<WorkflowRun>, (axum::http::StatusCode, String)> {
    let row = sqlx::query(
        "SELECT id, template_id, definition_json, status, current_step_id, title, repo_ref, context_json, created_at, updated_at FROM workflow_runs WHERE id = ?"
    )
    .bind(run_id.to_string())
    .fetch_one(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(row_to_run(row)?))
}

async fn list_run_events(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<Vec<WorkflowEvent>>, (axum::http::StatusCode, String)> {
    let rows = sqlx::query(
        "SELECT id, run_id, step_id, level, kind, message, payload_json, created_at FROM workflow_events WHERE run_id = ? ORDER BY sequence_no ASC, created_at ASC"
    )
    .bind(run_id.to_string())
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    let events = rows.into_iter().map(row_to_event).collect::<Result<Vec<_>, _>>()?;
    Ok(Json(events))
}

async fn delete_run(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    sqlx::query("DELETE FROM workflow_events WHERE run_id = ?")
        .bind(run_id.to_string())
        .execute(&state.db)
        .await
        .map_err(internal)?;

    sqlx::query("DELETE FROM workflow_runs WHERE id = ?")
        .bind(run_id.to_string())
        .execute(&state.db)
        .await
        .map_err(internal)?;

    Ok(Json(json!({ "ok": true })))
}

async fn run_action(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Json(req): Json<RunActionRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let action = req.action.as_str();
    let requested_step_id = req.step_id.clone();

    tracing::info!(
        run_id = %run_id,
        action = %action,
        step_id = ?requested_step_id,
        payload = %req.payload,
        "workflow run action requested"
    );

    let response = match action {
        "select_step" => {
            let _ = crate::engine::capabilities::inference::browser::mark_session_rearm_needed_if_browser_session_is_stale(&state, run_id).await;
            let step_id = req.step_id.as_deref().ok_or_else(|| (axum::http::StatusCode::BAD_REQUEST, "step_id required".to_string()))?;

            let mut run = engine::load_run(&state, run_id).await.map_err(internal)?;
            let definition = engine::load_template_definition(&state, &run)
                .await
                .map_err(internal)?
                .ok_or_else(|| (axum::http::StatusCode::BAD_REQUEST, "run has no template definition".to_string()))?;
            let step = definition
                .steps
                .iter()
                .find(|item| item.id == step_id)
                .ok_or_else(|| (axum::http::StatusCode::BAD_REQUEST, format!("unknown step_id {}", step_id)))?;

            run.current_step_id = Some(step.id.clone());
            let decisions = engine::governance::before_stage(&state, run_id, &mut run, step)
                .await
                .map_err(internal)?;
            engine::governance::apply_context_mutations(&mut run, &decisions, Some(step.id.as_str()), None)
                .map_err(internal)?;
            engine::persist_context(&state, run_id, &run.context).await.map_err(internal)?;
            engine::set_run_status(&state, run_id, RunStatus::Waiting, Some(step.id.as_str()))
                .await
                .map_err(internal)?;

            serde_json::json!({
                "ok": true,
                "run_id": run_id,
                "current_step_id": step.id,
                "status": "waiting"
            })
        }
        "patch_global_state" => {
            engine::patch_global_state(&state, run_id, req.payload).await.map_err(internal)?
        }
        "patch_stage_state" => {
            let step_id = req.step_id.as_deref().ok_or_else(|| (axum::http::StatusCode::BAD_REQUEST, "step_id required".to_string()))?;
            engine::patch_stage_state(&state, run_id, step_id, req.payload).await.map_err(internal)?
        }
        "start_run" => {
            engine::start_run(&state, run_id, req.step_id.as_deref()).await.map_err(internal)?
        }
        "resume_run" => {
            engine::resume_run(&state, run_id).await.map_err(internal)?
        }
        "pause_run" => {
            engine::pause_run(&state, run_id).await.map_err(internal)?
        }
        "force_wait_run" | "force_unlock_run" | "force_complete_stage" => {
            engine::force_wait_run(&state, run_id).await.map_err(internal)?
        }
        "run_step" | "run_current_step" => {
            if !req.payload.is_null() {
                let step_id = req.step_id.as_deref().ok_or_else(|| {
                    (axum::http::StatusCode::BAD_REQUEST, "step_id required when payload is provided".to_string())
                })?;
                engine::patch_stage_state(&state, run_id, step_id, req.payload.clone())
                    .await
                    .map_err(internal)?;
            }
            engine::run_step(&state, run_id, req.step_id.as_deref()).await.map_err(internal)?
        }
        "next_step" => {
            let run = engine::load_run(&state, run_id).await.map_err(internal)?;
            let definition = engine::load_template_definition(&state, &run).await.map_err(internal)?
                .ok_or_else(|| (axum::http::StatusCode::BAD_REQUEST, "run has no template definition".to_string()))?;
            let next_id = engine::next_step_id(&definition, run.current_step_id.as_deref())
                .ok_or_else(|| (axum::http::StatusCode::BAD_REQUEST, "no next step".to_string()))?;
            engine::select_step(&state, run_id, &next_id).await.map_err(internal)?
        }
        "previous_step" => {
            let run = engine::load_run(&state, run_id).await.map_err(internal)?;
            let definition = engine::load_template_definition(&state, &run).await.map_err(internal)?
                .ok_or_else(|| (axum::http::StatusCode::BAD_REQUEST, "run has no template definition".to_string()))?;
            let previous_id = engine::previous_step_id(&definition, run.current_step_id.as_deref())
                .ok_or_else(|| (axum::http::StatusCode::BAD_REQUEST, "no previous step".to_string()))?;
            engine::select_step(&state, run_id, &previous_id).await.map_err(internal)?
        }
        other => {
            return Err((axum::http::StatusCode::BAD_REQUEST, format!("unsupported action {other}")));
        }
    };

    tracing::info!(
        run_id = %run_id,
        action = %action,
        step_id = ?requested_step_id,
        "workflow run action completed"
    );

    Ok(Json(response))
}

fn seed_missing_browser_session_rearm(context: &mut Value) {
    let root = engine::ensure_engine_root(context);
    let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
    if !global_state.is_object() {
        *global_state = json!({});
    }
    let Some(global_state_obj) = global_state.as_object_mut() else {
        return;
    };

    let capabilities = global_state_obj.entry("capabilities".to_string()).or_insert_with(|| json!({}));
    if !capabilities.is_object() {
        *capabilities = json!({});
    }
    let Some(capabilities_obj) = capabilities.as_object_mut() else {
        return;
    };

    let inference = capabilities_obj.entry("inference".to_string()).or_insert_with(|| json!({}));
    if !inference.is_object() {
        *inference = json!({});
    }
    let Some(inference_obj) = inference.as_object_mut() else {
        return;
    };

    let transport = inference_obj
        .get("transport")
        .and_then(Value::as_str)
        .unwrap_or("browser");
    if !transport.eq_ignore_ascii_case("browser") {
        return;
    }

    let session_id = inference_obj
        .get("browser")
        .and_then(|v| v.get("session_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    if !session_id.is_empty() {
        return;
    }

    let connection_runtime = inference_obj
        .entry("connection_runtime".to_string())
        .or_insert_with(|| json!({}));
    if !connection_runtime.is_object() {
        *connection_runtime = json!({});
    }
    let Some(connection_runtime_obj) = connection_runtime.as_object_mut() else {
        return;
    };

    connection_runtime_obj.insert(
        "session_rearm".to_string(),
        json!({
            "needed": true,
            "reason": "browser_session_changed",
            "previous_session_id": "missing",
            "next_session_id": ""
        }),
    );

    inference_obj.insert("repo_context_armed".to_string(), Value::Bool(true));
    inference_obj.insert("changeset_schema_armed".to_string(), Value::Bool(true));
    inference_obj.remove("shared_inference_state");
    inference_obj.remove("next_prompt_fragments");
    inference_obj.remove("active_prompt_fragments");
}

fn seed_governance_context_from_definition(context: &mut Value, definition: &WorkflowTemplateDefinition) {
    let root = engine::ensure_engine_root(context);
    let governance = root.entry("governance".to_string()).or_insert_with(|| json!({}));
    if !governance.is_object() {
        *governance = json!({});
    }

    engine::merge_json_values(governance, &definition.governance);
}

async fn create_run(
    State(state): State<AppState>,
    Json(req): Json<CreateRunRequest>,
) -> Result<Json<WorkflowRun>, (axum::http::StatusCode, String)> {
    let now = Utc::now();
    let id = Uuid::new_v4();
    let status = RunStatus::Waiting;

    let definition = if let Some(definition) = req.definition.clone() {
        definition
    } else if let Some(template_id) = req.template_id {
        let template_row = sqlx::query("SELECT definition_json FROM workflow_templates WHERE id = ?")
            .bind(template_id.to_string())
            .fetch_optional(&state.db)
            .await
            .map_err(internal)?
            .ok_or_else(|| (axum::http::StatusCode::BAD_REQUEST, "template not found".to_string()))?;
        let definition_json: String = template_row.get("definition_json");
        serde_json::from_str(&definition_json).map_err(internal)?
    } else {
        return Err((axum::http::StatusCode::BAD_REQUEST, "definition or template_id is required".to_string()));
    };

    let mut run_context = req.context.clone();
    let current_step_id = definition.steps.first().map(|step| step.id.clone());

    let root = engine::ensure_engine_root(&mut run_context);
    let global_state = root.entry("global_state".to_string()).or_insert_with(|| json!({}));
    let mut seeded_global_state = serde_json::to_value(definition.globals.clone()).map_err(internal)?;

    if !seeded_global_state.is_object() {
        seeded_global_state = json!({});
    }

    if let Some(global_obj) = seeded_global_state.as_object_mut() {
        let resources = global_obj.entry("resources".to_string()).or_insert_with(|| json!({}));
        if !resources.is_object() {
            *resources = json!({});
        }
        let resources_obj = resources.as_object_mut().ok_or_else(|| internal("resources must be object"))?;
        let repo = resources_obj.entry("repo".to_string()).or_insert_with(|| json!({}));
        if !repo.is_object() {
            *repo = json!({});
        }
        let repo_obj = repo.as_object_mut().ok_or_else(|| internal("repo resource must be object"))?;
        repo_obj.insert("repo_ref".to_string(), json!(req.repo_ref));
        repo_obj.entry("git_ref".to_string()).or_insert_with(|| json!("WORKTREE"));
    }

    engine::merge_json_values(global_state, &seeded_global_state);

    let initial_step = current_step_id
        .as_deref()
        .and_then(|step_id| definition.steps.iter().find(|step| step.id == step_id));

    let mut seeded_run = WorkflowRun {
        id,
        template_id: req.template_id,
        definition: definition.clone(),
        status: status.clone(),
        current_step_id: current_step_id.clone(),
        title: req.title.clone(),
        repo_ref: req.repo_ref.clone(),
        context: run_context.clone(),
        created_at: now,
        updated_at: now,
    };
    seed_missing_browser_session_rearm(&mut seeded_run.context);
    seed_governance_context_from_definition(&mut seeded_run.context, &definition);
    seed_compile_command_context_from_definition(&mut seeded_run.context, &definition);
    run_context = seeded_run.context;

    sqlx::query(
        "INSERT INTO workflow_runs (id, template_id, definition_json, status, current_step_id, title, repo_ref, context_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(id.to_string())
    .bind(req.template_id.map(|v| v.to_string()))
    .bind(serde_json::to_string_pretty(&definition).map_err(internal)?)
    .bind("waiting")
    .bind(current_step_id.clone())
    .bind(&req.title)
    .bind(&req.repo_ref)
    .bind(serde_json::to_string(&run_context).map_err(internal)?)
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(&state.db)
    .await
    .map_err(internal)?;

    engine::append_event(
        &state.db,
        id,
        None,
        "info",
        "run_created",
        "Workflow run created",
        json!({}),
    )
    .await
    .map_err(internal)?;

    Ok(Json(WorkflowRun {
        id,
        template_id: req.template_id,
        definition,
        status,
        current_step_id: current_step_id.clone(),
        title: req.title,
        repo_ref: req.repo_ref,
        context: run_context,
        created_at: now,
        updated_at: now,
    }))
}

fn seed_compile_command_context_from_definition(context: &mut Value, definition: &WorkflowTemplateDefinition) {
    let root = engine::ensure_engine_root(context);
    let stage_state = root.entry("stage_state".to_string()).or_insert_with(|| json!({}));
    if !stage_state.is_object() {
        *stage_state = json!({});
    }
    let Some(stage_state_obj) = stage_state.as_object_mut() else {
        return;
    };

    for step in definition.steps.iter().filter(|step| step.step_type == "compile") {
        let commands = compile_commands_from_checks(&step.execution.compile_checks);
        let has_commands = commands
            .as_array()
            .map(|rows| !rows.is_empty())
            .unwrap_or(false);
        if !has_commands {
            continue;
        }

        let stage = stage_state_obj.entry(step.id.clone()).or_insert_with(|| json!({}));
        if !stage.is_object() {
            *stage = json!({});
        }
        let Some(stage_obj) = stage.as_object_mut() else {
            continue;
        };

        let execution = stage_obj.entry("execution".to_string()).or_insert_with(|| json!({}));
        if !execution.is_object() {
            *execution = json!({});
        }
        let Some(execution_obj) = execution.as_object_mut() else {
            continue;
        };

        execution_obj.insert("compile_checks".to_string(), json!({
            "commands": commands
        }));
    }
}

fn compile_commands_from_checks(checks: &Value) -> Value {
    if let Some(rows) = checks.get("commands").and_then(Value::as_array) {
        let commands = rows
            .iter()
            .filter_map(|item| {
                if let Some(command) = item.as_str() {
                    let command = command.trim();
                    return (!command.is_empty()).then(|| Value::String(command.to_string()));
                }

                let command = item
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                (!command.is_empty()).then(|| Value::String(command.to_string()))
            })
            .collect::<Vec<_>>();
        if !commands.is_empty() {
            return Value::Array(commands);
        }
    }

    let commands = checks
        .get("commands_text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| Value::String(line.to_string()))
        .collect::<Vec<_>>();

    Value::Array(commands)
}

fn row_to_event(row: sqlx::sqlite::SqliteRow) -> Result<WorkflowEvent, (axum::http::StatusCode, String)> {
    Ok(WorkflowEvent {
        id: Uuid::parse_str(row.get::<String, _>("id").as_str()).map_err(internal)?,
        run_id: Uuid::parse_str(row.get::<String, _>("run_id").as_str()).map_err(internal)?,
        step_id: row.get("step_id"),
        level: row.get("level"),
        kind: row.get("kind"),
        message: row.get("message"),
        payload: serde_json::from_str(row.get::<String, _>("payload_json").as_str()).map_err(internal)?,
        created_at: chrono::DateTime::parse_from_rfc3339(row.get::<String, _>("created_at").as_str()).map_err(internal)?.with_timezone(&Utc),
    })
}

fn row_to_run(row: sqlx::sqlite::SqliteRow) -> Result<WorkflowRun, (axum::http::StatusCode, String)> {
    Ok(WorkflowRun {
        id: Uuid::parse_str(row.get::<String, _>("id").as_str()).map_err(internal)?,
        template_id: row.get::<Option<String>, _>("template_id").map(|v| Uuid::parse_str(v.as_str())).transpose().map_err(internal)?,
        definition: serde_json::from_str(row.get::<String, _>("definition_json").as_str()).map_err(internal)?,
        status: match row.get::<String, _>("status").as_str() {
            "draft" => RunStatus::Waiting,
            "queued" => RunStatus::Queued,
            "running" => RunStatus::Running,
            "waiting" => RunStatus::Waiting,
            "paused" => RunStatus::Paused,
            "success" => RunStatus::Success,
            "cancelled" => RunStatus::Cancelled,
            _ => RunStatus::Error,
        },
        current_step_id: row.get("current_step_id"),
        title: row.get("title"),
        repo_ref: row.get("repo_ref"),
        context: serde_json::from_str(row.get::<String, _>("context_json").as_str()).map_err(internal)?,
        created_at: chrono::DateTime::parse_from_rfc3339(row.get::<String, _>("created_at").as_str()).map_err(internal)?.with_timezone(&Utc),
        updated_at: chrono::DateTime::parse_from_rfc3339(row.get::<String, _>("updated_at").as_str()).map_err(internal)?.with_timezone(&Utc),
    })
}

fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
