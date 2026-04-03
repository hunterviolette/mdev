use axum::{extract::State, routing::get, Json, Router};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{app_state::AppState, models::{CreateTemplateRequest, WorkflowTemplate, WorkflowTemplateDefinition}};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/workflow-templates", get(list_templates).post(create_template))
        .route("/api/workflow-builder-contract", get(get_workflow_builder_contract))
}

async fn list_templates(State(state): State<AppState>) -> Result<Json<Vec<WorkflowTemplate>>, (axum::http::StatusCode, String)> {
    let rows = sqlx::query(
        "SELECT id, name, description, definition_json, created_at, updated_at FROM workflow_templates ORDER BY updated_at DESC"
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let definition: WorkflowTemplateDefinition = serde_json::from_str(row.get::<String, _>("definition_json").as_str()).map_err(internal)?;
        out.push(WorkflowTemplate {
            id: parse_uuid(row.get("id"))?,
            name: row.get("name"),
            description: row.get("description"),
            definition,
            created_at: parse_ts(row.get("created_at"))?,
            updated_at: parse_ts(row.get("updated_at"))?,
        });
    }

    Ok(Json(out))
}

async fn create_template(
    State(state): State<AppState>,
    Json(req): Json<CreateTemplateRequest>,
) -> Result<Json<WorkflowTemplate>, (axum::http::StatusCode, String)> {
    let now = Utc::now();
    let id = Uuid::new_v4();
    let definition_json = serde_json::to_string_pretty(&req.definition).map_err(internal)?;

    sqlx::query(
        "INSERT INTO workflow_templates (id, name, description, definition_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)"
    )
    .bind(id.to_string())
    .bind(&req.name)
    .bind(&req.description)
    .bind(&definition_json)
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(WorkflowTemplate {
        id,
        name: req.name,
        description: req.description,
        definition: req.definition,
        created_at: now,
        updated_at: now,
    }))
}

async fn get_workflow_builder_contract(
    State(_state): State<AppState>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    Ok(Json(json!({
        "version": 1,
        "stages": [
            {
                "step_type": "design",
                "label": "Design",
                "automation_mode_default": "manual",
                "fields": [
                    {
                        "key": "automation.inject_context",
                        "label": "Inject context",
                        "type": "boolean",
                        "default": true
                    },
                    {
                        "key": "config.pause_policy.pause_on_enter",
                        "label": "Pause on enter",
                        "type": "boolean",
                        "default": false
                    }
                ],
                "transition_defaults": {
                    "on_success": "code",
                    "on_error": "design",
                    "on_paused": "design"
                }
            },
            {
                "step_type": "code",
                "label": "Code",
                "automation_mode_default": "automatic",
                "fields": [
                    {
                        "key": "automation.inject_context",
                        "label": "Inject context",
                        "type": "boolean",
                        "default": true
                    },
                    {
                        "key": "automation.inject_changeset_schema",
                        "label": "Inject ChangeSet schema",
                        "type": "boolean",
                        "default": true
                    },
                    {
                        "key": "automation.auto_apply_changeset",
                        "label": "Auto apply ChangeSet",
                        "type": "boolean",
                        "default": true
                    },
                    {
                        "key": "automation.max_consecutive_apply_failures",
                        "label": "Max apply failures",
                        "type": "integer",
                        "default": 1
                    },
                    {
                        "key": "config.pause_policy.pause_on_enter",
                        "label": "Pause on enter",
                        "type": "boolean",
                        "default": false
                    }
                ],
                "transition_defaults": {
                    "on_success": "compile",
                    "on_error": "code",
                    "on_paused": "code"
                }
            },
            {
                "step_type": "compile",
                "label": "Compile",
                "automation_mode_default": "automatic",
                "fields": [
                    {
                        "key": "execution.compile_checks.commands_text",
                        "label": "Compile commands",
                        "type": "multiline_text",
                        "default": "cargo check"
                    },
                    {
                        "key": "config.pause_policy.pause_on_enter",
                        "label": "Pause on enter",
                        "type": "boolean",
                        "default": false
                    }
                ],
                "transition_defaults": {
                    "on_success": "review",
                    "on_error": "code",
                    "on_paused": "compile"
                }
            },
            {
                "step_type": "review",
                "label": "Review",
                "automation_mode_default": "manual",
                "fields": [
                    {
                        "key": "execution_logic.require_manual_approval",
                        "label": "Require manual approval",
                        "type": "boolean",
                        "default": true
                    },
                    {
                        "key": "config.pause_policy.pause_on_enter",
                        "label": "Pause on enter",
                        "type": "boolean",
                        "default": false
                    }
                ],
                "transition_defaults": {
                    "on_success": "",
                    "on_error": "design",
                    "on_paused": "review"
                }
            }
        ]
    })))
}

fn parse_uuid(value: String) -> Result<Uuid, (axum::http::StatusCode, String)> {
    Uuid::parse_str(&value).map_err(internal)
}

fn parse_ts(value: String) -> Result<chrono::DateTime<Utc>, (axum::http::StatusCode, String)> {
    chrono::DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(internal)
}

fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
