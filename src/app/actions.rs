// src/app/actions.rs

use crate::app::state::FileViewAt;

#[derive(Clone, Copy, Debug)]
pub enum ExpandCmd {
    ExpandAll,
    CollapseAll,
}

pub type ComponentId = u64;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub enum ComponentKind {
    Tree,
    FileViewer,
    Summary,
    Terminal,
    ContextExporter,
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
    // File viewer actions
    // ---------------------------
    SelectCommit {
        viewer_id: ComponentId,
        sel: Option<String>,
    },
    RefreshFile {
        viewer_id: ComponentId,
    },

    /// Set a viewer's "View at" mode (the enum lives in state.rs).
    /// - FollowTopBar: uses the global top bar ref
    /// - WorkingTree: uses disk
    /// - Commit: (generally set automatically when selecting a commit)
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
    SetTerminalShell {
        terminal_id: ComponentId,
        shell: TerminalShell,
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
    ContextSetMaxBytes {
        exporter_id: ComponentId,
        max: usize,
    },
    ContextToggleSkipBinary {
        exporter_id: ComponentId,
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

    SaveWorkspace {
        canvas_size: [f32; 2],
        viewport_outer_pos: Option<[f32; 2]>,
        viewport_inner_size: Option<[f32; 2]>,
    },
    LoadWorkspace,

    ToggleCommandPalette,

    None,
}
