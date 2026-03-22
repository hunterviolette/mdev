use std::collections::HashMap;
use std::path::PathBuf;

use crate::app::actions::{Action, ComponentId, ComponentKind, ExpandCmd, TaskId};
use crate::app::layout::{
    ContextExporterSnapshot, ExecuteLoopSnapshot, FileViewerSnapshot, LayoutSnapshot, Preset,
    PresetKind, StateSnapshot, TaskSnapshot, WorkspaceFile,
};
use crate::app::state::{AppState, PendingWorkspaceApply, ViewportRestore, WORKTREE_REF};

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::SaveWorkspace {
            canvas_size,
            viewport_outer_pos,
            viewport_inner_size,
            pixels_per_point,
        } => {
            let name = state.palette_last_name.take();
            state.current_workspace_name = name.clone().unwrap_or_else(|| "workspace".to_string());
            state.save_workspace_to_appdata(
                *canvas_size,
                *viewport_outer_pos,
                *viewport_inner_size,
                *pixels_per_point,
                name.as_deref(),
            );
            true
        }

        Action::LoadWorkspace => {
            let name = state.palette_last_name.take();
            state.current_workspace_name = name.clone().unwrap_or_else(|| "workspace".to_string());
            state.load_workspace_from_appdata(name.as_deref());
            true
        }

        _ => false,
    }
}

impl AppState {

    fn app_data_dir(&self) -> anyhow::Result<PathBuf> {
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

    pub fn list_workspaces(&self) -> Vec<String> {
        let dir = match self.workspaces_dir() {
            Ok(d) => d,
            Err(_) => return vec![],
        };

        let Ok(rd) = std::fs::read_dir(dir) else {
            return vec![];
        };

        let mut names = Vec::new();
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

    fn workspace_path(&self, name: &str) -> anyhow::Result<PathBuf> {
        let dir = self.workspaces_dir()?;
        let safe = Self::sanitize_workspace_name(name);
        Ok(dir.join(format!("{safe}.json")))
    }

    fn startup_layout_override_path(&self) -> anyhow::Result<PathBuf> {
        let mut dir = self.app_data_dir()?;
        std::fs::create_dir_all(&dir)?;
        dir.push("startup_layout_override.json");
        Ok(dir)
    }

    pub fn startup_layout_override_exists(&self) -> bool {
        self.startup_layout_override_path()
            .ok()
            .map(|p| p.exists())
            .unwrap_or(false)
    }

    pub fn clear_startup_layout_override_from_appdata(&mut self) {
        let path = match self.startup_layout_override_path() {
            Ok(p) => p,
            Err(e) => {
                self.results.error = Some(format!("{:#}", e));
                return;
            }
        };
        match std::fs::remove_file(&path) {
            Ok(_) => self.results.error = None,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.results.error = None,
            Err(e) => self.results.error = Some(format!("{:#}", e)),
        }
    }

    fn build_startup_layout_workspace_file(
        &mut self,
        canvas_size: [f32; 2],
        viewport_outer_pos: Option<[f32; 2]>,
        viewport_inner_size: Option<[f32; 2]>,
    ) -> WorkspaceFile {
        let mut file_viewers = HashMap::new();
        for (id, _) in self.file_viewers.iter() {
            file_viewers.insert(
                *id,
                FileViewerSnapshot {
                    selected_file: None,
                    selected_commit: None,
                },
            );
        }

        let mut context_exporters = HashMap::new();
        for (id, ex) in self.context_exporters.iter() {
            context_exporters.insert(*id, ContextExporterSnapshot {
                mode: ex.mode,
                skip_binary: ex.skip_binary,
                skip_gitignore: ex.skip_gitignore,
                include_staged_diff: ex.include_staged_diff,
                save_path: ex.save_path.as_ref().map(|p| p.display().to_string()),
                selected_paths: ex.selection_defaults.iter().cloned().collect(),
            });
        }

        {
            let cw = canvas_size[0].max(1.0);
            let ch = canvas_size[1].max(1.0);
            let force_normalize = |layout: &mut crate::app::layout::LayoutConfig| {
                for w in layout.windows.values_mut() {
                    w.pos_norm = Some([w.pos[0] / cw, w.pos[1] / ch]);
                    w.size_norm = Some([w.size[0] / cw, w.size[1] / ch]);
                }
            };
            force_normalize(self.active_layout_mut());
            for c in self.canvases.iter_mut() {
                force_normalize(&mut c.layout);
            }
        }

        let state_snap = StateSnapshot {
            canvas_size,
            viewport_outer_pos,
            viewport_inner_size,
            repo: None,
            git_ref: WORKTREE_REF.to_string(),
            exclude_regex: self.inputs.exclude_regex.clone(),
            max_exts: self.inputs.max_exts,
            filter_text: self.ui.filter_text.clone(),
            show_top_level_stats: self.ui.show_top_level_stats,
            canvas_bg_tint: self.ui.canvas_bg_tint,
            theme_dark: self.theme.prefs.dark,
            theme_syntect: self.theme.prefs.syntect_theme.clone(),
            canvases: self
                .canvases
                .iter()
                .map(|c| crate::app::layout::CanvasSnapshot {
                    name: c.name.clone(),
                    layout: c.layout.clone(),
                    active_file_viewer: c.active_file_viewer,
                    active_diff_viewer: c.active_diff_viewer,
                })
                .collect(),
            active_canvas: self.active_canvas,
            next_component_id: self.next_component_id,
            file_viewers,
            active_file_viewer: self.active_file_viewer_id(),
            context_exporters,
            execute_loops: HashMap::new(),
            task_component_bindings: self.task_component_bindings.clone(),
            tasks: HashMap::new(),
        };

        WorkspaceFile {
            version: 1,
            default_preset: Some("Startup".to_string()),
            presets: vec![Preset {
                name: "Startup".to_string(),
                kind: PresetKind::FullState(state_snap),
            }],
        }
    }

    pub fn save_startup_layout_override_to_appdata(
        &mut self,
        canvas_size: [f32; 2],
        viewport_outer_pos: Option<[f32; 2]>,
        viewport_inner_size: Option<[f32; 2]>,
        _pixels_per_point: f32,
    ) {
        let path = match self.startup_layout_override_path() {
            Ok(p) => p,
            Err(e) => {
                self.results.error = Some(format!("{:#}", e));
                return;
            }
        };

        let ws_file = self.build_startup_layout_workspace_file(canvas_size, viewport_outer_pos, viewport_inner_size);
        let text = match serde_json::to_string_pretty(&ws_file) {
            Ok(t) => t,
            Err(e) => {
                self.results.error = Some(format!("{:#}", e));
                return;
            }
        };

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&path, text) {
            self.results.error = Some(format!("{:#}", e));
            return;
        }
        self.results.error = None;
    }

    pub fn export_built_in_startup_layout_to_repo_file(
        &mut self,
        canvas_size: [f32; 2],
        viewport_outer_pos: Option<[f32; 2]>,
        viewport_inner_size: Option<[f32; 2]>,
        _pixels_per_point: f32,
    ) {
        let path = PathBuf::from("assets").join("default_startup_layout.json");
        let ws_file = self.build_startup_layout_workspace_file(canvas_size, viewport_outer_pos, viewport_inner_size);
        let text = match serde_json::to_string_pretty(&ws_file) {
            Ok(t) => t,
            Err(e) => {
                self.results.error = Some(format!("{:#}", e));
                return;
            }
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&path, text) {
            self.results.error = Some(format!("{:#}", e));
            return;
        }
        self.results.error = None;
    }

    fn load_default_workspace_file(&self) -> WorkspaceFile {
        if let Ok(path) = self.startup_layout_override_path() {
            if let Ok(bytes) = std::fs::read(&path) {
                if let Ok(parsed) = serde_json::from_slice::<WorkspaceFile>(&bytes) {
                    return parsed;
                }
            }
        }

        if let Ok(parsed) = serde_json::from_str::<WorkspaceFile>(include_str!("../../../assets/default_startup_layout.json")) {
            return parsed;
        }

        WorkspaceFile {
            version: 1,
            default_preset: Some("Default".to_string()),
            presets: vec![
                Preset {
                    name: "Default".to_string(),
                    kind: PresetKind::FullState(StateSnapshot {
                        canvases: vec![],
                        active_canvas: 0,
                        next_component_id: 0,
                        canvas_size: [1200.0, 800.0],
                        viewport_outer_pos: None,
                        viewport_inner_size: None,
                        repo: None,
                        git_ref: WORKTREE_REF.to_string(),
                        exclude_regex: vec![r"\.lock$".into(), r"(^|/)package-lock\.json$".into()],
                        max_exts: 6,
                        filter_text: "".to_string(),
                        show_top_level_stats: true,
                        canvas_bg_tint: None,
                        theme_dark: true,
                        theme_syntect: "SolarizedDark".to_string(),
                        file_viewers: HashMap::new(),
                        active_file_viewer: Some(2),
                        context_exporters: HashMap::new(),
                        execute_loops: HashMap::new(),
                        task_component_bindings: HashMap::new(),
                        tasks: HashMap::new(),
                    }),
                },
                Preset {
                    name: "Layout Only".to_string(),
                    kind: PresetKind::LayoutOnly(LayoutSnapshot {
                        canvas_size: [1200.0, 800.0],
                        layout: crate::app::layout::LayoutConfig::default(),
                    }),
                },
            ],
        }
    }

    pub fn save_workspace_to_appdata(
        &mut self,
        canvas_size: [f32; 2],
        viewport_outer_pos: Option<[f32; 2]>,
        viewport_inner_size: Option<[f32; 2]>,
        _pixels_per_point: f32,
        name_opt: Option<&str>,
    ) {
        let name = name_opt.unwrap_or("workspace");

        let path = match self.workspace_path(name) {
            Ok(p) => p,
            Err(e) => {
                self.results.error = Some(format!("{:#}", e));
                return;
            }
        };

        let mut file_viewers = HashMap::new();
        for (id, fv) in self.file_viewers.iter() {
            file_viewers.insert(
                *id,
                FileViewerSnapshot {
                    selected_file: fv.selected_file.clone(),
                    selected_commit: fv.selected_commit.clone(),
                },
            );
        }

        let _canvases: Vec<crate::app::layout::CanvasSnapshot> = self
            .canvases
            .iter()
            .map(|c| crate::app::layout::CanvasSnapshot {
                name: c.name.clone(),
                layout: c.layout.clone(),
                active_file_viewer: c.active_file_viewer,
                active_diff_viewer: c.active_diff_viewer,
            })
            .collect();

        let mut context_exporters = HashMap::new();
        for (id, ex) in self.context_exporters.iter() {
            context_exporters.insert(
                *id,
                ContextExporterSnapshot {
                    mode: ex.mode,
                    skip_binary: ex.skip_binary,
                    skip_gitignore: ex.skip_gitignore,
                    include_staged_diff: ex.include_staged_diff,
                    save_path: ex.save_path.as_ref().map(|p| p.display().to_string()),
                    selected_paths: self.tree.context_selected_files.iter().cloned().collect(),
                },
            );
        }

        if self.task_store_dirty {
            self.save_repo_task_store();
        }

        let execute_loops: HashMap<ComponentId, ExecuteLoopSnapshot> = HashMap::new();
        let tasks: HashMap<TaskId, TaskSnapshot> = HashMap::new();

        {
            let cw = canvas_size[0].max(1.0);
            let ch = canvas_size[1].max(1.0);

            let force_normalize = |layout: &mut crate::app::layout::LayoutConfig| {
                for w in layout.windows.values_mut() {
                    w.pos_norm = Some([w.pos[0] / cw, w.pos[1] / ch]);
                    w.size_norm = Some([w.size[0] / cw, w.size[1] / ch]);
                }
            };

            force_normalize(self.active_layout_mut());

            for c in self.canvases.iter_mut() {
                force_normalize(&mut c.layout);
            }
        }

        let state_snap = StateSnapshot {
            canvas_size,
            viewport_outer_pos,
            viewport_inner_size,

            repo: self.inputs.repo.clone(),
            git_ref: self.inputs.git_ref.clone(),
            exclude_regex: self.inputs.exclude_regex.clone(),
            max_exts: self.inputs.max_exts,

            filter_text: self.ui.filter_text.clone(),
            show_top_level_stats: self.ui.show_top_level_stats,
            canvas_bg_tint: self.ui.canvas_bg_tint,

            theme_dark: self.theme.prefs.dark,
            theme_syntect: self.theme.prefs.syntect_theme.clone(),

            canvases: self
                .canvases
                .iter()
                .map(|c| crate::app::layout::CanvasSnapshot {
                    name: c.name.clone(),
                    layout: c.layout.clone(),
                    active_file_viewer: c.active_file_viewer,
                    active_diff_viewer: c.active_diff_viewer,
                })
                .collect(),
            active_canvas: self.active_canvas,
            next_component_id: self.next_component_id,

            file_viewers,
            active_file_viewer: self.active_file_viewer_id(),

            context_exporters,
            execute_loops,
            task_component_bindings: self.task_component_bindings.clone(),
            tasks,
        };

        let ws_file = WorkspaceFile {
            version: 1,
            default_preset: Some(name.to_string()),
            presets: vec![Preset {
                name: name.to_string(),
                kind: PresetKind::FullState(state_snap),
            }],
        };

        let text = match serde_json::to_string_pretty(&ws_file) {
            Ok(t) => t,
            Err(e) => {
                self.results.error = Some(format!("{:#}", e));
                return;
            }
        };

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        if let Err(e) = std::fs::write(&path, text) {
            self.results.error = Some(format!("{:#}", e));
            return;
        }
    }

    pub fn load_workspace_from_appdata(&mut self, name_opt: Option<&str>) {
        let name = name_opt.unwrap_or("workspace");
        if name_opt.is_none() || name == "workspace" {
            let def = self.load_default_workspace_file();
            self.apply_workspace_preset(&def, Some("workspace"));
            return;
        }
        if name_opt.is_none() || name == "workspace" {
            let def = self.load_default_workspace_file();
            self.apply_workspace_preset(&def, Some("workspace"));
            return;
        }
        let path = match self.workspace_path(name) {
            Ok(p) => p,
            Err(e) => {
                self.results.error = Some(format!("{:#}", e));
                return;
            }
        };

        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => {
                let def = self.load_default_workspace_file();
                let text = serde_json::to_string_pretty(&def).unwrap_or_default();
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&path, text);
                self.apply_workspace_preset(&def, Some(name));
                return;
            }
        };

        let parsed: WorkspaceFile = match serde_json::from_slice(&bytes) {
            Ok(p) => p,
            Err(e) => {
                self.results.error = Some(format!("Invalid workspace JSON: {e}"));
                return;
            }
        };

        self.apply_workspace_preset(&parsed, Some(name));
    }

    fn apply_workspace_preset(&mut self, ws: &WorkspaceFile, name: Option<&str>) {
        let preset_name = ws
            .default_preset
            .clone()
            .or_else(|| name.map(|s| s.to_string()))
            .unwrap_or_else(|| "Default".to_string());

        let Some(preset) = ws.presets.iter().find(|p| p.name == preset_name) else {
            self.results.error = Some(format!("Preset not found: {preset_name}"));
            return;
        };

        let mut target_inner_size = None;
        let mut target_canvas_size = None;

        match &preset.kind {
            PresetKind::FullState(st) => {
                self.pending_viewport_restore = Some(ViewportRestore {
                    outer_pos: st.viewport_outer_pos,
                    inner_size: st.viewport_inner_size,
                });
                target_inner_size = st.viewport_inner_size;
                target_canvas_size = Some(st.canvas_size);
            }
            PresetKind::LayoutOnly(ls) => {
                target_canvas_size = Some(ls.canvas_size);
            }
        }

        self.pending_workspace_apply = Some(PendingWorkspaceApply {
            preset: preset.kind.clone(),
            target_inner_size,
            target_canvas_size,
            wait_frames: 10,
            timeout_frames: 120,
        });

        self.results.error = None;
    }


    pub fn try_apply_pending_workspace(
        &mut self,
        current_canvas_size: [f32; 2],
        current_inner_size: Option<[f32; 2]>,
        pixels_per_point: f32,
    ) -> bool {
        let Some(mut pending) = self.pending_workspace_apply.clone() else {
            return false;
        };

        if pending.wait_frames > 0 {
            pending.wait_frames = pending.wait_frames.saturating_sub(1);
            self.pending_workspace_apply = Some(pending);
            return false;
        }

        let mut should_wait = false;

        if let (Some(target), Some(cur)) = (pending.target_inner_size, current_inner_size) {
            let dx = (target[0] - cur[0]).abs();
            let dy = (target[1] - cur[1]).abs();
            if dx > 2.0 || dy > 2.0 {
                should_wait = true;
            }
        }

        if let Some(target) = pending.target_canvas_size {
            let dx = (target[0] - current_canvas_size[0]).abs();
            let dy = (target[1] - current_canvas_size[1]).abs();
            if dx > 0.25 || dy > 0.25 {
                should_wait = true;
            }
        }

        if should_wait {
            if pending.timeout_frames > 0 {
                pending.timeout_frames = pending.timeout_frames.saturating_sub(1);
                self.pending_workspace_apply = Some(pending);
                return false;
            }
        }

        let saved_canvas_size: Option<[f32; 2]> = match &pending.preset {
            PresetKind::LayoutOnly(ls) => Some(ls.canvas_size),
            PresetKind::FullState(st) => Some(st.canvas_size),
        };

        tracing::info!(
            target: "workspace_geom",
            event = "apply",
            current_canvas_w = current_canvas_size[0],
            current_canvas_h = current_canvas_size[1],
            saved_canvas_w = saved_canvas_size.map(|s| s[0]),
            saved_canvas_h = saved_canvas_size.map(|s| s[1]),
            current_inner_w = current_inner_size.map(|s| s[0]),
            current_inner_h = current_inner_size.map(|s| s[1]),
            target_inner_w = pending.target_inner_size.map(|s| s[0]),
            target_inner_h = pending.target_inner_size.map(|s| s[1]),
            pixels_per_point = pixels_per_point,
        );

        self.apply_preset_kind(pending.preset, current_canvas_size);

        self.pending_workspace_apply = None;
        true
    }

    fn log_layout_windows(prefix: &str, layout: &crate::app::layout::LayoutConfig) {
        tracing::info!(target: "workspace_geom", event = "layout_dump", phase = prefix, window_count = layout.windows.len());
        for (id, w) in layout.windows.iter() {
            tracing::info!(
                target: "workspace_geom",
                event = "layout_window",
                phase = prefix,
                id = *id,
                pos_x = w.pos[0],
                pos_y = w.pos[1],
                size_w = w.size[0],
                size_h = w.size[1],
                pos_norm = ?w.pos_norm,
                size_norm = ?w.size_norm
            );
        }
    }

    fn apply_preset_kind(&mut self, kind: PresetKind, current_canvas_size: [f32; 2]) {
        match kind {
            PresetKind::LayoutOnly(layout_snap) => {
                tracing::info!(target: "workspace_geom", event = "apply_path", kind = "layout_only", saved_canvas_w = layout_snap.canvas_size[0], saved_canvas_h = layout_snap.canvas_size[1], current_canvas_w = current_canvas_size[0], current_canvas_h = current_canvas_size[1]);

                let mut layout = layout_snap.layout;

                Self::log_layout_windows("layout_only/before_migrate", &layout);

                layout.clamp_to_canvas_and_renormalize(current_canvas_size);
                Self::log_layout_windows("layout_only/after_clamp", &layout);


                layout.ensure_window_layouts();
                Self::log_layout_windows("layout_only/after_ensure", &layout);

                {
                    let canvas = self.active_canvas_state_mut();
                    canvas.layout = layout;
                    canvas.layout_epoch = canvas.layout_epoch.wrapping_add(1);
                }

                self.rebuild_terminals_from_layout();
                self.rebuild_context_exporters_from_layout();
                self.rebuild_tasks_from_layout();
                self.rebuild_source_controls_from_layout();
                self.rebuild_diff_viewers_from_layout();
                self.rebuild_execute_loops_from_layout();
                self.rebuild_tasks_from_layout();

                let _ = self.load_repo_task_store();

                self.layout_epoch = self.layout_epoch.wrapping_add(1);
            }

            PresetKind::FullState(state_snap) => {
                tracing::info!(target: "workspace_geom", event = "apply_path", kind = "full_state", saved_canvas_w = state_snap.canvas_size[0], saved_canvas_h = state_snap.canvas_size[1], current_canvas_w = current_canvas_size[0], current_canvas_h = current_canvas_size[1]);

                self.inputs.repo = state_snap.repo;
                self.inputs.git_ref = state_snap.git_ref;

                self.inputs.exclude_regex = state_snap.exclude_regex;
                self.inputs.max_exts = state_snap.max_exts;

                self.ui.filter_text = state_snap.filter_text;
                self.ui.show_top_level_stats = state_snap.show_top_level_stats;
                self.ui.canvas_bg_tint = state_snap.canvas_bg_tint;
                self.ui.canvas_tint_popup_open = false;

                self.theme.prefs.dark = state_snap.theme_dark;
                if state_snap.theme_syntect.trim().is_empty() {
                    self.theme.prefs.syntect_theme = if self.theme.prefs.dark {
                        "SolarizedDark".to_string()
                    } else {
                        "SolarizedLight".to_string()
                    };
                } else {
                    self.theme.prefs.syntect_theme = state_snap.theme_syntect;
                }

                self.task_component_bindings = state_snap.task_component_bindings.clone();

                let provisional_next_component_id = if state_snap.next_component_id == 0 {
                    self.next_component_id
                } else {
                    state_snap.next_component_id
                };

                if !state_snap.canvases.is_empty() {
                    self.canvases = state_snap
                        .canvases
                        .into_iter()
                        .map(|c| {
                            let mut layout = c.layout;
                            layout.rescale_from(state_snap.canvas_size, current_canvas_size);
                            layout.ensure_window_layouts();
                            crate::app::state::CanvasState {
                                name: c.name,
                                layout,
                                active_file_viewer: c.active_file_viewer,
                                active_diff_viewer: c.active_diff_viewer,
                                layout_epoch: 1,
                            }
                        })
                        .collect();

                    self.active_canvas = state_snap.active_canvas.min(self.canvases.len().saturating_sub(1));
                    if let Some(c) = self.canvases.get(self.active_canvas) {
                        Self::log_layout_windows("full_state/active_canvas_after_rescale_ensure", &c.layout);
                    }

                } else {
                    self.results.error = Some("Workspace preset is missing canvases (legacy single-canvas format is not supported).".into());
                    return;
                }

                let max_used_id = self
                    .all_layouts()
                    .flat_map(|l| l.components.iter().map(|c| c.id))
                    .max()
                    .unwrap_or(0);
                self.next_component_id = provisional_next_component_id.max(max_used_id + 1);


                self.rebuild_terminals_from_layout();
                self.rebuild_context_exporters_from_layout();
                self.rebuild_changeset_appliers_from_layout();
                self.rebuild_source_controls_from_layout();
                self.rebuild_diff_viewers_from_layout();
                self.rebuild_execute_loops_from_layout();
                self.rebuild_tasks_from_layout();

                for (id, snap) in state_snap.context_exporters.iter() {
                    if let Some(ex) = self.context_exporters.get_mut(id) {
                        ex.mode = snap.mode;
                        ex.skip_binary = snap.skip_binary;
                        ex.skip_gitignore = snap.skip_gitignore;
                        ex.include_staged_diff = snap.include_staged_diff;
                        ex.save_path = snap.save_path.as_ref().map(std::path::PathBuf::from);
                        ex.selection_defaults = snap.selected_paths.iter().cloned().collect();
                    }
                }

                if let Some(defaults) = state_snap
                    .context_exporters
                    .values()
                    .find_map(|snap| (!snap.selected_paths.is_empty()).then(|| snap.selected_paths.clone()))
                {
                    let selected: std::collections::HashSet<String> = defaults.into_iter().collect();
                    self.tree.context_selected_files = selected.clone();
                    let key = self.inputs.git_ref.clone();
                    self.tree.context_selected_by_ref.insert(key, selected);
                }

                let _ = self.load_repo_task_store();

                self.file_viewers.clear();
                for (id, snap) in state_snap.file_viewers.iter() {
                    let mut fv = crate::app::state::FileViewerState::new();
                    fv.selected_file = snap.selected_file.clone();
                    fv.selected_commit = snap.selected_commit.clone();
                    self.file_viewers.insert(*id, fv);
                }

                for canvas in self.canvases.iter_mut() {
                    canvas.active_file_viewer = canvas.active_file_viewer.or_else(|| {
                        canvas
                            .layout
                            .components
                            .iter()
                            .find(|c| c.kind == ComponentKind::FileViewer)
                            .map(|c| c.id)
                    });
                }

                self.layout_epoch = self.layout_epoch.wrapping_add(1);

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
                }
            }
        }
    }
}
