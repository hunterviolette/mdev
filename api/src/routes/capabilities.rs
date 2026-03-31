use axum::{extract::{Path, State}, routing::{get, post}, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use tracing::{error, info};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    executor::{append_event, execute_context_export, execute_model_inference_send_prompt, execute_payload_gateway, execute_terminal_command, update_run_context, update_run_status, CHANGESET_SCHEMA_EXAMPLE},
    engine::capabilities::inference::{self, InferenceConfig, ModelInferenceAction},
    models::RunStatus,
};

#[derive(Debug, Deserialize)]
pub struct CapabilityInvokeRequest {
    #[serde(default)]
    pub step_id: Option<String>,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Deserialize)]
pub struct ModelInferenceRequest {
    #[serde(default)]
    pub step_id: Option<String>,
    pub action: ModelInferenceAction,
    #[serde(default)]
    pub payload: Value,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/capabilities/payload-gateway/schema",
            get(get_payload_gateway_schema),
        )
        .route(
            "/api/workflow-runs/:run_id/capabilities/context-export",
            post(invoke_context_export),
        )
        .route(
            "/api/workflow-runs/:run_id/capabilities/payload-gateway",
            post(invoke_payload_gateway),
        )
        .route(
            "/api/workflow-runs/:run_id/capabilities/terminal",
            post(invoke_terminal),
        )
        .route(
            "/api/workflow-runs/:run_id/capabilities/model-inference",
            post(invoke_model_inference),
        )
}

async fn invoke_terminal(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Json(req): Json<CapabilityInvokeRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    update_run_status(&state.db, run_id, RunStatus::Running, req.step_id.as_deref())
        .await
        .map_err(internal)?;

    match execute_terminal_command(&state, run_id, req.step_id.clone(), req.payload).await {
        Ok(result) => {
            update_run_status(&state.db, run_id, if result.get("ok").and_then(Value::as_bool).unwrap_or(false) { RunStatus::Success } else { RunStatus::Error }, req.step_id.as_deref())
                .await
                .map_err(internal)?;
            Ok(Json(result))
        }
        Err(err) => {
            update_run_status(&state.db, run_id, RunStatus::Error, req.step_id.as_deref())
                .await
                .map_err(internal)?;
            append_event(
                &state.db,
                run_id,
                req.step_id.as_deref(),
                "error",
                "terminal_failed",
                "Terminal capability failed",
                json!({ "error": format!("{:#}", err) }),
            )
            .await
            .map_err(internal)?;
            Err(internal(err))
        }
    }
}

async fn get_payload_gateway_schema() -> Json<Value> {
    Json(json!({
        "ok": true,
        "name": "changeset_v1",
        "version": 1,
        "example": CHANGESET_SCHEMA_EXAMPLE,
    }))
}

async fn invoke_context_export(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Json(req): Json<CapabilityInvokeRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    update_run_status(&state.db, run_id, RunStatus::Running, req.step_id.as_deref())
        .await
        .map_err(internal)?;

    match execute_context_export(&state, run_id, req.step_id.clone(), req.payload).await {
        Ok(result) => {
            update_run_status(&state.db, run_id, RunStatus::Success, req.step_id.as_deref())
                .await
                .map_err(internal)?;
            Ok(Json(result))
        }
        Err(err) => {
            update_run_status(&state.db, run_id, RunStatus::Error, req.step_id.as_deref())
                .await
                .map_err(internal)?;
            append_event(
                &state.db,
                run_id,
                req.step_id.as_deref(),
                "error",
                "context_export_failed",
                "Context export failed",
                json!({ "error": format!("{:#}", err) }),
            )
            .await
            .map_err(internal)?;
            Err(internal(err))
        }
    }
}

async fn invoke_payload_gateway(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Json(req): Json<CapabilityInvokeRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    update_run_status(&state.db, run_id, RunStatus::Running, req.step_id.as_deref())
        .await
        .map_err(internal)?;

    match execute_payload_gateway(&state, run_id, req.step_id.clone(), req.payload).await {
        Ok(result) => {
            update_run_status(&state.db, run_id, RunStatus::Success, req.step_id.as_deref())
                .await
                .map_err(internal)?;
            Ok(Json(result))
        }
        Err(err) => {
            update_run_status(&state.db, run_id, RunStatus::Error, req.step_id.as_deref())
                .await
                .map_err(internal)?;
            append_event(
                &state.db,
                run_id,
                req.step_id.as_deref(),
                "error",
                "payload_gateway_failed",
                "Payload gateway failed",
                json!({ "error": format!("{:#}", err) }),
            )
            .await
            .map_err(internal)?;
            Err(internal(err))
        }
    }
}


async fn invoke_model_inference(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Json(req): Json<ModelInferenceRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    update_run_status(&state.db, run_id, RunStatus::Running, req.step_id.as_deref())
        .await
        .map_err(internal)?;

    match handle_model_inference(&state, run_id, &req).await {
        Ok(result) => {
            update_run_status(&state.db, run_id, RunStatus::Success, req.step_id.as_deref())
                .await
                .map_err(internal)?;
            Ok(Json(result))
        }
        Err(err) => {
            error!(run_id = %run_id, step_id = ?req.step_id, action = ?req.action, error = %format!("{:#}", err), "model inference request failed");
            update_run_status(&state.db, run_id, RunStatus::Error, req.step_id.as_deref())
                .await
                .map_err(internal)?;
            append_event(
                &state.db,
                run_id,
                req.step_id.as_deref(),
                "error",
                "model_inference_failed",
                "Model inference failed",
                json!({ "error": format!("{:#}", err) }),
            )
            .await
            .map_err(internal)?;
            Err(internal(err))
        }
    }
}

async fn handle_model_inference(
    state: &AppState,
    run_id: Uuid,
    req: &ModelInferenceRequest,
) -> anyhow::Result<Value> {
    info!(run_id = %run_id, step_id = ?req.step_id, action = ?req.action, payload = %req.payload, "handling model inference request");
    let row = sqlx::query("SELECT context_json FROM workflow_runs WHERE id = ?")
        .bind(run_id.to_string())
        .fetch_one(&state.db)
        .await?;

    let mut context: Value = serde_json::from_str(row.get::<String, _>("context_json").as_str())?;
    let mut inference_cfg: InferenceConfig = serde_json::from_value(
        context.get("model_inference").cloned().unwrap_or_else(|| json!({}))
    ).unwrap_or_default();

    match req.action {
        ModelInferenceAction::Configure => {
            inference::apply_configuration(&mut inference_cfg, &req.payload)?;
            context["model_inference"] = serde_json::to_value(&inference_cfg)?;
            update_run_context(&state.db, run_id, &context).await?;

            Ok(json!({ "ok": true, "config": inference_cfg }))
        }

        ModelInferenceAction::LaunchBrowser | ModelInferenceAction::ConnectBrowserSession => {
            let result = inference::browser::connect_session(&mut inference_cfg)?;
            context["model_inference"] = serde_json::to_value(&inference_cfg)?;
            update_run_context(&state.db, run_id, &context).await?;

            Ok(result)
        }

        ModelInferenceAction::OpenUrl => {
            let url = req.payload
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("payload.url is required"))?;

            let result = inference::browser::open_session_url(&mut inference_cfg, url)?;
            context["model_inference"] = serde_json::to_value(&inference_cfg)?;
            update_run_context(&state.db, run_id, &context).await?;

            Ok(result)
        }

        ModelInferenceAction::ProbeBrowser => {
            let result = inference::browser::probe_session(&mut inference_cfg)?;
            context["model_inference"] = serde_json::to_value(&inference_cfg)?;
            update_run_context(&state.db, run_id, &context).await?;
            Ok(result)
        }

        ModelInferenceAction::DisconnectBrowserSession => {
            let result = inference::browser::disconnect_session(&mut inference_cfg)?;
            context["model_inference"] = serde_json::to_value(&inference_cfg)?;
            update_run_context(&state.db, run_id, &context).await?;

            Ok(result)
        }

        ModelInferenceAction::GetConnectionStatus => {
            let result = inference::browser::connection_status(&mut inference_cfg)?;
            context["model_inference"] = serde_json::to_value(&inference_cfg)?;
            update_run_context(&state.db, run_id, &context).await?;
            Ok(result)
        }

        ModelInferenceAction::SendPrompt => {
            let prompt = req.payload
                .get("prompt")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("payload.prompt is required"))?
                .to_string();

            let include_repo_context = req.payload
                .get("include_repo_context")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            execute_model_inference_send_prompt(
                &state,
                run_id,
                req.step_id.clone(),
                crate::executor::ModelInferenceExecutionRequest {
                    prompt,
                    include_repo_context,
                    repo_context: req.payload.get("repo_context").cloned(),
                },
            )
            .await
        }
    }
}
fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
