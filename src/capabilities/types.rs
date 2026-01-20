use std::path::PathBuf;

use crate::app::actions::TerminalShell;
use crate::model::AnalysisResult;

#[derive(Clone, Debug)]
pub enum FileSource {
    /// Read from `git show <ref>:<path>`
    GitRef(String),
    /// Read from disk (working tree)
    Worktree,
}

#[derive(Clone, Debug)]
pub struct ContextExportReq {
    pub repo: PathBuf,
    pub out_path: PathBuf,
    pub git_ref: String,            // may be WORKTREE
    pub exclude_regex: Vec<String>, // raw patterns, compiled inside broker
    pub max_bytes_per_file: usize,
    pub skip_binary: bool,
    pub include_files: Option<Vec<String>>, // None => entire repo
}

// -----------------------------------------------------------------
// Source control (git) capability models
// -----------------------------------------------------------------
#[derive(Clone, Debug)]
pub struct GitStatusEntry {
    pub path: String,
    pub index_status: String,
    pub worktree_status: String,
    pub staged: bool,
    pub untracked: bool,
}

#[derive(Clone, Debug)]
pub struct GitStatusResult {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<GitStatusEntry>,
}

#[derive(Clone, Debug)]
pub enum CapabilityRequest {
    EnsureGitRepo { repo: PathBuf },
    ListGitRefs { repo: PathBuf },

    AnalyzeRepo {
        repo: PathBuf,
        git_ref: String,            // may be WORKTREE
        exclude_regex: Vec<String>, // raw patterns
        max_exts: usize,
    },

    ReadFile {
        repo: PathBuf,
        path: String, // repo-relative
        source: FileSource,
    },

    WriteWorktreeFile {
        repo: PathBuf,
        path: String, // repo-relative
        contents: Vec<u8>,
    },

    DeleteWorktreePath {
        repo: PathBuf,
        path: String, // repo-relative
    },

    MoveWorktreePath {
        repo: PathBuf,
        from: String,
        to: String,
    },

    ApplyGitPatch {
        repo: PathBuf,
        patch: String,
    },

    FileHistory {
        repo: PathBuf,
        path: String,
        max: usize,
    },

    DiffFileBetween {
        repo: PathBuf,
        from_ref: String,
        to_ref: String,
        path: String,
    },

    ExportContext(ContextExportReq),

    // -----------------------------------------------------------------
    // Source control (git) - brokered capabilities
    // -----------------------------------------------------------------
    GitStatus { repo: PathBuf },

    GitStagePaths { repo: PathBuf, paths: Vec<String> },
    GitUnstagePaths { repo: PathBuf, paths: Vec<String> },
    GitStageAll { repo: PathBuf },
    GitUnstageAll { repo: PathBuf },

    GitCurrentBranch { repo: PathBuf },
    GitListLocalBranches { repo: PathBuf },
    GitListRemotes { repo: PathBuf },

    GitCheckoutBranch {
        repo: PathBuf,
        branch: String,
        create_if_missing: bool,
    },

    GitFetch { repo: PathBuf, remote: Option<String> },
    GitPull {
        repo: PathBuf,
        remote: Option<String>,
        branch: Option<String>,
    },

    GitCommit {
        repo: PathBuf,
        message: String,
        branch: Option<String>,
    },

    RunShellCommand {
        shell: TerminalShell,
        cmd: String,
        cwd: Option<PathBuf>,
    },
}

#[derive(Clone, Debug)]
pub enum CapabilityResponse {
    Unit,

    GitRefs(Vec<String>),

    Analysis(AnalysisResult),

    Bytes(Vec<u8>),

    ShellOutput {
        code: i32,
        stdout: String,
        stderr: String,
    },

    Text(String),

    GitStatus(GitStatusResult),
    GitBranch(String),
    GitBranches(Vec<String>),
    GitRemotes(Vec<String>),
}
