use std::{fs, path::{Path, PathBuf}, process::Command};

use anyhow::{anyhow, Context, Result};

pub fn create_baseline(repo_path: &Path) -> Result<()> {
    run(repo_path, "git", &["init"])?;
    run(repo_path, "git", &["config", "user.email", "mdev-supervisor@example.invalid"])?;
    run(repo_path, "git", &["config", "user.name", "mdev supervisor"])?;
    run(repo_path, "git", &["add", "-A", "--", ".", ":(exclude).mdev", ":(exclude).mdev/**"])?;
    let _ = run(repo_path, "git", &["commit", "-m", "supervisor baseline"]);
    Ok(())
}

pub fn generate_patch(repo_path: &Path, patch_path: &Path) -> Result<()> {
    run(repo_path, "git", &["add", "-A", "--", ".", ":(exclude).mdev", ":(exclude).mdev/**"])?;

    let output = Command::new("git")
        .arg("diff")
        .arg("--cached")
        .arg("--binary")
        .arg("HEAD")
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("failed to diff {}", repo_path.display()))?;

    if !output.status.success() {
        return Err(anyhow!(String::from_utf8_lossy(&output.stderr).to_string()));
    }

    if let Some(parent) = patch_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(patch_path, output.stdout)?;
    Ok(())
}

pub fn apply_patch(repo_path: &Path, patch_path: &Path) -> Result<()> {
    let output = Command::new("git")
        .arg("apply")
        .arg("--3way")
        .arg(patch_path)
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("failed to apply {}", patch_path.display()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow!(String::from_utf8_lossy(&output.stderr).to_string()))
    }
}

pub fn apply_final_patch_to_root(root_repo_path: &Path, final_patch_path: &Path) -> Result<()> {
    apply_patch(root_repo_path, final_patch_path)
}

pub fn patch_path(patches_dir: &Path, execution_item_id: &str) -> PathBuf {
    patches_dir.join(format!("{}.patch", crate::supervisor::repo_snapshot::sanitize_path_segment(execution_item_id)))
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
