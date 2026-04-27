use axum::{extract::{Path, State}, routing::get, Json, Router};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{app_state::AppState, models::{CreateTemplateRequest, WorkflowTemplate, WorkflowTemplateDefinition}};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/workflow-templates", get(list_templates).post(create_template))
        .route("/api/workflow-templates/:template_id", axum::routing::delete(delete_template))
        .route("/api/workflow-builder-contract", get(get_workflow_builder_contract))
}

async fn list_templates(State(state): State<AppState>) -> Result<Json<Vec<WorkflowTemplate>>, (axum::http::StatusCode, String)> {
    let rows = sqlx::query(
        "SELECT id, name, description, repo_ref, definition_json, created_at, updated_at FROM workflow_templates ORDER BY updated_at DESC"
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
            repo_ref: row.get("repo_ref"),
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
    let definition_json = serde_json::to_string_pretty(&req.definition).map_err(internal)?;

    let existing = sqlx::query(
        "SELECT id, created_at FROM workflow_templates WHERE name = ?"
    )
    .bind(&req.name)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?;

    let (id, created_at) = if let Some(row) = existing {
        let id = parse_uuid(row.get("id"))?;
        let created_at = parse_ts(row.get("created_at"))?;

        sqlx::query(
            "UPDATE workflow_templates SET description = ?, repo_ref = ?, definition_json = ?, updated_at = ? WHERE id = ?"
        )
        .bind(&req.description)
        .bind(&req.repo_ref)
        .bind(&definition_json)
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .execute(&state.db)
        .await
        .map_err(internal)?;

        (id, created_at)
    } else {
        let id = Uuid::new_v4();

        sqlx::query(
            "INSERT INTO workflow_templates (id, name, description, repo_ref, definition_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(id.to_string())
        .bind(&req.name)
        .bind(&req.description)
        .bind(&req.repo_ref)
        .bind(&definition_json)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&state.db)
        .await
        .map_err(internal)?;

        (id, now)
    };

    Ok(Json(WorkflowTemplate {
        id,
        name: req.name,
        description: req.description,
        repo_ref: req.repo_ref,
        definition: req.definition,
        created_at,
        updated_at: now,
    }))
}

async fn delete_template(
    State(state): State<AppState>,
    Path(template_id): Path<Uuid>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let result = sqlx::query("DELETE FROM workflow_templates WHERE id = ?")
        .bind(template_id.to_string())
        .execute(&state.db)
        .await
        .map_err(internal)?;

    if result.rows_affected() == 0 {
        return Err((axum::http::StatusCode::NOT_FOUND, "Template not found".to_string()));
    }

    Ok(Json(json!({ "ok": true })))
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
                "step_type": "sap_import",
                "label": "SAP Import",
                "automation_mode_default": "automatic",
                "fields": [
                    {
                        "key": "capabilities.sap/import.package_name",
                        "label": "Package name",
                        "type": "text",
                        "default": ""
                    },
                    {
                        "key": "capabilities.sap/import.include_subpackages",
                        "label": "Include subpackages",
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
                    "on_success": "sap_export",
                    "on_error": "sap_import",
                    "on_paused": "sap_import"
                }
            },
            {
                "step_type": "sap_export",
                "label": "SAP Export",
                "automation_mode_default": "automatic",
                "fields": [
                    {
                        "key": "capabilities.sap/export.manifest_paths_text",
                        "label": "Manifest paths",
                        "type": "multiline_text",
                        "default": ""
                    },
                    {
                        "key": "capabilities.sap/export.auto_activate",
                        "label": "Auto activate",
                        "type": "boolean",
                        "default": true
                    },
                    {
                        "key": "capabilities.sap/export.corr_nr",
                        "label": "Transport request",
                        "type": "text",
                        "default": ""
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
                    "on_error": "sap_export",
                    "on_paused": "sap_export"
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
