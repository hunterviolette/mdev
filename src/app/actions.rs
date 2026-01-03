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
    Terminal, // NEW
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
    PickRepo,
    RunAnalysis,
    ExpandAll,
    CollapseAll,

    // Tree -> open file in ACTIVE file viewer
    OpenFile(String),

    // File viewer instance actions
    SelectCommit {
        viewer_id: ComponentId,
        sel: Option<String>,
    },
    RefreshFile {
        viewer_id: ComponentId,
    },

    // Diff actions (if you already have these, keep them)
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

    // Terminal actions (NEW)
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

    // Command palette + dynamic layout
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
