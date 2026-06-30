use std::{collections::hash_map::DefaultHasher, fs, hash::{Hash, Hasher}, path::{Path, PathBuf}, process::{Command, Stdio}, io::Write};

use anyhow::{anyhow, Context, Result};

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

pub fn generate_patch(repo_path: &Path, patch_path: &Path) -> Result<()> {
    let patch_text = generate_patch_text(repo_path)?;
    if let Some(parent) = patch_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(patch_path, patch_text)?;
    Ok(())
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

pub fn apply_patch(repo_path: &Path, patch_path: &Path) -> Result<()> {
    let patch_text = fs::read_to_string(patch_path)
        .with_context(|| format!("failed to read patch {}", patch_path.display()))?;
    apply_patch_text(repo_path, &patch_text)
        .with_context(|| format!("failed to apply patch {}", patch_path.display()))
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
