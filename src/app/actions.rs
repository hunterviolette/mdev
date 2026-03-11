
use crate::app::state::FileViewAt;

#[derive(Clone, Copy, Debug)]
pub enum ExpandCmd {
    ExpandAll,
    CollapseAll,
}

pub type ComponentId = u64;

pub type ConversationId = u64;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub enum ComponentKind {
    Tree,
    FileViewer,
    Summary,
    Terminal,
    ContextExporter,
    ChangeSetApplier,
    ExecuteLoop,
    SourceControl,
    Task,
    DiffViewer,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum TerminalShell {
    Auto,
    PowerShell,
    Bash,
    Zsh,
    Sh,
    Cmd,
}

#[derive(Clone, Debug)]
pub enum Action {
    ExecuteLoopRunOnce { loop_id: ComponentId },
    ExecuteLoopSend { loop_id: ComponentId },
    ExecuteLoopSetMode { loop_id: ComponentId, mode: crate::app::state::ExecuteLoopMode },
    ExecuteLoopInjectContext { loop_id: ComponentId },
    ExecuteLoopClearChat { loop_id: ComponentId },
    ExecuteLoopMarkReviewed { loop_id: ComponentId },
    ExecuteLoopRunPostprocess { loop_id: ComponentId },
    ExecuteLoopClear { loop_id: ComponentId },

    // ---------------------------
    // Task
    // ---------------------------
    TaskSetPaused { task_id: ComponentId, paused: bool },
    TaskBindExecuteLoop { task_id: ComponentId, loop_id: ComponentId },
    TaskOpenExecuteLoop { task_id: ComponentId },
    TaskCreateAndBindExecuteLoop { task_id: ComponentId },
    TaskCreateConversationAndOpen {
        task_id: ComponentId,
        transport: crate::app::state::ExecuteLoopTransport,
    },

    ExecuteLoopBrowserLaunchAndAttach { loop_id: ComponentId },
    ExecuteLoopBrowserProbe { loop_id: ComponentId },
    ExecuteLoopBrowserOpenUrl { loop_id: ComponentId },
    ExecuteLoopBrowserDetach { loop_id: ComponentId },
    TaskOpenConversation { task_id: ComponentId, conversation_id: ConversationId },
    TaskConversationsDelete { task_id: ComponentId, conversation_ids: Vec<ConversationId> },
    TaskConversationsSetPaused { task_id: ComponentId, conversation_ids: Vec<ConversationId>, paused: bool },
    ExecuteLoopDelete { loop_id: ComponentId },
    ExecuteLoopsDelete { loop_ids: Vec<ComponentId> },
    ExecuteLoopsSetPaused { loop_ids: Vec<ComponentId>, paused: bool },


    // ---------------------------
    // Repo + analysis
    // ---------------------------
    PickRepo,

    RefreshGitRefs,
    SetGitRef(String),

    RunAnalysis,

    ExpandAll,
    CollapseAll,

    OpenFile(String),

    // ---------------------------
    // UI prefs
    // ---------------------------
    OpenCanvasTintPopup,
    CloseCanvasTintPopup,
    SetCanvasBgTint { rgba: Option<[u8; 4]> },
    SaveStartupLayoutOverride {
        canvas_size: [f32; 2],
        viewport_outer_pos: Option<[f32; 2]>,
        viewport_inner_size: Option<[f32; 2]>,
        pixels_per_point: f32,
    },
    ClearStartupLayoutOverride,
    ExportBuiltInStartupLayout {
        canvas_size: [f32; 2],
        viewport_outer_pos: Option<[f32; 2]>,
        viewport_inner_size: Option<[f32; 2]>,
        pixels_per_point: f32,
    },

    // ---------------------------
    // Diff viewer actions
    // ---------------------------
    OpenDiffViewerForPath { path: String },

    OpenDiffViewerForPathWithRefs {
        path: String,
        from_ref: String,
        to_ref: String,
    },

    RefreshDiffViewer {
        viewer_id: ComponentId,
    },

    DiffViewerRevertPatch {
        viewer_id: ComponentId,
        patch: String,
    },

    // ---------------------------
    // File viewer actions
    // ---------------------------
    SelectCommit {
        viewer_id: ComponentId,
        sel: Option<String>,
    },
    RefreshFile {
        viewer_id: ComponentId,
    },

    SetViewerViewAt {
        viewer_id: ComponentId,
        view_at: FileViewAt,
    },

    ToggleEditWorkingTree {
        viewer_id: ComponentId,
    },
    SaveWorkingTreeFile {
        viewer_id: ComponentId,
    },

    OpenDiffPicker {
        viewer_id: ComponentId,
    },
    CloseDiffPicker {
        viewer_id: ComponentId,
    },

    ToggleDiff {
        viewer_id: ComponentId,
    },
    SetDiffBase {
        viewer_id: ComponentId,
        sel: Option<String>,
    },
    SetDiffTarget {
        viewer_id: ComponentId,
        sel: Option<String>,
    },
    RefreshDiff {
        viewer_id: ComponentId,
    },

    // ---------------------------
    // Terminal actions
    // ---------------------------
    RunTerminalCommand {
        terminal_id: ComponentId,
        cmd: String,
    },
    ClearTerminal {
        terminal_id: ComponentId,
    },
    InterruptTerminal {
        terminal_id: ComponentId,
    },
    SetTerminalShell {
        terminal_id: ComponentId,
        shell: TerminalShell,
    },
    StartTerminalSession {
        terminal_id: ComponentId,
        rows: u16,
        cols: u16,
    },
    ResizeTerminal {
        terminal_id: ComponentId,
        rows: u16,
        cols: u16,
    },
    TerminalSendInput {
        terminal_id: ComponentId,
        data: Vec<u8>,
    },

    // ---------------------------
    // Context exporter actions
    // ---------------------------
    ContextPickSavePath {
        exporter_id: ComponentId,
    },
    ContextGenerate {
        exporter_id: ComponentId,
    },
    ContextToggleSkipBinary {
        exporter_id: ComponentId,
    },
    ContextToggleSkipGitignore {
        exporter_id: ComponentId,
    },
    ContextToggleIncludeStagedDiff {
        exporter_id: ComponentId,
    },
    ContextToggleIncludeUnstagedDiff {
        exporter_id: ComponentId,
    },

    // ---------------------------
    // Change-set applier (AI patch payloads)
    // ---------------------------
    ApplyChangeSet {
        applier_id: ComponentId,
    },
    ClearChangeSet {
        applier_id: ComponentId,
    },

    // ---------------------------
    // Source control (git)
    // ---------------------------
    RefreshSourceControl {
        sc_id: ComponentId,
    },
    ToggleSourceControlSelect {
        sc_id: ComponentId,
        path: String,
    },
    StageSelected {
        sc_id: ComponentId,
    },
    UnstageSelected {
        sc_id: ComponentId,
    },
    StageAll {
        sc_id: ComponentId,
    },
    UnstageAll {
        sc_id: ComponentId,
    },

    StagePath {
        sc_id: ComponentId,
        path: String,
    },

    UnstagePath {
        sc_id: ComponentId,
        path: String,
    },

    DiscardPath {
        sc_id: ComponentId,
        path: String,
        untracked: bool,
    },

    DiscardAllUnstaged {
        sc_id: ComponentId,
    },
    SetSourceControlBranch {
        sc_id: ComponentId,
        branch: String,
    },
    SetSourceControlRemote {
        sc_id: ComponentId,
        remote: String,
    },
    RefreshSourceControlBranchRemoteLists {
        sc_id: ComponentId,
    },
    CheckoutBranch {
        sc_id: ComponentId,
        create_if_missing: bool,
    },
    FetchRemote {
        sc_id: ComponentId,
    },
    PullRemote {
        sc_id: ComponentId,
    },
    SetCommitMessage {
        sc_id: ComponentId,
        msg: String,
    },
    CommitStaged {
        sc_id: ComponentId,
    },

    CommitAndPush {
        sc_id: ComponentId,
    },

    // ---------------------------
    // Layout / components
    // ---------------------------
    AddComponent {
        kind: ComponentKind,
    },
    FocusFileViewer(ComponentId),
    CloseComponent(ComponentId),
    ToggleLock(ComponentId),

    ResetLayout,

    // ---------------------------
    // Canvases
    // ---------------------------
    CanvasSelect { index: usize },
    CanvasAdd,
    CanvasRename { index: usize, name: String },
    CanvasDelete { index: usize },

    SaveWorkspace {
        canvas_size: [f32; 2],
        viewport_outer_pos: Option<[f32; 2]>,
        viewport_inner_size: Option<[f32; 2]>,
        pixels_per_point: f32,
    },
    LoadWorkspace,

    ToggleCommandPalette,

    TreeDeletePath { path: String },
    TreeRenamePath { from: String, to: String },
    TreeCreateFile { path: String },
    TreeCreateFolder { path: String },

    None,
}
