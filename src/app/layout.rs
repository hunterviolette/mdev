use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::actions::{ComponentId, ComponentKind, ConversationId};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceFile {
    pub version: u32,
    pub default_preset: Option<String>,
    pub presets: Vec<Preset>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub kind: PresetKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum PresetKind {
    LayoutOnly(LayoutSnapshot),
    FullState(StateSnapshot),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayoutSnapshot {
    /// IMPORTANT: layout coordinates are canvas-local
    pub canvas_size: [f32; 2],
    pub layout: LayoutConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileViewerSnapshot {
    pub selected_file: Option<String>,
    pub selected_commit: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextExporterSnapshot {
    pub mode: crate::app::state::ContextExportMode,
}

// ---------------------------
// New: persisted ExecuteLoop + Task snapshots
// ---------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecuteLoopSnapshot {
    pub model: String,
    pub instruction: String,
    pub mode: crate::app::state::ExecuteLoopMode,
    pub include_context_next: bool,
    pub auto_fill_first_changeset_applier: bool,
    pub messages: Vec<crate::app::state::ExecuteLoopMessage>,

    /// OpenAI Conversations API id (conv_...). Stored so loops can resume without resending history.
    #[serde(default)]
    pub conversation_id: Option<String>,

    #[serde(default)]
    pub paused: bool,

    #[serde(default)]
    pub created_at_ms: u64,

    #[serde(default)]
    pub updated_at_ms: u64,

    // Persisted ExecuteLoop UI toggles/inputs (backward compatible)
    #[serde(default)]
    pub changeset_auto: bool,

    #[serde(default)]
    pub postprocess_cmd: String,

    // Best-effort stats (can be filled later; keep defaults for backward compat)
    #[serde(default)]
    pub changesets_total: u32,

    #[serde(default)]
    pub changesets_ok: u32,

    #[serde(default)]
    pub changesets_err: u32,

    #[serde(default)]
    pub postprocess_ok: u32,

    #[serde(default)]
    pub postprocess_err: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskSnapshot {
    #[serde(default)]
    pub bound_execute_loop: Option<ComponentId>,
    #[serde(default)]
    pub paused: bool,

    #[serde(default)]
    pub execute_loop_ids: Vec<ComponentId>,

    #[serde(default)]
    pub created_at_ms: u64,

    #[serde(default)]
    pub updated_at_ms: u64,

    #[serde(default)]
    pub conversations: HashMap<ConversationId, ExecuteLoopSnapshot>,

    #[serde(default)]
    pub active_conversation: Option<ConversationId>,
    
    #[serde(default)]
    pub next_conversation_id: ConversationId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CanvasSnapshot {
    pub name: String,
    pub layout: LayoutConfig,
    pub active_file_viewer: Option<ComponentId>,
    #[serde(default)]
    pub active_diff_viewer: Option<ComponentId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateSnapshot {
    /// Context exporters (per instance)
    /// Stored so exporter mode (EntireRepo vs TreeSelect) persists across workspace save/load.
    #[serde(default)]
    pub context_exporters: HashMap<ComponentId, ContextExporterSnapshot>,

    /// Execute loops (per instance)
    #[serde(default)]
    pub execute_loops: HashMap<ComponentId, ExecuteLoopSnapshot>,

    /// Tasks (per instance)
    #[serde(default)]
    pub tasks: HashMap<ComponentId, TaskSnapshot>,

    /// IMPORTANT: layout coordinates are canvas-local
    pub canvas_size: [f32; 2],

    /// Best-effort restore to same monitor/placement
    pub viewport_outer_pos: Option<[f32; 2]>,
    pub viewport_inner_size: Option<[f32; 2]>,

    /// Inputs
    pub repo: Option<PathBuf>,
    pub git_ref: String,
    pub exclude_regex: Vec<String>,
    pub max_exts: usize,

    /// UI prefs
    pub filter_text: String,
    pub show_top_level_stats: bool,

    /// UI prefs: optional canvas background tint.
    #[serde(default)]
    pub canvas_bg_tint: Option<[u8; 4]>,

    /// Theme prefs (persisted with workspace)
    #[serde(default)]
    pub theme_dark: bool,

    /// Syntect theme name for code highlighting (egui_extras).
    #[serde(default)]
    pub theme_syntect: String,

    #[serde(default)]
    pub canvases: Vec<CanvasSnapshot>,

    #[serde(default)]
    pub active_canvas: usize,

    #[serde(default)]
    pub next_component_id: ComponentId,

    /// Legacy single-canvas layout (kept for backward compatibility)
    #[serde(default)]
    pub layout: LayoutConfig,

    /// File viewers (per instance)
    #[serde(default)]
    pub file_viewers: HashMap<ComponentId, FileViewerSnapshot>,

    #[serde(default)]
    pub active_file_viewer: Option<ComponentId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentInstance {
    pub id: ComponentId,
    pub kind: ComponentKind,
    pub title: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowLayout {
    pub open: bool,
    pub locked: bool,

    #[serde(default)]
    pub pos_norm: Option<[f32; 2]>,

    #[serde(default)]
    pub size_norm: Option<[f32; 2]>,

    #[serde(default)]
    pub pos: [f32; 2],

    #[serde(default)]
    pub size: [f32; 2],
}

impl Default for WindowLayout {
    fn default() -> Self {
        Self {
            open: true,
            locked: false,
            pos_norm: None,
            size_norm: None,
            pos: [40.0, 40.0],
            size: [500.0, 500.0],
        }
    }
}

impl WindowLayout {
    fn norm_div(v: [f32; 2], canvas: [f32; 2]) -> [f32; 2] {
        let cw = canvas[0].max(1.0);
        let ch = canvas[1].max(1.0);
        [v[0] / cw, v[1] / ch]
    }

    fn norm_mul(v: [f32; 2], canvas: [f32; 2]) -> [f32; 2] {
        let cw = canvas[0].max(1.0);
        let ch = canvas[1].max(1.0);
        [v[0] * cw, v[1] * ch]
    }

    /// Ensure normalized fields exist, using legacy absolute fields as migration input.
    pub fn ensure_normalized_from_legacy(&mut self, legacy_canvas: [f32; 2]) {
        if self.pos_norm.is_none() {
            self.pos_norm = Some(Self::norm_div(self.pos, legacy_canvas));
        }
        if self.size_norm.is_none() {
            self.size_norm = Some(Self::norm_div(self.size, legacy_canvas));
        }
    }

    /// Get window position/size in pixels for the *current* canvas size.
    pub fn denormalized_px(&self, current_canvas: [f32; 2]) -> ([f32; 2], [f32; 2]) {
        match (self.pos_norm, self.size_norm) {
            (Some(pn), Some(sn)) => (Self::norm_mul(pn, current_canvas), Self::norm_mul(sn, current_canvas)),
            _ => (self.pos, self.size),
        }
    }

    /// Update normalized fields from pixel position/size (user move/resize), for the *current* canvas size.
    pub fn set_from_px(&mut self, pos_px: [f32; 2], size_px: [f32; 2], current_canvas: [f32; 2]) {
        self.pos_norm = Some(Self::norm_div(pos_px, current_canvas));
        self.size_norm = Some(Self::norm_div(size_px, current_canvas));

        // Keep legacy fields in sync (helps with debugging; also keeps older code paths sane).
        self.pos = pos_px;
        self.size = size_px;
    }
}


#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayoutConfig {
    pub components: Vec<ComponentInstance>,
    pub windows: HashMap<ComponentId, WindowLayout>,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        // Stable default IDs
        let tree_id: ComponentId = 1;
        let fv_id: ComponentId = 2;
        let sum_id: ComponentId = 3;

        let components = vec![
            ComponentInstance {
                id: tree_id,
                kind: ComponentKind::Tree,
                title: "Tree".to_string(),
            },
            ComponentInstance {
                id: fv_id,
                kind: ComponentKind::FileViewer,
                title: "File Viewer".to_string(),
            },
            ComponentInstance {
                id: sum_id,
                kind: ComponentKind::Summary,
                title: "Summary".to_string(),
            },
        ];

        let mut windows = HashMap::new();
        windows.insert(
            tree_id,
            WindowLayout {
                open: true,
                locked: false,
                pos_norm: None,
                size_norm: None,
                pos: [10.0, 20.0],
                size: [360.0, 700.0],
            },
        );
        windows.insert(
            fv_id,
            WindowLayout {
                open: true,
                locked: false,
                pos_norm: None,
                size_norm: None,
                pos: [380.0, 20.0],
                size: [760.0, 700.0],
            },
        );
        windows.insert(
            sum_id,
            WindowLayout {
                open: true,
                locked: false,
                pos_norm: None,
                size_norm: None,
                pos: [1150.0, 20.0],
                size: [420.0, 700.0],
            },
        );

        Self { components, windows }
    }
}

impl LayoutConfig {
    pub fn ensure_window_layouts(&mut self) {
        for c in self.components.iter() {
            if self.windows.contains_key(&c.id) {
                continue;
            }

            let (pos, size) = match c.kind {
                ComponentKind::Tree => ([24.0, 24.0], [340.0, 760.0]),
                ComponentKind::Summary => ([380.0, 24.0], [520.0, 360.0]),
                ComponentKind::FileViewer => ([380.0, 400.0], [900.0, 720.0]),
                ComponentKind::DiffViewer => ([380.0, 400.0], [900.0, 720.0]),
                _ => ([60.0, 60.0], [760.0, 700.0]),
            };

            self.windows.insert(
                c.id,
                WindowLayout {
                    open: true,
                    locked: false,
                    pos_norm: None,
                    size_norm: None,
                    pos,
                    size,
                },
            );
        }
    }

    pub fn migrate_legacy_abs_to_normalized(&mut self, legacy_canvas: [f32; 2]) {
        for w in self.windows.values_mut() {
            w.ensure_normalized_from_legacy(legacy_canvas);
        }
    }


    pub fn clamp_to_canvas_and_renormalize(&mut self, current_canvas: [f32; 2]) {
        let cw = current_canvas[0].max(1.0);
        let ch = current_canvas[1].max(1.0);

        for w in self.windows.values_mut() {
            // Ensure we have normalized fields first.
            if w.pos_norm.is_none() || w.size_norm.is_none() {
                w.ensure_normalized_from_legacy(current_canvas);
            }

            let (mut pos_px, mut size_px) = w.denormalized_px(current_canvas);

            let min_px = 1.0;
            size_px[0] = size_px[0].clamp(min_px, cw);
            size_px[1] = size_px[1].clamp(min_px, ch);

            // Clamp position so the window stays within the canvas, but don't force an extra margin.
            pos_px[0] = pos_px[0].clamp(0.0, (cw - min_px).max(0.0));
            pos_px[1] = pos_px[1].clamp(0.0, (ch - min_px).max(0.0));

            w.set_from_px(pos_px, size_px, current_canvas);
        }
    }

    pub fn get_window(&self, id: ComponentId) -> Option<&WindowLayout> {
        self.windows.get(&id)
    }
    pub fn get_window_mut(&mut self, id: ComponentId) -> Option<&mut WindowLayout> {
        self.windows.get_mut(&id)
    }

    pub fn merge_with_defaults(&mut self) {

        let mut ensure = |kind: ComponentKind, title: &str| {
            if !self.components.iter().any(|c| c.kind == kind) {
                let id = self.next_free_id();
                self.components.push(ComponentInstance {
                    id,
                    kind,
                    title: title.to_string(),
                });
            }
        };

        // Ensure default components exist by KIND
        ensure(ComponentKind::Tree, "Tree");
        ensure(ComponentKind::FileViewer, "File Viewer");
        ensure(ComponentKind::Summary, "Summary");

        // Ensure every component has a window layout (including newly added defaults)
        self.ensure_window_layouts();
    }

    pub fn next_free_id(&self) -> ComponentId {

        let max_component_id = self.components.iter().map(|c| c.id).max().unwrap_or(0);
        let max_window_id = self.windows.keys().copied().max().unwrap_or(0);
        max_component_id.max(max_window_id) + 1
    }

    pub fn rescale_from(&mut self, saved_canvas: [f32; 2], current_canvas: [f32; 2]) {
        self.migrate_legacy_abs_to_normalized(saved_canvas);
        self.clamp_to_canvas_and_renormalize(current_canvas);
    }
}
