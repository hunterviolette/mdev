use crate::app::actions::{Action, ExpandCmd};
use crate::app::state::{AppState, DiffStatsSnapshot, GitStatusSnapshot, SourceControlFile, WORKTREE_REF};
use crate::capabilities::{CapabilityRequest, CapabilityResponse};
use std::collections::{HashMap, HashSet};

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::ExpandAll => {
            state.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
            true
        }
        Action::CollapseAll => {
            state.tree.expand_cmd = Some(ExpandCmd::CollapseAll);
            true
        }
        Action::OpenFile(path) => {
            state.deferred.open_file = Some(path.clone());
            state.deferred.open_file_target_viewer = state.active_file_viewer_id();
            true
        }
        Action::TreeDeletePath { path } => {
            if state.inputs.git_ref != WORKTREE_REF {
                return true;
            }
            let Some(repo) = state.inputs.repo.clone() else {
                return true;
            };
            let _ = state.broker.exec(CapabilityRequest::DeleteWorktreePath {
                repo,
                path: path.clone(),
            });
            state.tree.context_selected_files.remove(path);
            state.tree.rename_target = None;
            state.tree.rename_draft.clear();
            state.tree.create_parent = None;
            state.tree.create_draft.clear();
            state.tree.confirm_delete_target = None;
            state.start_analysis_refresh_async();
            true
        }
        Action::TreeRenamePath { from, to } => {
            if state.inputs.git_ref != WORKTREE_REF {
                return true;
            }
            let Some(repo) = state.inputs.repo.clone() else {
                return true;
            };
            let mut dst = to.trim().replace('\\', "/");
            if !dst.contains('/') {
                if let Some((parent, _)) = from.rsplit_once('/') {
                    dst = format!("{}/{}", parent, dst);
                }
            }
            if !dst.is_empty() && dst != *from {
                let _ = state.broker.exec(CapabilityRequest::MoveWorktreePath {
                    repo,
                    from: from.clone(),
                    to: dst,
                });
            }
            state.tree.rename_target = None;
            state.tree.rename_draft.clear();
            state.tree.create_parent = None;
            state.tree.create_draft.clear();
            state.tree.confirm_delete_target = None;
            state.start_analysis_refresh_async();
            true
        }
        Action::TreeCreateFile { path } => {
            if state.inputs.git_ref != WORKTREE_REF {
                return true;
            }
            let Some(repo) = state.inputs.repo.clone() else {
                return true;
            };
            let _ = state.broker.exec(CapabilityRequest::WriteWorktreeFile {
                repo,
                path: path.clone(),
                contents: Vec::new(),
            });
            state.tree.create_parent = None;
            state.start_analysis_refresh_async();
            true
        }
        Action::TreeCreateFolder { path } => {
            if state.inputs.git_ref != WORKTREE_REF {
                return true;
            }
            let Some(repo) = state.inputs.repo.clone() else {
                return true;
            };
            let _ = state.broker.exec(CapabilityRequest::CreateWorktreeDir {
                repo,
                path: path.clone(),
            });
            state.tree.create_parent = None;
            state.start_analysis_refresh_async();
            true
        }
        _ => false,
    }
}

impl AppState {
    pub fn refresh_tree_git_status(&mut self) {
        self.request_git_status_refresh();
    }

    pub(crate) fn request_git_status_refresh(&mut self) {
        let Some(repo) = self.inputs.repo.clone() else {
            self.tree.modified_paths.clear();
            self.tree.staged_paths.clear();
            self.tree.untracked_paths.clear();
            self.tree.git_status_by_path.clear();
            for sc in self.source_controls.values_mut() {
                sc.files.clear();
                sc.branch_options.clear();
                sc.remote_options.clear();
                sc.loading = false;
            }
            return;
        };

        if self.inputs.git_ref != WORKTREE_REF {
            self.tree.modified_paths.clear();
            self.tree.staged_paths.clear();
            self.tree.untracked_paths.clear();
            self.tree.git_status_by_path.clear();
            for sc in self.source_controls.values_mut() {
                sc.files.clear();
                sc.branch_options.clear();
                sc.remote_options.clear();
                sc.loading = false;
            }
            return;
        }

        let broker = self.broker.clone();
        self.tree.git_status_job.start_latest(move || {
            let st = match broker.exec(CapabilityRequest::GitStatus { repo: repo.clone() }) {
                Ok(CapabilityResponse::GitStatus(v)) => v,
                Ok(_) => return Err("Unexpected response from GitStatus.".to_string()),
                Err(e) => return Err(format!("GitStatus failed: {:#}", e)),
            };

            let branch = match broker.exec(CapabilityRequest::GitCurrentBranch { repo: repo.clone() }) {
                Ok(CapabilityResponse::GitBranch(b)) => Some(b),
                Ok(_) => None,
                Err(_) => None,
            };

            let branch_options = match broker.exec(CapabilityRequest::GitListLocalBranches { repo: repo.clone() }) {
                Ok(CapabilityResponse::GitBranches(v)) => v,
                Ok(_) => vec![],
                Err(_) => vec![],
            };

            let remote_options = match broker.exec(CapabilityRequest::GitListRemotes { repo: repo.clone() }) {
                Ok(CapabilityResponse::GitRemotes(v)) => v,
                Ok(_) => vec![],
                Err(_) => vec![],
            };

            let mut files = Vec::new();
            let mut git_status_by_path = HashMap::new();
            let mut modified_paths = HashSet::new();
            let mut staged_paths = HashSet::new();
            let mut untracked_paths = HashSet::new();

            for f in st.files {
                let sc_file = SourceControlFile {
                    path: f.path.clone(),
                    index_status: f.index_status.clone(),
                    worktree_status: f.worktree_status.clone(),
                    staged: f.staged,
                    untracked: f.untracked,
                    staged_additions: None,
                    staged_deletions: None,
                    unstaged_additions: None,
                    unstaged_deletions: None,
                };

                git_status_by_path.insert(f.path.clone(), f.clone());

                if f.untracked {
                    untracked_paths.insert(f.path.clone());
                    modified_paths.insert(f.path.clone());
                } else {
                    if f.staged {
                        staged_paths.insert(f.path.clone());
                    }
                    if f.worktree_status.trim() != "" || f.index_status.trim() != "" {
                        modified_paths.insert(f.path.clone());
                    }
                }

                files.push(sc_file);
            }

            Ok(GitStatusSnapshot {
                files,
                git_status_by_path,
                modified_paths,
                staged_paths,
                untracked_paths,
                branch,
                branch_options,
                remote_options,
            })
        });

        for sc in self.source_controls.values_mut() {
            sc.loading = true;
        }
    }

    fn request_diff_stats_refresh(&mut self) {
        if self.source_controls.is_empty() {
            return;
        }

        let Some(repo) = self.inputs.repo.clone() else {
            return;
        };

        if self.inputs.git_ref != WORKTREE_REF {
            return;
        }

        let untracked_paths: Vec<String> = self
            .tree
            .git_status_by_path
            .values()
            .filter(|f| f.untracked)
            .map(|f| f.path.clone())
            .collect();

        self.tree.diff_stats_job.start_latest(move || {
            let staged_by_path = crate::git::git_diff_stats(&repo, true)
                .map_err(|e| format!("{:#}", e))?;

            let unstaged_by_path = crate::git::git_diff_stats(&repo, false)
                .map_err(|e| format!("{:#}", e))?;

            let untracked_by_path = crate::git::git_untracked_line_stats(&repo, &untracked_paths);

            Ok(DiffStatsSnapshot {
                staged_by_path,
                unstaged_by_path,
                untracked_by_path,
            })
        });
    }

    fn apply_git_status_snapshot(&mut self, snapshot: GitStatusSnapshot) {
        self.tree.modified_paths = snapshot.modified_paths;
        self.tree.staged_paths = snapshot.staged_paths;
        self.tree.untracked_paths = snapshot.untracked_paths;
        self.tree.git_status_by_path = snapshot.git_status_by_path;

        for sc in self.source_controls.values_mut() {
            let existing_stats: HashMap<String, (Option<u64>, Option<u64>, Option<u64>, Option<u64>)> = sc
                .files
                .iter()
                .map(|f| {
                    (
                        f.path.clone(),
                        (
                            f.staged_additions,
                            f.staged_deletions,
                            f.unstaged_additions,
                            f.unstaged_deletions,
                        ),
                    )
                })
                .collect();

            sc.files = snapshot
                .files
                .iter()
                .cloned()
                .map(|mut f| {
                    if let Some((staged_additions, staged_deletions, unstaged_additions, unstaged_deletions)) = existing_stats.get(&f.path) {
                        f.staged_additions = *staged_additions;
                        f.staged_deletions = *staged_deletions;
                        f.unstaged_additions = *unstaged_additions;
                        f.unstaged_deletions = *unstaged_deletions;
                    }
                    f
                })
                .collect();
            sc.branch_options = snapshot.branch_options.clone();
            sc.remote_options = snapshot.remote_options.clone();
            sc.loading = false;

            if sc.branch.is_empty() {
                if let Some(branch) = &snapshot.branch {
                    sc.branch = branch.clone();
                }
            }

            if sc.remote.is_empty() {
                sc.remote = "origin".to_string();
            }
            if !sc.remote_options.is_empty() && !sc.remote_options.iter().any(|r| r == &sc.remote) {
                sc.remote = sc.remote_options[0].clone();
            }

            let existing: HashSet<String> = sc.files.iter().map(|f| f.path.clone()).collect();
            sc.selected.retain(|p| existing.contains(p));
            sc.last_error = None;
        }

        self.request_diff_stats_refresh();
    }
}

pub fn poll_git_status_refresh(state: &mut AppState) -> bool {
    let Some((_, result)) = state.tree.git_status_job.poll() else {
        return false;
    };

    match result {
        Ok(snapshot) => {
            state.apply_git_status_snapshot(snapshot);
            true
        }
        Err(_) => {
            state.tree.modified_paths.clear();
            state.tree.staged_paths.clear();
            state.tree.untracked_paths.clear();
            state.tree.git_status_by_path.clear();
            for sc in state.source_controls.values_mut() {
                sc.loading = false;
            }
            true
        }
    }
}

pub fn poll_diff_stats_refresh(state: &mut AppState) -> bool {
    let Some((_, result)) = state.tree.diff_stats_job.poll() else {
        return false;
    };

    match result {
        Ok(snapshot) => {
            for sc in state.source_controls.values_mut() {
                for f in sc.files.iter_mut() {
                    if let Some((additions, deletions)) = snapshot.staged_by_path.get(&f.path) {
                        f.staged_additions = Some(*additions);
                        f.staged_deletions = Some(*deletions);
                    } else {
                        f.staged_additions = None;
                        f.staged_deletions = None;
                    }

                    if f.untracked {
                        if let Some((additions, deletions)) = snapshot.untracked_by_path.get(&f.path) {
                            f.unstaged_additions = Some(*additions);
                            f.unstaged_deletions = Some(*deletions);
                        } else {
                            f.unstaged_additions = None;
                            f.unstaged_deletions = None;
                        }
                    } else if let Some((additions, deletions)) = snapshot.unstaged_by_path.get(&f.path) {
                        f.unstaged_additions = Some(*additions);
                        f.unstaged_deletions = Some(*deletions);
                    } else {
                        f.unstaged_additions = None;
                        f.unstaged_deletions = None;
                    }
                }
            }
            true
        }
        Err(_) => {
            false
        }
    }
}
