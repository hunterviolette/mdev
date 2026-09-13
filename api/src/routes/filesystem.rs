use axum::{extract::{Path, Query, State}, routing::{get, post}, Json, Router};
use serde::{Deserialize, Serialize};

use crate::{
    app_state::AppState,
    engine::capabilities::filesystem,
};

use super::workflow_scope::resolve_workflow_scope;

#[derive(Debug, Deserialize)]
struct WorkflowFileQuery {
    path: String,
}

#[derive(Debug, Deserialize)]
struct WorkflowWriteFileBody {
    path: String,
    contents: String,
}

#[derive(Debug, Deserialize)]
struct WorkflowCreateFileBody {
    path: String,
    #[serde(default)]
    contents: String,
}

#[derive(Debug, Deserialize)]
struct WorkflowPathBody {
    path: String,
}

#[derive(Debug, Serialize)]
struct FileContentsResponse {
    ok: bool,
    repo_ref: String,
    path: String,
    contents: String,
}

#[derive(Debug, Serialize)]
struct MutatePathResponse {
    ok: bool,
    repo_ref: String,
    path: String,
    kind: String,
    bytes: u64,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/workflow-runs/:run_id/filesystem/read", get(read_workflow_file))
        .route("/api/workflow-runs/:run_id/filesystem/write", post(write_workflow_file))
        .route("/api/workflow-runs/:run_id/filesystem/create-file", post(create_workflow_file))
        .route("/api/workflow-runs/:run_id/filesystem/create-folder", post(create_workflow_folder))
        .route("/api/workflow-runs/:run_id/filesystem/delete", post(delete_workflow_path))
}

async fn read_workflow_file(
    State(state): State<AppState>,
    Path(run_id): Path<uuid::Uuid>,
    Query(query): Query<WorkflowFileQuery>,
) -> Result<Json<FileContentsResponse>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    let normalized = filesystem::normalize_rel_path(&query.path).map_err(internal)?;
    let contents = filesystem::read_text_file(&scope.repo_ref, &normalized).map_err(internal)?;
    Ok(Json(FileContentsResponse {
        ok: true,
        repo_ref: scope.repo_ref,
        path: normalized,
        contents,
    }))
}

async fn write_workflow_file(
    State(state): State<AppState>,
    Path(run_id): Path<uuid::Uuid>,
    Json(body): Json<WorkflowWriteFileBody>,
) -> Result<Json<MutatePathResponse>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    let normalized = filesystem::normalize_rel_path(&body.path).map_err(internal)?;
    let stat = filesystem::write_text_file(&scope.repo_ref, &normalized, &body.contents).map_err(internal)?;
    Ok(Json(MutatePathResponse {
        ok: true,
        repo_ref: scope.repo_ref,
        path: stat.path,
        kind: stat.kind,
        bytes: stat.bytes,
    }))
}

async fn create_workflow_file(
    State(state): State<AppState>,
    Path(run_id): Path<uuid::Uuid>,
    Json(body): Json<WorkflowCreateFileBody>,
) -> Result<Json<MutatePathResponse>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    let normalized = filesystem::normalize_rel_path(&body.path).map_err(internal)?;
    let stat = filesystem::create_file(&scope.repo_ref, &normalized, &body.contents).map_err(internal)?;
    Ok(Json(MutatePathResponse {
        ok: true,
        repo_ref: scope.repo_ref,
        path: stat.path,
        kind: stat.kind,
        bytes: stat.bytes,
    }))
}

async fn create_workflow_folder(
    State(state): State<AppState>,
    Path(run_id): Path<uuid::Uuid>,
    Json(body): Json<WorkflowPathBody>,
) -> Result<Json<MutatePathResponse>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    let normalized = filesystem::normalize_rel_path(&body.path).map_err(internal)?;
    let stat = filesystem::create_dir(&scope.repo_ref, &normalized).map_err(internal)?;
    Ok(Json(MutatePathResponse {
        ok: true,
        repo_ref: scope.repo_ref,
        path: stat.path,
        kind: stat.kind,
        bytes: stat.bytes,
    }))
}

async fn delete_workflow_path(
    State(state): State<AppState>,
    Path(run_id): Path<uuid::Uuid>,
    Json(body): Json<WorkflowPathBody>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    let normalized = filesystem::normalize_rel_path(&body.path).map_err(internal)?;
    filesystem::delete_path(&scope.repo_ref, &normalized).map_err(internal)?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "repo_ref": scope.repo_ref,
        "path": normalized,
    })))
}

fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
