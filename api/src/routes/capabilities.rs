use axum::{extract::{Path, State}, routing::{get, post}, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    executor::{append_event, execute_context_export, execute_model_inference_send_prompt, execute_payload_gateway, execute_terminal_command, update_run_context, update_run_status, CHANGESET_SCHEMA_EXAMPLE},
    inference::{browser::adapter as browser_adapter, BrowserProbeResult, InferenceConfig, InferenceTransport},
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
#[serde(rename_all = "snake_case")]
pub enum ModelInferenceAction {
    Configure,
    LaunchBrowser,
    OpenUrl,
    ProbeBrowser,
    SendPrompt,
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
            if let Some(transport) = req.payload.get("transport").and_then(|v| v.as_str()) {
                inference_cfg.transport = match transport {
                    "browser" => InferenceTransport::Browser,
                    _ => InferenceTransport::Api,
                };
            }

            if let Some(model) = req.payload.get("model").and_then(|v| v.as_str()) {
                inference_cfg.model = model.to_string();
            }

            if let Some(browser) = req.payload.get("browser") {
                inference_cfg.browser = serde_json::from_value(browser.clone())?;
            }

            context["model_inference"] = serde_json::to_value(&inference_cfg)?;
            update_run_context(&state.db, run_id, &context).await?;

            append_event(
                &state.db,
                run_id,
                req.step_id.as_deref(),
                "info",
                "model_inference_configured",
                "Model inference configuration saved",
                json!({
                    "transport": inference_cfg.transport,
                    "model": inference_cfg.model,
                    "browser_target_url": inference_cfg.browser.target_url,
                }),
            )
            .await?;

            Ok(json!({ "ok": true, "config": inference_cfg }))
        }

        ModelInferenceAction::LaunchBrowser => {
            let session_id = browser_adapter::launch_and_attach(&mut inference_cfg.browser)?;

            let target_url = inference_cfg.browser.target_url.trim().to_string();
            let mut probe_payload = Value::Null;
            let mut status = "attached";
            let mut message = "Browser attached for inference".to_string();

            if !target_url.is_empty() {
                browser_adapter::open_url(&mut inference_cfg.browser, &target_url)?;
                message = format!("Browser attached and opened {}", target_url);

                if let Ok(probe) = browser_adapter::probe(&mut inference_cfg.browser) {
                    probe_payload = serde_json::to_value(&probe)?;
                    status = if probe.ready { "ready" } else { "attached" };
                    message = if probe.ready {
                        format!("Browser attached and target URL is ready: {}", target_url)
                    } else {
                        format!("Browser attached and target URL opened: {}", target_url)
                    };
                }
            }

            context["model_inference"] = serde_json::to_value(&inference_cfg)?;
            update_run_context(&state.db, run_id, &context).await?;

            append_event(
                &state.db,
                run_id,
                req.step_id.as_deref(),
                "info",
                "browser_attached",
                &message,
                json!({
                    "session_id": session_id,
                    "target_url": if target_url.is_empty() { Value::Null } else { Value::String(target_url.clone()) },
                    "status": status,
                    "probe": probe_payload,
                }),
            )
            .await?;

            Ok(json!({
                "ok": true,
                "session_id": session_id,
                "target_url": if target_url.is_empty() { Value::Null } else { Value::String(target_url) },
                "status": status,
                "probe": probe_payload,
            }))
        }

        ModelInferenceAction::OpenUrl => {
            let url = req.payload
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("payload.url is required"))?;

            browser_adapter::open_url(&mut inference_cfg.browser, url)?;
            context["model_inference"] = serde_json::to_value(&inference_cfg)?;
            update_run_context(&state.db, run_id, &context).await?;

            append_event(
                &state.db,
                run_id,
                req.step_id.as_deref(),
                "info",
                "browser_url_opened",
                "Browser URL opened for inference",
                json!({ "url": url }),
            )
            .await?;

            Ok(json!({ "ok": true, "url": url }))
        }

        ModelInferenceAction::ProbeBrowser => {
            let probe: BrowserProbeResult = browser_adapter::probe(&mut inference_cfg.browser)?;
            context["model_inference"] = serde_json::to_value(&inference_cfg)?;
            update_run_context(&state.db, run_id, &context).await?;
            Ok(json!({ "ok": true, "probe": probe }))
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
