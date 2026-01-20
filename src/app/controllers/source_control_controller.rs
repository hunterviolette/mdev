use crate::app::actions::{Action, ComponentId, ComponentKind};
use crate::app::state::{AppState, SourceControlFile, SourceControlState};
use crate::capabilities::{CapabilityRequest, CapabilityResponse};

use std::collections::HashSet;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::RefreshSourceControl { sc_id } => {
            refresh(state, *sc_id);
            true
        }
        Action::ToggleSourceControlSelect { sc_id, path } => {
            toggle_select(state, *sc_id, path);
            true
        }
        Action::StageSelected { sc_id } => {
            stage_selected(state, *sc_id);
            true
        }
        Action::UnstageSelected { sc_id } => {
            unstage_selected(state, *sc_id);
            true
        }
        Action::StageAll { sc_id } => {
            stage_all(state, *sc_id);
            true
        }
        Action::UnstageAll { sc_id } => {
            unstage_all(state, *sc_id);
            true
        }
        Action::SetSourceControlBranch { sc_id, branch } => {
            if let Some(sc) = state.source_controls.get_mut(sc_id) {
                sc.branch = branch.clone();
            }
            true
        }
        Action::SetSourceControlRemote { sc_id, remote } => {
            if let Some(sc) = state.source_controls.get_mut(sc_id) {
                sc.remote = remote.clone();
            }
            true
        }
        Action::RefreshSourceControlBranchRemoteLists { sc_id } => {
            refresh_lists(state, *sc_id);
            true
        }
        Action::CheckoutBranch {
            sc_id,
            create_if_missing,
        } => {
            checkout(state, *sc_id, *create_if_missing);
            true
        }
        Action::FetchRemote { sc_id } => {
            fetch(state, *sc_id);
            true
        }
        Action::PullRemote { sc_id } => {
            pull(state, *sc_id);
            true
        }
        Action::SetCommitMessage { sc_id, msg } => {
            if let Some(sc) = state.source_controls.get_mut(sc_id) {
                sc.commit_message = msg.clone();
            }
            true
        }
        Action::CommitStaged { sc_id } => {
            commit(state, *sc_id);
            true
        }
        Action::CommitAndPush { sc_id } => {
            // Compose existing operations instead of duplicating logic.
            commit(state, *sc_id);
            push_remote(state, *sc_id);
            true
        }
        Action::StagePath { sc_id, path } => {
            stage_path(state, *sc_id, path);
            true
        }
        Action::UnstagePath { sc_id, path } => {
            unstage_path(state, *sc_id, path);
            true
        }
        Action::DiscardPath {
            sc_id,
            path,
            untracked,
        } => {
            discard_path(state, *sc_id, path, *untracked);
            true
        }

        _ => false,
    }
}

fn set_ok(state: &mut AppState, sc_id: ComponentId, msg: String) {
    if let Some(sc) = state.source_controls.get_mut(&sc_id) {
        sc.last_output = Some(msg);
        sc.last_error = None;
    }
}

fn set_err(state: &mut AppState, sc_id: ComponentId, msg: String) {
    if let Some(sc) = state.source_controls.get_mut(&sc_id) {
        sc.last_error = Some(msg);
    }
}

fn ensure_repo(state: &mut AppState, sc_id: ComponentId) -> Option<std::path::PathBuf> {
    let Some(repo) = state.inputs.repo.clone() else {
        set_err(state, sc_id, "No repo selected.".to_string());
        return None;
    };

    if state
        .broker
        .exec(CapabilityRequest::EnsureGitRepo { repo: repo.clone() })
        .is_err()
    {
        set_err(state, sc_id, "Selected folder is not a git repo (missing .git).".to_string());
        return None;
    }

    Some(repo)
}

fn refresh_lists(state: &mut AppState, sc_id: ComponentId) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };

    let branches = match state.broker.exec(CapabilityRequest::GitListLocalBranches { repo: repo.clone() }) {
        Ok(CapabilityResponse::GitBranches(v)) => v,
        Ok(_) => vec![],
        Err(_) => vec![],
    };

    let remotes = match state.broker.exec(CapabilityRequest::GitListRemotes { repo: repo.clone() }) {
        Ok(CapabilityResponse::GitRemotes(v)) => v,
        Ok(_) => vec![],
        Err(_) => vec![],
    };

    if let Some(sc) = state.source_controls.get_mut(&sc_id) {
        sc.branch_options = branches;
        sc.remote_options = remotes;

        if sc.remote.is_empty() {
            sc.remote = "origin".to_string();
        }
        if !sc.remote_options.is_empty() && !sc.remote_options.iter().any(|r| r == &sc.remote) {
            sc.remote = sc.remote_options[0].clone();
        }
    }
}

fn refresh(state: &mut AppState, sc_id: ComponentId) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };

    // Status
    let st = match state.broker.exec(CapabilityRequest::GitStatus { repo: repo.clone() }) {
        Ok(CapabilityResponse::GitStatus(v)) => v,
        Ok(_) => {
            set_err(state, sc_id, "Unexpected response from GitStatus.".to_string());
            return;
        }
        Err(e) => {
            set_err(state, sc_id, format!("GitStatus failed: {:#}", e));
            return;
        }
    };

    // Current branch (best-effort)
    let cur = match state.broker.exec(CapabilityRequest::GitCurrentBranch { repo: repo.clone() }) {
        Ok(CapabilityResponse::GitBranch(b)) => Some(b),
        Ok(_) => None,
        Err(_) => None,
    };

    // Lists
    refresh_lists(state, sc_id);

    if let Some(sc) = state.source_controls.get_mut(&sc_id) {
        // Files
        sc.files = st
            .files
            .into_iter()
            .map(|f| SourceControlFile {
                path: f.path,
                index_status: f.index_status,
                worktree_status: f.worktree_status,
                staged: f.staged,
                untracked: f.untracked,
            })
            .collect();

        // Branch selection
        if sc.branch.is_empty() {
            if let Some(b) = cur {
                sc.branch = b;
            }
        }

        // Keep selection only for existing paths
        let existing: HashSet<String> = sc.files.iter().map(|f| f.path.clone()).collect();
        sc.selected.retain(|p| existing.contains(p));
    }
}


fn toggle_select(state: &mut AppState, sc_id: ComponentId, path: &str) {
    if let Some(sc) = state.source_controls.get_mut(&sc_id) {
        if sc.selected.contains(path) {
            sc.selected.remove(path);
        } else {
            sc.selected.insert(path.to_string());
        }
    }
}

fn stage_path(state: &mut AppState, sc_id: ComponentId, path: &str) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };
    let paths = vec![path.to_string()];

    match state.broker.exec(CapabilityRequest::GitStagePaths { repo, paths }) {
        Ok(_) => {
            set_ok(state, sc_id, format!("Staged: {}", path));
            refresh(state, sc_id);
        }
        Err(e) => set_err(state, sc_id, format!("Stage failed: {:#}", e)),
    }
}

fn unstage_path(state: &mut AppState, sc_id: ComponentId, path: &str) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };
    let paths = vec![path.to_string()];

    match state.broker.exec(CapabilityRequest::GitUnstagePaths { repo, paths }) {
        Ok(_) => {
            set_ok(state, sc_id, format!("Unstaged: {}", path));
            refresh(state, sc_id);
        }
        Err(e) => set_err(state, sc_id, format!("Unstage failed: {:#}", e)),
    }
}

fn discard_path(state: &mut AppState, sc_id: ComponentId, path: &str, untracked: bool) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };

    if untracked {
        match state.broker.exec(CapabilityRequest::DeleteWorktreePath {
            repo,
            path: path.to_string(),
        }) {
            Ok(_) => {
                set_ok(state, sc_id, format!("Deleted untracked: {}", path));
                refresh(state, sc_id);
            }
            Err(e) => set_err(state, sc_id, format!("Delete failed: {:#}", e)),
        }
        return;
    }

    let paths = vec![path.to_string()];
    match state.broker.exec(CapabilityRequest::GitRestorePaths { repo, paths }) {
        Ok(_) => {
            set_ok(state, sc_id, format!("Discarded changes: {}", path));
            refresh(state, sc_id);
        }
        Err(e) => set_err(state, sc_id, format!("Discard failed: {:#}", e)),
    }
}

fn stage_selected(state: &mut AppState, sc_id: ComponentId) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };

    let paths: Vec<String> = state
        .source_controls
        .get(&sc_id)
        .map(|sc| sc.selected.iter().cloned().collect())
        .unwrap_or_default();

    if paths.is_empty() {
        set_ok(state, sc_id, "No files selected.".to_string());
        return;
    }

    match state
        .broker
        .exec(CapabilityRequest::GitStagePaths { repo, paths })
    {
        Ok(_) => set_ok(state, sc_id, "Staged selected files.".to_string()),
        Err(e) => set_err(state, sc_id, format!("Stage failed: {:#}", e)),
    }
}

fn unstage_selected(state: &mut AppState, sc_id: ComponentId) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };

    let paths: Vec<String> = state
        .source_controls
        .get(&sc_id)
        .map(|sc| sc.selected.iter().cloned().collect())
        .unwrap_or_default();

    if paths.is_empty() {
        set_ok(state, sc_id, "No files selected.".to_string());
        return;
    }

    match state
        .broker
        .exec(CapabilityRequest::GitUnstagePaths { repo, paths })
    {
        Ok(_) => set_ok(state, sc_id, "Unstaged selected files.".to_string()),
        Err(e) => set_err(state, sc_id, format!("Unstage failed: {:#}", e)),
    }
}

fn stage_all(state: &mut AppState, sc_id: ComponentId) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };

    match state.broker.exec(CapabilityRequest::GitStageAll { repo }) {
        Ok(_) => set_ok(state, sc_id, "Staged all changes.".to_string()),
        Err(e) => set_err(state, sc_id, format!("Stage all failed: {:#}", e)),
    }
}

fn unstage_all(state: &mut AppState, sc_id: ComponentId) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };

    match state.broker.exec(CapabilityRequest::GitUnstageAll { repo }) {
        Ok(_) => set_ok(state, sc_id, "Unstaged all changes.".to_string()),
        Err(e) => set_err(state, sc_id, format!("Unstage all failed: {:#}", e)),
    }
}

fn checkout(state: &mut AppState, sc_id: ComponentId, create_if_missing: bool) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };

    let branch = state
        .source_controls
        .get(&sc_id)
        .map(|sc| sc.branch.trim().to_string())
        .unwrap_or_default();

    if branch.is_empty() {
        set_err(state, sc_id, "Branch is empty.".to_string());
        return;
    }

    match state.broker.exec(CapabilityRequest::GitCheckoutBranch {
        repo,
        branch,
        create_if_missing,
    }) {
        Ok(CapabilityResponse::Text(out)) => set_ok(state, sc_id, out),
        Ok(_) => set_err(state, sc_id, "Unexpected response from GitCheckoutBranch.".to_string()),
        Err(e) => set_err(state, sc_id, format!("Checkout failed: {:#}", e)),
    }
}

fn fetch(state: &mut AppState, sc_id: ComponentId) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };

    let remote = state
        .source_controls
        .get(&sc_id)
        .map(|sc| sc.remote.trim().to_string())
        .filter(|s| !s.is_empty());

    match state.broker.exec(CapabilityRequest::GitFetch { repo, remote }) {
        Ok(CapabilityResponse::Text(out)) => set_ok(state, sc_id, out),
        Ok(_) => set_err(state, sc_id, "Unexpected response from GitFetch.".to_string()),
        Err(e) => set_err(state, sc_id, format!("Fetch failed: {:#}", e)),
    }
}

fn pull(state: &mut AppState, sc_id: ComponentId) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };

    let (remote, branch) = state
        .source_controls
        .get(&sc_id)
        .map(|sc| {
            (
                sc.remote.trim().to_string(),
                sc.branch.trim().to_string(),
            )
        })
        .unwrap_or_default();

    let remote = if remote.is_empty() { None } else { Some(remote) };
    let branch = if branch.is_empty() { None } else { Some(branch) };

    match state.broker.exec(CapabilityRequest::GitPull {
        repo,
        remote,
        branch,
    }) {
        Ok(CapabilityResponse::Text(out)) => set_ok(state, sc_id, out),
        Ok(_) => set_err(state, sc_id, "Unexpected response from GitPull.".to_string()),
        Err(e) => set_err(state, sc_id, format!("Pull failed: {:#}", e)),
    }
}

fn commit(state: &mut AppState, sc_id: ComponentId) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };

    let (msg, branch) = state
        .source_controls
        .get(&sc_id)
        .map(|sc| (sc.commit_message.trim().to_string(), sc.branch.trim().to_string()))
        .unwrap_or_default();

    if msg.is_empty() {
        set_err(state, sc_id, "Commit message is empty.".to_string());
        return;
    }

    let branch = if branch.is_empty() { None } else { Some(branch) };

    match state.broker.exec(CapabilityRequest::GitCommit {
        repo,
        message: msg,
        branch,
    }) {
        Ok(CapabilityResponse::Text(out)) => {
            set_ok(state, sc_id, out);
            if let Some(sc) = state.source_controls.get_mut(&sc_id) {
                sc.commit_message.clear();
            }
        }
        Ok(_) => set_err(state, sc_id, "Unexpected response from GitCommit.".to_string()),
        Err(e) => set_err(state, sc_id, format!("Commit failed: {:#}", e)),
    }
}

fn commit_and_push(state: &mut AppState, sc_id: ComponentId) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };

    let (msg, branch, remote) = state
        .source_controls
        .get(&sc_id)
        .map(|sc| {
            (
                sc.commit_message.trim().to_string(),
                sc.branch.trim().to_string(),
                sc.remote.trim().to_string(),
            )
        })
        .unwrap_or_default();

    if msg.is_empty() {
        set_err(state, sc_id, "Commit message is empty.".to_string());
        return;
    }

    let branch_opt = if branch.is_empty() { None } else { Some(branch) };
    let remote_opt = if remote.is_empty() { None } else { Some(remote) };

    // 1) Commit
    let commit_out = match state.broker.exec(CapabilityRequest::GitCommit {
        repo: repo.clone(),
        message: msg,
        branch: branch_opt.clone(),
    }) {
        Ok(CapabilityResponse::Text(out)) => out,
        Ok(_) => {
            set_err(state, sc_id, "Unexpected response from GitCommit.".to_string());
            return;
        }
        Err(e) => {
            set_err(state, sc_id, format!("Commit failed: {:#}", e));
            return;
        }
    };

    // 2) Push
    let push_out = match state.broker.exec(CapabilityRequest::GitPush {
        repo: repo.clone(),
        remote: remote_opt,
        branch: branch_opt,
    }) {
        Ok(CapabilityResponse::Text(out)) => out,
        Ok(_) => {
            set_err(state, sc_id, "Unexpected response from GitPush.".to_string());
            return;
        }
        Err(e) => {
            set_err(state, sc_id, format!("Push failed: {:#}", e));
            return;
        }
    };

    if let Some(sc) = state.source_controls.get_mut(&sc_id) {
        sc.commit_message.clear();
    }

    set_ok(state, sc_id, format!("{}\n\n{}", commit_out, push_out));
}

fn push_remote(state: &mut AppState, sc_id: ComponentId) {
    let Some(repo) = ensure_repo(state, sc_id) else { return; };

    let (remote, branch) = state
        .source_controls
        .get(&sc_id)
        .map(|sc| (sc.remote.trim().to_string(), sc.branch.trim().to_string()))
        .unwrap_or_default();

    let remote = if remote.is_empty() { None } else { Some(remote) };
    let branch = if branch.is_empty() { None } else { Some(branch) };

    match state.broker.exec(CapabilityRequest::GitPush { repo, remote, branch }) {
        Ok(CapabilityResponse::Text(out)) => {
            // Append push output if we already have commit output.
            if let Some(sc) = state.source_controls.get_mut(&sc_id) {
                if let Some(existing) = sc.last_output.take() {
                    sc.last_output = Some(format!("{}\n\n{}", existing, out));
                } else {
                    sc.last_output = Some(out);
                }
                sc.last_error = None;
            }
        }
        Ok(_) => set_err(state, sc_id, "Unexpected response from GitPush.".to_string()),
        Err(e) => set_err(state, sc_id, format!("Push failed: {:#}", e)),
    }
}

impl AppState {
    /// Called by layout/workspace controllers after layout changes.
    pub fn rebuild_source_controls_from_layout(&mut self) {
        self.source_controls.clear();

        let ids: Vec<ComponentId> = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::SourceControl)
            .map(|c| c.id)
            .collect();

        for id in ids {
            self.source_controls.insert(
                id,
                SourceControlState {
                    branch: String::new(),
                    branch_options: vec![],
                    remote: "origin".to_string(),
                    remote_options: vec!["origin".to_string()],
                    commit_message: String::new(),
                    files: vec![],
                    selected: HashSet::new(),
                    last_output: None,
                    last_error: None,
                    needs_refresh: true,
                },
            );
        }
    }
}
