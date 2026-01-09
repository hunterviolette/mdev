
use anyhow::Result;
use std::path::PathBuf;

use crate::app::actions::TerminalShell;

/// Result of running a command through the platform layer.
#[derive(Clone, Debug, Default)]
pub struct CommandOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// All OS / environment specific behavior belongs here.
///
/// - Native builds: dialogs, filesystem app-data, spawning processes
/// - Web builds (wasm): use browser APIs (or disable unsupported functionality)
pub trait Platform: Send + Sync {
    // ---------------------------
    // File dialogs
    // ---------------------------

    /// Pick a folder (e.g., select git repo).
    fn pick_folder(&self, title: &str) -> Option<PathBuf>;

    /// Save a file (e.g., export context).
    fn save_file(&self, title: &str, default_name: &str) -> Option<PathBuf>;

    // ---------------------------
    // App data paths
    // ---------------------------

    /// Returns an app-specific appdata dir, e.g.:
    /// - Windows: %APPDATA%/<org>/<app>
    /// - macOS: ~/Library/Application Support/<app>
    /// - Linux: ~/.local/share/<app>
    fn app_data_dir(&self, app_name: &str) -> Result<PathBuf>;

    // ---------------------------
    // Process / shell execution
    // ---------------------------

    /// Run a command with a selected shell and optional cwd.
    fn run_shell_command(
        &self,
        shell: TerminalShell,
        cmd: &str,
        cwd: Option<PathBuf>,
    ) -> Result<CommandOutput>;
}

pub mod native;
pub mod web;
