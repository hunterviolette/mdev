// src/git.rs
use anyhow::{bail, Context, Result};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::app::state::WORKTREE_REF;

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

fn split_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Dropdown refs: HEAD first, then local branches, then remotes (excluding */HEAD).
pub fn list_git_refs_for_dropdown(repo: &Path) -> Result<Vec<String>> {
    ensure_git_repo(repo)?;

    let locals = run_git(repo, &["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .context("listing local branches failed")?;
    let mut local_refs = split_lines(&locals);

    let remotes = run_git(
        repo,
        &["for-each-ref", "--format=%(refname:short)", "refs/remotes"],
    )
    .context("listing remote branches failed")?;
    let mut remote_refs = split_lines(&remotes);
    remote_refs.retain(|r| !r.ends_with("/HEAD"));

    let mut all = Vec::new();
    all.push("HEAD".to_string());
    all.append(&mut local_refs);
    all.append(&mut remote_refs);

    // stable cleanup
    all.sort();
    all.dedup();

    // enforce HEAD first
    if let Some(pos) = all.iter().position(|s| s == "HEAD") {
        all.remove(pos);
    }
    all.insert(0, "HEAD".to_string());

    Ok(all)
}

/// Returns raw bytes of file content at `spec` where spec is like "<ref>:<path>".
pub fn show_file_at(repo: &Path, spec: &str) -> Result<Vec<u8>> {
    run_git(repo, &["show", spec])
}

/// Returns history lines for a file at repo-relative `path`.
/// Output format: "<hash>\x1f<date>\x1f<subject>\n"
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

/// List all repo-relative file paths from the working tree:
/// tracked + untracked (respecting .gitignore)
pub fn list_worktree_files(repo: &Path) -> Result<Vec<String>> {
    ensure_git_repo(repo)?;
    let bytes = run_git(
        repo,
        &["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
    )?;

    let mut out = Vec::new();
    let mut start = 0usize;
    for i in 0..bytes.len() {
        if bytes[i] == 0u8 {
            if i > start {
                let s = String::from_utf8_lossy(&bytes[start..i]).to_string();
                if !s.trim().is_empty() {
                    out.push(s);
                }
            }
            start = i + 1;
        }
    }
    if start < bytes.len() {
        let s = String::from_utf8_lossy(&bytes[start..]).to_string();
        if !s.trim().is_empty() {
            out.push(s);
        }
    }
    Ok(out)
}

fn exists_in_ref(repo: &Path, git_ref: &str, path: &str) -> Result<bool> {
    let spec = format!("{git_ref}:{path}");
    let (code, _out, _err) = run_git_allow_fail(repo, &["cat-file", "-e", &spec])?;
    Ok(code == 0)
}

fn exists_in_worktree(repo: &Path, rel_path: &str) -> bool {
    let rel = rel_path.trim_start_matches("./").replace('\\', "/");
    repo.join(Path::new(&rel)).is_file()
}

/// WORKTREE-aware single-file diff.
pub fn diff_file_between(repo: &Path, from_ref: &str, to_ref: &str, path: &str) -> Result<Vec<u8>> {
    // Commit -> WORKTREE
    if to_ref == WORKTREE_REF {
        if from_ref != WORKTREE_REF && exists_in_ref(repo, from_ref, path).unwrap_or(false) {
            let args_owned: Vec<String> = vec![
                "diff".to_string(),
                "--no-color".to_string(),
                from_ref.to_string(),
                "--".to_string(),
                path.to_string(),
            ];
            let args_refs: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();
            return run_git(repo, &args_refs);
        }

        // untracked/added file in worktree
        if exists_in_worktree(repo, path) {
            let abs = repo.join(Path::new(&path.replace('\\', "/")));
            let try_null = {
                let args_owned: Vec<String> = vec![
                    "diff".to_string(),
                    "--no-color".to_string(),
                    "--no-index".to_string(),
                    "--".to_string(),
                    "/dev/null".to_string(),
                    abs.to_string_lossy().to_string(),
                ];
                let args_refs: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();
                run_git(repo, &args_refs)
            };
            if let Ok(bytes) = try_null {
                return Ok(bytes);
            }

            let args_owned2: Vec<String> = vec![
                "diff".to_string(),
                "--no-color".to_string(),
                "--no-index".to_string(),
                "--".to_string(),
                "NUL".to_string(),
                abs.to_string_lossy().to_string(),
            ];
            let args_refs2: Vec<&str> = args_owned2.iter().map(|s| s.as_str()).collect();
            return run_git(repo, &args_refs2);
        }

        return Ok(Vec::new());
    }

    // WORKTREE -> Commit
    if from_ref == WORKTREE_REF {
        if exists_in_ref(repo, to_ref, path).unwrap_or(false) {
            let args_owned: Vec<String> = vec![
                "diff".to_string(),
                "--no-color".to_string(),
                "--reverse".to_string(),
                to_ref.to_string(),
                "--".to_string(),
                path.to_string(),
            ];
            let args_refs: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();
            return run_git(repo, &args_refs);
        }
        return Ok(Vec::new());
    }

    // Normal ref..ref diff
    let range = format!("{from_ref}..{to_ref}");
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

// -----------------------------------------------------------------------------
// Working tree read/write
// -----------------------------------------------------------------------------

fn safe_join_repo_path(repo: &Path, rel_path: &str) -> Result<PathBuf> {
    let rel = rel_path.trim_start_matches("./").replace('\\', "/");
    if rel.contains("..") {
        bail!("refusing to access path with '..': {}", rel_path);
    }
    Ok(repo.join(Path::new(&rel)))
}

pub fn read_worktree_file(repo: &Path, rel_path: &str) -> Result<Vec<u8>> {
    let p = safe_join_repo_path(repo, rel_path)?;
    std::fs::read(&p).with_context(|| format!("failed to read {}", p.display()))
}

pub fn write_worktree_file(repo: &Path, rel_path: &str, bytes: &[u8]) -> Result<()> {
    let p = safe_join_repo_path(repo, rel_path)?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create dirs for {}", parent.display()))?;
    }
    std::fs::write(&p, bytes).with_context(|| format!("failed to write {}", p.display()))
}

// -----------------------------------------------------------------------------
// Context exporter
// -----------------------------------------------------------------------------

pub struct ContextExportOptions<'a> {
    pub git_ref: &'a str,                 // may be WORKTREE
    pub exclude: &'a [Regex],
    pub max_bytes_per_file: usize,
    pub skip_binary: bool,
    pub include_files: Option<&'a [String]>, // None => full repo selection
}

fn is_excluded(ex: &[Regex], path: &str) -> bool {
    ex.iter().any(|r| r.is_match(path))
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    // Simple heuristic: any NUL byte
    bytes.iter().any(|&b| b == 0)
}

fn write_section(out: &mut String, header: &str, bytes: &[u8], limit: usize) {
    out.push_str("\n");
    out.push_str("==== ");
    out.push_str(header);
    out.push_str(" ====\n");

    if bytes.len() > limit {
        out.push_str(&format!(
            "[TRUNCATED] {} bytes (limit {})\n",
            bytes.len(),
            limit
        ));
        out.push_str(&String::from_utf8_lossy(&bytes[..limit]));
        out.push_str("\n");
    } else {
        out.push_str(&String::from_utf8_lossy(bytes));
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
}

pub fn export_repo_context(repo: &Path, out_path: &Path, opts: ContextExportOptions<'_>) -> Result<()> {
    ensure_git_repo(repo)?;

    let files: Vec<String> = if let Some(sel) = opts.include_files {
        sel.to_vec()
    } else if opts.git_ref == WORKTREE_REF {
        list_worktree_files(repo)?
    } else {
        // list files at ref
        let spec = format!("{}", opts.git_ref);
        let bytes = run_git(repo, &["ls-tree", "-r", "--name-only", &spec])?;
        split_lines(&bytes)
    };

    let mut out = String::new();
    out.push_str("## Repo Context Export\n");
    out.push_str(&format!("repo: {}\n", repo.display()));
    out.push_str(&format!("ref: {}\n", opts.git_ref));
    out.push_str(&format!("files: {}\n", files.len()));
    out.push_str("\n");

    for f in files {
        let f_norm = f.replace('\\', "/");
        if is_excluded(opts.exclude, &f_norm) {
            continue;
        }

        let bytes = if opts.git_ref == WORKTREE_REF {
            match read_worktree_file(repo, &f_norm) {
                Ok(b) => b,
                Err(_) => continue,
            }
        } else {
            let spec = format!("{}:{}", opts.git_ref, f_norm);
            match show_file_at(repo, &spec) {
                Ok(b) => b,
                Err(_) => continue,
            }
        };

        if opts.skip_binary && is_probably_binary(&bytes) {
            continue;
        }

        write_section(&mut out, &f_norm, &bytes, opts.max_bytes_per_file);
    }

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create dirs for {}", parent.display()))?;
    }
    std::fs::write(out_path, out).with_context(|| format!("failed to write {}", out_path.display()))?;
    Ok(())
}
