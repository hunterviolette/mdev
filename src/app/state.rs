use std::collections::HashMap;
use std::path::PathBuf;

use crate::model::{AnalysisResult, CommitEntry};
use egui_extras::syntax_highlighting::CodeTheme;

use super::actions::{ComponentId, ExpandCmd, TerminalShell};
use super::layout::{LayoutConfig, PresetKind};

#[derive(Clone, Debug)]
pub struct ViewportRestore {
    pub outer_pos: Option<[f32; 2]>,
    pub inner_size: Option<[f32; 2]>,
}

#[derive(Clone, Debug)]
pub struct PendingWorkspaceApply {
    pub preset: PresetKind,
    pub target_inner_size: Option<[f32; 2]>,
    pub wait_frames: u8,
}

#[derive(Clone, Debug, Default)]
pub struct CommandPaletteState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
}

pub struct TerminalState {
    pub shell: TerminalShell,
    pub cwd: Option<PathBuf>,
    pub input: String,
    pub output: String,
    pub last_status: Option<i32>,
}

pub struct AppState {
    pub inputs: InputsState,
    pub results: ResultsState,
    pub ui: UiState,
    pub tree: TreeState,

    // MULTI viewer instances
    pub file_viewers: HashMap<ComponentId, FileViewerState>,
    pub active_file_viewer: Option<ComponentId>,

    // Terminal instances (ephemeral)
    pub terminals: HashMap<ComponentId, TerminalState>,

    pub theme: ThemeState,
    pub deferred: DeferredActions,
    pub layout: LayoutConfig,

    pub layout_epoch: u64,

    pub pending_viewport_restore: Option<ViewportRestore>,
    pub pending_workspace_apply: Option<PendingWorkspaceApply>,

    // Command palette
    pub palette: CommandPaletteState,
    pub palette_last_name: Option<String>,

    // Tree click -> prompt for viewer if multiple
    pub pending_open_file_path: Option<String>,
    pub pending_open_file_viewer: Option<ComponentId>,
}

pub struct InputsState {
    pub repo: Option<PathBuf>,
    pub git_ref: String,
    pub exclude_regex: Vec<String>,
    pub max_exts: usize,
}

pub struct ResultsState {
    pub result: Option<AnalysisResult>,
    pub error: Option<String>,
}

pub struct UiState {
    pub show_top_level_stats: bool,
    pub filter_text: String,
}

pub struct TreeState {
    pub expand_cmd: Option<ExpandCmd>,
}

pub struct FileViewerState {
    pub selected_file: Option<String>,
    pub selected_commit: Option<String>,
    pub file_commits: Vec<CommitEntry>,
    pub file_content: String,
    pub file_content_err: Option<String>,

    // Diff fields
    pub show_diff: bool,
    pub diff_base: Option<String>,
    pub diff_target: Option<String>,
    pub diff_text: String,
    pub diff_err: Option<String>,
}

#[derive(Clone)]
pub struct ThemePrefs {
    pub dark: bool,
    pub syntect_theme: String,
}

pub struct ThemeState {
    pub code_theme: CodeTheme,
    pub prefs: ThemePrefs,
}

pub struct DeferredActions {
    pub open_file: Option<String>,
    pub open_file_target_viewer: Option<ComponentId>,
    pub select_commit: Option<(ComponentId, Option<String>)>,
    pub refresh_viewer: Option<ComponentId>,
}

impl Default for AppState {
    fn default() -> Self {
        let layout = LayoutConfig::default();

        // default file viewer instance is ID=2 (matches LayoutConfig::default)
        let mut file_viewers = HashMap::new();
        file_viewers.insert(
            2,
            FileViewerState {
                selected_file: None,
                selected_commit: None,
                file_commits: vec![],
                file_content: "".to_string(),
                file_content_err: None,

                show_diff: false,
                diff_base: None,
                diff_target: None,
                diff_text: "".to_string(),
                diff_err: None,
            },
        );

        Self {
            inputs: InputsState {
                repo: None,
                git_ref: "HEAD".to_string(),
                exclude_regex: vec![r"\.lock$".into(), r"(^|/)package-lock\.json$".into()],
                max_exts: 6,
            },
            results: ResultsState {
                result: None,
                error: None,
            },
            ui: UiState {
                show_top_level_stats: true,
                filter_text: "".to_string(),
            },
            tree: TreeState { expand_cmd: None },

            file_viewers,
            active_file_viewer: Some(2),

            terminals: HashMap::new(),

            theme: ThemeState {
                code_theme: CodeTheme::dark(),
                prefs: ThemePrefs {
                    dark: true,
                    syntect_theme: "SolarizedDark".to_string(),
                },
            },

            deferred: DeferredActions {
                open_file: None,
                open_file_target_viewer: None,
                select_commit: None,
                refresh_viewer: None,
            },

            layout,
            layout_epoch: 0,

            pending_viewport_restore: None,
            pending_workspace_apply: None,

            palette: CommandPaletteState::default(),
            palette_last_name: None,

            pending_open_file_path: None,
            pending_open_file_viewer: None,
        }
    }
}
