use std::collections::HashMap;
use std::path::PathBuf;

use axum::{routing::post, Json, Router};
use serde::{Deserialize, Serialize};

use crate::{
    app_state::AppState,
    engine::capabilities::git::git::{
        git_diff_stats,
        git_status,
        git_untracked_line_stats,
        run_git,
    },
};

#[derive(Debug, Deserialize)]
pub struct ReviewRepoRequest {
    pub repo_ref: String,
}

#[derive(Debug, Deserialize)]
pub struct ReviewDiffRequest {
    pub repo_ref: String,
    pub scope: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub context_lines: Option<u32>,
    #[serde(default)]
    pub whole_file: bool,
}

#[derive(Debug, Deserialize)]
pub struct ReviewDiffManifestRequest {
    pub repo_ref: String,
    pub scope: String,
}

#[derive(Debug, Deserialize)]
pub struct ReviewFilePatchRequest {
    pub repo_ref: String,
    pub scope: String,
    pub path: String,
    #[serde(default)]
    pub context_lines: Option<u32>,
    #[serde(default)]
    pub whole_file: bool,
}

#[derive(Debug, Deserialize)]
pub struct ReviewStageActionRequest {
    pub repo_ref: String,
    pub scope: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ReviewStatusFileEntry {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
    pub index_status: String,
    pub worktree_status: String,
    pub untracked: bool,
}

#[derive(Debug, Serialize)]
pub struct ReviewStatusResponse {
    pub ok: bool,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: Vec<ReviewStatusFileEntry>,
    pub unstaged: Vec<ReviewStatusFileEntry>,
}

#[derive(Debug, Serialize)]
pub struct ReviewDiffResponse {
    pub ok: bool,
    pub scope: String,
    pub path: Option<String>,
    pub from_ref: String,
    pub to_ref: String,
    pub patch: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct ReviewDiffManifestFileEntry {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
    pub index_status: String,
    pub worktree_status: String,
    pub untracked: bool,
}

#[derive(Debug, Serialize)]
pub struct ReviewDiffManifestResponse {
    pub ok: bool,
    pub scope: String,
    pub from_ref: String,
    pub to_ref: String,
    pub files: Vec<ReviewDiffManifestFileEntry>,
}

#[derive(Debug, Serialize)]
pub struct ReviewFilePatchResponse {
    pub ok: bool,
    pub scope: String,
    pub path: String,
    pub from_ref: String,
    pub to_ref: String,
    pub patch: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/review/status", post(review_status))
        .route("/api/review/diff", post(review_diff))
        .route("/api/review/diff/manifest", post(review_diff_manifest))
        .route("/api/review/diff/file", post(review_file_patch))
        .route("/api/review/stage", post(review_stage))
        .route("/api/review/unstage", post(review_unstage))
}

async fn review_status(
    Json(req): Json<ReviewRepoRequest>,
) -> Result<Json<ReviewStatusResponse>, (axum::http::StatusCode, String)> {
    let repo = PathBuf::from(&req.repo_ref);
    let status = git_status(&repo).map_err(internal)?;

    let staged_stats = git_diff_stats(&repo, true).unwrap_or_else(|_| HashMap::new());
    let unstaged_stats = git_diff_stats(&repo, false).unwrap_or_else(|_| HashMap::new());
    let untracked_paths: Vec<String> = status
        .files
        .iter()
        .filter(|item| item.untracked)
        .map(|item| item.path.clone())
        .collect();
    let untracked_stats = git_untracked_line_stats(&repo, &untracked_paths);

    let mut staged = Vec::new();
    let mut unstaged = Vec::new();

    for file in status.files {
        let staged_counts = staged_stats.get(&file.path).copied().unwrap_or((0, 0));
        let unstaged_counts = if file.untracked {
            untracked_stats.get(&file.path).copied().unwrap_or((0, 0))
        } else {
            unstaged_stats.get(&file.path).copied().unwrap_or((0, 0))
        };

        if file.staged {
            staged.push(ReviewStatusFileEntry {
                path: file.path.clone(),
                additions: staged_counts.0,
                deletions: staged_counts.1,
                index_status: file.index_status.clone(),
                worktree_status: file.worktree_status.clone(),
                untracked: file.untracked,
            });
        }

        if file.untracked || file.worktree_status != "." {
            unstaged.push(ReviewStatusFileEntry {
                path: file.path.clone(),
                additions: unstaged_counts.0,
                deletions: unstaged_counts.1,
                index_status: file.index_status.clone(),
                worktree_status: file.worktree_status.clone(),
                untracked: file.untracked,
            });
        }
    }

    staged.sort_by(|a, b| a.path.cmp(&b.path));
    unstaged.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(Json(ReviewStatusResponse {
        ok: true,
        branch: status.branch,
        upstream: status.upstream,
        ahead: status.ahead,
        behind: status.behind,
        staged,
        unstaged,
    }))
}

fn refs_for_scope(scope: &str) -> Result<(String, String, bool), (axum::http::StatusCode, String)> {
    match scope {
        "staged" => Ok(("HEAD".to_string(), "INDEX".to_string(), true)),
        "unstaged" => Ok(("INDEX".to_string(), "WORKTREE".to_string(), false)),
        other => Err((
            axum::http::StatusCode::BAD_REQUEST,
            format!("unsupported review diff scope {other}"),
        )),
    }
}

async fn review_diff_manifest(
    Json(req): Json<ReviewDiffManifestRequest>,
) -> Result<Json<ReviewDiffManifestResponse>, (axum::http::StatusCode, String)> {
    let repo = PathBuf::from(&req.repo_ref);
    let status = git_status(&repo).map_err(internal)?;
    let staged_stats = git_diff_stats(&repo, true).unwrap_or_else(|_| HashMap::new());
    let unstaged_stats = git_diff_stats(&repo, false).unwrap_or_else(|_| HashMap::new());
    let untracked_paths: Vec<String> = status
        .files
        .iter()
        .filter(|item| item.untracked)
        .map(|item| item.path.clone())
        .collect();
    let untracked_stats = git_untracked_line_stats(&repo, &untracked_paths);

    let (from_ref, to_ref, use_cached) = refs_for_scope(&req.scope)?;
    let mut files = Vec::new();

    for file in status.files {
        let include = if use_cached {
            file.staged
        } else {
            file.untracked || file.worktree_status != "."
        };
        if !include {
            continue;
        }

        let counts = if use_cached {
            staged_stats.get(&file.path).copied().unwrap_or((0, 0))
        } else if file.untracked {
            untracked_stats.get(&file.path).copied().unwrap_or((0, 0))
        } else {
            unstaged_stats.get(&file.path).copied().unwrap_or((0, 0))
        };

        files.push(ReviewDiffManifestFileEntry {
            path: file.path.clone(),
            additions: counts.0,
            deletions: counts.1,
            index_status: file.index_status.clone(),
            worktree_status: file.worktree_status.clone(),
            untracked: file.untracked,
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(Json(ReviewDiffManifestResponse {
        ok: true,
        scope: req.scope,
        from_ref,
        to_ref,
        files,
    }))
}

async fn review_file_patch(
    Json(req): Json<ReviewFilePatchRequest>,
) -> Result<Json<ReviewFilePatchResponse>, (axum::http::StatusCode, String)> {
    let repo = PathBuf::from(&req.repo_ref);
    let effective_context = if req.whole_file {
        2147483647
    } else {
        req.context_lines.unwrap_or(10).min(1000)
    };
    let unified_arg = format!("--unified={}", effective_context);
    let (from_ref, to_ref, use_cached) = refs_for_scope(&req.scope)?;

    let mut args = vec!["diff".to_string()];
    if use_cached {
        args.push("--cached".to_string());
    }
    args.push(unified_arg);
    args.push("--".to_string());
    args.push(req.path.clone());

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let patch = String::from_utf8(run_git(&repo, &arg_refs).map_err(internal)?)
        .map_err(internal)?;

    Ok(Json(ReviewFilePatchResponse {
        ok: true,
        scope: req.scope,
        path: req.path,
        from_ref,
        to_ref,
        patch,
    }))
}

async fn review_diff(
    Json(req): Json<ReviewDiffRequest>,
) -> Result<Json<ReviewDiffResponse>, (axum::http::StatusCode, String)> {
    let repo = PathBuf::from(&req.repo_ref);
    let effective_context = if req.whole_file {
        2147483647
    } else {
        req.context_lines.unwrap_or(10).min(1000)
    };
    let unified_arg = format!("--unified={}", effective_context);

    let path_args: Vec<String> = req
        .path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| vec!["--".to_string(), value.to_string()])
        .unwrap_or_default();

    let (from_ref, to_ref, mut args): (String, String, Vec<String>) = match req.scope.as_str() {
        "staged" => (
            "HEAD".to_string(),
            "INDEX".to_string(),
            vec!["diff".to_string(), "--cached".to_string(), unified_arg.clone()],
        ),
        "unstaged" => (
            "INDEX".to_string(),
            "WORKTREE".to_string(),
            vec!["diff".to_string(), unified_arg.clone()],
        ),
        other => {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                format!("unsupported review diff scope {other}"),
            ));
        }
    };

    args.extend(path_args);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let patch = String::from_utf8(run_git(&repo, &arg_refs).map_err(internal)?)
        .map_err(internal)?;

    Ok(Json(ReviewDiffResponse {
        ok: true,
        scope: req.scope,
        path: req.path,
        from_ref,
        to_ref,
        patch,
    }))
}

async fn review_stage(
    Json(req): Json<ReviewStageActionRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let repo = PathBuf::from(&req.repo_ref);
    match req.path.as_deref().filter(|value| !value.trim().is_empty()) {
        Some(path) => {
            run_git(&repo, &["add", "--", path]).map_err(internal)?;
        }
        None => {
            let target = match req.scope.as_str() {
                "unstaged" => ".",
                "staged" => ".",
                other => {
                    return Err((
                        axum::http::StatusCode::BAD_REQUEST,
                        format!("unsupported review stage scope {other}"),
                    ));
                }
            };
            run_git(&repo, &["add", "-A", "--", target]).map_err(internal)?;
        }
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn review_unstage(
    Json(req): Json<ReviewStageActionRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let repo = PathBuf::from(&req.repo_ref);
    match req.path.as_deref().filter(|value| !value.trim().is_empty()) {
        Some(path) => {
            run_git(&repo, &["restore", "--staged", "--", path]).map_err(internal)?;
        }
        None => {
            run_git(&repo, &["restore", "--staged", "."]).map_err(internal)?;
        }
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
