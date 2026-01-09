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
}
