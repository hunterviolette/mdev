use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    hash::{Hash, Hasher},
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

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

    pub fn has_changes(&self) -> bool {
        self.has_staged_changes() || !self.unstaged.is_empty()
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

pub fn generate_patch_text(repo_path: &Path) -> Result<String> {
    run(repo_path, "git", &["add", "-A", "--", ".", ":(exclude).mdev", ":(exclude).mdev/**"])?;

    let output = Command::new("git")
        .arg("diff")
        .arg("--cached")
        .arg("--binary")
        .arg("HEAD")
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("failed to diff {}", repo_path.display()))?;

    let unstage_result = run(repo_path, "git", &["reset", "-q", "HEAD", "--", "."]);

    if !output.status.success() {
        let _ = unstage_result;
        return Err(anyhow!(String::from_utf8_lossy(&output.stderr).to_string()));
    }

    unstage_result?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn generate_staged_patch_text(repo_path: &Path) -> Result<String> {
    let output = Command::new("git")
        .arg("diff")
        .arg("--cached")
        .arg("--binary")
        .arg("HEAD")
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("failed to diff staged changes in {}", repo_path.display()))?;

    if !output.status.success() {
        return Err(anyhow!(String::from_utf8_lossy(&output.stderr).to_string()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}




pub fn current_head(repo_path: &Path) -> Result<Option<String>> {
    match run(repo_path, "git", &["rev-parse", "HEAD"]) {
        Ok(value) => Ok(Some(value.trim().to_string()).filter(|value| !value.is_empty())),
        Err(_) => Ok(None),
    }
}

pub fn patch_content_hash(contents: &str) -> String {
    let mut hasher = DefaultHasher::new();
    contents.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub fn apply_patch_text(repo_path: &Path, patch_text: &str) -> Result<()> {
    if patch_text.trim().is_empty() {
        return Ok(());
    }

    run_git_apply_stdin(repo_path, patch_text, &["apply", "--check", "--whitespace=nowarn", "-"])
        .with_context(|| format!("patch check failed in {}", repo_path.display()))?;

    run_git_apply_stdin(repo_path, patch_text, &["apply", "--whitespace=nowarn", "-"])
        .with_context(|| format!("patch apply failed in {}", repo_path.display()))?;

    Ok(())
}

fn run_git_apply_stdin(repo_path: &Path, patch_text: &str, args: &[&str]) -> Result<()> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start git {} in {}", args.join(" "), repo_path.display()))?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(patch_text.as_bytes())?;
    }

    let output = child
        .wait_with_output()
        .with_context(|| format!("failed to run git {} in {}", args.join(" "), repo_path.display()))?;

    if output.status.success() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let message = match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => format!("git {} failed", args.join(" ")),
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{}\n{}", stdout, stderr),
    };
    Err(anyhow!(message))
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
