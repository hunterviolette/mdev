// src/app/controllers/workspace_controller.rs
use std::collections::HashMap;
use std::path::PathBuf;

use crate::app::actions::{Action, ComponentId, ComponentKind, ExpandCmd};
use crate::app::layout::{
    FileViewerSnapshot, LayoutSnapshot, Preset, PresetKind, StateSnapshot, WorkspaceFile,
};
use crate::app::state::{AppState, ContextExporterState, ContextExportMode, PendingWorkspaceApply, ViewportRestore, WORKTREE_REF};

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::SaveWorkspace {
            canvas_size,
            viewport_outer_pos,
            viewport_inner_size,
        } => {
            let name = state.palette_last_name.take();
            state.save_workspace_to_appdata(
                *canvas_size,
                *viewport_outer_pos,
                *viewport_inner_size,
                name.as_deref(), // ✅ 4th arg restored
            );
            true
        }

        Action::LoadWorkspace => {
            let name = state.palette_last_name.take();
            state.load_workspace_from_appdata(name.as_deref());
            true
        }

        _ => false,
    }
}

impl AppState {
    // ---------------------------
    // Platform-backed workspace paths
    // ---------------------------

    fn app_data_dir(&self) -> anyhow::Result<PathBuf> {
        // Use the platform boundary (native implementation uses directories::ProjectDirs).
        self.platform.app_data_dir("DescribeRepo")
    }

    fn workspaces_dir(&self) -> anyhow::Result<PathBuf> {
        let mut dir = self.app_data_dir()?;
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

    fn workspace_path(&self, name: &str) -> anyhow::Result<PathBuf> {
        let dir = self.workspaces_dir()?;
        let safe = Self::sanitize_workspace_name(name);
        Ok(dir.join(format!("{safe}.json")))
    }

    pub fn list_workspaces(&self) -> Vec<String> {
        let dir = match self.workspaces_dir() {
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

    // ---------------------------
    // Save / Load
    // ---------------------------

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

        let path = match self.workspace_path(&name) {
            Ok(p) => p,
            Err(e) => {
                self.results.error =
                    Some(format!("Failed to resolve workspace directory: {:#}", e));
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
            Err(e) => {
                self.results.error = Some(format!("Failed to serialize workspace: {}", e));
            }
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

        let path = match self.workspace_path(&name) {
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

    // ---------------------------
    // Workspace apply (called from app each frame)
    // ---------------------------

    pub fn try_apply_pending_workspace(
        &mut self,
        current_canvas_size: [f32; 2],
        current_inner_size: Option<[f32; 2]>,
    ) -> bool {
        let Some(pending) = self.pending_workspace_apply.clone() else {
            return false;
        };

        // Wait a few frames for resize to settle.
        if pending.wait_frames > 0 {
            self.pending_workspace_apply = Some(PendingWorkspaceApply {
                wait_frames: pending.wait_frames - 1,
                ..pending
            });
            return false;
        }

        // If we have a target inner size, wait until we're close enough or time out.
        if let (Some(target), Some(cur)) = (pending.target_inner_size, current_inner_size) {
            let dx = (target[0] - cur[0]).abs();
            let dy = (target[1] - cur[1]).abs();
            if dx > 2.0 || dy > 2.0 {
                // Keep trying; request repaints happens elsewhere.
                return false;
            }
        }

        // Apply
        self.apply_preset_kind(pending.preset, current_canvas_size);

        self.pending_workspace_apply = None;
        true
    }

    fn apply_preset_kind(&mut self, kind: PresetKind, current_canvas_size: [f32; 2]) {
        match kind {
            PresetKind::LayoutOnly(layout_snap) => {
                let mut layout = layout_snap.layout;
                layout.rescale_from(layout_snap.canvas_size, current_canvas_size);
                layout.merge_with_defaults();
                self.layout = layout;

                self.rebuild_terminals_from_layout();
                self.rebuild_context_exporters_from_layout();

                self.layout_epoch = self.layout_epoch.wrapping_add(1);
            }

            PresetKind::FullState(state_snap) => {
                self.inputs.repo = state_snap.repo;

                // Prevent loading WORKTREE as the ref; keep old behavior.
                self.inputs.git_ref = if state_snap.git_ref == WORKTREE_REF {
                    "HEAD".to_string()
                } else {
                    state_snap.git_ref
                };

                self.inputs.exclude_regex = state_snap.exclude_regex;
                self.inputs.max_exts = state_snap.max_exts;

                self.ui.filter_text = state_snap.filter_text;
                self.ui.show_top_level_stats = state_snap.show_top_level_stats;

                let mut layout = state_snap.layout;
                layout.rescale_from(state_snap.canvas_size, current_canvas_size);
                layout.merge_with_defaults();
                self.layout = layout;

                // Ephemeral
                self.rebuild_terminals_from_layout();
                self.rebuild_context_exporters_from_layout();

                // Restore file viewer instances (selection state only)
                self.file_viewers.clear();
                for (id, snap) in state_snap.file_viewers.iter() {
                    let mut fv = crate::app::state::FileViewerState::new();
                    fv.selected_file = snap.selected_file.clone();
                    fv.selected_commit = snap.selected_commit.clone();
                    self.file_viewers.insert(*id, fv);
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

                // Recompute refs + rerun analysis + refresh viewers
                self.refresh_git_refs();
                self.results.result = None;
                self.results.error = None;

                if self.inputs.repo.is_some() {
                    self.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
                    self.run_analysis();

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
    }

    // ---------------------------
    // Restored helpers (other code expects these to exist)
    // ---------------------------

    pub(crate) fn rebuild_context_exporters_from_layout(&mut self) {
        self.context_exporters.clear();

        let ids: Vec<ComponentId> = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::ContextExporter)
            .map(|c| c.id)
            .collect();

        for id in ids {
            self.context_exporters.insert(
                id,
                ContextExporterState {
                    save_path: None,
                    max_bytes_per_file: 200_000,
                    skip_binary: true,
                    mode: ContextExportMode::EntireRepo,
                    status: None,
                },
            );
        }
    }

    pub(crate) fn set_context_selection_all(&mut self, res: &crate::model::AnalysisResult) {
        let mut files = Vec::new();
        Self::collect_all_files(&res.root, &mut files);
        self.tree.context_selected_files = files.into_iter().collect();
    }

    fn collect_all_files(node: &crate::model::DirNode, out: &mut Vec<String>) {
        for f in &node.files {
            out.push(f.full_path.clone());
        }
        for c in &node.children {
            Self::collect_all_files(c, out);
        }
    }
}
