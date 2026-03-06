use crate::app::actions::{Action, ComponentId};
use crate::app::state::{AppState, FileViewAt, WORKTREE_REF};
use crate::capabilities::{CapabilityRequest, CapabilityResponse, FileSource};

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::OpenFile(path) => {
            let viewer_id = state
                .active_file_viewer_id()
                .or_else(|| state.file_viewers.keys().cloned().next());

            if let Some(viewer_id) = viewer_id {
                open_path_in_viewer(state, viewer_id, path.clone());
            } else {
                state.deferred.open_file = Some(path.clone());
                state.deferred.open_file_target_viewer = None;
            }
            true
        }

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

        Action::ToggleEditWorkingTree { viewer_id } => {
            if let Some(v) = state.file_viewers.get_mut(viewer_id) {
                v.edit_working_tree = !v.edit_working_tree;
                v.edit_status = None;
                v.edit_buffer = v.file_content.clone();
                v.editor = crate::app::ui::code_editor::CodeEditorState::default();
            }
            true
        }

        Action::SaveWorkingTreeFile { viewer_id } => {
            state.save_working_tree_edits(*viewer_id);
            true
        }

        Action::ToggleDiff { viewer_id } => {
            if let Some(v) = state.file_viewers.get_mut(viewer_id) {
                let turning_on = !v.show_diff;
                v.show_diff = turning_on;

                v.diff_err = None;
                v.diff_text.clear();

                if turning_on {
                    v.diff_picker_open = true;

                    if v.diff_base.is_none() {
                        v.diff_base = Some("HEAD".to_string());
                    }
                    if v.diff_target.is_none() {
                        v.diff_target = Some(state.inputs.git_ref.clone());
                    }
                } else {
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

        Action::RefreshDiff { viewer_id } => {
            state.refresh_diff(*viewer_id);
            if let Some(v) = state.file_viewers.get_mut(viewer_id) {
                v.diff_picker_open = false; 
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
        v.file_content.clear();
        v.edit_buffer.clear();

        v.editor = crate::app::ui::code_editor::CodeEditorState::default();
        v.viewer_editor = crate::app::ui::code_editor::CodeEditorState::default();

        v.file_load_pending = false;
        v.file_load_rx = None;
        v.history_load_pending = false;
        v.history_load_rx = None;
        v.file_commits.clear();

        v.show_diff = false;
        v.diff_picker_open = false;
        v.diff_text.clear();
        v.diff_err = None;
    }
    state.set_active_file_viewer_id(Some(viewer_id));
    state.load_file_at_current_selection(viewer_id);
}

impl AppState {
    pub fn load_file_at_current_selection(&mut self, viewer_id: ComponentId) {
        self.start_file_load_async(viewer_id);
    }

    pub fn any_file_load_pending(&self) -> bool {
        self.file_viewers.values().any(|v| v.file_load_pending || v.history_load_pending)
    }

    pub fn start_file_load_async(&mut self, viewer_id: ComponentId) {
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

        let seq = if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
            v.file_content_err = None;
            v.file_load_pending = true;
            v.file_load_rx = None;
            v.file_load_seq = v.file_load_seq.wrapping_add(1);
            v.file_load_path = Some(path.clone());
            v.file_load_seq
        } else {
            0
        };

        let (tx, rx) = std::sync::mpsc::channel::<(u64, String, Result<Vec<u8>, String>)>();
        if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
            v.file_load_rx = Some(rx);
        }

        let platform = self.platform.clone();
        std::thread::spawn(move || {
            let broker = crate::capabilities::CapabilityBroker::new(platform);
            let res = match broker.exec(CapabilityRequest::ReadFile {
                repo,
                path: path.clone(),
                source,
            }) {
                Ok(CapabilityResponse::Bytes(bytes)) => Ok(bytes),
                Ok(_) => Err("Unexpected response reading file.".to_string()),
                Err(e) => Err(format!("{:#}", e)),
            };
            let _ = tx.send((seq, path, res));
        });
    }

    pub fn start_file_history_async(&mut self, viewer_id: ComponentId) {
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

        if let Some(v) = self.file_viewers.get(&viewer_id) {
            if v.history_load_pending {
                return;
            }
        }

        if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
            v.history_load_pending = true;
            v.history_load_rx = None;
        }

        let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<u8>, String>>();
        if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
            v.history_load_rx = Some(rx);
        }

        let platform = self.platform.clone();
        std::thread::spawn(move || {
            let broker = crate::capabilities::CapabilityBroker::new(platform);
            let res = match broker.exec(CapabilityRequest::FileHistory {
                repo,
                path,
                max: 80,
            }) {
                Ok(CapabilityResponse::Bytes(bytes)) => Ok(bytes),
                Ok(_) => Err("Unexpected response loading history.".to_string()),
                Err(e) => Err(format!("{:#}", e)),
            };
            let _ = tx.send(res);
        });
    }

    pub fn poll_file_loads(&mut self) -> bool {
        let mut changed = false;
        let viewer_ids: Vec<ComponentId> = self.file_viewers.keys().cloned().collect();

        for viewer_id in viewer_ids {
            let mut should_start_history = false;

            if let Some(v) = self.file_viewers.get(&viewer_id) {
                if v.file_load_pending {
                    if let Some(rx) = &v.file_load_rx {
                        match rx.try_recv() {
                            Ok((seq, loaded_path, Ok(bytes))) => {
                                if let Some(v2) = self.file_viewers.get_mut(&viewer_id) {
                                    let current_path_ok = v2.selected_file.as_deref() == Some(loaded_path.as_str());
                                    let seq_ok = v2.file_load_seq == seq;

                                    if current_path_ok && seq_ok {
                                        let text = String::from_utf8_lossy(&bytes).to_string();
                                        v2.file_content = text;
                                        v2.file_content_err = None;
                                        v2.edit_buffer = v2.file_content.clone();
                                        v2.editor = crate::app::ui::code_editor::CodeEditorState::default();
                                        v2.viewer_editor = crate::app::ui::code_editor::CodeEditorState::default();
                                        should_start_history = v2.file_commits.is_empty();
                                    }

                                    v2.file_load_pending = false;
                                    v2.file_load_rx = None;
                                }
                                changed = true;
                            }
                            Ok((seq, loaded_path, Err(err))) => {
                                if let Some(v2) = self.file_viewers.get_mut(&viewer_id) {
                                    let current_path_ok = v2.selected_file.as_deref() == Some(loaded_path.as_str());
                                    let seq_ok = v2.file_load_seq == seq;

                                    if current_path_ok && seq_ok {
                                        v2.file_content_err = Some(err);
                                        v2.file_content.clear();
                                        v2.edit_buffer.clear();
                                        v2.editor = crate::app::ui::code_editor::CodeEditorState::default();
                                        v2.viewer_editor = crate::app::ui::code_editor::CodeEditorState::default();
                                    }

                                    v2.file_load_pending = false;
                                    v2.file_load_rx = None;
                                }
                                changed = true;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                if let Some(v2) = self.file_viewers.get_mut(&viewer_id) {
                                    v2.file_content_err = Some("File load channel disconnected.".to_string());
                                    v2.file_load_pending = false;
                                    v2.file_load_rx = None;
                                }
                                changed = true;
                            }
                        }
                    }
                }
            }

            if should_start_history {
                self.start_file_history_async(viewer_id);
            }

            if let Some(v) = self.file_viewers.get(&viewer_id) {
                if v.history_load_pending {
                    if let Some(rx) = &v.history_load_rx {
                        match rx.try_recv() {
                            Ok(Ok(bytes)) => {
                                let commits = Self::parse_history(&bytes);
                                if let Some(v2) = self.file_viewers.get_mut(&viewer_id) {
                                    v2.file_commits = commits;
                                    v2.history_load_pending = false;
                                    v2.history_load_rx = None;
                                }
                                changed = true;
                            }
                            Ok(Err(_)) => {
                                if let Some(v2) = self.file_viewers.get_mut(&viewer_id) {
                                    v2.file_commits.clear();
                                    v2.history_load_pending = false;
                                    v2.history_load_rx = None;
                                }
                                changed = true;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                if let Some(v2) = self.file_viewers.get_mut(&viewer_id) {
                                    v2.file_commits.clear();
                                    v2.history_load_pending = false;
                                    v2.history_load_rx = None;
                                }
                                changed = true;
                            }
                        }
                    }
                }
            }
        }

        changed
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

pub fn finalize_frame(state: &mut AppState) {
    if let Some(path) = state.deferred.open_file.take() {
        let target = state
            .deferred
            .open_file_target_viewer
            .take()
            .or(state.active_file_viewer_id())
            .or_else(|| state.file_viewers.keys().cloned().next());

        if let Some(viewer_id) = target {
            open_path_in_viewer(state, viewer_id, path);
        } else {
            state.deferred.open_file = Some(path);
        }
    }

    if let Some((viewer_id, commit)) = state.deferred.select_commit.take() {
        if let Some(v) = state.file_viewers.get_mut(&viewer_id) {
            v.selected_commit = commit;
            v.view_at = FileViewAt::Commit;
        }
        state.load_file_at_current_selection(viewer_id);
    }

    if let Some(viewer_id) = state.deferred.refresh_viewer.take() {
        state.load_file_at_current_selection(viewer_id);
    }
}
