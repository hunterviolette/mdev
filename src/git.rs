// src/git.rs  (only the Context export section needs replacing)
use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{BufWriter, Write};
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

// -----------------------------------------------------------------------------
// NEW: Git ref dropdown helpers
// -----------------------------------------------------------------------------

fn split_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Returns refs suitable for a dropdown:
/// - "HEAD"
/// - local branches (e.g. "main")
/// - remote branches (e.g. "origin/main", "upstream/dev")
///
/// NOTE: filters out "*/HEAD" symbolic refs like "origin/HEAD".
pub fn list_git_refs_for_dropdown(repo: &Path) -> Result<Vec<String>> {
    ensure_git_repo(repo)?;

    // Local branches (short)
    let locals = run_git(repo, &["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .context("listing local branches failed")?;
    let mut local_refs = split_lines(&locals);

    // Remote branches (short, like origin/main)
    let remotes = run_git(
        repo,
        &["for-each-ref", "--format=%(refname:short)", "refs/remotes"],
    )
    .context("listing remote branches failed")?;
    let mut remote_refs = split_lines(&remotes);
    remote_refs.retain(|r| !r.ends_with("/HEAD"));

    // Merge + de-dupe
    let mut all = Vec::new();
    all.push("HEAD".to_string());
    all.append(&mut local_refs);
    all.append(&mut remote_refs);

    all.sort();
    all.dedup();

    // Keep HEAD at top if present
    if let Some(pos) = all.iter().position(|s| s == "HEAD") {
        all.remove(pos);
    }
    all.insert(0, "HEAD".to_string());

    Ok(all)
}

// -----------------------------------------------------------------------------
// NEW: Working tree read/write (for file viewer + agents)
// -----------------------------------------------------------------------------

fn safe_join_repo_path(repo: &Path, rel_path: &str) -> Result<PathBuf> {
    // Normalize forward slashes -> platform path
    let rel = rel_path.trim_start_matches("./").replace('\\', "/");
    let p = repo.join(Path::new(&rel));

    // Minimal safety: prevent obvious traversal
    // (this is intentionally simple; you can harden further if you want)
    if rel.contains("..") {
        bail!("refusing to access path with '..': {}", rel_path);
    }

    Ok(p)
}

pub fn read_worktree_file(repo: &Path, rel_path: &str) -> Result<Vec<u8>> {
    let p = safe_join_repo_path(repo, rel_path)?;
    std::fs::read(&p).with_context(|| format!("failed to read {}", p.display()))
}

pub fn write_worktree_file(repo: &Path, rel_path: &str, contents: &[u8]) -> Result<()> {
    let p = safe_join_repo_path(repo, rel_path)?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create dirs for {}", parent.display()))?;
    }
    std::fs::write(&p, contents).with_context(|| format!("failed to write {}", p.display()))
}

// -----------------------------------------------------------------------------
// AI Context export (tree + contents) - kept in git.rs per request
// -----------------------------------------------------------------------------

pub struct ContextExportOptions<'a> {
    pub git_ref: &'a str,
    pub exclude: &'a [regex::Regex],
    pub max_bytes_per_file: usize,
    pub skip_binary: bool,
    pub include_files: Option<&'a [String]>, // NEW
}

/// Exports a single text file containing:
/// 1) list of tracked files (tree)
/// 2) the contents of each tracked file at `git_ref`
///
/// WORKTREE behavior:
/// - If opts.git_ref == WORKTREE_REF, reads file bytes from disk (working tree)
///   instead of `git show`.
///
/// Notes:
/// - Uses `git ls-files -z` to list tracked files.
/// - Applies exclude regex to paths.
/// - Skips binary files by default.
/// - Truncates large files for safety.
pub fn export_repo_context(
    repo: &Path,
    out_path: &Path,
    opts: ContextExportOptions<'_>,
) -> Result<()> {
    // 1) Build file list:
    //    - If include_files is Some => ONLY those files (TreeSelect mode)
    //    - Else => all tracked files (EntireRepo mode)
    let mut files: Vec<String> = if let Some(list) = opts.include_files {
        if list.is_empty() {
            bail!("TreeSelect mode: include_files is empty");
        }
        let mut v: Vec<String> = list.iter().map(|s| normalize_repo_rel(s)).collect();
        v.sort();
        v.dedup();
        v
    } else {
        let raw = run_git(repo, &["ls-files", "-z"]).context("git ls-files failed")?;
        let mut v = split_nul(&raw);
        v.sort();
        v
    };

    // 2) Apply exclude regex patterns (always)
    files.retain(|p| !opts.exclude.iter().any(|rx| rx.is_match(p)));

    // 3) Optional safety: if TreeSelect list contains untracked paths, filter them out.
    if opts.include_files.is_some() {
        let tracked_raw = run_git(repo, &["ls-files", "-z"]).context("git ls-files failed")?;
        let tracked: std::collections::HashSet<String> =
            split_nul(&tracked_raw).into_iter().collect();
        files.retain(|p| tracked.contains(p));
        if files.is_empty() {
            bail!("TreeSelect mode: none of the selected files are tracked (after filtering).");
        }
    }

    let f = File::create(out_path).with_context(|| format!("failed to create {:?}", out_path))?;
    let mut w = BufWriter::new(f);

    writeln!(w, "# REPO CONTEXT")?;
    writeln!(w, "# repo: {:?}", repo)?;
    writeln!(w, "# ref: {}", opts.git_ref)?;
    writeln!(w, "# files: {}", files.len())?;
    writeln!(w)?;

    writeln!(w, "## FILE TREE (tracked)")?;
    for p in &files {
        writeln!(w, "{p}")?;
    }
    writeln!(w)?;

    writeln!(w, "## FILE CONTENTS")?;

    let use_worktree = opts.git_ref == WORKTREE_REF;

    for p in &files {
        writeln!(w)?;
        writeln!(w, "===== FILE: {p} =====")?;

        // ---- read bytes from the right source ----
        let read_res: Result<Vec<u8>> = if use_worktree {
            // Working tree: read from disk
            read_worktree_file(repo, p)
                .with_context(|| format!("failed to read working tree file '{}'", p))
        } else {
            // Git ref: read from git object store
            let spec = format!("{}:{}", opts.git_ref, p);
            show_file_at(repo, &spec).with_context(|| format!("failed to git-show '{}'", spec))
        };

        match read_res {
            Ok(blob) => {
                if opts.skip_binary && is_binary(&blob) {
                    writeln!(w, "[[SKIPPED binary file]]")?;
                    continue;
                }

                if opts.max_bytes_per_file > 0 && blob.len() > opts.max_bytes_per_file {
                    writeln!(
                        w,
                        "[[TRUNCATED {} bytes -> {} bytes]]",
                        blob.len(),
                        opts.max_bytes_per_file
                    )?;
                    w.write_all(&blob[..opts.max_bytes_per_file])?;
                    if !blob[..opts.max_bytes_per_file].ends_with(b"\n") {
                        writeln!(w)?;
                    }
                } else {
                    w.write_all(&blob)?;
                    if !blob.ends_with(b"\n") {
                        writeln!(w)?;
                    }
                }
            }
            Err(e) => {
                if use_worktree {
                    writeln!(w, "[[ERROR reading working tree file {p}: {:#}]]", e)?;
                } else {
                    let spec = format!("{}:{}", opts.git_ref, p);
                    writeln!(w, "[[ERROR reading {spec}: {:#}]]", e)?;
                }
            }
        }
    }

    w.flush()?;
    Ok(())
}

// --- helpers (keep your existing ones; included here for completeness) ---

fn normalize_repo_rel(s: &str) -> String {
    let mut out = s.replace('\\', "/");
    while let Some(rest) = out.strip_prefix("./") {
        out = rest.to_string();
    }
    while let Some(rest) = out.strip_prefix('/') {
        out = rest.to_string();
    }
    out
}

fn is_binary(blob: &[u8]) -> bool {
    blob.iter().any(|&b| b == 0)
}

fn split_nul(buf: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (i, &b) in buf.iter().enumerate() {
        if b == 0 {
            if i > start {
                out.push(String::from_utf8_lossy(&buf[start..i]).to_string());
            }
            start = i + 1;
        }
    }
    out
}
