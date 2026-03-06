
use anyhow::Result;
use std::path::PathBuf;

use crate::app::actions::TerminalShell;

#[derive(Clone, Debug, Default)]
pub struct CommandOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait Platform: Send + Sync {

    fn pick_folder(&self, title: &str) -> Option<PathBuf>;

    fn save_file(&self, title: &str, default_name: &str) -> Option<PathBuf>;


    fn app_data_dir(&self, app_name: &str) -> Result<PathBuf>;


    fn run_shell_command(
        &self,
        shell: TerminalShell,
        cmd: &str,
        cwd: Option<PathBuf>,
    ) -> Result<CommandOutput>;
}

pub mod native;
pub mod web;
