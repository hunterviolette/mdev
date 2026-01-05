use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use egui_extras::syntax_highlighting::CodeTheme;

use crate::model::{AnalysisResult, CommitEntry};

use super::actions::{ComponentId, ExpandCmd, TerminalShell};
use super::layout::{LayoutConfig, PresetKind};

/// Special ref name used to indicate "show the working tree (uncommitted) version".
pub const WORKTREE_REF: &str = "WORKTREE";

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

// Context exporter
pub struct ContextExporterState {
    pub save_path: Option<PathBuf>,
    pub max_bytes_per_file: usize,
    pub skip_binary: bool,
    pub mode: ContextExportMode,
    pub status: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextExportMode {
    EntireRepo,
    TreeSelect,
}

/// Per-file-viewer "where am I viewing this file from?"
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileViewAt {
    /// Use the global top-bar selection (inputs.git_ref).
    FollowTopBar,
    /// Always show the local working tree version.
    WorkingTree,
    /// Show a specific commit (hash stored in `selected_commit`).
    Commit,
}

// Pull the editor state type from UI module
pub use crate::app::ui::code_editor::CodeEditorState;

pub struct AppState {
    pub inputs: InputsState,
    pub results: ResultsState,
    pub ui: UiState,
    pub tree: TreeState,

    // Multi viewer instances
    pub file_viewers: HashMap<ComponentId, FileViewerState>,
    pub active_file_viewer: Option<ComponentId>,

    // Terminal instances (ephemeral)
    pub terminals: HashMap<ComponentId, TerminalState>,

    // Context exporters (ephemeral)
    pub context_exporters: HashMap<ComponentId, ContextExporterState>,

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
    /// ACTIVE repo used by analysis/file viewer/export/terminal
    pub repo: Option<PathBuf>,

    /// Last-picked local repo path (for display / future convenience)
    pub local_repo: Option<PathBuf>,

    /// Currently selected ref (used by analysis + git-show reads)
    /// NOTE: may be WORKTREE_REF
    pub git_ref: String,

    /// Cached dropdown options for git_ref (remote + local + HEAD + WORKTREE)
    pub git_ref_options: Vec<String>,

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
    pub context_selected_files: HashSet<String>,
}

pub struct FileViewerState {
    pub selected_file: Option<String>,

    // commit selection (only meaningful when view_at == Commit)
    pub selected_commit: Option<String>,

    // viewer view mode
    pub view_at: FileViewAt,

    pub file_commits: Vec<CommitEntry>,
    pub file_content: String,
    pub file_content_err: Option<String>,

    // Editing working tree
    pub edit_working_tree: bool,
    pub edit_buffer: String,
    pub edit_status: Option<String>,

    // NEW: real editor state (cursor/selection/cache)
    pub editor: CodeEditorState,

    // Diff fields
    pub show_diff: bool,
    pub diff_base: Option<String>,
    pub diff_target: Option<String>,
    pub diff_text: String,
    pub diff_err: Option<String>,
}

impl FileViewerState {
    pub fn new() -> Self {
        Self {
            selected_file: None,
            selected_commit: None,
            view_at: FileViewAt::FollowTopBar,

            file_commits: vec![],
            file_content: String::new(),
            file_content_err: None,

            edit_working_tree: false,
            edit_buffer: String::new(),
            edit_status: None,

            editor: CodeEditorState::default(),

            show_diff: false,
            diff_base: None,
            diff_target: None,
            diff_text: String::new(),
            diff_err: None,
        }
    }
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

        let mut file_viewers = HashMap::new();
        file_viewers.insert(2, FileViewerState::new());

        Self {
            inputs: InputsState {
                repo: None,
                local_repo: None,

                // HEAD is default
                git_ref: "HEAD".to_string(),
                // ensure HEAD first, WORKTREE second
                git_ref_options: vec!["HEAD".to_string(), WORKTREE_REF.to_string()],

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
            tree: TreeState {
                expand_cmd: None,
                context_selected_files: HashSet::new(),
            },

            file_viewers,
            active_file_viewer: Some(2),

            terminals: HashMap::new(),
            context_exporters: HashMap::new(),

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

impl AppState {
    /// Set the global ref and immediately refresh any file viewers that follow the top bar.
    pub fn set_git_ref(&mut self, git_ref: String) {
        self.inputs.git_ref = git_ref;
        self.refresh_follow_top_bar_viewers();
    }

    /// Replace ref dropdown options, enforcing:
    /// - HEAD is ALWAYS first
    /// - WORKTREE is ALWAYS second
    pub fn set_git_ref_options(&mut self, mut refs: Vec<String>) {
        // normalize
        refs.retain(|r| !r.trim().is_empty());
        refs.retain(|r| r != WORKTREE_REF);
        refs.retain(|r| r != "HEAD");

        // de-dupe while preserving rough order
        let mut seen = std::collections::HashSet::new();
        refs.retain(|r| seen.insert(r.clone()));

        // HEAD first, WORKTREE second, then the rest
        let mut out = Vec::with_capacity(refs.len() + 2);
        out.push("HEAD".to_string());
        out.push(WORKTREE_REF.to_string());
        out.extend(refs);

        self.inputs.git_ref_options = out;

        // If current selection vanished, fall back safely to HEAD.
        let cur = self.inputs.git_ref.clone();
        if !self.inputs.git_ref_options.iter().any(|r| r == &cur) {
            self.set_git_ref("HEAD".to_string());
        }
    }

    /// Reload all viewers in FollowTopBar mode that have a selected file.
    pub fn refresh_follow_top_bar_viewers(&mut self) {
        let ids: Vec<ComponentId> = self.file_viewers.keys().cloned().collect();
        for id in ids {
            let should = self
                .file_viewers
                .get(&id)
                .map(|v| v.view_at == FileViewAt::FollowTopBar && v.selected_file.is_some())
                .unwrap_or(false);

            if should {
                // NOTE: This method is implemented in file_viewer_controller.rs (single source of truth).
                self.load_file_at_current_selection(id);
            }
        }
    }
}
