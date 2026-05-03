use axum::{extract::{Path, Query, State}, routing::{delete, get, post, put}, Json, Router};
use serde::{Deserialize, Serialize};

use crate::{
    app_state::AppState,
    engine::capabilities::filesystem,
};

use super::workflow_scope::resolve_workflow_scope;

#[derive(Debug, Deserialize)]
struct FileQuery {
    repo_ref: String,
    path: String,
}

#[derive(Debug, Deserialize)]
struct WriteFileBody {
    repo_ref: String,
    path: String,
    contents: String,
}

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
struct CreateFileBody {
    repo_ref: String,
    path: String,
    #[serde(default)]
    contents: String,
}

#[derive(Debug, Deserialize)]
struct CreateFolderBody {
    repo_ref: String,
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
        .route("/api/file", get(read_file).put(write_file).post(create_file).delete(delete_file))
        .route("/api/folder", post(create_folder))
        .route("/api/workflow-runs/:run_id/filesystem/read", get(read_workflow_file))
        .route("/api/workflow-runs/:run_id/filesystem/write", post(write_workflow_file))
}

async fn read_file(
    Query(query): Query<FileQuery>,
) -> Result<Json<FileContentsResponse>, (axum::http::StatusCode, String)> {
    let normalized = filesystem::normalize_rel_path(&query.path).map_err(internal)?;
    let contents = filesystem::read_text_file(&query.repo_ref, &normalized).map_err(internal)?;
    Ok(Json(FileContentsResponse {
        ok: true,
        repo_ref: query.repo_ref,
        path: normalized,
        contents,
    }))
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

async fn write_file(
    State(_state): State<AppState>,
    Json(body): Json<WriteFileBody>,
) -> Result<Json<MutatePathResponse>, (axum::http::StatusCode, String)> {
    let normalized = filesystem::normalize_rel_path(&body.path).map_err(internal)?;
    let stat = filesystem::write_text_file(&body.repo_ref, &normalized, &body.contents).map_err(internal)?;
    Ok(Json(MutatePathResponse {
        ok: true,
        repo_ref: body.repo_ref,
        path: stat.path,
        kind: stat.kind,
        bytes: stat.bytes,
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

async fn create_file(
    State(_state): State<AppState>,
    Json(body): Json<CreateFileBody>,
) -> Result<Json<MutatePathResponse>, (axum::http::StatusCode, String)> {
    let normalized = filesystem::normalize_rel_path(&body.path).map_err(internal)?;
    let stat = filesystem::create_file(&body.repo_ref, &normalized, &body.contents).map_err(internal)?;
    Ok(Json(MutatePathResponse {
        ok: true,
        repo_ref: body.repo_ref,
        path: stat.path,
        kind: stat.kind,
        bytes: stat.bytes,
    }))
}

async fn create_folder(
    State(_state): State<AppState>,
    Json(body): Json<CreateFolderBody>,
) -> Result<Json<MutatePathResponse>, (axum::http::StatusCode, String)> {
    let normalized = filesystem::normalize_rel_path(&body.path).map_err(internal)?;
    let stat = filesystem::create_dir(&body.repo_ref, &normalized).map_err(internal)?;
    Ok(Json(MutatePathResponse {
        ok: true,
        repo_ref: body.repo_ref,
        path: stat.path,
        kind: stat.kind,
        bytes: stat.bytes,
    }))
}

async fn delete_file(
    Query(query): Query<FileQuery>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let normalized = filesystem::normalize_rel_path(&query.path).map_err(internal)?;
    filesystem::delete_path(&query.repo_ref, &normalized).map_err(internal)?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "repo_ref": query.repo_ref,
        "path": normalized,
    })))
}

fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
