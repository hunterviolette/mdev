// src/app/controllers/file_viewer_controller.rs
use crate::app::actions::{Action, ComponentId};
use crate::app::state::{AppState, FileViewAt, WORKTREE_REF};
use crate::git;
use crate::model::CommitEntry;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::SelectCommit { viewer_id, sel } => {
            state.deferred.select_commit = Some((*viewer_id, sel.clone()));
            true
        }
        Action::RefreshFile { viewer_id } => {
            state.deferred.refresh_viewer = Some(*viewer_id);
            true
        }

        Action::SetViewerViewAt { viewer_id, view_at } => {
            if let Some(v) = state.file_viewers.get_mut(viewer_id) {
                v.view_at = *view_at;
                v.selected_commit = None;
                v.file_content_err = None;
                v.edit_status = None;

                reset_editor_view_state(v);
            }
            state.load_file_at_current_selection(*viewer_id);
            true
        }

        Action::ToggleEditWorkingTree { viewer_id } => {
            state.toggle_edit_working_tree(*viewer_id);
            true
        }

        Action::SaveWorkingTreeFile { viewer_id } => {
            state.save_working_tree_file(*viewer_id);
            true
        }

        // ---- DIFF actions ----
        Action::ToggleDiff { viewer_id } => {
            if let Some(v) = state.file_viewers.get_mut(viewer_id) {
                v.show_diff = !v.show_diff;
                v.diff_err = None;
                if !v.show_diff {
                    v.diff_text.clear();
                }
            }
            true
        }
        Action::SetDiffBase { viewer_id, sel } => {
            if let Some(v) = state.file_viewers.get_mut(viewer_id) {
                v.diff_base = sel.clone();
            }
            true
        }
        Action::SetDiffTarget { viewer_id, sel } => {
            if let Some(v) = state.file_viewers.get_mut(viewer_id) {
                v.diff_target = sel.clone();
            }
            true
        }
        Action::RefreshDiff { viewer_id } => {
            state.load_diff_for_viewer(*viewer_id);
            true
        }
        // ----------------------

        _ => false,
    }
}

pub fn finalize_frame(state: &mut AppState) {
    state.apply_deferred_actions();
}

/// Reset editor UI state after reloading/replacing buffer.
fn reset_editor_view_state(v: &mut crate::app::state::FileViewerState) {
    v.editor.cursor_cc = 0;
    v.editor.selection_anchor_cc = None;
    v.editor.has_focus = false;
    v.editor.buffer_version = v.editor.buffer_version.wrapping_add(1);
    v.editor.line_cache.clear();
}

impl AppState {
    fn toggle_edit_working_tree(&mut self, viewer_id: ComponentId) {
        let Some(v) = self.file_viewers.get_mut(&viewer_id) else {
            return;
        };

        v.edit_status = None;
        v.edit_working_tree = !v.edit_working_tree;

        reset_editor_view_state(v);

        if v.edit_working_tree {
            v.view_at = FileViewAt::WorkingTree;
            v.selected_commit = None;

            let Some(repo) = self.inputs.repo.clone() else {
                v.edit_status = Some("No active repo selected.".into());
                return;
            };
            let Some(path) = v.selected_file.clone() else {
                v.edit_status = Some("No file selected.".into());
                return;
            };

            match git::read_worktree_file(&repo, &path) {
                Ok(bytes) => {
                    v.edit_buffer = String::from_utf8_lossy(&bytes).to_string();
                    v.edit_status = Some("Loaded from working tree.".into());
                    reset_editor_view_state(v);
                }
                Err(e) => {
                    v.edit_buffer.clear();
                    v.edit_status = Some(format!("Failed to read working tree: {:#}", e));
                }
            }
        }

        self.load_file_at_current_selection(viewer_id);
    }

    fn save_working_tree_file(&mut self, viewer_id: ComponentId) {
        let Some(repo) = self.inputs.repo.clone() else {
            if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                v.edit_status = Some("No active repo selected.".into());
            }
            return;
        };

        let (path, text) = {
            let Some(v) = self.file_viewers.get(&viewer_id) else {
                return;
            };
            let Some(path) = v.selected_file.clone() else {
                return;
            };
            (path, v.edit_buffer.clone())
        };

        match git::write_worktree_file(&repo, &path, text.as_bytes()) {
            Ok(()) => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.edit_status = Some("Saved to working tree.".into());
                    v.view_at = FileViewAt::WorkingTree;
                    v.selected_commit = None;
                    reset_editor_view_state(v);
                }
                self.load_file_at_current_selection(viewer_id);
            }
            Err(e) => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.edit_status = Some(format!("Save failed: {:#}", e));
                }
            }
        }
    }

    fn apply_deferred_actions(&mut self) {
        if let Some(path) = self.deferred.open_file.take() {
            let target = self
                .deferred
                .open_file_target_viewer
                .take()
                .or(self.active_file_viewer);

            if let Some(viewer_id) = target {
                self.load_file_view(viewer_id, &path);
            }
        }

        if let Some((viewer_id, sel)) = self.deferred.select_commit.take() {
            if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                v.selected_commit = sel.clone();
                if sel.is_some() {
                    v.view_at = FileViewAt::Commit;
                } else if v.view_at == FileViewAt::Commit {
                    v.view_at = FileViewAt::FollowTopBar;
                }

                reset_editor_view_state(v);
            }

            if self
                .file_viewers
                .get(&viewer_id)
                .and_then(|v| v.selected_file.clone())
                .is_some()
            {
                self.load_file_at_current_selection(viewer_id);
            }
        }

        if let Some(viewer_id) = self.deferred.refresh_viewer.take() {
            self.load_file_at_current_selection(viewer_id);
        }
    }

    fn parse_history(bytes: &[u8]) -> Vec<CommitEntry> {
        let s = String::from_utf8_lossy(bytes);
        s.lines()
            .filter_map(|line| {
                let mut parts = line.split('\x1f');
                let hash = parts.next()?.to_string();
                let date = parts.next()?.to_string();
                let summary = parts.next().unwrap_or("").to_string();
                Some(CommitEntry { hash, date, summary })
            })
            .collect()
    }

    fn is_binary(blob: &[u8]) -> bool {
        blob.iter().any(|&b| b == 0)
    }

    fn load_diff_for_viewer(&mut self, viewer_id: ComponentId) {
        let Some(repo) = self.inputs.repo.clone() else {
            if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                v.diff_err = Some("No repo selected".into());
            }
            return;
        };

        let Some(path) = self
            .file_viewers
            .get(&viewer_id)
            .and_then(|v| v.selected_file.clone())
        else {
            if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                v.diff_err = Some("No file selected".into());
            }
            return;
        };

        let (from_sel, to_sel) = {
            let v = self.file_viewers.get(&viewer_id).unwrap();
            (v.diff_base.clone(), v.diff_target.clone())
        };

        let from_ref = from_sel.unwrap_or_else(|| self.inputs.git_ref.clone());
        let to_ref = to_sel.unwrap_or_else(|| self.inputs.git_ref.clone());

        if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
            v.diff_err = None;
            v.diff_text.clear();
        }

        match git::diff_file_between(&repo, &from_ref, &to_ref, &path) {
            Ok(bytes) => {
                let s = String::from_utf8_lossy(&bytes).to_string();
                let out = if s.trim().is_empty() {
                    "(no changes)".to_string()
                } else {
                    s
                };
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.diff_text = out;
                }
            }
            Err(e) => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.diff_err = Some(format!("Failed to diff: {:#}", e));
                }
            }
        }
    }

    pub fn load_file_view(&mut self, viewer_id: ComponentId, file_path: &str) {
        let Some(repo) = self.inputs.repo.clone() else {
            if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                v.file_content_err = Some("No repo selected".into());
            }
            return;
        };

        let v = self
            .file_viewers
            .entry(viewer_id)
            .or_insert_with(crate::app::state::FileViewerState::new);

        v.selected_file = Some(file_path.to_string());
        v.selected_commit = None;

        if v.view_at == FileViewAt::Commit && v.selected_commit.is_none() {
            v.view_at = FileViewAt::FollowTopBar;
        }

        v.file_content.clear();
        v.file_content_err = None;
        v.file_commits.clear();

        reset_editor_view_state(v);

        match git::file_history(&repo, file_path, 80) {
            Ok(bytes) => v.file_commits = Self::parse_history(&bytes),
            Err(e) => v.file_content_err = Some(format!("Failed to load history: {:#}", e)),
        }

        self.load_file_at_current_selection(viewer_id);
    }

    pub fn load_file_at_current_selection(&mut self, viewer_id: ComponentId) {
        let Some(repo) = self.inputs.repo.clone() else {
            if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                v.file_content_err = Some("No repo selected".into());
            }
            return;
        };

        let Some(path) = self
            .file_viewers
            .get(&viewer_id)
            .and_then(|v| v.selected_file.clone())
        else {
            if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                v.file_content_err = Some("No file selected".into());
            }
            return;
        };

        // refresh history (doesn't require mutable self until after git call)
        let history_res = git::file_history(&repo, &path, 80);

        if let Ok(bytes) = &history_res {
            if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                v.file_commits = Self::parse_history(bytes);
            }
        }

        let history_err: Option<String> = match history_res {
            Ok(_) => None,
            Err(e) => Some(format!("Failed to load history: {:#}", e)),
        };

        let (mode, selected_commit, top_ref) = {
            let v = self.file_viewers.get(&viewer_id).unwrap();
            (v.view_at, v.selected_commit.clone(), self.inputs.git_ref.clone())
        };

        let content_result: Result<Vec<u8>, String> = match mode {
            FileViewAt::WorkingTree => git::read_worktree_file(&repo, &path)
                .map_err(|e| format!("Failed to read working tree: {:#}", e)),

            FileViewAt::Commit => match selected_commit {
                Some(hash) => {
                    let spec = format!("{}:{}", hash, path);
                    git::show_file_at(&repo, &spec)
                        .map_err(|e| format!("Failed to load {}: {:#}", spec, e))
                }
                None => Err("NO_COMMIT_SELECTED".to_string()),
            },

            FileViewAt::FollowTopBar => {
                if top_ref == WORKTREE_REF {
                    git::read_worktree_file(&repo, &path)
                        .map_err(|e| format!("Failed to read working tree: {:#}", e))
                } else {
                    let spec = format!("{}:{}", top_ref, path);
                    git::show_file_at(&repo, &spec)
                        .map_err(|e| format!("Failed to load {}: {:#}", spec, e))
                }
            }
        };

        match content_result {
            Ok(bytes) => {
                let resolved_worktree = match mode {
                    FileViewAt::WorkingTree => true,
                    FileViewAt::FollowTopBar => top_ref == WORKTREE_REF,
                    FileViewAt::Commit => false,
                };

                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.file_content_err = None;

                    if Self::is_binary(&bytes) {
                        v.file_content = "(binary file)".into();
                        v.edit_working_tree = false;
                    } else {
                        let text = String::from_utf8_lossy(&bytes).to_string();
                        v.file_content = text.clone();

                        // NOTE: you wanted worktree NOT to be default in general,
                        // but when *actually* in WORKTREE view, editor should be on.
                        if resolved_worktree {
                            v.edit_working_tree = true;
                            v.edit_buffer = text;
                            v.view_at = FileViewAt::WorkingTree;
                            v.selected_commit = None;
                            reset_editor_view_state(v);
                        } else {
                            // If not worktree, do not auto-force edit mode.
                            // Keep user toggle.
                        }
                    }
                }
            }
            Err(msg) if msg == "NO_COMMIT_SELECTED" => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.view_at = FileViewAt::FollowTopBar;
                    v.selected_commit = None;
                }
                self.load_file_at_current_selection(viewer_id);
                return;
            }
            Err(msg) => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.file_content.clear();
                    v.file_content_err = Some(msg);
                }
            }
        }

        let final_err = match (
            history_err,
            self.file_viewers
                .get(&viewer_id)
                .and_then(|v| v.file_content_err.clone()),
        ) {
            (None, None) => None,
            (Some(h), None) => Some(h),
            (None, Some(c)) => Some(c),
            (Some(h), Some(c)) => Some(format!("{h}\n{c}")),
        };

        if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
            v.file_content_err = final_err;
        }
    }
}
