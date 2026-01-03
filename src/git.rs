use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

pub fn ensure_git_installed() -> Result<()> {
    let out = Command::new("git").arg("--version").output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        _ => bail!("git not found or not runnable. Install git and ensure it's in PATH."),
    }
}

pub fn ensure_git_repo(repo: &Path) -> Result<()> {
    let _ = run_git(repo, &["rev-parse", "--is-inside-work-tree"])
        .with_context(|| format!("{:?} does not appear to be a git repo", repo))?;
    Ok(())
}

pub fn run_git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git -C {:?} {}", repo, args.join(" ")))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(out.stdout)
}

pub fn run_git_allow_fail(repo: &Path, args: &[&str]) -> Result<(i32, Vec<u8>, Vec<u8>)> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git -C {:?} {}", repo, args.join(" ")))?;

    let code = out.status.code().unwrap_or(1);
    Ok((code, out.stdout, out.stderr))
}

/// Returns raw bytes of file content at `spec` where spec is like "<ref>:<path>".
/// This fails if file doesn't exist at that ref.
pub fn show_file_at(repo: &Path, spec: &str) -> Result<Vec<u8>> {
    run_git(repo, &["show", spec])
}

/// Returns history lines for a file at repo-relative `path`.
/// Output format is: "<hash>\x1f<date>\x1f<subject>\n"
pub fn file_history(repo: &Path, path: &str, max: usize) -> Result<Vec<u8>> {
    let max_s = max.to_string();
    run_git(
        repo,
        &[
            "log",
            "--date=short",
            "--pretty=format:%H%x1f%ad%x1f%s",
            "-n",
            &max_s,
            "--",
            path,
        ],
    )
}

/// Diff a single file between two refs/commits (e.g. "HEAD", "main", or a full hash).
/// Uses: `git diff <from>..<to> -- <path>`
/// Returns stdout bytes (unified diff). If there are no changes, stdout may be empty.
pub fn diff_file_between(repo: &Path, from_ref: &str, to_ref: &str, path: &str) -> Result<Vec<u8>> {
    let range = format!("{from_ref}..{to_ref}");

    // Use the lower-level runner so we can pass the constructed range as &str temporarily.
    // We build owned Strings to keep lifetimes simple.
    let args_owned: Vec<String> = vec![
        "diff".to_string(),
        "--no-color".to_string(),
        range,
        "--".to_string(),
        path.to_string(),
    ];

    let args_refs: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();
    run_git(repo, &args_refs)
}
