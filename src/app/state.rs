use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::Receiver;
use std::io::Write;

use egui_extras::syntax_highlighting::CodeTheme;
use std::collections::BTreeSet;

use crate::capabilities::CapabilityBroker;
use crate::capabilities::GitStatusEntry;

use crate::model::{AnalysisResult, CommitEntry};
use crate::platform::Platform;

use super::openai::OpenAIClient;

use serde::{Deserialize, Serialize};

use super::actions::{ComponentId, ConversationId, ExpandCmd, TerminalShell};
use super::actions::ComponentKind;
use super::layout::{ExecuteLoopSnapshot, LayoutConfig, PresetKind};

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
    pub target_canvas_size: Option<[f32; 2]>,
    pub wait_frames: u8,
    pub timeout_frames: u16,
}

#[derive(Clone, Debug, Default)]
pub struct CommandPaletteState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
}
#[derive(Clone, Debug)]
pub enum TerminalEvent {
    Stdout(String),
    Stderr(String),
    Error(String),
}

pub struct TerminalState {
    pub vt: Option<vt100::Parser>,
    pub rendered_output: String,

    pub scrollback: VecDeque<String>,
    pub scrollback_partial: String,
    pub scrollback_max_lines: usize,

    pub follow_output: bool,

    pub pty_master: Option<std::sync::Arc<std::sync::Mutex<Box<dyn portable_pty::MasterPty + Send>>>>,
    pub pty_child: Option<std::sync::Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send>>>>,
    pub pty_size: Option<(u16, u16)>,

    pub shell: TerminalShell,
    pub cwd: Option<PathBuf>,

    pub pending_rx: Option<Receiver<TerminalEvent>>,
    pub pty_in: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
}
    pub struct ContextExporterState {
    pub save_path: Option<PathBuf>,
    pub skip_binary: bool,
    pub skip_gitignore: bool,
    pub include_staged_diff: bool,
    pub include_unstaged_diff: bool,
    pub mode: ContextExportMode,
    pub status: Option<String>,
    pub selection_defaults: std::collections::HashSet<String>,
    pub export_pending: bool,
    pub export_rx: Option<std::sync::mpsc::Receiver<Result<u128, String>>>,
}

pub struct ChangeSetApplierState {
    pub payload: String,
    pub status: Option<String>,
    pub last_attempted_paths: Vec<String>,
    pub last_failed_paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecuteLoopMode {
    Conversation,
    ChangeSet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecuteLoopWorkflowStage {
    Design,
    Code,
    Compile,
    Test,
    Review,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecuteLoopStageAutomation {
    Manual,
    Auto,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecuteLoopWorkflowStageConfig {
    pub stage: ExecuteLoopWorkflowStage,
    pub enabled: bool,
    pub automation: ExecuteLoopStageAutomation,
    #[serde(default)]
    pub commands: Vec<String>,
}

impl ExecuteLoopWorkflowStageConfig {
    pub fn new(
        stage: ExecuteLoopWorkflowStage,
        enabled: bool,
        automation: ExecuteLoopStageAutomation,
    ) -> Self {
        Self {
            stage,
            enabled,
            automation,
            commands: vec![],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecuteLoopMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecuteLoopManualMessageFragments {
    pub include_system_instruction: bool,
    pub include_repo_context: bool,
    pub include_changeset_schema: bool,
}

impl Default for ExecuteLoopManualMessageFragments {
    fn default() -> Self {
        Self {
            include_system_instruction: true,
            include_repo_context: false,
            include_changeset_schema: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecuteLoopAutomaticMessageFragments {
    pub apply_error: Option<String>,
    pub changeset_validation_error: Option<String>,
    pub compile_error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecuteLoopApplyFailureFocusedContextPolicy {
    pub enabled: bool,
    pub consecutive_failure_threshold: u32,
}

impl Default for ExecuteLoopApplyFailureFocusedContextPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            consecutive_failure_threshold: 2,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecuteLoopAutomationPolicies {
    #[serde(default)]
    pub apply_failure_focused_context: ExecuteLoopApplyFailureFocusedContextPolicy,
}


#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecuteLoopFragmentOverrides {
    pub system_instruction: Option<String>,
    pub changeset_schema: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserBridgeStatus {
    Detached,
    Attached,
    Ready,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowserProbeResult {
    pub session_id: String,
    pub browser_connected: bool,
    pub page_open: bool,
    pub url: String,
    pub profile: String,
    pub chat_input_found: bool,
    pub chat_input_visible: bool,
    pub chat_submit_found: bool,
    pub ready: bool,
}

#[derive(Clone, Debug)]
pub struct ExecuteLoopTurnResult {
    pub text: String,
    pub conversation_id: Option<String>,
    pub browser_session_id: Option<String>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExecuteLoopTransport {
    Api,
    BrowserBridge,
}

pub struct ExecuteLoopState {
    pub model: String,
    pub paused: bool,

    pub changesets_total: u32,
    pub changesets_ok: u32,
    pub changesets_err: u32,
    pub postprocess_ok: u32,
    pub postprocess_err: u32,

    pub model_options: Vec<String>,

    pub transport: ExecuteLoopTransport,
    pub browser_profile: String,
    pub browser_bridge_dir: String,
    pub browser_cdp_url: String,
    pub browser_page_url_contains: String,
    pub browser_target_url: String,
    pub browser_edge_executable: String,
    pub browser_user_data_dir: String,
    pub browser_session_id: Option<String>,
    pub browser_status: BrowserBridgeStatus,
    pub browser_last_probe: Option<BrowserProbeResult>,
    pub browser_probe_pending: bool,
    pub browser_probe_error: Option<String>,
    pub browser_attached: bool,
    pub browser_auto_launch_edge: bool,
    pub browser_response_timeout_ms: u64,
    pub browser_response_poll_ms: u64,
    pub active_browser_runtime_key: Option<String>,
    pub browser_response_timeout_input: String,
    pub browser_timeout_confirm_pending: bool,

    pub instruction: String,

    pub messages: Vec<ExecuteLoopMessage>,

    pub draft: String,

    pub include_context_next: bool,

    pub manual_fragments: ExecuteLoopManualMessageFragments,
    pub automatic_fragments: ExecuteLoopAutomaticMessageFragments,
    pub fragment_overrides: ExecuteLoopFragmentOverrides,
    pub automation_policies: ExecuteLoopAutomationPolicies,
    pub apply_failure_focused_context_counts: HashMap<String, u32>,
    pub pending_focused_context_paths: BTreeSet<String>,

    pub conversation_id: Option<String>,

    pub history_sync_pending: bool,
    pub history_sync_rx: Option<std::sync::mpsc::Receiver<Result<Vec<ExecuteLoopMessage>, String>>>,
    pub history_synced_conversation_id: Option<String>,

    pub auto_fill_first_changeset_applier: bool,

    pub awaiting_review: bool,

    pub changeset_auto: bool,

    pub workflow_stages: Vec<ExecuteLoopWorkflowStageConfig>,
    pub workflow_active_stage: ExecuteLoopWorkflowStage,

    pub pending: bool,
    pub pending_rx: Option<std::sync::mpsc::Receiver<Result<ExecuteLoopTurnResult, String>>>,

    pub postprocess_cmd: String,
    pub postprocess_pending: bool,
    pub postprocess_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,

    pub last_auto_applier_id: Option<ComponentId>,
    pub last_auto_applier_status: Option<String>,

    pub awaiting_apply_result: bool,

    pub last_status: Option<String>,

    pub iterations: Vec<ExecuteLoopIteration>,
}

impl ExecuteLoopState {
    pub fn new() -> Self {

        let instruction = "".to_string();
        Self {
            model: "gpt-4o-mini".to_string(),
            model_options: vec![],
            transport: ExecuteLoopTransport::Api,
            browser_profile: String::new(),
            browser_bridge_dir: String::new(),
            browser_cdp_url: "http://127.0.0.1:9222".to_string(),
            browser_page_url_contains: String::new(),
            browser_target_url: String::new(),
            browser_edge_executable: String::new(),
            browser_user_data_dir: String::new(),
            browser_session_id: None,
            browser_status: BrowserBridgeStatus::Detached,
            browser_last_probe: None,
            browser_probe_pending: false,
            browser_probe_error: None,
            browser_attached: false,
            browser_auto_launch_edge: true,
            browser_response_timeout_ms: 180_000,
            browser_response_poll_ms: 2_000,
            active_browser_runtime_key: None,
            browser_response_timeout_input: "180".to_string(),
            browser_timeout_confirm_pending: false,
            instruction,
            messages: vec![],
            draft: String::new(),
            include_context_next: true,
            manual_fragments: ExecuteLoopManualMessageFragments {
                include_system_instruction: true,
                include_repo_context: true,
                include_changeset_schema: false,
            },
            automatic_fragments: ExecuteLoopAutomaticMessageFragments::default(),
            fragment_overrides: ExecuteLoopFragmentOverrides::default(),
            automation_policies: ExecuteLoopAutomationPolicies::default(),
            apply_failure_focused_context_counts: HashMap::new(),
            pending_focused_context_paths: BTreeSet::new(),
            conversation_id: None,
            history_sync_pending: false,
            history_sync_rx: None,
            history_synced_conversation_id: None,
            auto_fill_first_changeset_applier: true,
            awaiting_review: false,
            changeset_auto: true,
            workflow_stages: vec![
                ExecuteLoopWorkflowStageConfig::new(
                    ExecuteLoopWorkflowStage::Design,
                    true,
                    ExecuteLoopStageAutomation::Manual,
                ),
                ExecuteLoopWorkflowStageConfig::new(
                    ExecuteLoopWorkflowStage::Code,
                    true,
                    ExecuteLoopStageAutomation::Auto,
                ),
                ExecuteLoopWorkflowStageConfig {
                    stage: ExecuteLoopWorkflowStage::Compile,
                    enabled: true,
                    automation: ExecuteLoopStageAutomation::Auto,
                    commands: vec!["cargo check".to_string()],
                },
                ExecuteLoopWorkflowStageConfig::new(
                    ExecuteLoopWorkflowStage::Finished,
                    true,
                    ExecuteLoopStageAutomation::Manual,
                ),
            ],
            workflow_active_stage: ExecuteLoopWorkflowStage::Design,
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

impl ExecuteLoopState {
    pub fn ensure_default_changeset_workflow(&mut self) {
        if self.workflow_stages.is_empty() {
            self.workflow_stages = vec![
                ExecuteLoopWorkflowStageConfig::new(
                    ExecuteLoopWorkflowStage::Design,
                    true,
                    ExecuteLoopStageAutomation::Manual,
                ),
                ExecuteLoopWorkflowStageConfig::new(
                    ExecuteLoopWorkflowStage::Code,
                    true,
                    if self.changeset_auto {
                        ExecuteLoopStageAutomation::Auto
                    } else {
                        ExecuteLoopStageAutomation::Manual
                    },
                ),
                ExecuteLoopWorkflowStageConfig {
                    stage: ExecuteLoopWorkflowStage::Compile,
                    enabled: true,
                    automation: if self.changeset_auto {
                        ExecuteLoopStageAutomation::Auto
                    } else {
                        ExecuteLoopStageAutomation::Manual
                    },
                    commands: self
                        .postprocess_cmd
                        .lines()
                        .map(|line| line.trim())
                        .filter(|line| !line.is_empty())
                        .map(|line| line.to_string())
                        .collect(),
                },
                ExecuteLoopWorkflowStageConfig::new(
                    ExecuteLoopWorkflowStage::Finished,
                    true,
                    ExecuteLoopStageAutomation::Manual,
                ),
            ];
        }

        if !self.workflow_stages.iter().any(|cfg| cfg.stage == ExecuteLoopWorkflowStage::Finished) {
            self.workflow_stages.push(ExecuteLoopWorkflowStageConfig::new(
                ExecuteLoopWorkflowStage::Finished,
                true,
                ExecuteLoopStageAutomation::Manual,
            ));
        }

        if let Some(idx) = self
            .workflow_stages
            .iter()
            .position(|cfg| cfg.stage == ExecuteLoopWorkflowStage::Finished)
        {
            if idx + 1 != self.workflow_stages.len() {
                let finished = self.workflow_stages.remove(idx);
                self.workflow_stages.push(finished);
            }
        }

        let active_exists = self
            .workflow_stages
            .iter()
            .any(|cfg| cfg.enabled && cfg.stage == self.workflow_active_stage);
        if !active_exists {
            self.workflow_active_stage = self
                .workflow_stages
                .iter()
                .find(|cfg| cfg.enabled)
                .map(|cfg| cfg.stage)
                .unwrap_or(ExecuteLoopWorkflowStage::Finished);
        }

        self.sync_legacy_changeset_fields();
        self.sync_message_fragment_defaults_for_stage();
    }

    pub fn sync_legacy_changeset_fields(&mut self) {
        if let Some(cfg) = self
            .workflow_stages
            .iter()
            .find(|cfg| cfg.stage == ExecuteLoopWorkflowStage::Code)
        {
            self.changeset_auto = cfg.automation == ExecuteLoopStageAutomation::Auto;
        }

        if let Some(cfg) = self
            .workflow_stages
            .iter()
            .find(|cfg| cfg.stage == ExecuteLoopWorkflowStage::Compile)
        {
            let joined = cfg
                .commands
                .iter()
                .map(|cmd| cmd.trim())
                .filter(|cmd| !cmd.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            if !joined.is_empty() {
                self.postprocess_cmd = joined;
            }
        }
    }

    pub fn workflow_stage_config(
        &self,
        stage: ExecuteLoopWorkflowStage,
    ) -> Option<&ExecuteLoopWorkflowStageConfig> {
        self.workflow_stages.iter().find(|cfg| cfg.stage == stage)
    }

    pub fn workflow_stage_config_mut(
        &mut self,
        stage: ExecuteLoopWorkflowStage,
    ) -> Option<&mut ExecuteLoopWorkflowStageConfig> {
        self.workflow_stages.iter_mut().find(|cfg| cfg.stage == stage)
    }

    pub fn workflow_next_stage(
        &self,
        stage: ExecuteLoopWorkflowStage,
    ) -> Option<ExecuteLoopWorkflowStage> {
        let mut seen = false;
        for cfg in self.workflow_stages.iter() {
            if !cfg.enabled {
                continue;
            }
            if seen {
                return Some(cfg.stage);
            }
            if cfg.stage == stage {
                seen = true;
            }
        }
        None
    }

    pub fn workflow_set_active_stage(&mut self, stage: ExecuteLoopWorkflowStage) {
        self.workflow_active_stage = stage;
        self.awaiting_review = false;
    }

    pub fn workflow_stage_is_auto(&self, stage: ExecuteLoopWorkflowStage) -> bool {
        self.workflow_stage_config(stage)
            .map(|cfg| cfg.automation == ExecuteLoopStageAutomation::Auto)
            .unwrap_or(false)
    }

    pub fn effective_mode(&self) -> ExecuteLoopMode {
        match self.workflow_active_stage {
            ExecuteLoopWorkflowStage::Design => ExecuteLoopMode::Conversation,
            ExecuteLoopWorkflowStage::Code => ExecuteLoopMode::ChangeSet,
            _ => ExecuteLoopMode::ChangeSet,
        }
    }

    pub fn sync_message_fragment_defaults_for_stage(&mut self) {}

    pub fn clear_automatic_message_fragments(&mut self) {
        self.automatic_fragments = ExecuteLoopAutomaticMessageFragments::default();
    }

    pub fn reset_apply_failure_focused_context_runtime(&mut self) {
        self.apply_failure_focused_context_counts.clear();
        self.pending_focused_context_paths.clear();
    }

    pub fn record_apply_failure_attempt(
        &mut self,
        attempted_paths: &[String],
        failed_paths: &[String],
    ) -> Vec<String> {
        if !self.automation_policies.apply_failure_focused_context.enabled {
            return Vec::new();
        }

        let threshold = self
            .automation_policies
            .apply_failure_focused_context
            .consecutive_failure_threshold
            .max(1);

        let failed_lookup: HashSet<&str> = failed_paths.iter().map(|path| path.as_str()).collect();
        for path in attempted_paths {
            if !failed_lookup.contains(path.as_str()) {
                self.apply_failure_focused_context_counts.remove(path);
            }
        }

        let mut triggered = BTreeSet::new();
        for path in failed_paths {
            let next = self
                .apply_failure_focused_context_counts
                .get(path)
                .copied()
                .unwrap_or(0)
                .saturating_add(1);
            self.apply_failure_focused_context_counts.insert(path.clone(), next);
            if next >= threshold {
                self.pending_focused_context_paths.insert(path.clone());
                triggered.insert(path.clone());
            }
        }

        triggered.into_iter().collect()
    }

    pub fn take_pending_focused_context_paths(&mut self) -> Vec<String> {
        let paths: Vec<String> = self.pending_focused_context_paths.iter().cloned().collect();
        for path in &paths {
            self.apply_failure_focused_context_counts.remove(path);
        }
        self.pending_focused_context_paths.clear();
        paths
    }

    pub fn clear_manual_message_fragments(&mut self) {
        self.manual_fragments = ExecuteLoopManualMessageFragments::default();
        self.include_context_next = false;
    }

    pub fn effective_system_instruction_fragment(&self) -> String {
        self.fragment_overrides
            .system_instruction
            .as_deref()
            .unwrap_or(self.instruction.as_str())
            .trim()
            .to_string()
    }

    pub fn effective_changeset_schema_fragment(&self) -> String {
        self.fragment_overrides
            .changeset_schema
            .as_deref()
            .unwrap_or(crate::app::ui::changeset_applier::CHANGESET_SCHEMA_EXAMPLE)
            .trim()
            .to_string()
    }

    pub fn compile_command_list(&self) -> Vec<String> {
        self.workflow_stage_config(ExecuteLoopWorkflowStage::Compile)
            .map(|cfg| {
                cfg.commands
                    .iter()
                    .map(|cmd| cmd.trim())
                    .filter(|cmd| !cmd.is_empty())
                    .map(|cmd| cmd.to_string())
                    .collect::<Vec<_>>()
            })
            .filter(|cmds| !cmds.is_empty())
            .unwrap_or_else(|| {
                self.postprocess_cmd
                    .lines()
                    .map(|line| line.trim())
                    .filter(|line| !line.is_empty())
                    .map(|line| line.to_string())
                    .collect::<Vec<_>>()
            })
    }
}

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

    pub needs_refresh: bool,
}


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

    pub from_ref: String,
    pub to_ref: String,

    pub rows: Vec<DiffRow>,

    pub only_changes: bool,

    pub context_lines: usize,

    pub last_error: Option<String>,

    pub needs_refresh: bool,
}


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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileViewAt {
    FollowTopBar,
    WorkingTree,
    Commit,
}

pub use crate::app::ui::code_editor::CodeEditorState;

#[derive(Clone, Debug)]
pub struct CanvasState {
    pub name: String,
    pub layout: LayoutConfig,
    pub active_file_viewer: Option<ComponentId>,
    pub active_diff_viewer: Option<ComponentId>,
    pub layout_epoch: u32,
}

pub struct AppState {
    pub platform: Arc<dyn Platform>,
    pub broker: CapabilityBroker,

    pub openai: OpenAIClient,

    pub inputs: InputsState,
    pub results: ResultsState,
    pub ui: UiState,
    pub tree: TreeState,

    pub canvases: Vec<CanvasState>,
    pub active_canvas: usize,
    pub next_component_id: ComponentId,

    pub file_viewers: HashMap<ComponentId, FileViewerState>,

    pub diff_viewers: HashMap<ComponentId, DiffViewerState>,

    pub terminals: HashMap<ComponentId, TerminalState>,
    pub context_exporters: HashMap<ComponentId, ContextExporterState>,

    pub changeset_appliers: HashMap<ComponentId, ChangeSetApplierState>,
    pub execute_loops: HashMap<ComponentId, ExecuteLoopState>,

    pub execute_loop_store: HashMap<ComponentId, ExecuteLoopSnapshot>,

    pub tasks: HashMap<ComponentId, TaskState>,

    pub task_store_dirty: bool,

    pub source_controls: HashMap<ComponentId, SourceControlState>,

    pub theme: ThemeState,
    pub deferred: DeferredActions,

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

    pub canvas_rename_index: Option<usize>,
    pub canvas_rename_draft: String,
}

impl UiState {
    pub fn task_panel_selected_loops_mut(&mut self) -> &mut HashMap<ComponentId, BTreeSet<ConversationId>> {
        self.task_panel_selected_loops.get_or_insert_with(HashMap::new)
    }
}


pub struct TreeState {
    pub expand_cmd: Option<ExpandCmd>,
    pub context_selected_files: HashSet<String>,
    pub context_selected_by_ref: HashMap<String, HashSet<String>>,

    pub context_initialized: bool,

    pub modified_paths: HashSet<String>,
    pub staged_paths: HashSet<String>,
    pub untracked_paths: HashSet<String>,

    pub git_status_by_path: HashMap<String, GitStatusEntry>,

    pub last_auto_refresh_s: f64,
    pub auto_refresh_interval_s: f64,

    pub last_git_status_refresh_s: f64,
    pub git_status_interval_s: f64,

    pub analysis_refresh_pending: bool,
    pub analysis_refresh_rx: Option<Receiver<Result<crate::model::AnalysisResult, String>>>,

    pub rename_target: Option<String>,
    pub rename_draft: String,

    pub create_parent: Option<String>,
    pub create_draft: String,
    pub create_is_dir: bool,

    pub confirm_delete_target: Option<String>,

    pub modal_focus_request: bool,
}


pub struct FileViewerState {
    pub selected_file: Option<String>,
    pub selected_commit: Option<String>,
    pub view_at: FileViewAt,

    pub file_commits: Vec<CommitEntry>,
    pub file_content: String,
    pub file_content_err: Option<String>,

    pub file_load_pending: bool,
    pub file_load_rx: Option<std::sync::mpsc::Receiver<(u64, String, Result<Vec<u8>, String>)>>,
    pub file_load_seq: u64,
    pub file_load_path: Option<String>,

    pub history_load_pending: bool,
    pub history_load_rx: Option<std::sync::mpsc::Receiver<Result<Vec<u8>, String>>>,

    pub edit_working_tree: bool,
    pub edit_buffer: String,
    pub edit_status: Option<String>,

    pub editor: CodeEditorState,
    pub viewer_editor: CodeEditorState,


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

            file_load_pending: false,
            file_load_rx: None,
            file_load_seq: 0,
            file_load_path: None,

            history_load_pending: false,
            history_load_rx: None,

            edit_working_tree: false,
            edit_buffer: String::new(),
            edit_status: None,

            editor: CodeEditorState::default(),
            viewer_editor: CodeEditorState::default(),


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

    pub last_applied_dark: Option<bool>,
    pub last_applied_syntect_theme: Option<String>,
}

pub struct DeferredActions {
    pub open_file: Option<String>,
    pub open_file_target_viewer: Option<ComponentId>,
    pub select_commit: Option<(ComponentId, Option<String>)>,
    pub refresh_viewer: Option<ComponentId>,
}

impl AppState {
    pub(crate) fn bump_next_component_id(&mut self) {
        let max_id = self
            .all_layouts()
            .flat_map(|l| l.components.iter().map(|c| c.id))
            .max()
            .unwrap_or(0);

        let needed = max_id.saturating_add(1);
        if self.next_component_id < needed {
            self.next_component_id = needed;
        }
    }

    pub fn active_canvas_state(&self) -> &CanvasState {
        let idx = self.active_canvas.min(self.canvases.len().saturating_sub(1));
        &self.canvases[idx]
    }

    pub fn active_canvas_state_mut(&mut self) -> &mut CanvasState {
        if self.canvases.is_empty() {
            let layout = LayoutConfig {
                components: vec![],
                windows: HashMap::new(),
            };
            self.canvases.push(CanvasState {
                name: "Canvas 1".to_string(),
                layout,
                active_file_viewer: None,
                active_diff_viewer: None,
                layout_epoch: 0,
            });
            self.active_canvas = 0;
            self.next_component_id = 1;
        }
        let idx = self.active_canvas.min(self.canvases.len().saturating_sub(1));
        &mut self.canvases[idx]
    }

    pub fn active_layout(&self) -> &LayoutConfig {
        &self.active_canvas_state().layout
    }

    pub fn active_layout_mut(&mut self) -> &mut LayoutConfig {
        &mut self.active_canvas_state_mut().layout
    }

    pub fn active_file_viewer_id(&self) -> Option<ComponentId> {
        self.active_canvas_state().active_file_viewer
    }

    pub fn set_active_file_viewer_id(&mut self, id: Option<ComponentId>) {
        self.active_canvas_state_mut().active_file_viewer = id;
    }

    pub fn active_diff_viewer_id(&self) -> Option<ComponentId> {
        self.active_canvas_state().active_diff_viewer
    }

    pub fn set_active_diff_viewer_id(&mut self, id: Option<ComponentId>) {
        self.active_canvas_state_mut().active_diff_viewer = id;
    }

    pub fn alloc_component_id(&mut self) -> ComponentId {
        let id = self.next_component_id;
        self.next_component_id = self.next_component_id.wrapping_add(1);
        id
    }


    fn bump_next_component_id_unused(&mut self) {
        let max_id = self
            .all_layouts()
            .flat_map(|l| l.components.iter().map(|c| c.id))
            .max()
            .unwrap_or(0);

        let needed = max_id.saturating_add(1);
        if self.next_component_id < needed {
            self.next_component_id = needed;
        }
    }
    pub fn all_layouts(&self) -> impl Iterator<Item = &LayoutConfig> {
        self.canvases.iter().map(|c| &c.layout)
    }

    pub fn canvas_select(&mut self, index: usize) {
        if self.canvases.is_empty() {
            return;
        }
        self.active_canvas = index.min(self.canvases.len() - 1);
    }

    pub fn canvas_add(&mut self) {
        if self.canvases.len() >= 10 {
            return;
        }

        let idx = self.canvases.len();

        let layout = LayoutConfig {
            components: vec![],
            windows: HashMap::new(),
        };

        self.canvases.push(CanvasState {
            name: format!("Canvas {}", idx + 1),
            layout,
            active_file_viewer: None,
            active_diff_viewer: None,
            layout_epoch: 0,
        });

        self.active_canvas = idx;

        self.rebuild_context_exporters_from_layout();
        self.rebuild_execute_loops_from_layout();
        self.rebuild_tasks_from_layout();
    }

    pub fn canvas_rename(&mut self, index: usize, name: String) {
        if let Some(c) = self.canvases.get_mut(index) {
            let n = name.trim();
            if !n.is_empty() {
                c.name = n.to_string();
            }
        }
    }

    pub fn canvas_delete(&mut self, index: usize) {
        if self.canvases.len() <= 1 {
            return;
        }
        if index >= self.canvases.len() {
            return;
        }

        let ids: std::collections::HashSet<ComponentId> = self.canvases[index]
            .layout
            .components
            .iter()
            .map(|c| c.id)
            .collect();

        for id in ids.iter().copied() {
            self.context_exporters.remove(&id);
            self.execute_loops.remove(&id);
            self.file_viewers.remove(&id);
            self.diff_viewers.remove(&id);
        }

        self.canvases.remove(index);

        if self.active_canvas >= self.canvases.len() {
            self.active_canvas = self.canvases.len() - 1;
        }

        self.rebuild_context_exporters_from_layout();
        self.rebuild_execute_loops_from_layout();
        self.rebuild_tasks_from_layout();

        self.layout_epoch = self.layout_epoch.wrapping_add(1);
    }

    pub fn new(platform: Arc<dyn Platform>) -> Self {
        let broker = CapabilityBroker::new(platform.clone());

        let mut state = Self {
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
                canvas_rename_index: None,
                canvas_rename_draft: String::new(),
                task_panel_selected_loops: None,
            },

            tree: TreeState {
                expand_cmd: None,
                context_selected_files: HashSet::new(),
                context_selected_by_ref: HashMap::new(),
                context_initialized: false,

                modified_paths: HashSet::new(),
                staged_paths: HashSet::new(),
                untracked_paths: HashSet::new(),
                git_status_by_path: HashMap::new(),

                last_auto_refresh_s: 0.0,
                auto_refresh_interval_s: 1.0,

                last_git_status_refresh_s: 0.0,
                git_status_interval_s: 1.0,

                analysis_refresh_pending: false,
                analysis_refresh_rx: None,

                rename_target: None,
                rename_draft: String::new(),

                create_parent: None,
                create_draft: String::new(),
                create_is_dir: false,

                confirm_delete_target: None,
                modal_focus_request: false,
            },

            canvases: vec![CanvasState {
                name: "Canvas 1".to_string(),
                layout: LayoutConfig {
                    components: vec![],
                    windows: HashMap::new(),
                },
                active_file_viewer: None,
                active_diff_viewer: None,
                layout_epoch: 0,
            }],
            active_canvas: 0,
            next_component_id: 1,

            file_viewers: HashMap::new(),

            diff_viewers: HashMap::new(),

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
                last_applied_dark: None,
                last_applied_syntect_theme: None,
            },

            deferred: DeferredActions {
                open_file: None,
                open_file_target_viewer: None,
                select_commit: None,
                refresh_viewer: None,
            },

            layout_epoch: 0,

            pending_viewport_restore: None,
            pending_workspace_apply: None,

            palette: CommandPaletteState::default(),
            palette_last_name: None,

            current_workspace_name: "workspace".to_string(),
            last_window_title: None,

            pending_open_file_path: None,
            pending_open_file_viewer: None,
        };

        state.load_workspace_from_appdata(None);
        state
    }

    pub fn set_git_ref(&mut self, git_ref: String) {
        let prev = self.inputs.git_ref.clone();
        self.tree
            .context_selected_by_ref
            .insert(prev, self.tree.context_selected_files.clone());

        self.inputs.git_ref = git_ref.clone();

        if let Some(saved) = self.tree.context_selected_by_ref.get(&git_ref).cloned() {
            self.tree.context_selected_files = saved;
        }

        self.refresh_follow_top_bar_viewers();
    }


    pub(crate) fn rebuild_context_exporters_from_layout(&mut self) {
        let ids: std::collections::HashSet<ComponentId> = self
            .all_layouts()
            .flat_map(|l| l.components.iter())
            .filter(|c| c.kind == ComponentKind::ContextExporter)
            .map(|c| c.id)
            .collect();

        self.context_exporters.retain(|id, _| ids.contains(id));

        for id in ids {
            self.context_exporters.entry(id).or_insert_with(|| ContextExporterState {
                save_path: None,
                skip_binary: true,
                skip_gitignore: true,
                include_staged_diff: false,
                include_unstaged_diff: false,
                mode: ContextExportMode::TreeSelect,
                status: None,
                selection_defaults: HashSet::new(),
                export_pending: false,
                export_rx: None,
            });
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

    pub fn set_git_ref_options_worktree_only(&mut self) {
        self.inputs.git_ref_options = vec![WORKTREE_REF.to_string()];
        if self.inputs.git_ref != WORKTREE_REF {
            self.set_git_ref(WORKTREE_REF.to_string());
        }
    }

    pub fn rebuild_execute_loops_from_layout(&mut self) {
        let ids: std::collections::HashSet<ComponentId> = self
            .all_layouts()
            .flat_map(|l| l.components.iter())
            .filter(|c| c.kind == ComponentKind::ExecuteLoop)
            .map(|c| c.id)
            .collect();

        self.execute_loops.retain(|id, _| ids.contains(id));

        for id in ids {
            self.execute_loops.entry(id).or_insert_with(ExecuteLoopState::new);
        }
    }

    pub fn rebuild_tasks_from_layout(&mut self) {
        let existing = std::mem::take(&mut self.tasks);
        self.tasks = HashMap::new();

        let ids: Vec<ComponentId> = self
            .all_layouts()
            .flat_map(|l| l.components.iter())
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
