use anyhow::{Context, Result};
use directories::ProjectDirs;
use rfd::FileDialog;
use std::path::PathBuf;
use std::process::Command;

use crate::app::actions::TerminalShell;

use super::{CommandOutput, Platform};

#[derive(Clone, Debug, Default)]
pub struct NativePlatform;

impl NativePlatform {
    pub fn new() -> Self {
        Self
    }
}

impl Platform for NativePlatform {
    fn pick_folder(&self, title: &str) -> Option<PathBuf> {
        FileDialog::new().set_title(title).pick_folder()
    }

    fn save_file(&self, title: &str, default_name: &str) -> Option<PathBuf> {
        FileDialog::new()
            .set_title(title)
            .set_file_name(default_name)
            .save_file()
    }

    fn app_data_dir(&self, app_name: &str) -> Result<PathBuf> {
        // org/qualifier can be anything stable for your app. Keep it constant.
        let pd = ProjectDirs::from("com", "RepoAnalyzer", app_name)
            .context("Failed to resolve platform app data directory (ProjectDirs::from)")?;
        Ok(pd.data_dir().to_path_buf())
    }

    fn run_shell_command(
        &self,
        shell: TerminalShell,
        cmd: &str,
        cwd: Option<PathBuf>,
    ) -> Result<CommandOutput> {
        let (program, args): (&str, Vec<String>) = match shell {
            TerminalShell::Auto => {
                if cfg!(windows) {
                    (
                        "powershell",
                        vec!["-NoProfile".into(), "-Command".into(), cmd.into()],
                    )
                } else {
                    ("bash", vec!["-lc".into(), cmd.into()])
                }
            }
            TerminalShell::PowerShell => (
                "powershell",
                vec!["-NoProfile".into(), "-Command".into(), cmd.into()],
            ),
            TerminalShell::Cmd => ("cmd", vec!["/C".into(), cmd.into()]),
            TerminalShell::Bash => ("bash", vec!["-lc".into(), cmd.into()]),
            TerminalShell::Zsh => ("zsh", vec!["-lc".into(), cmd.into()]),
            TerminalShell::Sh => ("sh", vec!["-lc".into(), cmd.into()]),
        };

        let mut c = Command::new(program);
        c.args(args);

        if let Some(dir) = cwd {
            c.current_dir(dir);
        }

        let out = c
            .output()
            .with_context(|| format!("Failed to run shell program: {program}"))?;

        Ok(CommandOutput {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        })
    }
}
