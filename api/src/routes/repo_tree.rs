use std::{collections::BTreeMap, fs, path::{Path, PathBuf}, process::Command};

use anyhow::Context;
use axum::{extract::{Path as AxumPath, Query, State}, routing::get, Json, Router};
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;

use super::workflow_scope::resolve_workflow_scope;

#[derive(Debug, Deserialize)]
pub struct RepoTreeQuery {
    #[serde(default)]
    pub repo_ref: String,
    #[serde(default = "default_git_ref")]
    pub git_ref: String,
    #[serde(default)]
    pub base_path: String,
    #[serde(default)]
    pub skip_binary: bool,
    #[serde(default)]
    pub skip_gitignore: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct RepoTreeEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub has_children: bool,
}

#[derive(Debug, Serialize)]
pub struct RepoTreeResponse {
    pub repo_ref: String,
    pub git_ref: String,
    pub base_path: String,
    pub entries: Vec<RepoTreeEntry>,
    pub refreshed_at: String,
}

#[derive(Debug, Serialize)]
pub struct RepoFilesResponse {
    pub repo_ref: String,
    pub git_ref: String,
    pub files: Vec<String>,
    pub refreshed_at: String,
}

#[derive(Debug, Deserialize)]
pub struct RepoValidateQuery {
    pub repo_ref: String,
}

#[derive(Debug, Serialize)]
pub struct RepoValidateResponse {
    pub ok: bool,
    pub repo_ref: String,
    pub exists: bool,
    pub is_dir: bool,
    pub git_repo: bool,
    pub message: String,
}

fn default_git_ref() -> String {
    "WORKTREE".to_string()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/repo-tree", get(get_repo_tree))
        .route("/api/repo-files", get(get_repo_files))
        .route("/api/repo/validate", get(validate_repo_ref))
        .route("/api/workflow-runs/:run_id/repository/tree", get(get_workflow_repo_tree))
}

async fn get_repo_tree(
    Query(query): Query<RepoTreeQuery>,
) -> Result<Json<RepoTreeResponse>, (axum::http::StatusCode, String)> {
    let repo = PathBuf::from(&query.repo_ref);
    let base_path = normalize_rel_path(&query.base_path);

    let mut entries = if effective_ref(&query.git_ref) == "WORKTREE" {
        collect_worktree_entries(&repo, &base_path, query.skip_binary, query.skip_gitignore).map_err(internal)?
    } else {
        collect_git_entries(&repo, effective_ref(&query.git_ref), &base_path, query.skip_binary).map_err(internal)?
    };

    entries.sort_by(|a, b| {
        if a.kind != b.kind {
            return if a.kind == "dir" { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater };
        }
        a.name.cmp(&b.name)
    });

    Ok(Json(RepoTreeResponse {
        repo_ref: query.repo_ref,
        git_ref: query.git_ref,
        base_path,
        entries,
        refreshed_at: chrono::Utc::now().to_rfc3339(),
    }))
}

async fn get_repo_files(
    Query(query): Query<RepoTreeQuery>,
) -> Result<Json<RepoFilesResponse>, (axum::http::StatusCode, String)> {
    let repo = PathBuf::from(&query.repo_ref);

    let mut files = if effective_ref(&query.git_ref) == "WORKTREE" {
        collect_worktree_tracked_files_flat(&repo, query.skip_binary).map_err(internal)?
    } else {
        collect_git_files_flat(&repo, effective_ref(&query.git_ref), query.skip_binary).map_err(internal)?
    };

    files.sort();
    files.dedup();

    Ok(Json(RepoFilesResponse {
        repo_ref: query.repo_ref,
        git_ref: query.git_ref,
        files,
        refreshed_at: chrono::Utc::now().to_rfc3339(),
    }))
}

async fn get_workflow_repo_tree(
    State(state): State<AppState>,
    AxumPath(run_id): AxumPath<uuid::Uuid>,
    Query(query): Query<RepoTreeQuery>,
) -> Result<Json<RepoTreeResponse>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    get_repo_tree(Query(RepoTreeQuery {
        repo_ref: scope.repo_ref,
        git_ref: if query.git_ref.trim().is_empty() { scope.git_ref } else { query.git_ref },
        base_path: query.base_path,
        skip_binary: query.skip_binary,
        skip_gitignore: query.skip_gitignore,
    })).await
}

async fn validate_repo_ref(
    Query(query): Query<RepoValidateQuery>,
) -> Result<Json<RepoValidateResponse>, (axum::http::StatusCode, String)> {
    let repo_ref = query.repo_ref.trim().to_string();
    if repo_ref.is_empty() {
        return Ok(Json(RepoValidateResponse {
            ok: false,
            repo_ref,
            exists: false,
            is_dir: false,
            git_repo: false,
            message: "Repository path is required.".to_string(),
        }));
    }

    let repo = PathBuf::from(&repo_ref);
    let exists = repo.exists();
    let is_dir = exists && repo.is_dir();
    let git_repo = if is_dir {
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["rev-parse", "--is-inside-work-tree"])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    } else {
        false
    };

    let message = if !exists {
        format!("Repository path does not exist: {}", repo_ref)
    } else if !is_dir {
        format!("Repository path is not a directory: {}", repo_ref)
    } else if !git_repo {
        format!("Repository path is not a git repository: {}", repo_ref)
    } else {
        format!("Repository path is valid: {}", repo_ref)
    };

    Ok(Json(RepoValidateResponse {
        ok: exists && is_dir && git_repo,
        repo_ref,
        exists,
        is_dir,
        git_repo,
        message,
    }))
}

fn collect_worktree_entries(
    repo: &Path,
    base_path: &str,
    skip_binary: bool,
    skip_gitignore: bool,
) -> anyhow::Result<Vec<RepoTreeEntry>> {
    let dir = if base_path.is_empty() { repo.to_path_buf() } else { repo.join(base_path) };
    let mut out = Vec::new();

    for entry in fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }

        let rel = path
            .strip_prefix(repo)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");

        if file_type.is_dir() {
            if skip_gitignore && (is_fast_ignored_dir_name(&name) || is_gitignored(repo, &rel)?) {
                continue;
            }
            let has_children = dir_has_visible_children(repo, &rel, skip_binary, skip_gitignore)?;
            out.push(RepoTreeEntry {
                name,
                path: rel,
                kind: "dir".to_string(),
                has_children,
            });
        } else if file_type.is_file() {
            if skip_gitignore && is_gitignored(repo, &rel)? {
                continue;
            }
            if skip_binary {
                let bytes = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
                if is_probably_binary(&bytes) {
                    continue;
                }
            }
            out.push(RepoTreeEntry {
                name,
                path: rel,
                kind: "file".to_string(),
                has_children: false,
            });
        }
    }

    Ok(out)
}

fn dir_has_visible_children(
    repo: &Path,
    rel_dir: &str,
    skip_binary: bool,
    skip_gitignore: bool,
) -> anyhow::Result<bool> {
    let dir = repo.join(rel_dir);
    for entry in fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }

        let rel = path
            .strip_prefix(repo)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");

        if file_type.is_dir() {
            if skip_gitignore && (is_fast_ignored_dir_name(&name) || is_gitignored(repo, &rel)?) {
                continue;
            }
            return Ok(true);
        }

        if file_type.is_file() {
            if skip_gitignore && is_gitignored(repo, &rel)? {
                continue;
            }
            if skip_binary {
                let bytes = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
                if is_probably_binary(&bytes) {
                    continue;
                }
            }
            return Ok(true);
        }
    }
    Ok(false)
}

fn collect_worktree_tracked_files_flat(
    repo: &Path,
    skip_binary: bool,
) -> anyhow::Result<Vec<String>> {
    let stdout = run_git_capture_string(repo, &["ls-files"])?;
    let mut out = Vec::new();

    for rel in stdout.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if skip_binary {
            let bytes = fs::read(repo.join(rel))
                .with_context(|| format!("failed to read {}", rel))?;
            if is_probably_binary(&bytes) {
                continue;
            }
        }

        out.push(rel.to_string());
    }

    Ok(out)
}

fn collect_git_entries(
    repo: &Path,
    git_ref: &str,
    base_path: &str,
    skip_binary: bool,
) -> anyhow::Result<Vec<RepoTreeEntry>> {
    let stdout = run_git_capture_string(repo, &["ls-tree", "-r", "--name-only", git_ref])?;
    let prefix = if base_path.is_empty() {
        String::new()
    } else {
        format!("{}/", base_path)
    };

    let mut grouped = BTreeMap::<String, RepoTreeEntry>::new();

    for line in stdout.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if !prefix.is_empty() && !line.starts_with(&prefix) {
            continue;
        }

        let remainder = if prefix.is_empty() { line } else { &line[prefix.len()..] };
        if remainder.is_empty() {
            continue;
        }

        if let Some((head, _tail)) = remainder.split_once('/') {
            let path = if base_path.is_empty() { head.to_string() } else { format!("{}/{}", base_path, head) };
            grouped.entry(path.clone()).or_insert(RepoTreeEntry {
                name: head.to_string(),
                path,
                kind: "dir".to_string(),
                has_children: true,
            });
        } else {
            let rel = if base_path.is_empty() { remainder.to_string() } else { format!("{}/{}", base_path, remainder) };
            if skip_binary {
                let bytes = read_git_file_bytes(repo, git_ref, &rel)?;
                if is_probably_binary(&bytes) {
                    continue;
                }
            }
            grouped.entry(rel.clone()).or_insert(RepoTreeEntry {
                name: remainder.to_string(),
                path: rel,
                kind: "file".to_string(),
                has_children: false,
            });
        }
    }

    Ok(grouped.into_values().collect())
}

fn collect_git_files_flat(
    repo: &Path,
    git_ref: &str,
    skip_binary: bool,
) -> anyhow::Result<Vec<String>> {
    let stdout = run_git_capture_string(repo, &["ls-tree", "-r", "--name-only", git_ref])?;
    let mut out = Vec::new();

    for rel in stdout.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if skip_binary {
            let bytes = read_git_file_bytes(repo, git_ref, rel)?;
            if is_probably_binary(&bytes) {
                continue;
            }
        }
        out.push(rel.to_string());
    }

    Ok(out)
}

fn normalize_rel_path(path: &str) -> String {
    path.trim().trim_matches('/').replace('\\', "/")
}

fn is_fast_ignored_dir_name(name: &str) -> bool {
    matches!(name, "node_modules" | "target" | "dist" | "build" | ".git" | ".next" | ".turbo" | ".cache" | ".data")
}

fn read_git_file_bytes(repo: &Path, git_ref: &str, rel: &str) -> anyhow::Result<Vec<u8>> {
    run_git_capture(repo, &["show", &format!("{}:{}", git_ref, rel)])
}

fn effective_ref(git_ref: &str) -> &str {
    if git_ref.trim().is_empty() {
        "WORKTREE"
    } else {
        git_ref
    }
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    bytes.iter().any(|b| *b == 0)
}

fn is_gitignored(repo: &Path, rel: &str) -> anyhow::Result<bool> {
    let output = Command::new("git")
        .arg("check-ignore")
        .arg("--quiet")
        .arg("--")
        .arg(rel)
        .current_dir(repo)
        .output()
        .with_context(|| "failed to run git check-ignore")?;
    Ok(output.status.code() == Some(0))
}

fn run_git_capture(repo: &Path, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .with_context(|| format!("failed to run git {:?}", args))?;
    if !output.status.success() {
        anyhow::bail!("git {:?} failed: {}", args, String::from_utf8_lossy(&output.stderr));
    }
    Ok(output.stdout)
}

fn run_git_capture_string(repo: &Path, args: &[&str]) -> anyhow::Result<String> {
    Ok(String::from_utf8(run_git_capture(repo, args)?)?)
}

fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
