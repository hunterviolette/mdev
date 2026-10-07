use std::collections::BTreeMap;

use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::{
        self,
        capabilities::inference::{api::openai, session},
    },
};

#[derive(Debug, Deserialize)]
struct ListInferenceSessionsQuery {
    repo_ref: String,
    #[serde(default)]
    run_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct InferenceSessionsResponse {
    ok: bool,
    sessions: Vec<session::InferenceSession>,
    bindings: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
struct OpenAiModelsResponse {
    ok: bool,
    models: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SelectInferenceSessionRequest {
    run_id: String,
    stage_type: String,
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ArchiveInferenceSessionRequest {
    run_id: String,
    session_id: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/inference-sessions", get(list_inference_sessions))
        .route("/api/inference-sessions/select", post(select_inference_session))
        .route("/api/inference-sessions/archive", post(archive_inference_session))
        .route("/api/inference-providers/openai/models", get(list_openai_models))
}

async fn list_openai_models(
) -> Result<Json<OpenAiModelsResponse>, (axum::http::StatusCode, String)> {
    let models = openai::list_models().await.map_err(internal)?;
    Ok(Json(OpenAiModelsResponse {
        ok: true,
        models,
    }))
}

async fn list_inference_sessions(
    State(state): State<AppState>,
    Query(query): Query<ListInferenceSessionsQuery>,
) -> Result<Json<InferenceSessionsResponse>, (axum::http::StatusCode, String)> {
    let sessions = session::list_repo_sessions(&state.db, query.repo_ref.trim())
        .await
        .map_err(internal)?;

    let bindings = match query
        .run_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(run_id) => {
            let run_id = Uuid::parse_str(run_id).map_err(bad_request)?;
            session::bindings_for_run(&state.db, run_id)
                .await
                .map_err(internal)?
        }
        None => BTreeMap::new(),
    };

    Ok(Json(InferenceSessionsResponse {
        ok: true,
        sessions,
        bindings,
    }))
}

async fn select_inference_session(
    State(state): State<AppState>,
    Json(request): Json<SelectInferenceSessionRequest>,
) -> Result<Json<InferenceSessionsResponse>, (axum::http::StatusCode, String)> {
    let run_id = Uuid::parse_str(request.run_id.trim()).map_err(bad_request)?;
    let stage_type = request.stage_type.trim();
    if stage_type.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "stage_type is required".to_string(),
        ));
    }

    let run = engine::load_run(&state, run_id).await.map_err(internal)?;
    let step = run
        .definition
        .steps
        .iter()
        .find(|step| step.step_type == stage_type)
        .ok_or_else(|| (
            axum::http::StatusCode::BAD_REQUEST,
            format!("stage type '{}' is not present in the workflow", stage_type),
        ))?;

    if !crate::engine::stages::stage_supports_capability(step, "inference") {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            format!("stage type '{}' does not support inference", stage_type),
        ));
    }
    let session_spec = session::resolve_inference_route_from_run(&run, step)
        .map_err(bad_request)?;
    let requested_session_id = request
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let created = match requested_session_id {
        Some(session_id) => {
            let selected = session::get_session(&state.db, session_id)
                .await
                .map_err(internal)?
                .ok_or_else(|| {
                    (
                        axum::http::StatusCode::NOT_FOUND,
                        "inference session not found".to_string(),
                    )
                })?;

            if selected.repo_ref != run.repo_ref {
                return Err((
                    axum::http::StatusCode::BAD_REQUEST,
                    "inference session belongs to a different repository".to_string(),
                ));
            }

            session::bind_session(&state.db, run_id, stage_type, session_id)
                .await
                .map_err(internal)?;
            false
        }
        None => {
            session::create_session(
                &state.db,
                &run.repo_ref,
                run_id,
                &session_spec,
            )
            .await
            .map_err(internal)?;
            true
        }
    };

    if created {
        if let Some(step_id) = run.current_step_id.as_deref() {
            if let Some(step) = run
                .definition
                .steps
                .iter()
                .find(|step| step.id == step_id)
            {
                crate::engine::automation::apply_inference_session_transition(
                    &state,
                    run_id,
                    step,
                )
                .await
                .map_err(internal)?;
            }
        }
    }

    let sessions = session::list_repo_sessions(&state.db, &run.repo_ref)
        .await
        .map_err(internal)?;
    let bindings = session::bindings_for_run(&state.db, run_id)
        .await
        .map_err(internal)?;

    Ok(Json(InferenceSessionsResponse {
        ok: true,
        sessions,
        bindings,
    }))
}

async fn archive_inference_session(
    State(state): State<AppState>,
    Json(request): Json<ArchiveInferenceSessionRequest>,
) -> Result<Json<InferenceSessionsResponse>, (axum::http::StatusCode, String)> {
    let run_id = Uuid::parse_str(request.run_id.trim()).map_err(bad_request)?;
    let session_id = request.session_id.trim();
    if session_id.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "session_id is required".to_string(),
        ));
    }

    let run = engine::load_run(&state, run_id).await.map_err(internal)?;
    let selected = session::get_session(&state.db, session_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            (
                axum::http::StatusCode::NOT_FOUND,
                "inference session not found".to_string(),
            )
        })?;

    if selected.repo_ref != run.repo_ref {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "inference session belongs to a different repository".to_string(),
        ));
    }

    session::archive_session(&state.db, session_id)
        .await
        .map_err(internal)?;

    let sessions = session::list_repo_sessions(&state.db, &run.repo_ref)
        .await
        .map_err(internal)?;
    let bindings = session::bindings_for_run(&state.db, run_id)
        .await
        .map_err(internal)?;

    Ok(Json(InferenceSessionsResponse {
        ok: true,
        sessions,
        bindings,
    }))
}

fn bad_request<E: std::fmt::Display>(
    error: E,
) -> (axum::http::StatusCode, String) {
    (
        axum::http::StatusCode::BAD_REQUEST,
        error.to_string(),
    )
}

fn internal<E: std::fmt::Display>(
    error: E,
) -> (axum::http::StatusCode, String) {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        error.to_string(),
    )
}
