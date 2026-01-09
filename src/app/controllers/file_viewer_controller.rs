use crate::app::actions::{Action, ComponentId};
use crate::app::state::{AppState, FileViewAt, WORKTREE_REF};
use crate::capabilities::{CapabilityRequest, CapabilityResponse, FileSource};

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        // ------------------------------------------------------------
        // Open file (generic open; usually opens in active viewer)
        // ------------------------------------------------------------
        Action::OpenFile(path) => {
            let viewer_id = state
                .active_file_viewer
                .or_else(|| state.file_viewers.keys().cloned().next());

            if let Some(viewer_id) = viewer_id {
                open_path_in_viewer(state, viewer_id, path.clone());
            } else {
                state.deferred.open_file = Some(path.clone());
                state.deferred.open_file_target_viewer = None;
            }
            true
        }

        // ------------------------------------------------------------
        // View mode changes
        // ------------------------------------------------------------
        Action::SetViewerViewAt { viewer_id, view_at } => {
            if let Some(v) = state.file_viewers.get_mut(viewer_id) {
                v.view_at = *view_at;
                if *view_at != FileViewAt::Commit {
                    v.selected_commit = None;
                }
            }
            state.load_file_at_current_selection(*viewer_id);
            true
        }

        Action::SelectCommit { viewer_id, sel } => {
            if let Some(v) = state.file_viewers.get_mut(viewer_id) {
                v.selected_commit = sel.clone();
                v.view_at = FileViewAt::Commit;
            }
            state.load_file_at_current_selection(*viewer_id);
            true
        }

        Action::RefreshFile { viewer_id } => {
            state.load_file_at_current_selection(*viewer_id);
            true
        }

        // ------------------------------------------------------------
        // Editing
        // ------------------------------------------------------------
        Action::ToggleEditWorkingTree { viewer_id } => {
            if let Some(v) = state.file_viewers.get_mut(viewer_id) {
                v.edit_working_tree = !v.edit_working_tree;
                v.edit_status = None;
                if v.edit_working_tree {
                    v.edit_buffer = v.file_content.clone();
                }
            }
            true
        }

        Action::SaveWorkingTreeFile { viewer_id } => {
            state.save_working_tree_edits(*viewer_id);
            true
        }

        // ------------------------------------------------------------
        // Diff (do NOT auto-run on toggle)
        // ------------------------------------------------------------
        Action::ToggleDiff { viewer_id } => {
            if let Some(v) = state.file_viewers.get_mut(viewer_id) {
                let turning_on = !v.show_diff;
                v.show_diff = turning_on;

                // Clear stale results when opening/closing
                v.diff_err = None;
                v.diff_text.clear();

                if turning_on {
                    // Open picker immediately; Generate closes it, diff stays visible.
                    v.diff_picker_open = true;

                    if v.diff_base.is_none() {
                        v.diff_base = Some("HEAD".to_string());
                    }
                    if v.diff_target.is_none() {
                        v.diff_target = Some(state.inputs.git_ref.clone());
                    }
                } else {
                    // Turning off diff also closes the picker.
                    v.diff_picker_open = false;
                }
            }
            true
        }

        Action::SetDiffBase { viewer_id, sel } => {
            if let Some(v) = state.file_viewers.get_mut(viewer_id) {
                v.diff_base = sel.clone();
                v.diff_err = None;
                v.diff_text.clear();
            }
            true
        }

        Action::SetDiffTarget { viewer_id, sel } => {
            if let Some(v) = state.file_viewers.get_mut(viewer_id) {
                v.diff_target = sel.clone();
                v.diff_err = None;
                v.diff_text.clear();
            }
            true
        }

        // Generate diff button
        Action::RefreshDiff { viewer_id } => {
            state.refresh_diff(*viewer_id);
            if let Some(v) = state.file_viewers.get_mut(viewer_id) {
                v.diff_picker_open = false; // ✅ THIS IS THE KEY UX FIX
            }
            true
        }

        _ => false,
    }
}

fn open_path_in_viewer(state: &mut AppState, viewer_id: ComponentId, path: String) {
    if let Some(v) = state.file_viewers.get_mut(&viewer_id) {
        v.selected_file = Some(path);
        v.file_content_err = None;
        v.edit_status = None;

        // Optional: when switching files, keep diff mode off by default
        // (prevents surprising "stale diff" feeling between files)
        v.show_diff = false;
        v.diff_picker_open = false;
        v.diff_text.clear();
        v.diff_err = None;
    }
    state.active_file_viewer = Some(viewer_id);
    state.load_file_at_current_selection(viewer_id);
}

impl AppState {
    pub fn load_file_at_current_selection(&mut self, viewer_id: ComponentId) {
        let Some(repo) = self.inputs.repo.clone() else {
            if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                v.file_content_err = Some("No repo selected.".into());
                v.file_content.clear();
            }
            return;
        };

        let Some(path) = self
            .file_viewers
            .get(&viewer_id)
            .and_then(|v| v.selected_file.clone())
        else {
            return;
        };

        let source: FileSource = match self.file_viewers.get(&viewer_id).map(|v| v.view_at) {
            Some(FileViewAt::WorkingTree) => FileSource::Worktree,
            Some(FileViewAt::Commit) => {
                let commit = self
                    .file_viewers
                    .get(&viewer_id)
                    .and_then(|v| v.selected_commit.clone())
                    .unwrap_or_else(|| "HEAD".to_string());
                FileSource::GitRef(commit)
            }
            _ => crate::capabilities::CapabilityBroker::file_source_from_ref(&self.inputs.git_ref),
        };

        match self.broker.exec(CapabilityRequest::ReadFile {
            repo: repo.clone(),
            path: path.clone(),
            source,
        }) {
            Ok(CapabilityResponse::Bytes(bytes)) => {
                let text = String::from_utf8_lossy(&bytes).to_string();
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.file_content = text;
                    v.file_content_err = None;
                    if v.edit_working_tree {
                        v.edit_buffer = v.file_content.clone();
                    }
                }
            }
            Ok(_) => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.file_content_err = Some("Unexpected response reading file.".into());
                    v.file_content.clear();
                }
            }
            Err(e) => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.file_content_err = Some(format!("{:#}", e));
                    v.file_content.clear();
                }
            }
        }

        // History for picker (your repo returns Bytes you parse)
        match self.broker.exec(CapabilityRequest::FileHistory {
            repo,
            path: path.clone(),
            max: 80,
        }) {
            Ok(CapabilityResponse::Bytes(bytes)) => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.file_commits = Self::parse_history(&bytes);
                }
            }
            _ => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.file_commits.clear();
                }
            }
        }
    }

    pub fn save_working_tree_edits(&mut self, viewer_id: ComponentId) {
        let Some(repo) = self.inputs.repo.clone() else {
            if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                v.edit_status = Some("No repo selected.".into());
            }
            return;
        };

        let Some(path) = self
            .file_viewers
            .get(&viewer_id)
            .and_then(|v| v.selected_file.clone())
        else {
            return;
        };

        let text = match self.file_viewers.get(&viewer_id) {
            Some(v) => v.edit_buffer.clone(),
            None => return,
        };

        match self.broker.exec(CapabilityRequest::WriteWorktreeFile {
            repo,
            path: path.clone(),
            contents: text.as_bytes().to_vec(),
        }) {
            Ok(CapabilityResponse::Unit) => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.edit_status = Some("Saved to working tree.".into());
                    v.file_content = v.edit_buffer.clone();
                    v.file_content_err = None;
                }
                if self.inputs.git_ref == WORKTREE_REF {
                    self.run_analysis();
                }
            }
            Ok(_) => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.edit_status = Some("Unexpected response saving file.".into());
                }
            }
            Err(e) => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.edit_status = Some(format!("{:#}", e));
                }
            }
        }
    }

    fn refresh_diff(&mut self, viewer_id: ComponentId) {
        let Some(repo) = self.inputs.repo.clone() else {
            return;
        };

        let Some(path) = self
            .file_viewers
            .get(&viewer_id)
            .and_then(|v| v.selected_file.clone())
        else {
            return;
        };

        let (show_diff, base, target) = match self.file_viewers.get(&viewer_id) {
            Some(v) => (v.show_diff, v.diff_base.clone(), v.diff_target.clone()),
            None => return,
        };

        if !show_diff {
            return;
        }

        // Allow None => "use top-bar ref"
        let from_ref = base.unwrap_or_else(|| self.inputs.git_ref.clone());
        let to_ref = target.unwrap_or_else(|| self.inputs.git_ref.clone());

        match self.broker.exec(CapabilityRequest::DiffFileBetween {
            repo,
            from_ref,
            to_ref,
            path,
        }) {
            Ok(CapabilityResponse::Bytes(bytes)) => {
                let text = String::from_utf8_lossy(&bytes).to_string();
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.diff_text = text;
                    v.diff_err = None;
                }
            }
            Ok(_) => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.diff_err = Some("Unexpected response generating diff.".into());
                    v.diff_text.clear();
                }
            }
            Err(e) => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.diff_err = Some(format!("{:#}", e));
                    v.diff_text.clear();
                }
            }
        }
    }

    fn parse_history(bytes: &[u8]) -> Vec<crate::model::CommitEntry> {
        let s = String::from_utf8_lossy(bytes);
        let mut out = vec![];
        for line in s.lines() {
            let mut parts = line.split('\x1f');
            let hash = parts.next().unwrap_or("").to_string();
            let date = parts.next().unwrap_or("").to_string();
            let summary = parts.next().unwrap_or("").to_string();
            if !hash.is_empty() {
                out.push(crate::model::CommitEntry { hash, date, summary });
            }
        }
        out
    }
}

/// Applies deferred “open file”, “select commit”, “refresh viewer” actions at end-of-frame.
pub fn finalize_frame(state: &mut AppState) {
    // 1) open file if deferred
    if let Some(path) = state.deferred.open_file.take() {
        let target = state
            .deferred
            .open_file_target_viewer
            .take()
            .or(state.active_file_viewer)
            .or_else(|| state.file_viewers.keys().cloned().next());

        if let Some(viewer_id) = target {
            open_path_in_viewer(state, viewer_id, path);
        } else {
            // no viewer yet; keep it deferred
            state.deferred.open_file = Some(path);
        }
    }

    // 2) select commit if deferred
    if let Some((viewer_id, commit)) = state.deferred.select_commit.take() {
        if let Some(v) = state.file_viewers.get_mut(&viewer_id) {
            v.selected_commit = commit;
            v.view_at = FileViewAt::Commit;
        }
        state.load_file_at_current_selection(viewer_id);
    }

    // 3) refresh viewer if deferred
    if let Some(viewer_id) = state.deferred.refresh_viewer.take() {
        state.load_file_at_current_selection(viewer_id);
    }
}
