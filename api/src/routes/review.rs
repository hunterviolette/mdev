use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use axum::{extract::{Path, Query, State}, routing::{get, post}, Json, Router};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::{
    app_state::AppState,
    engine::capabilities::git::git::{
        generate_git_apply_patch,
        git_diff_stats,
        git_status,
        git_untracked_line_stats,
        run_git,
        run_git_allow_fail,
        GitPatchScope,
    },
};

use super::workflow_scope::resolve_workflow_scope;


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
pub struct ReviewMultiFileContentsRequest {
    pub repo_ref: String,
    pub scope: String,
    #[serde(default)]
    pub paths: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct ReviewGitPatchRequest {
    pub repo_ref: String,
    pub scope: String,
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    #[serde(default)]
    pub context_lines: Option<u32>,
}


#[derive(Debug, Deserialize)]
pub struct ReviewCommitListRequest {
    pub repo_ref: String,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub exclude_regex: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct ReviewCommitRequest {
    pub repo_ref: String,
    pub commit: String,
}

#[derive(Debug, Deserialize)]
pub struct ReviewCommitDiffRequest {
    pub repo_ref: String,
    pub commit: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub context_lines: Option<u32>,
    #[serde(default)]
    pub whole_file: bool,
}

#[derive(Debug, Deserialize)]
pub struct ReviewCommitReportRequest {
    pub repo_ref: String,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub ref_name: Option<String>,
    #[serde(default)]
    pub aggregation_window: Option<String>,
    #[serde(default)]
    pub color_by: Option<String>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub include_paths: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_paths: Option<Vec<String>>,
    #[serde(default)]
    pub include_extensions: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_extensions: Option<Vec<String>>,
    #[serde(default)]
    pub include_regex: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_regex: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ReviewCommitReportExtensionBucket {
    pub extension: String,
    pub additions: u64,
    pub deletions: u64,
    pub net: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewCommitReportGroupBucket {
    pub key: String,
    pub label: String,
    pub additions: u64,
    pub deletions: u64,
    pub net: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct ReviewCommitReportMonthBucket {
    pub month: String,
    pub additions: u64,
    pub deletions: u64,
    pub net: i64,
    pub files_changed: u64,
    pub commits: u64,
    pub extensions: Vec<ReviewCommitReportExtensionBucket>,
    pub groups: Vec<ReviewCommitReportGroupBucket>,
}

#[derive(Debug, Serialize)]
pub struct ReviewCommitReportBucket {
    pub period: String,
    pub additions: u64,
    pub deletions: u64,
    pub net: i64,
    pub files_changed: u64,
    pub commits: u64,
    pub groups: Vec<ReviewCommitReportGroupBucket>,
}

#[derive(Debug, Serialize)]
pub struct ReviewCommitReportResponse {
    pub ok: bool,
    pub commits: Vec<ReviewCommitSummary>,
    pub months: Vec<ReviewCommitReportMonthBucket>,
    pub buckets: Vec<ReviewCommitReportBucket>,
    pub aggregation_window: String,
    pub color_by: String,
    pub exclude_regex: Vec<String>,
    pub next_offset: Option<u32>,
    pub has_more: bool,
}

#[derive(Debug, Serialize)]
pub struct ReviewCommitRefOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct ReviewCommitOptionsResponse {
    pub ok: bool,
    pub refs: Vec<ReviewCommitRefOption>,
    pub default_ref: String,
    pub default_since: Option<String>,
}


#[derive(Debug, Deserialize)]
pub struct ReviewStageActionRequest {
    pub repo_ref: String,
    pub scope: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowReviewDiffQuery {
    pub scope: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub context_lines: Option<u32>,
    #[serde(default)]
    pub whole_file: bool,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowReviewManifestQuery {
    pub scope: String,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowStageActionRequest {
    pub scope: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowCommitListQuery {
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowCommitListRequest {
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub exclude_regex: Option<Vec<String>>,
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

#[derive(Debug, Serialize, Clone)]
pub struct ReviewMultiFileContentsEntry {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
    pub index_status: String,
    pub worktree_status: String,
    pub untracked: bool,
    pub old_contents: String,
    pub new_contents: String,
}

#[derive(Debug, Serialize)]
pub struct ReviewMultiFileContentsResponse {
    pub ok: bool,
    pub scope: String,
    pub from_ref: String,
    pub to_ref: String,
    pub files: Vec<ReviewMultiFileContentsEntry>,
}

#[derive(Debug, Serialize)]
pub struct ReviewGitPatchResponse {
    pub ok: bool,
    pub scope: String,
    pub from_ref: String,
    pub to_ref: String,
    pub base_head: String,
    pub patch: String,
}


#[derive(Debug, Serialize, Clone)]
pub struct ReviewCommitSummary {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub author_name: String,
    pub author_email: String,
    pub authored_at: String,
    pub files_changed: Option<u64>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct ReviewCommitListResponse {
    pub ok: bool,
    pub commits: Vec<ReviewCommitSummary>,
    pub next_offset: Option<u32>,
    pub has_more: bool,
}

#[derive(Debug, Serialize)]
pub struct ReviewCommitDiffManifestResponse {
    pub ok: bool,
    pub commit: String,
    pub from_ref: String,
    pub to_ref: String,
    pub files: Vec<ReviewDiffManifestFileEntry>,
}

#[derive(Debug, Serialize)]
pub struct ReviewCommitDiffResponse {
    pub ok: bool,
    pub commit: String,
    pub path: Option<String>,
    pub from_ref: String,
    pub to_ref: String,
    pub patch: String,
}

#[derive(Debug, Clone)]
struct ReviewCommitFileStat {
    path: String,
    additions: u64,
    deletions: u64,
}

#[derive(Debug, Clone)]
struct ReviewCommitHistoryRow {
    summary: ReviewCommitSummary,
    files: Vec<ReviewCommitFileStat>,
}

#[derive(Debug)]
struct ReviewCommitHistoryQueryResult {
    rows: Vec<ReviewCommitHistoryRow>,
    next_offset: Option<u32>,
    has_more: bool,
    exclude_regex: Vec<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/review/status", post(review_status))
        .route("/api/review/diff", post(review_diff))
        .route("/api/review/diff/manifest", post(review_diff_manifest))
        .route("/api/review/diff/file", post(review_file_patch))
        .route("/api/review/diff/multifile", post(review_multifile_contents))
        .route("/api/review/git-patch", post(review_git_patch))
        .route("/api/review/commits", post(review_commits))
        .route("/api/review/commit-report", post(review_commit_report))
        .route("/api/review/commit-dataset", post(review_commit_report))
        .route("/api/review/commit-options", post(review_commit_options))
        .route("/api/review/commit/diff", post(review_commit_diff))
        .route("/api/review/commit/diff/manifest", post(review_commit_diff_manifest))
        .route("/api/review/stage", post(review_stage))
        .route("/api/review/unstage", post(review_unstage))
        .route("/api/workflow-runs/:run_id/review/status", get(workflow_review_status))
        .route("/api/workflow-runs/:run_id/review/diff", get(workflow_review_diff))
        .route("/api/workflow-runs/:run_id/review/diff/manifest", get(workflow_review_diff_manifest))
        .route("/api/workflow-runs/:run_id/review/stage", post(workflow_review_stage))
        .route("/api/workflow-runs/:run_id/review/unstage", post(workflow_review_unstage))
        .route("/api/workflow-runs/:run_id/commits", get(workflow_review_commits_get).post(workflow_review_commits_post))
}


async fn workflow_review_status(
    State(state): State<AppState>,
    Path(run_id): Path<uuid::Uuid>,
) -> Result<Json<ReviewStatusResponse>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    review_status(Json(ReviewRepoRequest { repo_ref: scope.repo_ref })).await
}

async fn workflow_review_diff(
    State(state): State<AppState>,
    Path(run_id): Path<uuid::Uuid>,
    Query(query): Query<WorkflowReviewDiffQuery>,
) -> Result<Json<ReviewDiffResponse>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    review_diff(Json(ReviewDiffRequest {
        repo_ref: scope.repo_ref,
        scope: query.scope,
        path: query.path,
        context_lines: query.context_lines,
        whole_file: query.whole_file,
    })).await
}

async fn workflow_review_diff_manifest(
    State(state): State<AppState>,
    Path(run_id): Path<uuid::Uuid>,
    Query(query): Query<WorkflowReviewManifestQuery>,
) -> Result<Json<ReviewDiffManifestResponse>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    review_diff_manifest(Json(ReviewDiffManifestRequest {
        repo_ref: scope.repo_ref,
        scope: query.scope,
    })).await
}

async fn workflow_review_stage(
    State(state): State<AppState>,
    Path(run_id): Path<uuid::Uuid>,
    Json(req): Json<WorkflowStageActionRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    review_stage(Json(ReviewStageActionRequest {
        repo_ref: scope.repo_ref,
        scope: req.scope,
        path: req.path,
    })).await
}

async fn workflow_review_unstage(
    State(state): State<AppState>,
    Path(run_id): Path<uuid::Uuid>,
    Json(req): Json<WorkflowStageActionRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    review_unstage(Json(ReviewStageActionRequest {
        repo_ref: scope.repo_ref,
        scope: req.scope,
        path: req.path,
    })).await
}

async fn workflow_review_commits_get(
    State(state): State<AppState>,
    Path(run_id): Path<uuid::Uuid>,
    Query(query): Query<WorkflowCommitListQuery>,
) -> Result<Json<ReviewCommitListResponse>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    review_commits(Json(ReviewCommitListRequest {
        repo_ref: scope.repo_ref,
        limit: query.limit,
        offset: query.offset,
        since: query.since,
        until: query.until,
        exclude_regex: None,
    })).await
}

async fn workflow_review_commits_post(
    State(state): State<AppState>,
    Path(run_id): Path<uuid::Uuid>,
    Json(req): Json<WorkflowCommitListRequest>,
) -> Result<Json<ReviewCommitListResponse>, (axum::http::StatusCode, String)> {
    let scope = resolve_workflow_scope(&state, run_id).await?;
    review_commits(Json(ReviewCommitListRequest {
        repo_ref: scope.repo_ref,
        limit: req.limit,
        offset: req.offset,
        since: req.since,
        until: req.until,
        exclude_regex: req.exclude_regex,
    })).await
}

fn parse_numstat_line(line: &str) -> Option<(String, u64, u64)> {
    let mut parts = line.splitn(3, '\t');
    let additions = parts.next()?.trim();
    let deletions = parts.next()?.trim();
    let path = parts.next()?.trim();
    if path.is_empty() || additions == "-" || deletions == "-" {
        return None;
    }
    Some((
        path.to_string(),
        additions.parse::<u64>().ok()?,
        deletions.parse::<u64>().ok()?,
    ))
}

fn default_review_exclude_regex() -> Vec<String> {
    vec![
        r"(^|/)Cargo\.lock$".to_string(),
        r"(^|/)package-lock\.json$".to_string(),
        r"(^|/)pnpm-lock\.yaml$".to_string(),
        r"(^|/)yarn\.lock$".to_string(),
    ]
}

fn clean_review_filter_values(input: Option<Vec<String>>) -> Vec<String> {
    input
        .unwrap_or_default()
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn effective_review_exclude_regex(input: Option<Vec<String>>) -> Vec<String> {
    let mut patterns = default_review_exclude_regex();
    patterns.extend(clean_review_filter_values(input));
    patterns
}

fn normalize_review_extension(value: &str) -> String {
    let trimmed = value.trim().trim_start_matches('.').to_ascii_lowercase();
    if trimmed.is_empty() { "[none]".to_string() } else { format!(".{trimmed}") }
}

fn compile_review_exclude_regex(patterns: &[String]) -> Result<Vec<Regex>, (axum::http::StatusCode, String)> {
    patterns
        .iter()
        .map(|pattern| Regex::new(pattern).map_err(|err| (axum::http::StatusCode::BAD_REQUEST, format!("invalid exclude regex '{pattern}': {err}"))))
        .collect()
}

fn review_path_excluded(path: &str, compiled: &[Regex]) -> bool {
    let normalized = path.replace('\\', "/");
    compiled.iter().any(|pattern| pattern.is_match(&normalized))
}

fn review_path_included(path: &str, compiled: &[Regex]) -> bool {
    if compiled.is_empty() {
        return true;
    }
    let normalized = path.replace('\\', "/");
    compiled.iter().any(|pattern| pattern.is_match(&normalized))
}

fn review_path_matches_any_prefix(path: &str, filters: &[String]) -> bool {
    if filters.is_empty() {
        return false;
    }
    let normalized = path.replace('\\', "/");
    filters.iter().any(|filter| {
        let filter = filter.replace('\\', "/").trim_matches('/').to_string();
        !filter.is_empty() && (normalized == filter || normalized.starts_with(&format!("{filter}/")))
    })
}

fn review_extension_allowed(extension: &str, includes: &[String], excludes: &[String]) -> bool {
    if !includes.is_empty() && !includes.iter().any(|value| value == extension) {
        return false;
    }
    !excludes.iter().any(|value| value == extension)
}


fn review_extension_for_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    if let Some((_, ext)) = name.rsplit_once('.') {
        let ext = ext.trim().to_ascii_lowercase();
        if !ext.is_empty() {
            return format!(".{ext}");
        }
    }
    "[none]".to_string()
}

fn review_month_for_authored_at(value: &str) -> String {
    value.get(0..7).unwrap_or(value).to_string()
}

fn review_period_for_authored_at(value: &str, aggregation_window: &str) -> String {
    match aggregation_window {
        "daily" => value.get(0..10).unwrap_or(value).to_string(),
        "yearly" => value.get(0..4).unwrap_or(value).to_string(),
        _ => value.get(0..7).unwrap_or(value).to_string(),
    }
}

fn normalize_review_aggregation_window(value: Option<String>) -> String {
    match value.as_deref().map(str::trim) {
        Some("daily") => "daily".to_string(),
        Some("yearly") => "yearly".to_string(),
        _ => "monthly".to_string(),
    }
}

fn normalize_review_color_by(value: Option<String>) -> String {
    match value.as_deref().map(str::trim) {
        Some("author") => "author".to_string(),
        _ => "extension".to_string(),
    }
}

fn is_review_stat_ignored_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    matches!(name, "Cargo.lock" | "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock")
}

fn commit_parent_ref(repo: &std::path::Path, commit: &str) -> Result<String, (axum::http::StatusCode, String)> {
    let parent_spec = format!("{commit}^1");
    let (code, stdout, _stderr) = run_git_allow_fail(repo, &["rev-parse", "--verify", &parent_spec]).map_err(internal)?;
    if code == 0 {
        let parent = String::from_utf8(stdout).map_err(internal)?.trim().to_string();
        if !parent.is_empty() {
            return Ok(parent);
        }
    }
    Ok("4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string())
}

fn commit_diff_entries(
    repo: &std::path::Path,
    commit: &str,
) -> Result<(String, String, Vec<ReviewDiffManifestFileEntry>), (axum::http::StatusCode, String)> {
    let from_ref = commit_parent_ref(repo, commit)?;
    let to_ref = commit.to_string();
    let numstat = String::from_utf8(run_git(repo, &["diff", "--numstat", &from_ref, &to_ref]).map_err(internal)?).map_err(internal)?;
    let name_status = String::from_utf8(run_git(repo, &["diff", "--name-status", &from_ref, &to_ref]).map_err(internal)?).map_err(internal)?;

    let mut status_by_path: HashMap<String, String> = HashMap::new();
    for line in name_status.lines() {
        let mut parts = line.split('\t');
        let status = parts.next().unwrap_or("M").to_string();
        let path = parts.last().unwrap_or_default().trim();
        if !path.is_empty() && !is_review_stat_ignored_path(path) {
            status_by_path.insert(path.to_string(), status);
        }
    }

    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    for line in numstat.lines() {
        if let Some((path, additions, deletions)) = parse_numstat_line(line) {
            if is_review_stat_ignored_path(&path) {
                continue;
            }
            seen.insert(path.clone());
            files.push(ReviewDiffManifestFileEntry {
                index_status: status_by_path.get(&path).cloned().unwrap_or_else(|| "M".to_string()),
                path,
                additions,
                deletions,
                worktree_status: ".".to_string(),
                untracked: false,
            });
        }
    }

    for (path, status) in status_by_path {
        if seen.contains(&path) {
            continue;
        }
        files.push(ReviewDiffManifestFileEntry {
            path,
            additions: 0,
            deletions: 0,
            index_status: status,
            worktree_status: ".".to_string(),
            untracked: false,
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((from_ref, to_ref, files))
}

#[derive(Debug)]
struct ReviewCommitHistoryFilters {
    limit: Option<u32>,
    offset: Option<u32>,
    ref_name: Option<String>,
    since: Option<String>,
    until: Option<String>,
    include_paths: Vec<String>,
    exclude_paths: Vec<String>,
    include_extensions: Vec<String>,
    exclude_extensions: Vec<String>,
    include_regex: Vec<String>,
    exclude_regex: Vec<String>,
}

fn collect_review_commit_history(
    repo: &std::path::Path,
    filters: ReviewCommitHistoryFilters,
) -> Result<ReviewCommitHistoryQueryResult, (axum::http::StatusCode, String)> {
    let compiled_includes = compile_review_exclude_regex(&filters.include_regex)?;
    let compiled_excludes = compile_review_exclude_regex(&filters.exclude_regex)?;

    let mut args = vec![
        "log".to_string(),
        "--numstat".to_string(),
        "--date=iso-strict".to_string(),
    ];

    if let Some(since) = filters.since.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
        args.push(format!("--since={since}"));
    }

    if let Some(until) = filters.until.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
        args.push(format!("--until={until}"));
    }

    args.push("--pretty=format:%x1e%H%x1f%h%x1f%an%x1f%ae%x1f%aI%x1f%s".to_string());

    if let Some(ref_name) = filters.ref_name.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
        args.push(ref_name.to_string());
    }

    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let raw = String::from_utf8(run_git(repo, &arg_refs).map_err(internal)?).map_err(internal)?;

    let mut rows = Vec::new();
    for record in raw.split('\x1e') {
        let record = record.trim_matches('\n');
        if record.trim().is_empty() {
            continue;
        }

        let mut lines = record.lines();
        let Some(header) = lines.next() else {
            continue;
        };
        let fields: Vec<&str> = header.split('\x1f').collect();
        if fields.len() < 6 {
            continue;
        }

        let mut files = Vec::new();
        let mut files_changed = 0u64;
        let mut additions = 0u64;
        let mut deletions = 0u64;

        for line in lines {
            if let Some((path, added, removed)) = parse_numstat_line(line) {
                let extension = review_extension_for_path(&path);
                let included_by_path = filters.include_paths.is_empty() || review_path_matches_any_prefix(&path, &filters.include_paths);

                if !included_by_path {
                    continue;
                }

                if review_path_matches_any_prefix(&path, &filters.exclude_paths) {
                    continue;
                }

                if !review_extension_allowed(&extension, &filters.include_extensions, &filters.exclude_extensions) {
                    continue;
                }

                if !review_path_included(&path, &compiled_includes) {
                    continue;
                }

                if review_path_excluded(&path, &compiled_excludes) {
                    continue;
                }

                files_changed = files_changed.saturating_add(1);
                additions = additions.saturating_add(added);
                deletions = deletions.saturating_add(removed);
                files.push(ReviewCommitFileStat { path, additions: added, deletions: removed });
            }
        }

        if files.is_empty() {
            continue;
        }

        rows.push(ReviewCommitHistoryRow {
            summary: ReviewCommitSummary {
                sha: fields[0].to_string(),
                short_sha: fields[1].to_string(),
                author_name: fields[2].to_string(),
                author_email: fields[3].to_string(),
                authored_at: fields[4].to_string(),
                subject: fields[5].to_string(),
                files_changed: Some(files_changed),
                additions: Some(additions),
                deletions: Some(deletions),
            },
            files,
        });
    }

    let offset_value = filters.offset.unwrap_or(0) as usize;
    let total_rows = rows.len();
    let (paged_rows, next_offset) = if let Some(limit) = filters.limit {
        let limit_value = limit.clamp(1, 500) as usize;
        let paged_rows = rows
            .into_iter()
            .skip(offset_value)
            .take(limit_value)
            .collect::<Vec<_>>();
        let next_offset = if offset_value + paged_rows.len() < total_rows {
            Some((offset_value + paged_rows.len()) as u32)
        } else {
            None
        };
        (paged_rows, next_offset)
    } else {
        (rows, None)
    };

    Ok(ReviewCommitHistoryQueryResult {
        rows: paged_rows,
        next_offset,
        has_more: next_offset.is_some(),
        exclude_regex: filters.exclude_regex,
    })
}

fn collect_full_review_commit_history(
    repo: &std::path::Path,
    req: &ReviewCommitReportRequest,
) -> Result<ReviewCommitHistoryQueryResult, (axum::http::StatusCode, String)> {
    collect_review_commit_history(
        repo,
        ReviewCommitHistoryFilters {
            limit: None,
            offset: None,
            ref_name: req.ref_name.clone(),
            since: req.since.clone(),
            until: req.until.clone(),
            include_paths: clean_review_filter_values(req.include_paths.clone()),
            exclude_paths: clean_review_filter_values(req.exclude_paths.clone()),
            include_extensions: clean_review_filter_values(req.include_extensions.clone()).into_iter().map(|value| normalize_review_extension(&value)).collect(),
            exclude_extensions: clean_review_filter_values(req.exclude_extensions.clone()).into_iter().map(|value| normalize_review_extension(&value)).collect(),
            include_regex: clean_review_filter_values(req.include_regex.clone()),
            exclude_regex: effective_review_exclude_regex(req.exclude_regex.clone()),
        },
    )
}

async fn review_commits(
    Json(req): Json<ReviewCommitListRequest>,
) -> Result<Json<ReviewCommitListResponse>, (axum::http::StatusCode, String)> {
    let report = review_commit_report(Json(ReviewCommitReportRequest {
        repo_ref: req.repo_ref,
        limit: Some(req.limit.unwrap_or(50).clamp(1, 200)),
        offset: req.offset,
        ref_name: None,
        aggregation_window: None,
        color_by: None,
        since: req.since,
        until: req.until,
        include_paths: None,
        exclude_paths: None,
        include_extensions: None,
        exclude_extensions: None,
        include_regex: None,
        exclude_regex: req.exclude_regex,
    })).await?.0;

    Ok(Json(ReviewCommitListResponse {
        ok: true,
        commits: report.commits,
        next_offset: report.next_offset,
        has_more: report.has_more,
    }))
}

async fn review_commit_options(
    Json(req): Json<ReviewRepoRequest>,
) -> Result<Json<ReviewCommitOptionsResponse>, (axum::http::StatusCode, String)> {
    let repo = PathBuf::from(&req.repo_ref);
    let mut refs = Vec::new();

    let raw_refs = String::from_utf8(
        run_git(&repo, &["for-each-ref", "--format=%(refname:short)", "refs/remotes"]).map_err(internal)?
    ).map_err(internal)?;

    for value in raw_refs.lines().map(str::trim).filter(|value| !value.is_empty()) {
        if value.ends_with("/HEAD") {
            continue;
        }
        refs.push(ReviewCommitRefOption { value: value.to_string(), label: value.to_string() });
    }

    refs.sort_by(|a, b| a.label.cmp(&b.label));
    refs.dedup_by(|a, b| a.value == b.value);

    let remote_head_ref = run_git_allow_fail(&repo, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .ok()
        .and_then(|(code, stdout, _stderr)| {
            if code == 0 {
                String::from_utf8(stdout).ok().map(|value| value.trim().to_string())
            } else {
                None
            }
        })
        .filter(|value| !value.is_empty());

    let default_ref = remote_head_ref
        .filter(|value| refs.iter().any(|item| item.value == *value))
        .or_else(|| refs.iter().find(|item| item.value.ends_with("/main")).map(|item| item.value.clone()))
        .or_else(|| refs.iter().find(|item| item.value.ends_with("/master")).map(|item| item.value.clone()))
        .or_else(|| refs.first().map(|item| item.value.clone()))
        .unwrap_or_default();

    let default_since = if default_ref.is_empty() {
        None
    } else {
        String::from_utf8(
            run_git(&repo, &["log", "--reverse", "--format=%as", &default_ref]).map_err(internal)?
        )
        .map_err(internal)?
        .lines()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
    };

    Ok(Json(ReviewCommitOptionsResponse {
        ok: true,
        refs,
        default_ref,
        default_since,
    }))
}

async fn review_commit_report(
    Json(req): Json<ReviewCommitReportRequest>,
) -> Result<Json<ReviewCommitReportResponse>, (axum::http::StatusCode, String)> {
    let repo = PathBuf::from(&req.repo_ref);
    let aggregation_window = normalize_review_aggregation_window(req.aggregation_window.clone());
    let color_by = normalize_review_color_by(req.color_by.clone());
    let result = collect_full_review_commit_history(&repo, &req)?;
    let mut group_rows: BTreeMap<String, BTreeMap<String, ReviewCommitReportGroupBucket>> = BTreeMap::new();
    let mut extension_rows: BTreeMap<String, BTreeMap<String, ReviewCommitReportExtensionBucket>> = BTreeMap::new();
    let mut totals: BTreeMap<String, (u64, u64, u64, u64)> = BTreeMap::new();

    for row in &result.rows {
        let period = review_period_for_authored_at(&row.summary.authored_at, &aggregation_window);
        let total = totals.entry(period.clone()).or_insert((0, 0, 0, 0));
        total.3 = total.3.saturating_add(1);

        for file in &row.files {
            total.0 = total.0.saturating_add(file.additions);
            total.1 = total.1.saturating_add(file.deletions);
            total.2 = total.2.saturating_add(1);

            let extension = review_extension_for_path(&file.path);
            let extension_bucket = extension_rows
                .entry(period.clone())
                .or_default()
                .entry(extension.clone())
                .or_insert(ReviewCommitReportExtensionBucket { extension: extension.clone(), additions: 0, deletions: 0, net: 0 });
            extension_bucket.additions = extension_bucket.additions.saturating_add(file.additions);
            extension_bucket.deletions = extension_bucket.deletions.saturating_add(file.deletions);
            extension_bucket.net = extension_bucket.additions as i64 - extension_bucket.deletions as i64;

            let (key, label) = if color_by == "author" {
                let label = if row.summary.author_name.trim().is_empty() {
                    row.summary.author_email.clone()
                } else {
                    row.summary.author_name.clone()
                };
                let key = if row.summary.author_name.trim().is_empty() {
                    row.summary.author_email.trim().to_ascii_lowercase()
                } else {
                    row.summary.author_name.trim().to_ascii_lowercase()
                };
                (key, label)
            } else {
                (extension.clone(), extension)
            };

            let group_bucket = group_rows
                .entry(period.clone())
                .or_default()
                .entry(key.clone())
                .or_insert(ReviewCommitReportGroupBucket { key, label, additions: 0, deletions: 0, net: 0 });
            group_bucket.additions = group_bucket.additions.saturating_add(file.additions);
            group_bucket.deletions = group_bucket.deletions.saturating_add(file.deletions);
            group_bucket.net = group_bucket.additions as i64 - group_bucket.deletions as i64;
        }
    }

    let mut buckets = Vec::new();
    let mut months = Vec::new();
    for (period, (additions, deletions, files_changed, commits)) in totals {
        let mut groups = group_rows
            .remove(&period)
            .unwrap_or_default()
            .into_values()
            .collect::<Vec<_>>();
        groups.sort_by(|a, b| b.net.abs().cmp(&a.net.abs()).then_with(|| a.label.cmp(&b.label)));

        let mut extensions = extension_rows
            .remove(&period)
            .unwrap_or_default()
            .into_values()
            .collect::<Vec<_>>();
        extensions.sort_by(|a, b| b.net.abs().cmp(&a.net.abs()).then_with(|| a.extension.cmp(&b.extension)));

        buckets.push(ReviewCommitReportBucket {
            period: period.clone(),
            additions,
            deletions,
            net: additions as i64 - deletions as i64,
            files_changed,
            commits,
            groups: groups.clone(),
        });

        months.push(ReviewCommitReportMonthBucket {
            month: period,
            additions,
            deletions,
            net: additions as i64 - deletions as i64,
            files_changed,
            commits,
            extensions,
            groups,
        });
    }

    let offset = req.offset.unwrap_or(0) as usize;
    let limit = req.limit.unwrap_or(75).clamp(1, 500) as usize;
    let total_rows = result.rows.len();
    let commits = result
        .rows
        .iter()
        .skip(offset)
        .take(limit)
        .map(|row| row.summary.clone())
        .collect::<Vec<_>>();
    let next_offset = if offset + commits.len() < total_rows {
        Some((offset + commits.len()) as u32)
    } else {
        None
    };

    Ok(Json(ReviewCommitReportResponse {
        ok: true,
        commits,
        months,
        buckets,
        aggregation_window,
        color_by,
        exclude_regex: result.exclude_regex,
        next_offset,
        has_more: next_offset.is_some(),
    }))
}

async fn review_commit_diff_manifest(
    Json(req): Json<ReviewCommitRequest>,
) -> Result<Json<ReviewCommitDiffManifestResponse>, (axum::http::StatusCode, String)> {
    let repo = PathBuf::from(&req.repo_ref);
    let (from_ref, to_ref, files) = commit_diff_entries(&repo, &req.commit)?;
    Ok(Json(ReviewCommitDiffManifestResponse {
        ok: true,
        commit: req.commit,
        from_ref,
        to_ref,
        files,
    }))
}

async fn review_commit_diff(
    Json(req): Json<ReviewCommitDiffRequest>,
) -> Result<Json<ReviewCommitDiffResponse>, (axum::http::StatusCode, String)> {
    let repo = PathBuf::from(&req.repo_ref);
    let from_ref = commit_parent_ref(&repo, &req.commit)?;
    let to_ref = req.commit.clone();
    let effective_context = if req.whole_file { 2147483647 } else { req.context_lines.unwrap_or(10).min(1000) };
    let unified_arg = format!("--unified={}", effective_context);

    let mut args = vec!["diff".to_string(), unified_arg, from_ref.clone(), to_ref.clone()];
    if let Some(path) = req.path.as_deref().filter(|value| !value.trim().is_empty()) {
        args.push("--".to_string());
        args.push(path.to_string());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let patch = String::from_utf8(run_git(&repo, &arg_refs).map_err(internal)?).map_err(internal)?;

    Ok(Json(ReviewCommitDiffResponse {
        ok: true,
        commit: req.commit,
        path: req.path,
        from_ref,
        to_ref,
        patch,
    }))
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

fn git_patch_scope(scope: &str) -> Result<(GitPatchScope, String, String), (axum::http::StatusCode, String)> {
    match scope {
        "staged" => Ok((GitPatchScope::Staged, "HEAD".to_string(), "INDEX".to_string())),
        "unstaged" => Ok((GitPatchScope::Unstaged, "INDEX".to_string(), "WORKTREE".to_string())),
        "both" => Ok((GitPatchScope::Both, "HEAD".to_string(), "WORKTREE".to_string())),
        other => Err((
            axum::http::StatusCode::BAD_REQUEST,
            format!("unsupported git patch scope {other}"),
        )),
    }
}

fn read_text_for_review_ref(repo: &std::path::Path, git_ref: &str, path: &str) -> Result<String, (axum::http::StatusCode, String)> {
    let bytes = match git_ref {
        "WORKTREE" => match std::fs::read(repo.join(path)) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
            Err(err) => return Err(internal(err)),
        },
        "INDEX" => {
            let spec = format!(":{path}");
            let (code, stdout, _stderr) = run_git_allow_fail(repo, &["show", &spec]).map_err(internal)?;
            if code != 0 {
                return Ok(String::new());
            }
            stdout
        }
        other => {
            let spec = format!("{other}:{path}");
            let (code, stdout, _stderr) = run_git_allow_fail(repo, &["show", &spec]).map_err(internal)?;
            if code != 0 {
                return Ok(String::new());
            }
            stdout
        }
    };

    match String::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(_) => Ok(String::new()),
    }
}

fn review_scope_entries(
    repo: &std::path::Path,
    scope: &str,
) -> Result<(String, String, Vec<ReviewDiffManifestFileEntry>), (axum::http::StatusCode, String)> {
    let (from_ref, to_ref, _use_cached) = refs_for_scope(scope)?;
    let status = git_status(repo).map_err(internal)?;

    let staged_stats = git_diff_stats(repo, true).unwrap_or_else(|_| HashMap::new());
    let unstaged_stats = git_diff_stats(repo, false).unwrap_or_else(|_| HashMap::new());
    let untracked_paths: Vec<String> = status
        .files
        .iter()
        .filter(|item| item.untracked)
        .map(|item| item.path.clone())
        .collect();
    let untracked_stats = git_untracked_line_stats(repo, &untracked_paths);

    let mut files = Vec::new();
    for file in status.files {
        let staged_counts = staged_stats.get(&file.path).copied().unwrap_or((0, 0));
        let unstaged_counts = if file.untracked {
            untracked_stats.get(&file.path).copied().unwrap_or((0, 0))
        } else {
            unstaged_stats.get(&file.path).copied().unwrap_or((0, 0))
        };

        let include = match scope {
            "staged" => file.staged,
            "unstaged" => file.untracked || file.worktree_status != ".",
            _ => false,
        };
        if !include {
            continue;
        }

        let (additions, deletions) = match scope {
            "staged" => staged_counts,
            "unstaged" => unstaged_counts,
            _ => (0, 0),
        };

        files.push(ReviewDiffManifestFileEntry {
            path: file.path.clone(),
            additions,
            deletions,
            index_status: file.index_status.clone(),
            worktree_status: file.worktree_status.clone(),
            untracked: file.untracked,
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((from_ref, to_ref, files))
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

async fn review_git_patch(
    Json(req): Json<ReviewGitPatchRequest>,
) -> Result<Json<ReviewGitPatchResponse>, (axum::http::StatusCode, String)> {
    let repo = PathBuf::from(&req.repo_ref);
    let (scope, from_ref, to_ref) = git_patch_scope(&req.scope)?;
    let patch = generate_git_apply_patch(
        &repo,
        scope,
        req.paths.as_deref(),
        req.context_lines.map(|value| value.min(1000)),
    )
    .map_err(internal)?;
    let base_head = String::from_utf8(run_git(&repo, &["rev-parse", "HEAD"]).map_err(internal)?)
        .map_err(internal)?
        .trim()
        .to_string();

    Ok(Json(ReviewGitPatchResponse {
        ok: true,
        scope: req.scope,
        from_ref,
        to_ref,
        base_head,
        patch,
    }))
}

async fn review_multifile_contents(
    Json(req): Json<ReviewMultiFileContentsRequest>,
) -> Result<Json<ReviewMultiFileContentsResponse>, (axum::http::StatusCode, String)> {
    let repo = PathBuf::from(&req.repo_ref);
    let (from_ref, to_ref, mut files) = review_scope_entries(&repo, &req.scope)?;

    if let Some(paths) = req.paths.as_ref().filter(|paths| !paths.is_empty()) {
        let wanted: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
        files.retain(|file| wanted.contains(file.path.as_str()));
    }

    let mut response_files = Vec::with_capacity(files.len());
    for file in files {
        let old_contents = read_text_for_review_ref(&repo, &from_ref, &file.path)?;
        let new_contents = read_text_for_review_ref(&repo, &to_ref, &file.path)?;
        response_files.push(ReviewMultiFileContentsEntry {
            path: file.path,
            additions: file.additions,
            deletions: file.deletions,
            index_status: file.index_status,
            worktree_status: file.worktree_status,
            untracked: file.untracked,
            old_contents,
            new_contents,
        });
    }

    Ok(Json(ReviewMultiFileContentsResponse {
        ok: true,
        scope: req.scope,
        from_ref,
        to_ref,
        files: response_files,
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
