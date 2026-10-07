use std::{collections::HashMap, path::Path, process::Command};

use anyhow::{anyhow, Context, Result};

use crate::engine::capabilities::git::git::{git_diff_stats, git_status, git_untracked_line_stats};

#[derive(Debug, Clone)]
pub struct ChangeFile {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
    pub index_status: String,
    pub worktree_status: String,
    pub untracked: bool,
}

#[derive(Debug, Clone)]
pub struct ChangeStatus {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: Vec<ChangeFile>,
    pub unstaged: Vec<ChangeFile>,
}

impl ChangeStatus {
    pub fn has_staged_changes(&self) -> bool {
        !self.staged.is_empty()
    }

    pub fn has_unstaged_changes(&self) -> bool {
        !self.unstaged.is_empty()
    }

    pub fn has_changes(&self) -> bool {
        self.has_staged_changes() || self.has_unstaged_changes()
    }
}

pub fn change_status(repo_path: &Path) -> Result<ChangeStatus> {
    let status = git_status(repo_path)?;
    let staged_stats = git_diff_stats(repo_path, true).unwrap_or_else(|_| HashMap::new());
    let unstaged_stats = git_diff_stats(repo_path, false).unwrap_or_else(|_| HashMap::new());
    let untracked_paths = status
        .files
        .iter()
        .filter(|file| file.untracked)
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let untracked_stats = git_untracked_line_stats(repo_path, &untracked_paths);
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();

    for file in status.files {
        let staged_counts = staged_stats.get(&file.path).copied().unwrap_or((0, 0));
        let unstaged_counts = if file.untracked {
            untracked_stats.get(&file.path).copied().unwrap_or((0, 0))
        } else {
            unstaged_stats.get(&file.path).copied().unwrap_or((0, 0))
        };

        if file.staged {
            staged.push(ChangeFile {
                path: file.path.clone(),
                additions: staged_counts.0,
                deletions: staged_counts.1,
                index_status: file.index_status.clone(),
                worktree_status: file.worktree_status.clone(),
                untracked: file.untracked,
            });
        }

        if file.untracked || file.worktree_status != "." {
            unstaged.push(ChangeFile {
                path: file.path,
                additions: unstaged_counts.0,
                deletions: unstaged_counts.1,
                index_status: file.index_status,
                worktree_status: file.worktree_status,
                untracked: file.untracked,
            });
        }
    }

    staged.sort_by(|a, b| a.path.cmp(&b.path));
    unstaged.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(ChangeStatus {
        branch: status.branch,
        upstream: status.upstream,
        ahead: status.ahead,
        behind: status.behind,
        staged,
        unstaged,
    })
}

pub fn create_baseline(repo_path: &Path) -> Result<()> {
    run(repo_path, "git", &["init"])?;
    run(repo_path, "git", &["config", "user.email", "mdev-supervisor@example.invalid"])?;
    run(repo_path, "git", &["config", "user.name", "mdev supervisor"])?;
    run(repo_path, "git", &["add", "-A", "--", ".", ":(exclude).mdev", ":(exclude).mdev/**"])?;
    let _ = run(repo_path, "git", &["commit", "-m", "supervisor baseline"]);
    Ok(())
}

fn run(repo_path: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("failed to run {} in {}", program, repo_path.display()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(anyhow!(String::from_utf8_lossy(&output.stderr).to_string()))
    }
}
