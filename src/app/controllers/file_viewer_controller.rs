use crate::app::actions::{Action, ComponentId};
use crate::app::state::AppState;
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

impl AppState {
    fn apply_deferred_actions(&mut self) {
        // Open file in target viewer
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

        // Select commit in viewer
        if let Some((viewer_id, sel)) = self.deferred.select_commit.take() {
            if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                v.selected_commit = sel;
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

        // Refresh viewer
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
        let Some(repo) = &self.inputs.repo else {
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

        // Determine from/to refs (fallback to current git_ref)
        let (from_sel, to_sel) = {
            let v = self.file_viewers.get(&viewer_id).unwrap();
            (v.diff_base.clone(), v.diff_target.clone())
        };

        let from_ref = from_sel.unwrap_or_else(|| self.inputs.git_ref.clone());
        let to_ref = to_sel.unwrap_or_else(|| self.inputs.git_ref.clone());

        // Clear old output/error
        if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
            v.diff_err = None;
            v.diff_text.clear();
        }

        match git::diff_file_between(repo, &from_ref, &to_ref, &path) {
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
        let Some(repo) = &self.inputs.repo else {
            if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                v.file_content_err = Some("No repo selected".into());
            }
            return;
        };

        let v = self
            .file_viewers
            .entry(viewer_id)
            .or_insert_with(|| super::super::state::FileViewerState {
                selected_file: None,
                selected_commit: None,
                file_commits: vec![],
                file_content: "".into(),
                file_content_err: None,

                show_diff: false,
                diff_base: None,
                diff_target: None,
                diff_text: "".into(),
                diff_err: None,
            });

        v.selected_file = Some(file_path.to_string());
        v.selected_commit = None;
        v.file_content.clear();
        v.file_content_err = None;
        v.file_commits.clear();

        match git::file_history(repo, file_path, 80) {
            Ok(bytes) => v.file_commits = Self::parse_history(&bytes),
            Err(e) => v.file_content_err = Some(format!("Failed to load history: {:#}", e)),
        }

        self.load_file_at_current_selection(viewer_id);
    }

    pub fn load_file_at_current_selection(&mut self, viewer_id: ComponentId) {
        let Some(repo) = &self.inputs.repo else {
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

        // Always refresh history for this viewer/path (fixes non-original viewers)
        let mut history_err: Option<String> = None;
        match git::file_history(repo, &path, 80) {
            Ok(bytes) => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.file_commits = Self::parse_history(&bytes);
                }
            }
            Err(e) => {
                history_err = Some(format!("Failed to load history: {:#}", e));
            }
        }

        let selected_commit = self
            .file_viewers
            .get(&viewer_id)
            .and_then(|v| v.selected_commit.clone());

        let spec = if let Some(hash) = selected_commit {
            format!("{}:{}", hash, path)
        } else {
            format!("{}:{}", self.inputs.git_ref, path)
        };

        let mut content_err: Option<String> = None;

        match git::show_file_at(repo, &spec) {
            Ok(blob) => {
                let v = self.file_viewers.get_mut(&viewer_id).unwrap();
                if Self::is_binary(&blob) {
                    v.file_content = "(binary file)".into();
                } else {
                    v.file_content = String::from_utf8_lossy(&blob).to_string();
                }
            }
            Err(e) => {
                content_err = Some(format!("Failed to load {}: {:#}", spec, e));
                let v = self.file_viewers.get_mut(&viewer_id).unwrap();
                v.file_content.clear();
            }
        }

        let final_err = match (history_err, content_err) {
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
