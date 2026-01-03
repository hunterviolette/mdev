use std::collections::HashMap;

use crate::app::actions::{Action, ComponentId, ComponentKind, ExpandCmd};
use crate::app::layout::{
    FileViewerSnapshot, LayoutSnapshot, Preset, PresetKind, StateSnapshot, WorkspaceFile,
};
use crate::app::state::{AppState, PendingWorkspaceApply, ViewportRestore};

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::SaveWorkspace {
            canvas_size,
            viewport_outer_pos,
            viewport_inner_size,
        } => {
            let name = state.palette_last_name.take();
            state.save_workspace_dialog(
                *canvas_size,
                *viewport_outer_pos,
                *viewport_inner_size,
                name.as_deref(),
            );
            true
        }

        Action::LoadWorkspace => {
            let name = state.palette_last_name.take();
            state.load_workspace_dialog(name.as_deref());
            true
        }

        _ => false,
    }
}

impl AppState {
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
                if out.is_empty() {
                    "workspace".to_string()
                } else {
                    out
                }
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
            Err(e) => {
                self.results.error = Some(format!("Failed to serialize workspace: {}", e));
            }
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

    /// Called from app.rs every frame.
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

                // terminals + context exporters are ephemeral; recreate from the restored layout
                self.rebuild_terminals_from_layout();
                self.rebuild_context_exporters_from_layout();

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

                // terminals + context exporters are ephemeral; recreate from layout (repo already restored)
                self.rebuild_terminals_from_layout();
                self.rebuild_context_exporters_from_layout();

                // Restore file viewer instances (selection state only)
                self.file_viewers.clear();
                for (id, snap) in state_snap.file_viewers.iter() {
                    self.file_viewers.insert(
                        *id,
                        crate::app::state::FileViewerState {
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
    // appdata helpers (unchanged, still used by command palette)
    // ---------------------------

    fn app_data_dir() -> anyhow::Result<std::path::PathBuf> {
        use anyhow::anyhow;
        use directories::ProjectDirs;

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

        let out = out.trim().trim_end_matches('.').to_string();
        if out.is_empty() {
            "workspace".to_string()
        } else {
            out
        }
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

    // NOTE: keeping these in place unchanged (even if redundant with dialog methods),
    // because they existed in the old controller and might be referenced elsewhere.
    fn save_workspace_to_appdata(
        &mut self,
        canvas_size: [f32; 2],
        viewport_outer_pos: Option<[f32; 2]>,
        viewport_inner_size: Option<[f32; 2]>,
        suggested_name: Option<&str>,
    ) {
        let name = suggested_name
            .map(|s| s.to_string())
            .or_else(|| self.palette_last_name.clone())
            .unwrap_or_else(|| "workspace".to_string());

        let path = match Self::workspace_path(&name) {
            Ok(p) => p,
            Err(e) => {
                self.results.error =
                    Some(format!("Failed to resolve workspace directory: {:#}", e));
                return;
            }
        };

        self.layout.merge_with_defaults();

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
                self.results.error =
                    Some(format!("Failed to resolve workspace directory: {:#}", e));
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

        self.pending_workspace_apply = Some(PendingWorkspaceApply {
            preset: preset.kind.clone(),
            target_inner_size,
            wait_frames: 10,
        });

        self.results.error = None;
    }
}
