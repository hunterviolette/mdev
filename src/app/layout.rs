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
    ///
    /// `#[serde(default)]` keeps older workspace files loadable.
    #[serde(default)]
    pub canvas_bg_tint: Option<[u8; 4]>,

    /// Layout
    pub layout: LayoutConfig,

    /// File viewers (per instance)
    pub file_viewers: HashMap<ComponentId, FileViewerSnapshot>,
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

    /// Canvas-local egui points: position of the WINDOW (outer top-left)
    pub pos: [f32; 2],

    /// IMPORTANT: this is the window **content size** (what egui::Window::default_size expects)
    pub size: [f32; 2],
}

impl Default for WindowLayout {
    fn default() -> Self {
        Self {
            open: true,
            locked: false,
            pos: [40.0, 40.0],
            size: [500.0, 500.0],
        }
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
                pos: [10.0, 20.0],
                size: [360.0, 700.0],
            },
        );
        windows.insert(
            fv_id,
            WindowLayout {
                open: true,
                locked: false,
                pos: [380.0, 20.0],
                size: [760.0, 700.0],
            },
        );
        windows.insert(
            sum_id,
            WindowLayout {
                open: true,
                locked: false,
                pos: [1150.0, 20.0],
                size: [420.0, 700.0],
            },
        );

        Self { components, windows }
    }
}

impl LayoutConfig {
    pub fn get_window(&self, id: ComponentId) -> Option<&WindowLayout> {
        self.windows.get(&id)
    }
    pub fn get_window_mut(&mut self, id: ComponentId) -> Option<&mut WindowLayout> {
        self.windows.get_mut(&id)
    }

    pub fn merge_with_defaults(&mut self) {
        let d = LayoutConfig::default();

        // Ensure default components exist (Tree + one FV + Summary)
        for dc in d.components {
            if !self.components.iter().any(|c| c.id == dc.id) {
                self.components.push(dc);
            }
        }

        // Ensure window layouts exist
        for (id, wl) in d.windows {
            self.windows.entry(id).or_insert(wl);
        }
    }

    pub fn next_free_id(&self) -> ComponentId {
        self.components.iter().map(|c| c.id).max().unwrap_or(0) + 1
    }

    pub fn rescale_from(&mut self, saved_canvas: [f32; 2], current_canvas: [f32; 2]) {
        let saved_w = saved_canvas[0].max(1.0);
        let saved_h = saved_canvas[1].max(1.0);

        let cur_w = current_canvas[0].max(1.0);
        let cur_h = current_canvas[1].max(1.0);

        // Avoid micro-rescale due to DPI rounding
        let same = (saved_w - cur_w).abs() < 2.0 && (saved_h - cur_h).abs() < 2.0;
        if same {
            return;
        }

        let sx = cur_w / saved_w;
        let sy = cur_h / saved_h;

        for w in self.windows.values_mut() {
            w.pos[0] *= sx;
            w.pos[1] *= sy;

            w.size[0] *= sx;
            w.size[1] *= sy;

            w.pos[0] = w.pos[0].clamp(0.0, (cur_w - 50.0).max(0.0));
            w.pos[1] = w.pos[1].clamp(0.0, (cur_h - 50.0).max(0.0));

            w.size[0] = w.size[0].clamp(150.0, cur_w);
            w.size[1] = w.size[1].clamp(120.0, cur_h);
        }
    }
}
