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

    let unstage_result = run(repo_path, "git", &["reset", "-q", "HEAD", "--", "."]);

    if !output.status.success() {
        return Err(anyhow!(String::from_utf8_lossy(&output.stderr).to_string()));
    }

    unstage_result?;

    if let Some(parent) = patch_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(patch_path, output.stdout)?;
    Ok(())
}

pub fn apply_patch(repo_path: &Path, patch_path: &Path) -> Result<()> {
    let git_dir_output = Command::new("git")
        .arg("rev-parse")
        .arg("--git-dir")
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("failed to resolve git dir for {}", repo_path.display()))?;
    if !git_dir_output.status.success() {
        return Err(anyhow!(String::from_utf8_lossy(&git_dir_output.stderr).to_string()));
    }

    let git_dir_text = String::from_utf8_lossy(&git_dir_output.stdout).trim().to_string();
    let git_dir_path = PathBuf::from(git_dir_text.as_str());
    let index_path = if git_dir_path.is_absolute() {
        git_dir_path
    } else {
        repo_path.join(git_dir_path)
    }.join("index");
    let backup_path = std::env::temp_dir().join(format!("workflow-index-{}.backup", uuid::Uuid::new_v4()));
    std::fs::copy(&index_path, &backup_path)
        .with_context(|| format!("failed to snapshot git index for {}", repo_path.display()))?;

    let output = Command::new("git")
        .arg("apply")
        .arg("--3way")
        .arg(patch_path)
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("failed to apply {}", patch_path.display()))?;

    let restore_result = std::fs::copy(&backup_path, &index_path);
    let _ = std::fs::remove_file(&backup_path);
    restore_result.with_context(|| format!("failed to restore git index after applying patch {}", patch_path.display()))?;

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
