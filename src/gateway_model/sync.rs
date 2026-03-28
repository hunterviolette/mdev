use crate::app::actions::ComponentId;
use crate::app::state::{AppState, WORKTREE_REF};
use crate::capabilities::{CapabilityRequest, CapabilityResponse, FileSource};
use crate::gateway_model::SyncMode;

use anyhow::{anyhow, bail, Result};
use regex::Regex;


fn sync_is_probably_binary(bytes: &[u8]) -> bool {
    bytes.iter().any(|&b| b == 0)
}

fn sync_is_gitignored(repo: &std::path::PathBuf, path: &str) -> Result<bool> {
    let (code, _stdout, _stderr) = crate::git::run_git_allow_fail(repo, &["check-ignore", "--quiet", "--", path])?;
    Ok(code == 0)
}

fn compiled_exclude_regexes(state: &AppState) -> Result<Vec<Regex>> {
    state
        .inputs
        .exclude_regex
        .iter()
        .map(|p| Regex::new(p).map_err(|e| anyhow!("Bad exclude regex '{p}': {e}")))
        .collect()
}

fn is_excluded_path(exclude: &[Regex], path: &str) -> bool {
    exclude.iter().any(|rx| rx.is_match(path))
}

fn normalize_sync_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn current_file_source(git_ref: &str) -> FileSource {
    if git_ref == WORKTREE_REF {
        FileSource::Worktree
    } else {
        FileSource::GitRef(git_ref.to_string())
    }
}

fn selected_sync_paths(state: &AppState, repo: &std::path::PathBuf, sync_mode: SyncMode) -> Result<Vec<String>> {
    let mut paths: Vec<String> = match sync_mode {
        SyncMode::Entire => {
            if state.inputs.git_ref == WORKTREE_REF {
                crate::git::list_worktree_files(repo)?
            } else {
                let ls = crate::git::run_git(repo, &["ls-tree", "-r", "--name-only", &state.inputs.git_ref])?;
                String::from_utf8_lossy(&ls)
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect()
            }
        }
        SyncMode::Tree => state.tree.context_selected_files.iter().cloned().collect(),
        SyncMode::Diff => {
            if state.inputs.git_ref != WORKTREE_REF {
                bail!("Diff sync mode requires Ref=WORKTREE.");
            }
            let mut out: Vec<String> = state.tree.modified_paths.iter().cloned().collect();
            out.extend(state.tree.staged_paths.iter().cloned());
            out.extend(state.tree.untracked_paths.iter().cloned());
            out
        }
    };

    for path in &mut paths {
        *path = normalize_sync_path(path);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn read_sync_bytes(state: &mut AppState, repo: &std::path::PathBuf, git_ref: &str, path: &str) -> Result<Vec<u8>> {
    let resp = state.broker.exec(CapabilityRequest::ReadFile {
        repo: repo.clone(),
        path: path.to_string(),
        source: current_file_source(git_ref),
    })?;
    let CapabilityResponse::Bytes(bytes) = resp else {
        bail!("Unexpected response reading {path}");
    };
    Ok(bytes)
}

#[derive(serde::Serialize)]
struct GeneratedChangeSet {
    version: u32,
    description: String,
    operations: Vec<GeneratedOperation>,
}

#[derive(serde::Serialize)]
#[serde(tag = "op")]
enum GeneratedOperation {
    #[serde(rename = "write")]
    Write { path: String, contents: String },
}

fn set_sync_payload_error(state: &mut AppState, applier_id: ComponentId, message: String) {
    if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
        st.sync_payload = message;
        st.status = None;
    }
}

pub fn generate_payload(state: &mut AppState, applier_id: ComponentId) {
    let Some(repo) = state.inputs.repo.clone() else {
        set_sync_payload_error(state, applier_id, "No repo selected.".to_string());
        return;
    };

    let (sync_mode, git_ref, skip_binary, skip_gitignore) = match state.changeset_appliers.get(&applier_id) {
        Some(st) => (
            st.sync_mode,
            state.inputs.git_ref.clone(),
            st.sync_skip_binary,
            st.sync_skip_gitignore,
        ),
        None => return,
    };

    let exclude = match compiled_exclude_regexes(state) {
        Ok(v) => v,
        Err(e) => {
            set_sync_payload_error(state, applier_id, format!("Failed to compile excludes: {e}"));
            return;
        }
    };

    let paths = match selected_sync_paths(state, &repo, sync_mode) {
        Ok(paths) => paths,
        Err(e) => {
            set_sync_payload_error(state, applier_id, format!("Failed to collect files: {e}"));
            return;
        }
    };

    let mut files = Vec::with_capacity(paths.len());
    let mut skipped_binary = 0usize;
    let mut skipped_gitignore = 0usize;
    let mut skipped_excluded = 0usize;

    for path in paths {
        if is_excluded_path(&exclude, &path) {
            skipped_excluded += 1;
            continue;
        }

        if skip_gitignore {
            match sync_is_gitignored(&repo, &path) {
                Ok(true) => {
                    skipped_gitignore += 1;
                    continue;
                }
                Ok(false) => {}
                Err(e) => {
                    set_sync_payload_error(state, applier_id, format!("Failed to check .gitignore for {path}: {e}"));
                    return;
                }
            }
        }

        if skip_binary && crate::git::is_binary_path(&path) {
            skipped_binary += 1;
            continue;
        }

        let bytes = match read_sync_bytes(state, &repo, &git_ref, &path) {
            Ok(bytes) => bytes,
            Err(e) => {
                set_sync_payload_error(state, applier_id, format!("Failed to read {path}: {e}"));
                return;
            }
        };

        if skip_binary && sync_is_probably_binary(&bytes) {
            skipped_binary += 1;
            continue;
        }

        let contents = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                set_sync_payload_error(state, applier_id, format!("Failed to decode {path} as UTF-8."));
                return;
            }
        };

        files.push(GeneratedOperation::Write { path, contents });
    }

    let payload = GeneratedChangeSet {
        version: 1,
        description: "Generated from SYNC mode. Each selected file is emitted as a write operation.".to_string(),
        operations: files,
    };
    let text = match serde_json::to_string_pretty(&payload) {
        Ok(text) => text,
        Err(e) => {
            set_sync_payload_error(state, applier_id, format!("Failed to serialize sync payload: {e}"));
            return;
        }
    };

    if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
        st.sync_payload = text;
        st.status = None;
    }
}

fn set_applier_status(state: &mut AppState, applier_id: ComponentId, status: String) {
    if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
        st.status = Some(status);
    }
}
