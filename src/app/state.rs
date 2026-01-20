// src/app/state.rs
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use egui_extras::syntax_highlighting::CodeTheme;

use crate::capabilities::CapabilityBroker;
use crate::model::{AnalysisResult, CommitEntry};
use crate::platform::Platform;

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

// ChangeSet applier
pub struct ChangeSetApplierState {
    pub payload: String,
    pub status: Option<String>,
}

// Source control
#[derive(Clone, Debug)]
pub struct SourceControlFile {
    pub path: String,
    pub index_status: String,
    pub worktree_status: String,
    pub staged: bool,
    pub untracked: bool,
}

#[derive(Clone, Debug)]
pub struct SourceControlState {
    pub branch: String,
    pub branch_options: Vec<String>,
    pub remote: String,
    pub remote_options: Vec<String>,

    pub commit_message: String,

    pub files: Vec<SourceControlFile>,
    pub selected: HashSet<String>,

    pub last_output: Option<String>,
    pub last_error: Option<String>,

    // internal: trigger initial refresh
    pub needs_refresh: bool,
}

// -----------------------------------------------------------------
// Diff viewer
// -----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffRowKind {
    Equal,
    Add,
    Delete,
    Change,
}

#[derive(Clone, Debug)]
pub struct DiffRow {
    pub left_no: Option<usize>,
    pub right_no: Option<usize>,
    pub left: Option<String>,
    pub right: Option<String>,
    pub kind: DiffRowKind,
}

#[derive(Clone, Debug)]
pub struct DiffViewerState {
    pub path: Option<String>,

    /// Left side (old)
    pub from_ref: String,
    /// Right side (new)
    pub to_ref: String,

    /// Full, raw side-by-side rows (entire file)
    pub rows: Vec<DiffRow>,

    /// UI: if true, show only hunks (changes + surrounding context)
    pub only_changes: bool,

    /// UI: number of surrounding context lines when `only_changes` is enabled
    pub context_lines: usize,

    pub last_error: Option<String>,

    pub needs_refresh: bool,
}

// Note: additional UI preferences for the diff viewer

impl DiffViewerState {
    pub fn new() -> Self {
        Self {
            path: None,
            from_ref: "HEAD".to_string(),
            to_ref: WORKTREE_REF.to_string(),
            rows: vec![],
            only_changes: true,
            context_lines: 3,
            last_error: None,
            needs_refresh: false,
        }
    }
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

pub use crate::app::ui::code_editor::CodeEditorState;

pub struct AppState {
    pub platform: Arc<dyn Platform>,
    pub broker: CapabilityBroker,

    pub inputs: InputsState,
    pub results: ResultsState,
    pub ui: UiState,
    pub tree: TreeState,

    pub file_viewers: HashMap<ComponentId, FileViewerState>,
    pub active_file_viewer: Option<ComponentId>,

    pub diff_viewers: HashMap<ComponentId, DiffViewerState>,
    pub active_diff_viewer: Option<ComponentId>,

    pub terminals: HashMap<ComponentId, TerminalState>,
    pub context_exporters: HashMap<ComponentId, ContextExporterState>,

    pub changeset_appliers: HashMap<ComponentId, ChangeSetApplierState>,

    pub source_controls: HashMap<ComponentId, SourceControlState>,

    pub theme: ThemeState,
    pub deferred: DeferredActions,
    pub layout: LayoutConfig,

    pub layout_epoch: u64,

    pub pending_viewport_restore: Option<ViewportRestore>,
    pub pending_workspace_apply: Option<PendingWorkspaceApply>,

    pub palette: CommandPaletteState,
    pub palette_last_name: Option<String>,

    pub current_workspace_name: String,
    pub last_window_title: Option<String>,

    pub pending_open_file_path: Option<String>,
    pub pending_open_file_viewer: Option<ComponentId>,
}

pub struct InputsState {
    pub repo: Option<PathBuf>,
    pub local_repo: Option<PathBuf>,
    pub git_ref: String,
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

    /// Preserve tree-select checkbox state per git ref label (HEAD/main/origin/.../WORKTREE).
    pub context_selected_by_ref: HashMap<String, HashSet<String>>,
}

pub struct FileViewerState {
    pub selected_file: Option<String>,
    pub selected_commit: Option<String>,
    pub view_at: FileViewAt,

    pub file_commits: Vec<CommitEntry>,
    pub file_content: String,
    pub file_content_err: Option<String>,

    pub edit_working_tree: bool,
    pub edit_buffer: String,
    pub edit_status: Option<String>,

    pub editor: CodeEditorState,

    pub show_diff: bool,
    pub diff_picker_open: bool,

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
            diff_picker_open: false,
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

impl AppState {
    pub fn new(platform: Arc<dyn Platform>) -> Self {
        let layout = LayoutConfig::default();
        let broker = CapabilityBroker::new(platform.clone());

        let mut file_viewers = HashMap::new();
        file_viewers.insert(2, FileViewerState::new());

        Self {
            platform,
            broker,

            inputs: InputsState {
                repo: None,
                local_repo: None,
                git_ref: "HEAD".to_string(),
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
                context_selected_by_ref: HashMap::new(),
            },

            file_viewers,
            active_file_viewer: Some(2),

            diff_viewers: HashMap::new(),
            active_diff_viewer: None,

            terminals: HashMap::new(),
            context_exporters: HashMap::new(),
            changeset_appliers: HashMap::new(),
            source_controls: HashMap::new(),

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

            current_workspace_name: "workspace".to_string(),
            last_window_title: None,

            pending_open_file_path: None,
            pending_open_file_viewer: None,
        }
    }

    pub fn set_git_ref(&mut self, git_ref: String) {
        // Save selection for previous ref.
        let prev = self.inputs.git_ref.clone();
        self.tree
            .context_selected_by_ref
            .insert(prev, self.tree.context_selected_files.clone());

        // Switch ref.
        self.inputs.git_ref = git_ref.clone();

        // Restore selection for new ref if available.
        if let Some(saved) = self.tree.context_selected_by_ref.get(&git_ref).cloned() {
            self.tree.context_selected_files = saved;
        }

        self.refresh_follow_top_bar_viewers();
    }

    pub fn set_git_ref_options(&mut self, mut refs: Vec<String>) {
        refs.retain(|r| !r.trim().is_empty());
        refs.retain(|r| r != WORKTREE_REF);
        refs.retain(|r| r != "HEAD");

        let mut seen = std::collections::HashSet::new();
        refs.retain(|r| seen.insert(r.clone()));

        let mut out = Vec::with_capacity(refs.len() + 2);
        out.push("HEAD".to_string());
        out.push(WORKTREE_REF.to_string());
        out.extend(refs);

        self.inputs.git_ref_options = out;

        let cur = self.inputs.git_ref.clone();
        if !self.inputs.git_ref_options.iter().any(|r| r == &cur) {
            self.set_git_ref("HEAD".to_string());
        }
    }

    /// NEW: when the chosen folder isn't a git repo, the app becomes "WORKTREE only".
    pub fn set_git_ref_options_worktree_only(&mut self) {
        self.inputs.git_ref_options = vec![WORKTREE_REF.to_string()];
        if self.inputs.git_ref != WORKTREE_REF {
            self.set_git_ref(WORKTREE_REF.to_string());
        }
    }

    pub fn refresh_follow_top_bar_viewers(&mut self) {
        let ids: Vec<ComponentId> = self
            .file_viewers
            .iter()
            .filter_map(|(id, v)| {
                if v.view_at == FileViewAt::FollowTopBar && v.selected_file.is_some() {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();

        for id in ids {
            self.load_file_at_current_selection(id);
        }
    }
}
