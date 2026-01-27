// src/app/state.rs
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use egui_extras::syntax_highlighting::CodeTheme;
use std::collections::BTreeSet;

use crate::capabilities::CapabilityBroker;
use crate::model::{AnalysisResult, CommitEntry};
use crate::platform::Platform;

use super::openai::OpenAIClient;

use serde::{Deserialize, Serialize};

use super::actions::{ComponentId, ConversationId, ExpandCmd, TerminalShell};
use super::actions::ComponentKind;
use super::layout::{ExecuteLoopSnapshot, LayoutConfig, PresetKind};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecuteLoopMode {
    /// Normal assistant conversation (freeform).
    Conversation,
    /// Ask the assistant to output ONLY a ChangeSet JSON.
    ChangeSet,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecuteLoopMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct ExecuteLoopTurnResult {
    pub text: String,
    pub conversation_id: String,
}

#[derive(Clone, Debug)]
pub struct ExecuteLoopIteration {
    pub request: String,
    pub response: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TaskState {
    pub bound_execute_loop: Option<ComponentId>,
    pub paused: bool,
    pub execute_loop_ids: Vec<ComponentId>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub conversations: HashMap<ConversationId, ExecuteLoopSnapshot>,
    pub active_conversation: Option<ConversationId>,
    pub next_conversation_id: ConversationId,
}

impl Default for TaskState {
    fn default() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Self {
            bound_execute_loop: None,
            paused: false,
            execute_loop_ids: Vec::new(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            conversations: HashMap::new(),
            active_conversation: None,
            next_conversation_id: 1,
        }
    }
}

pub struct ExecuteLoopState {
    /// Selected model id (shown in UI). Options are fetched from the OpenAI models API.
    pub model: String,
    /// Task-level pause (disables auto-flow when true).
    pub paused: bool,

    /// Persisted stats (best-effort; increment where you already detect outcomes).
    pub changesets_total: u32,
    pub changesets_ok: u32,
    pub changesets_err: u32,
    pub postprocess_ok: u32,
    pub postprocess_err: u32,

    /// Cached options for dropdown. Empty => not yet fetched / failed.
    pub model_options: Vec<String>,

    /// Current mode for the next assistant turn.
    pub mode: ExecuteLoopMode,

    /// Optional “goal” instruction shown/used as a system prompt.
    pub instruction: String,

    /// Conversation transcript (role = "system"|"user"|"assistant").
    pub messages: Vec<ExecuteLoopMessage>,

    /// Draft text box input.
    pub draft: String,

    /// If true, the next send injects fresh context (generated in-memory).
    pub include_context_next: bool,

    /// OpenAI Conversations API id (conv_...). When set, we send only delta turns.
    pub conversation_id: Option<String>,

    /// True while we are fetching conversation history from the server.
    pub history_sync_pending: bool,
    /// Receives fetched conversation messages from the background thread.
    pub history_sync_rx: Option<std::sync::mpsc::Receiver<Result<Vec<ExecuteLoopMessage>, String>>>,
    /// The last conversation id we successfully synced (prevents refetch every frame).
    pub history_synced_conversation_id: Option<String>,

    /// If true, auto-fill the first ChangeSet Applier when in ChangeSet mode.
    pub auto_fill_first_changeset_applier: bool,

    /// When in ChangeSet mode, after we get a response we switch to a review state.
    pub awaiting_review: bool,

    /// ChangeSet mode: if true, do not pause for review after each response.
    /// If false, the loop pauses after each ChangeSet response (awaiting_review=true) so you can step manually.
    pub changeset_auto: bool,

    /// True while an OpenAI request is in-flight (done on a background thread).
    pub pending: bool,
    /// Receives the next assistant response (or error) from the background thread.
    pub pending_rx: Option<std::sync::mpsc::Receiver<Result<ExecuteLoopTurnResult, String>>>,

    /// Postprocess command to run after applying a ChangeSet (e.g. `cargo check`).
    pub postprocess_cmd: String,
    /// True while postprocess command is running.
    pub postprocess_pending: bool,
    /// Receives postprocess output (Ok=success output, Err=failure output).
    pub postprocess_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,

    /// Last ChangeSetApplier we auto-applied into (so we can log apply results back into chat).
    pub last_auto_applier_id: Option<ComponentId>,
    /// Last observed status string from that applier (dedupe repeated logs).
    pub last_auto_applier_status: Option<String>,

    /// Internal: whether we are waiting on an auto-apply result to decide next steps.
    pub awaiting_apply_result: bool,

    pub last_status: Option<String>,

    /// Legacy iterations (kept for compatibility / history). New UI primarily uses `messages`.
    pub iterations: Vec<ExecuteLoopIteration>,
}

impl ExecuteLoopState {
    pub fn new() -> Self {

        let instruction = "".to_string();
        Self {
            model: "gpt-4o-mini".to_string(),
            model_options: vec![],
            mode: ExecuteLoopMode::Conversation,
            instruction,
            messages: vec![],
            draft: String::new(),
            include_context_next: true,
            conversation_id: None,
            history_sync_pending: false,
            history_sync_rx: None,
            history_synced_conversation_id: None,
            auto_fill_first_changeset_applier: true,
            awaiting_review: false,
            changeset_auto: true,
            paused: false,
            changesets_total: 0,
            changesets_ok: 0,
            changesets_err: 0,
            postprocess_ok: 0,
            postprocess_err: 0,
            pending: false,
            pending_rx: None,
            postprocess_cmd: "cargo check".to_string(),
            postprocess_pending: false,
            postprocess_rx: None,
            last_auto_applier_id: None,
            last_auto_applier_status: None,
            awaiting_apply_result: false,
            last_status: None,
            iterations: vec![],
        }
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

    pub openai: OpenAIClient,

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
    pub execute_loops: HashMap<ComponentId, ExecuteLoopState>,

    /// Repo-global persisted ExecuteLoop snapshots (loaded at repo select).
    /// ExecuteLoopState is ephemeral and is hydrated on-demand from this store.
    pub execute_loop_store: HashMap<ComponentId, ExecuteLoopSnapshot>,

    pub tasks: HashMap<ComponentId, TaskState>,

    /// Dirty flag for global per-repo task/chat store autosave.
    pub task_store_dirty: bool,

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

    pub canvas_bg_tint: Option<[u8; 4]>,
    pub canvas_tint_popup_open: bool,
    pub canvas_tint_draft: Option<[u8; 4]>,

    pub task_panel_selected_loops: Option<HashMap<ComponentId, BTreeSet<ConversationId>>>,
}

impl UiState {
    pub fn task_panel_selected_loops_mut(&mut self) -> &mut HashMap<ComponentId, BTreeSet<ConversationId>> {
        // Lazily allocate via Option to keep struct init stable.
        self.task_panel_selected_loops.get_or_insert_with(HashMap::new)
    }
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

            openai: OpenAIClient::from_env(),

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
                canvas_bg_tint: None,
                canvas_tint_popup_open: false,
                canvas_tint_draft: None,
                task_panel_selected_loops: None,
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
            execute_loops: HashMap::new(),
            execute_loop_store: HashMap::new(),
            tasks: HashMap::new(),
            task_store_dirty: false,
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

    // -----------------------------------------------------------------
    // Restored helpers (other code expects these to exist)
    // -----------------------------------------------------------------

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
                    mode: ContextExportMode::TreeSelect,
                    status: None,
                },
            );
        }
    }

    pub(crate) fn set_context_selection_all(&mut self, res: &crate::model::AnalysisResult) {
        let mut files = Vec::new();
        Self::collect_all_files(&res.root, &mut files);
        let all: std::collections::HashSet<String> = files.into_iter().collect();

        let key = self.inputs.git_ref.clone();

        let mut selected = self
            .tree
            .context_selected_by_ref
            .get(&key)
            .cloned()
            .unwrap_or_else(|| self.tree.context_selected_files.clone());

        selected.retain(|p| all.contains(p));

        if selected.is_empty() {
            selected = all.clone();
        }

        self.tree.context_selected_files = selected.clone();
        self.tree.context_selected_by_ref.insert(key, selected);
    }

    fn collect_all_files(node: &crate::model::DirNode, out: &mut Vec<String>) {
        for f in &node.files {
            out.push(f.full_path.clone());
        }
        for c in &node.children {
            Self::collect_all_files(c, out);
        }
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

    /// Rebuild Execute Loop backing state from the current layout.
    ///
    /// Execute Loops are ephemeral UI components; their backing state map must be
    /// re-synced after workspace/layout load to avoid "missing/broken" state.
    pub fn rebuild_execute_loops_from_layout(&mut self) {
        self.execute_loops.clear();

        let ids: Vec<ComponentId> = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::ExecuteLoop)
            .map(|c| c.id)
            .collect();

        for id in ids {
            self.execute_loops.insert(id, ExecuteLoopState::new());
        }
    }

    pub fn rebuild_tasks_from_layout(&mut self) {
        let existing = std::mem::take(&mut self.tasks);
        self.tasks = HashMap::new();

        let ids: Vec<ComponentId> = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::Task)
            .map(|c| c.id)
            .collect();

        for id in ids {
            let st = existing.get(&id).cloned().unwrap_or_default();
            self.tasks.insert(id, st);
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
