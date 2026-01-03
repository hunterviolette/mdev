use std::collections::HashMap;

use crate::{analyze, format, git};
use crate::model::CommitEntry;

use anyhow::Result;
use regex::Regex;
use rfd::FileDialog;

use super::actions::{Action, ComponentId, ComponentKind, ExpandCmd};
use super::layout::{
    ComponentInstance, FileViewerSnapshot, LayoutConfig, LayoutSnapshot, Preset, PresetKind,
    StateSnapshot, WindowLayout, WorkspaceFile,
};
use super::state::{AppState, PendingWorkspaceApply, ViewportRestore};

impl AppState {
    pub fn apply_action(&mut self, action: Action) {
        match action {
            Action::ToggleCommandPalette => {
                self.palette.open = !self.palette.open;
                if self.palette.open {
                    self.palette.query.clear();
                    self.palette.selected = 0;
                }
            }

            Action::PickRepo => self.pick_repo_and_run(),

            Action::RunAnalysis => {
                self.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
                self.run_analysis();
            }

            Action::ExpandAll => self.tree.expand_cmd = Some(ExpandCmd::ExpandAll),
            Action::CollapseAll => self.tree.expand_cmd = Some(ExpandCmd::CollapseAll),

            Action::OpenFile(path) => {
                self.deferred.open_file = Some(path);
                self.deferred.open_file_target_viewer = self.active_file_viewer;
            }

            Action::SelectCommit { viewer_id, sel } => {
                self.deferred.select_commit = Some((viewer_id, sel));
            }

            Action::RefreshFile { viewer_id } => {
                self.deferred.refresh_viewer = Some(viewer_id);
            }

            // ---- DIFF actions ----
            Action::ToggleDiff { viewer_id } => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.show_diff = !v.show_diff;
                    v.diff_err = None;
                    if !v.show_diff {
                        v.diff_text.clear();
                    }
                }
            }

            Action::SetDiffBase { viewer_id, sel } => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.diff_base = sel;
                }
            }

            Action::SetDiffTarget { viewer_id, sel } => {
                if let Some(v) = self.file_viewers.get_mut(&viewer_id) {
                    v.diff_target = sel;
                }
            }

            Action::RefreshDiff { viewer_id } => {
                self.load_diff_for_viewer(viewer_id);
            }
            // ----------------------

            // ---- TERMINAL actions ----
            Action::SetTerminalShell { terminal_id, shell } => {
                if let Some(t) = self.terminals.get_mut(&terminal_id) {
                    t.shell = shell;
                }
            }

            Action::ClearTerminal { terminal_id } => {
                if let Some(t) = self.terminals.get_mut(&terminal_id) {
                    t.output.clear();
                    t.last_status = None;
                }
            }

            Action::RunTerminalCommand { terminal_id, cmd } => {
                self.run_terminal_command(terminal_id, &cmd);
            }
            // --------------------------

            Action::AddComponent { kind } => self.add_component(kind),

            Action::FocusFileViewer(id) => self.active_file_viewer = Some(id),

            Action::CloseComponent(id) => self.close_component(id),

            Action::ToggleLock(id) => {
                if let Some(w) = self.layout.get_window_mut(id) {
                    w.locked = !w.locked;
                }
            }

            Action::ResetLayout => {
                self.layout = LayoutConfig::default();
                self.layout.merge_with_defaults();
                self.layout_epoch = self.layout_epoch.wrapping_add(1);

                // Ensure a default file viewer state exists
                self.file_viewers
                    .entry(2)
                    .or_insert_with(|| super::state::FileViewerState {
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
                self.active_file_viewer = Some(2);

                // terminals are ephemeral; rebuild them from layout (default layout likely has none)
                self.rebuild_terminals_from_layout();
            }

            Action::SaveWorkspace {
                canvas_size,
                viewport_outer_pos,
                viewport_inner_size,
            } => {
                let name = self.palette_last_name.take();
                self.save_workspace_dialog(
                    canvas_size,
                    viewport_outer_pos,
                    viewport_inner_size,
                    name.as_deref(),
                );
            }

            Action::LoadWorkspace => {
                let name = self.palette_last_name.take();
                self.load_workspace_dialog(name.as_deref());
            }

            Action::None => {}
        }
    }

    pub fn finalize_frame(&mut self) {
        self.apply_deferred_actions();
    }

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

    // ---------------------------
    // Component creation
    // ---------------------------

    fn add_component(&mut self, kind: ComponentKind) {
        match kind {
            ComponentKind::FileViewer => self.new_file_viewer(),
            ComponentKind::Terminal => self.new_terminal(),

            ComponentKind::Tree | ComponentKind::Summary => {
                self.layout.merge_with_defaults();

                let id = self.layout.next_free_id();
                let title = match kind {
                    ComponentKind::Tree => format!("Tree {}", id),
                    ComponentKind::Summary => format!("Summary {}", id),
                    ComponentKind::FileViewer | ComponentKind::Terminal => unreachable!(),
                };

                self.layout.components.push(super::layout::ComponentInstance { id, kind, title });

                self.layout.windows.insert(
                    id,
                    super::layout::WindowLayout {
                        open: true,
                        locked: false,
                        pos: [80.0, 80.0],
                        size: [520.0, 700.0],
                    },
                );

                self.layout_epoch = self.layout_epoch.wrapping_add(1);
            }
        }
    }

    fn new_file_viewer(&mut self) {
        self.layout.merge_with_defaults();

        let id = self.layout.next_free_id();

        // Title should be numbered by "how many file viewers exist", not by global component id.
        let fv_count = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::FileViewer)
            .count();
        let title = format!("File Viewer {}", fv_count + 1);

        self.layout.components.push(ComponentInstance {
            id,
            kind: ComponentKind::FileViewer,
            title,
        });

        self.layout.windows.insert(
            id,
            WindowLayout {
                open: true,
                locked: false,
                pos: [60.0, 60.0],
                size: [760.0, 700.0],
            },
        );

        self.file_viewers.insert(
            id,
            super::state::FileViewerState {
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
            },
        );

        self.active_file_viewer = Some(id);
        self.layout_epoch = self.layout_epoch.wrapping_add(1);
    }

    fn close_component(&mut self, id: ComponentId) {
        if let Some(w) = self.layout.get_window_mut(id) {
            w.open = false;
        }

        // If it was active FV, pick another FV if available
        if self.active_file_viewer == Some(id) {
            self.active_file_viewer = self
                .layout
                .components
                .iter()
                .find(|c| c.kind == ComponentKind::FileViewer && c.id != id)
                .map(|c| c.id);
        }
    }

    // ---------------------------
    // Terminal ephemeral restore helpers
    // ---------------------------

    fn rebuild_terminals_from_layout(&mut self) {
        use super::actions::TerminalShell;
        use super::state::TerminalState;

        // Terminals are ephemeral. Workspace load restores the *layout* and terminal component IDs,
        // but the runtime terminal state must be recreated fresh each time.
        self.terminals.clear();

        let term_ids: Vec<ComponentId> = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::Terminal)
            .map(|c| c.id)
            .collect();

        for id in term_ids {
            self.terminals.insert(
                id,
                TerminalState {
                    shell: TerminalShell::Auto,
                    cwd: self.inputs.repo.clone(),
                    input: String::new(),
                    output: String::new(),
                    last_status: None,
                },
            );
        }
    }

    // ---------------------------
    // Workspace save/load (dialogs)
    // ---------------------------

    fn save_workspace_dialog(
        &mut self,
        canvas_size: [f32; 2],
        viewport_outer_pos: Option<[f32; 2]>,
        viewport_inner_size: Option<[f32; 2]>,
        suggested_name: Option<&str>,
    ) {
        use directories::ProjectDirs;

        // Determine name
        let name = suggested_name
            .map(|s| s.to_string())
            .or_else(|| self.palette_last_name.clone())
            .unwrap_or_else(|| "workspace".to_string());

        // Sanitize for filesystem
        let safe_name = {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                "workspace".to_string()
            } else {
                let mut out = String::with_capacity(trimmed.len());
                for ch in trimmed.chars() {
                    let ok = ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == ' ';
                    out.push(if ok { ch } else { '_' });
                }
                let out = out.trim().trim_end_matches('.').to_string();
                if out.is_empty() { "workspace".to_string() } else { out }
            }
        };

        // Resolve platform data dir
        let proj = match ProjectDirs::from("", "", "DescribeRepo") {
            Some(p) => p,
            None => {
                self.results.error = Some("Failed to resolve platform data directory.".into());
                return;
            }
        };

        // Workspaces directory
        let ws_dir = proj.data_dir().join("workspaces");
        if let Err(e) = std::fs::create_dir_all(&ws_dir) {
            self.results.error = Some(format!(
                "Failed to create workspace directory {:?}: {}",
                ws_dir, e
            ));
            return;
        }

        let path = ws_dir.join(format!("{safe_name}.json"));

        self.layout.merge_with_defaults();

        // snapshot viewer selections only (we reload content from git)
        let mut fv_snap: HashMap<ComponentId, FileViewerSnapshot> = HashMap::new();
        for (id, v) in self.file_viewers.iter() {
            fv_snap.insert(
                *id,
                FileViewerSnapshot {
                    selected_file: v.selected_file.clone(),
                    selected_commit: v.selected_commit.clone(),
                },
            );
        }

        let layout_preset = Preset {
            name: "layout".to_string(),
            kind: PresetKind::LayoutOnly(LayoutSnapshot {
                canvas_size,
                layout: self.layout.clone(),
            }),
        };

        let state_preset = Preset {
            name: "state".to_string(),
            kind: PresetKind::FullState(StateSnapshot {
                canvas_size,
                viewport_outer_pos,
                viewport_inner_size,

                repo: self.inputs.repo.clone(),
                git_ref: self.inputs.git_ref.clone(),
                exclude_regex: self.inputs.exclude_regex.clone(),
                max_exts: self.inputs.max_exts,

                filter_text: self.ui.filter_text.clone(),
                show_top_level_stats: self.ui.show_top_level_stats,

                layout: self.layout.clone(),

                file_viewers: fv_snap,
                active_file_viewer: self.active_file_viewer,
            }),
        };

        let ws = WorkspaceFile {
            version: 2,
            default_preset: Some("state".to_string()),
            presets: vec![layout_preset, state_preset],
        };

        match serde_json::to_string_pretty(&ws) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&path, s) {
                    self.results.error = Some(format!("Failed to save workspace: {}", e));
                } else {
                    self.results.error = None;
                }
            }
            Err(e) => self.results.error = Some(format!("Failed to serialize workspace: {}", e)),
        }
    }

    fn load_workspace_dialog(&mut self, suggested_name: Option<&str>) {
        use directories::ProjectDirs;

        // Determine name (must be provided via command palette or stored state)
        let name = suggested_name
            .map(|s| s.to_string())
            .or_else(|| self.palette_last_name.clone());

        let Some(name) = name else {
            self.results.error =
                Some("No workspace name provided. Use: workspace/load/<name>".into());
            return;
        };

        // Sanitize name the same way as save
        let safe_name = {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                self.results.error =
                    Some("Workspace name is empty. Use: workspace/load/<name>".into());
                return;
            }
            let mut out = String::with_capacity(trimmed.len());
            for ch in trimmed.chars() {
                let ok = ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == ' ';
                out.push(if ok { ch } else { '_' });
            }
            let out = out.trim().trim_end_matches('.').to_string();
            if out.is_empty() {
                self.results.error =
                    Some("Workspace name is invalid. Use: workspace/load/<name>".into());
                return;
            }
            out
        };

        // Resolve platform data dir
        let proj = match ProjectDirs::from("", "", "DescribeRepo") {
            Some(p) => p,
            None => {
                self.results.error = Some("Failed to resolve platform data directory.".into());
                return;
            }
        };

        let path = proj
            .data_dir()
            .join("workspaces")
            .join(format!("{safe_name}.json"));

        match std::fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<WorkspaceFile>(&s) {
                Ok(ws) => {
                    let preset = ws
                        .default_preset
                        .as_ref()
                        .and_then(|name| ws.presets.iter().find(|p| &p.name == name))
                        .or_else(|| ws.presets.first());

                    let Some(preset) = preset else {
                        self.results.error = Some("Workspace has no presets.".into());
                        return;
                    };

                    let mut target_inner_size: Option<[f32; 2]> = None;

                    if let PresetKind::FullState(st) = &preset.kind {
                        self.pending_viewport_restore = Some(ViewportRestore {
                            outer_pos: st.viewport_outer_pos,
                            inner_size: st.viewport_inner_size,
                        });
                        target_inner_size = st.viewport_inner_size;
                    }

                    // IMPORTANT: don't apply immediately; wait for viewport/canvas to settle
                    self.pending_workspace_apply = Some(PendingWorkspaceApply {
                        preset: preset.kind.clone(),
                        target_inner_size,
                        wait_frames: 10,
                    });

                    self.results.error = None;
                }
                Err(e) => {
                    self.results.error = Some(format!("Failed to parse workspace: {}", e));
                }
            },
            Err(e) => {
                self.results.error = Some(format!("Failed to read workspace '{}': {}", name, e));
            }
        }
    }

    /// This is called from app.rs every frame.
    /// Applies pending workspace once viewport resize has taken effect (or after a short timeout).
    pub fn try_apply_pending_workspace(
        &mut self,
        current_canvas_size: [f32; 2],
        current_viewport_inner_size: Option<[f32; 2]>,
    ) -> bool {
        let Some(p) = self.pending_workspace_apply.as_mut() else {
            return false;
        };

        // Wait for viewport size to match requested (within tolerance), or time out.
        if let Some(target) = p.target_inner_size {
            if let Some(cur) = current_viewport_inner_size {
                let dx = (cur[0] - target[0]).abs();
                let dy = (cur[1] - target[1]).abs();
                let close = dx < 2.0 && dy < 2.0;

                if !close && p.wait_frames > 0 {
                    p.wait_frames -= 1;
                    return false;
                }
            } else if p.wait_frames > 0 {
                p.wait_frames -= 1;
                return false;
            }
        } else if p.wait_frames > 0 {
            // Layout-only preset: wait a tiny beat for rects to stabilize.
            p.wait_frames = p.wait_frames.saturating_sub(1);
            return false;
        }

        // Ready (or timed out): apply now.
        let pending = self.pending_workspace_apply.take().unwrap();

        match pending.preset {
            PresetKind::LayoutOnly(layout_snap) => {
                let mut layout = layout_snap.layout;
                layout.rescale_from(layout_snap.canvas_size, current_canvas_size);
                layout.merge_with_defaults();
                self.layout = layout;

                //  terminals are ephemeral; recreate from the restored layout
                self.rebuild_terminals_from_layout();

                self.layout_epoch = self.layout_epoch.wrapping_add(1);
            }

            PresetKind::FullState(state_snap) => {
                // Restore inputs/UI
                self.inputs.repo = state_snap.repo;
                self.inputs.git_ref = state_snap.git_ref;
                self.inputs.exclude_regex = state_snap.exclude_regex;
                self.inputs.max_exts = state_snap.max_exts;

                self.ui.filter_text = state_snap.filter_text;
                self.ui.show_top_level_stats = state_snap.show_top_level_stats;

                // Layout
                let mut layout = state_snap.layout;
                layout.rescale_from(state_snap.canvas_size, current_canvas_size);
                layout.merge_with_defaults();
                self.layout = layout;

                //  terminals are ephemeral; recreate from layout (repo already restored)
                self.rebuild_terminals_from_layout();

                // Restore file viewer instances (selection state only)
                self.file_viewers.clear();
                for (id, snap) in state_snap.file_viewers.iter() {
                    self.file_viewers.insert(
                        *id,
                        super::state::FileViewerState {
                            selected_file: snap.selected_file.clone(),
                            selected_commit: snap.selected_commit.clone(),
                            file_commits: vec![],
                            file_content: "".into(),
                            file_content_err: None,

                            show_diff: false,
                            diff_base: None,
                            diff_target: None,
                            diff_text: "".into(),
                            diff_err: None,
                        },
                    );
                }

                // Restore active FV (fallback to first FV component)
                self.active_file_viewer = state_snap.active_file_viewer.or_else(|| {
                    self.layout
                        .components
                        .iter()
                        .find(|c| c.kind == ComponentKind::FileViewer)
                        .map(|c| c.id)
                });

                self.layout_epoch = self.layout_epoch.wrapping_add(1);

                // Reset analysis results then re-run
                self.results.result = None;
                self.results.error = None;

                if self.inputs.repo.is_some() {
                    self.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
                    self.run_analysis();

                    // Reload content+history for viewers with selected files
                    let ids: Vec<ComponentId> = self.file_viewers.keys().cloned().collect();
                    for id in ids {
                        if self
                            .file_viewers
                            .get(&id)
                            .and_then(|v| v.selected_file.clone())
                            .is_some()
                        {
                            self.load_file_at_current_selection(id);
                        }
                    }
                } else {
                    self.results.error = Some("Loaded state has no repo selected.".into());
                }
            }
        }

        true
    }

    // ---------------------------
    // Analysis
    // ---------------------------

    fn compile_excludes(&self) -> Result<Vec<Regex>> {
        let mut compiled = Vec::new();
        for rx in &self.inputs.exclude_regex {
            compiled.push(
                Regex::new(rx).map_err(|e| anyhow::anyhow!("Bad exclude regex '{}': {}", rx, e))?,
            );
        }
        Ok(compiled)
    }

    fn pick_repo_and_run(&mut self) {
        if let Some(p) = FileDialog::new()
            .set_title("Select a git repo folder")
            .pick_folder()
        {
            self.inputs.repo = Some(p);
            self.results.result = None;
            self.results.error = None;
            self.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
            self.run_analysis();
        }
    }

    fn run_analysis(&mut self) {
        self.results.error = None;
        self.results.result = None;

        let repo = match &self.inputs.repo {
            Some(r) => r.clone(),
            None => {
                self.results.error = Some("Select a repo folder first.".into());
                return;
            }
        };

        if let Err(e) = git::ensure_git_repo(&repo) {
            self.results.error = Some(format!("{:#}", e));
            return;
        }

        let compiled = match self.compile_excludes() {
            Ok(c) => c,
            Err(e) => {
                self.results.error = Some(format!("{:#}", e));
                return;
            }
        };

        match analyze::analyze_repo(&repo, &self.inputs.git_ref, &compiled, self.inputs.max_exts) {
            Ok(res) => {
                self.results.result = Some(res);
                self.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
            }
            Err(e) => self.results.error = Some(format!("{:#}", e)),
        }
    }

    // ---------------------------
    // File viewer (per instance)
    // ---------------------------

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
            .or_insert_with(|| super::state::FileViewerState {
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

    fn app_data_dir() -> anyhow::Result<std::path::PathBuf> {
        use anyhow::anyhow;
        use directories::ProjectDirs;

        // Change these identifiers if you want a different folder name.
        // On Windows: %APPDATA%\DescribeRepo
        // On macOS: ~/Library/Application Support/DescribeRepo
        // On Linux: ~/.local/share/describerepo (depending on dirs rules)
        let proj = ProjectDirs::from("", "", "DescribeRepo")
            .ok_or_else(|| anyhow!("Failed to resolve platform data directory"))?;

        Ok(proj.data_dir().to_path_buf())
    }

    fn workspaces_dir() -> anyhow::Result<std::path::PathBuf> {
        let mut dir = Self::app_data_dir()?;
        dir.push("workspaces");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn sanitize_workspace_name(name: &str) -> String {
        // Keep it simple and safe across platforms:
        // - trim
        // - replace path separators and weird chars with '_'
        // - fallback to "workspace" if empty
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return "workspace".to_string();
        }

        let mut out = String::with_capacity(trimmed.len());
        for ch in trimmed.chars() {
            let ok = ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == ' ';
            if ok {
                out.push(ch);
            } else {
                out.push('_');
            }
        }

        // Avoid trailing dots/spaces which Windows hates in filenames
        let out = out.trim().trim_end_matches('.').to_string();
        if out.is_empty() { "workspace".to_string() } else { out }
    }

    fn workspace_path(name: &str) -> anyhow::Result<std::path::PathBuf> {
        let dir = Self::workspaces_dir()?;
        let safe = Self::sanitize_workspace_name(name);
        Ok(dir.join(format!("{safe}.json")))
    }

    pub fn list_workspaces(&self) -> Vec<String> {
        let dir = match Self::workspaces_dir() {
            Ok(d) => d,
            Err(_) => return vec![],
        };

        let mut names = vec![];

        let Ok(rd) = std::fs::read_dir(dir) else {
            return vec![];
        };

        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }

        names.sort();
        names
    }

    fn save_workspace_to_appdata(
        &mut self,
        canvas_size: [f32; 2],
        viewport_outer_pos: Option<[f32; 2]>,
        viewport_inner_size: Option<[f32; 2]>,
        suggested_name: Option<&str>,
    ) {
        // Name logic:
        // - if user provided a name -> use it
        // - else reuse last palette name if available
        // - else default "workspace"
        let name = suggested_name
            .map(|s| s.to_string())
            .or_else(|| self.palette_last_name.clone())
            .unwrap_or_else(|| "workspace".to_string());

        let path = match Self::workspace_path(&name) {
            Ok(p) => p,
            Err(e) => {
                self.results.error = Some(format!("Failed to resolve workspace directory: {:#}", e));
                return;
            }
        };

        self.layout.merge_with_defaults();

        // snapshot viewer selections only (we reload content from git)
        let mut fv_snap: HashMap<ComponentId, FileViewerSnapshot> = HashMap::new();
        for (id, v) in self.file_viewers.iter() {
            fv_snap.insert(
                *id,
                FileViewerSnapshot {
                    selected_file: v.selected_file.clone(),
                    selected_commit: v.selected_commit.clone(),
                },
            );
        }

        let layout_preset = Preset {
            name: "layout".to_string(),
            kind: PresetKind::LayoutOnly(LayoutSnapshot {
                canvas_size,
                layout: self.layout.clone(),
            }),
        };

        let state_preset = Preset {
            name: "state".to_string(),
            kind: PresetKind::FullState(StateSnapshot {
                canvas_size,
                viewport_outer_pos,
                viewport_inner_size,

                repo: self.inputs.repo.clone(),
                git_ref: self.inputs.git_ref.clone(),
                exclude_regex: self.inputs.exclude_regex.clone(),
                max_exts: self.inputs.max_exts,

                filter_text: self.ui.filter_text.clone(),
                show_top_level_stats: self.ui.show_top_level_stats,

                layout: self.layout.clone(),

                file_viewers: fv_snap,
                active_file_viewer: self.active_file_viewer,
            }),
        };

        let ws = WorkspaceFile {
            version: 2,
            default_preset: Some("state".to_string()),
            presets: vec![layout_preset, state_preset],
        };

        match serde_json::to_string_pretty(&ws) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&path, s) {
                    self.results.error = Some(format!("Failed to save workspace: {}", e));
                } else {
                    self.results.error = None;
                }
            }
            Err(e) => self.results.error = Some(format!("Failed to serialize workspace: {}", e)),
        }
    }

    fn load_workspace_from_appdata(&mut self, suggested_name: Option<&str>) {
        // If user provided a name use it; else try palette_last_name; else refuse gracefully.
        let name = suggested_name
            .map(|s| s.to_string())
            .or_else(|| self.palette_last_name.clone());

        let Some(name) = name else {
            self.results.error = Some(
                "No workspace name provided. Try: workspace/load/<name> (or list available names)."
                    .into(),
            );
            return;
        };

        let path = match Self::workspace_path(&name) {
            Ok(p) => p,
            Err(e) => {
                self.results.error = Some(format!("Failed to resolve workspace directory: {:#}", e));
                return;
            }
        };

        let s = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                self.results.error = Some(format!("Failed to read workspace '{}': {}", name, e));
                return;
            }
        };

        let ws = match serde_json::from_str::<WorkspaceFile>(&s) {
            Ok(ws) => ws,
            Err(e) => {
                self.results.error = Some(format!("Failed to parse workspace '{}': {}", name, e));
                return;
            }
        };

        let preset = ws
            .default_preset
            .as_ref()
            .and_then(|n| ws.presets.iter().find(|p| &p.name == n))
            .or_else(|| ws.presets.first());

        let Some(preset) = preset else {
            self.results.error = Some("Workspace has no presets.".into());
            return;
        };

        let mut target_inner_size: Option<[f32; 2]> = None;

        if let PresetKind::FullState(st) = &preset.kind {
            self.pending_viewport_restore = Some(ViewportRestore {
                outer_pos: st.viewport_outer_pos,
                inner_size: st.viewport_inner_size,
            });
            target_inner_size = st.viewport_inner_size;
        }

        // IMPORTANT: don't apply immediately; wait for viewport/canvas to settle
        self.pending_workspace_apply = Some(PendingWorkspaceApply {
            preset: preset.kind.clone(),
            target_inner_size,
            wait_frames: 10,
        });

        self.results.error = None;
    }

    fn new_terminal(&mut self) {
        use super::layout::{ComponentInstance, WindowLayout};
        use super::state::TerminalState;

        self.layout.merge_with_defaults();

        let id = self.layout.next_free_id();

        let term_count = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::Terminal)
            .count();

        let title = format!("Terminal {}", term_count + 1);

        self.layout.components.push(ComponentInstance {
            id,
            kind: ComponentKind::Terminal,
            title,
        });

        self.layout.windows.insert(
            id,
            WindowLayout {
                open: true,
                locked: false,
                pos: [90.0, 90.0],
                size: [760.0, 420.0],
            },
        );

        self.terminals.insert(
            id,
            TerminalState {
                shell: super::actions::TerminalShell::Auto,
                cwd: self.inputs.repo.clone(),
                input: String::new(),
                output: String::new(),
                last_status: None,
            },
        );

        self.layout_epoch = self.layout_epoch.wrapping_add(1);
    }

    fn run_terminal_command(&mut self, terminal_id: ComponentId, cmd: &str) {
        use std::process::Command;

        let Some(t) = self.terminals.get_mut(&terminal_id) else {
            return;
        };

        let cwd = t.cwd.clone().or_else(|| self.inputs.repo.clone());

        let (program, args): (&str, Vec<String>) = match t.shell {
            super::actions::TerminalShell::Auto => {
                if cfg!(windows) {
                    ("powershell", vec!["-NoProfile".into(), "-Command".into(), cmd.into()])
                } else {
                    ("bash", vec!["-lc".into(), cmd.into()])
                }
            }
            super::actions::TerminalShell::PowerShell => (
                "powershell",
                vec!["-NoProfile".into(), "-Command".into(), cmd.into()],
            ),
            super::actions::TerminalShell::Cmd => ("cmd", vec!["/C".into(), cmd.into()]),
            super::actions::TerminalShell::Bash => ("bash", vec!["-lc".into(), cmd.into()]),
            super::actions::TerminalShell::Zsh => ("zsh", vec!["-lc".into(), cmd.into()]),
            super::actions::TerminalShell::Sh => ("sh", vec!["-lc".into(), cmd.into()]),
        };

        t.output.push_str(&format!("\n$ {}\n", cmd));

        let mut c = Command::new(program);
        c.args(args);

        if let Some(dir) = cwd {
            c.current_dir(dir);
        }

        match c.output() {
            Ok(out) => {
                let code = out.status.code().unwrap_or(-1);
                t.last_status = Some(code);

                if !out.stdout.is_empty() {
                    t.output.push_str(&String::from_utf8_lossy(&out.stdout));
                    if !t.output.ends_with('\n') {
                        t.output.push('\n');
                    }
                }
                if !out.stderr.is_empty() {
                    t.output.push_str(&String::from_utf8_lossy(&out.stderr));
                    if !t.output.ends_with('\n') {
                        t.output.push('\n');
                    }
                }

                t.output.push_str(&format!("[exit: {}]\n", code));
            }
            Err(e) => {
                t.last_status = Some(-1);
                t.output.push_str(&format!("Failed to run command: {}\n", e));
            }
        }
    }

    // helpers used by UI
    pub fn excludes_joined(&self) -> String {
        format::join_excludes(&self.inputs.exclude_regex)
    }
    pub fn set_excludes_from_joined(&mut self, joined: &str) {
        self.inputs.exclude_regex = format::parse_excludes(joined);
    }
}
