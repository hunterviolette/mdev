use anyhow::{bail, Result};
use std::path::PathBuf;

use crate::app::actions::TerminalShell;

use super::{CommandOutput, Platform};

/// Future wasm32 implementation.
///
/// In a browser you can't:
/// - access arbitrary local folders
/// - spawn shell processes
/// - write to arbitrary disk paths
///
/// So this is intentionally a stub to keep foundations clean.
#[derive(Clone, Debug, Default)]
pub struct WebPlatform;

impl WebPlatform {
    pub fn new() -> Self {
        Self
    }
}

impl Platform for WebPlatform {
    fn pick_folder(&self, _title: &str) -> Option<PathBuf> {
        None
    }

    fn save_file(&self, _title: &str, _default_name: &str) -> Option<PathBuf> {
        None
    }

    fn app_data_dir(&self, _app_name: &str) -> Result<PathBuf> {
        bail!("app_data_dir is not available on web targets")
    }

    fn run_shell_command(
        &self,
        _shell: TerminalShell,
        _cmd: &str,
        _cwd: Option<PathBuf>,
    ) -> Result<CommandOutput> {
        bail!("run_shell_command is not available on web targets")
    }
}
