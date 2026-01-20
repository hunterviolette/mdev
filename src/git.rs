// src/git.rs
use anyhow::{bail, Context, Result};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::collections::HashSet;
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

// -----------------------------------------------------------------------------
// Source control (git) capabilities
// -----------------------------------------------------------------------------

fn run_git_text_allow_fail(repo: &Path, args: &[&str]) -> Result<(i32, String, String)> {
    let (code, stdout, stderr) = run_git_allow_fail(repo, args)?;
    Ok((
        code,
        String::from_utf8_lossy(&stdout).to_string(),
        String::from_utf8_lossy(&stderr).to_string(),
    ))
}

pub fn git_list_local_branches(repo: &Path) -> Result<Vec<String>> {
    ensure_git_repo(repo)?;
    let out = run_git(repo, &["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .context("listing local branches failed")?;
    let mut v = split_lines(&out);
    v.sort();
    v.dedup();
    Ok(v)
}

pub fn git_list_remotes(repo: &Path) -> Result<Vec<String>> {
    ensure_git_repo(repo)?;
    let out = run_git(repo, &["remote"]).context("listing remotes failed")?;
    let mut v = split_lines(&out);
    v.sort();
    v.dedup();
    Ok(v)
}

pub fn git_current_branch(repo: &Path) -> Result<String> {
    ensure_git_repo(repo)?;
    let out = run_git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).context("current branch failed")?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

pub fn git_fetch(repo: &Path, remote: Option<&str>) -> Result<String> {
    ensure_git_repo(repo)?;
    let r = remote.unwrap_or("origin");
    let (code, stdout, stderr) = run_git_text_allow_fail(repo, &["fetch", r])?;
    Ok(format!("git fetch {}\n[exit: {}]\n{}{}", r, code, stdout, stderr))
}

pub fn git_pull(repo: &Path, remote: Option<&str>, branch: Option<&str>) -> Result<String> {
    ensure_git_repo(repo)?;
    let r = remote.unwrap_or("origin");
    let args: Vec<&str> = match branch {
        Some(b) if !b.trim().is_empty() => vec!["pull", r, b],
        _ => vec!["pull", r],
    };
    let (code, stdout, stderr) = run_git_text_allow_fail(repo, &args)?;
    Ok(format!("git {}\n[exit: {}]\n{}{}", args.join(" "), code, stdout, stderr))
}

pub fn git_checkout_branch(repo: &Path, branch: &str, create_if_missing: bool) -> Result<String> {
    ensure_git_repo(repo)?;
    let b = branch.trim();
    if b.is_empty() {
        bail!("branch is empty");
    }

    let args: Vec<&str> = if create_if_missing {
        vec!["checkout", "-b", b]
    } else {
        vec!["checkout", b]
    };

    let (code, stdout, stderr) = run_git_text_allow_fail(repo, &args)?;
    Ok(format!("git {}\n[exit: {}]\n{}{}", args.join(" "), code, stdout, stderr))
}

pub fn git_stage_paths(repo: &Path, paths: &[String]) -> Result<()> {
    ensure_git_repo(repo)?;
    if paths.is_empty() {
        return Ok(());
    }

    for p in paths {
        if p.contains("..") {
            bail!("refusing to stage path with '..': {}", p);
        }
    }

    let mut args: Vec<&str> = vec!["add", "--"];
    let owned: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
    let refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
    args.extend(refs);

    let _ = run_git(repo, &args)?;
    Ok(())
}

pub fn git_unstage_paths(repo: &Path, paths: &[String]) -> Result<()> {
    ensure_git_repo(repo)?;
    if paths.is_empty() {
        return Ok(());
    }

    for p in paths {
        if p.contains("..") {
            bail!("refusing to unstage path with '..': {}", p);
        }
    }

    let mut args: Vec<&str> = vec!["restore", "--staged", "--"];
    let owned: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
    let refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
    args.extend(refs);

    let _ = run_git(repo, &args)?;
    Ok(())
}

pub fn git_restore_paths(repo: &Path, paths: &[String]) -> Result<()> {
    ensure_git_repo(repo)?;
    if paths.is_empty() {
        return Ok(());
    }

    for p in paths {
        if p.contains("..") {
            bail!("refusing to restore path with '..': {}", p);
        }
    }

    let mut args: Vec<&str> = vec!["restore", "--worktree", "--"];
    let owned: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
    let refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
    args.extend(refs);

    let _ = run_git(repo, &args)?;
    Ok(())
}

pub fn git_stage_all(repo: &Path) -> Result<()> {
    ensure_git_repo(repo)?;
    let _ = run_git(repo, &["add", "-A"])?;
    Ok(())
}

pub fn git_unstage_all(repo: &Path) -> Result<()> {
    ensure_git_repo(repo)?;
    // `git restore` does not support `-A`. To unstage everything, provide a pathspec.
    // Running from repo root, "." covers the whole worktree.
    let _ = run_git(repo, &["restore", "--staged", "--", "."])?;
    Ok(())
}

pub fn git_commit(repo: &Path, message: &str, branch: Option<&str>) -> Result<String> {
    ensure_git_repo(repo)?;
    let msg = message.trim();
    if msg.is_empty() {
        bail!("commit message is empty");
    }

    let mut log = String::new();

    if let Some(b) = branch {
        let b = b.trim();
        if !b.is_empty() {
            let out = git_checkout_branch(repo, b, false)?;
            log.push_str(&out);
            if !log.ends_with('\n') {
                log.push('\n');
            }
        }
    }

    let (code, stdout, stderr) = run_git_text_allow_fail(repo, &["commit", "-m", msg])?;
    log.push_str(&format!("git commit -m <msg>\n[exit: {}]\n{}{}", code, stdout, stderr));
    Ok(log)
}

pub fn git_status(repo: &Path) -> Result<crate::capabilities::GitStatusResult> {
    use crate::capabilities::{GitStatusEntry, GitStatusResult};

    ensure_git_repo(repo)?;

    let out = run_git(repo, &["status", "--porcelain=v2", "-b", "-z"]).context("git status failed")?;
    let s = String::from_utf8_lossy(&out);
    let parts: Vec<&str> = s.split('\0').collect();

    let mut branch: Option<String> = None;
    let mut upstream: Option<String> = None;
    let mut ahead: u32 = 0;
    let mut behind: u32 = 0;

    let mut files: Vec<GitStatusEntry> = Vec::new();

    for p in parts {
        if p.is_empty() {
            continue;
        }

        if let Some(rest) = p.strip_prefix("# ") {
            if let Some(v) = rest.strip_prefix("branch.head ") {
                let v = v.trim();
                if v != "(detached)" {
                    branch = Some(v.to_string());
                } else {
                    branch = Some("HEAD".to_string());
                }
                continue;
            }
            if let Some(v) = rest.strip_prefix("branch.upstream ") {
                upstream = Some(v.trim().to_string());
                continue;
            }
            if let Some(v) = rest.strip_prefix("branch.ab ") {
                let toks: Vec<&str> = v.split_whitespace().collect();
                for t in toks {
                    if let Some(a) = t.strip_prefix('+') {
                        ahead = a.parse::<u32>().unwrap_or(0);
                    } else if let Some(b) = t.strip_prefix('-') {
                        behind = b.parse::<u32>().unwrap_or(0);
                    }
                }
                continue;
            }
            continue;
        }

        if let Some(rest) = p.strip_prefix("? ") {
            let path = rest.trim().to_string();
            files.push(GitStatusEntry {
                path,
                index_status: "?".to_string(),
                worktree_status: "?".to_string(),
                staged: false,
                untracked: true,
            });
            continue;
        }

        if p.starts_with("1 ") || p.starts_with("2 ") {
            let mut it = p.split_whitespace();
            let _rec_type = it.next();
            let xy = it.next().unwrap_or("..");
            let x = xy.chars().nth(0).unwrap_or('.');
            let y = xy.chars().nth(1).unwrap_or('.');

            let path = p.rsplit_once(' ').map(|(_, b)| b).unwrap_or("").trim();

            let index_status = x.to_string();
            let worktree_status = y.to_string();
            let staged = x != '.' && x != ' ';
            let untracked = false;

            if !path.is_empty() {
                files.push(GitStatusEntry {
                    path: path.to_string(),
                    index_status,
                    worktree_status,
                    staged,
                    untracked,
                });
            }
            continue;
        }
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut dedup: Vec<GitStatusEntry> = Vec::new();
    for f in files.into_iter().rev() {
        if seen.insert(f.path.clone()) {
            dedup.push(f);
        }
    }
    dedup.reverse();

    Ok(GitStatusResult {
        branch,
        upstream,
        ahead,
        behind,
        files: dedup,
    })
}


/// Returns raw bytes of file content at `spec` where spec is like "<ref>:<path>".
pub fn show_file_at(repo: &Path, spec: &str) -> Result<Vec<u8>> {
    run_git(repo, &["show", spec])
}

/// Returns history lines for a file at repo-relative `path`.
/// If not a git repo, returns empty (so UI still works in plain working trees).
pub fn file_history(repo: &Path, path: &str, max: usize) -> Result<Vec<u8>> {
    if ensure_git_installed().is_err() || ensure_git_repo(repo).is_err() {
        return Ok(Vec::new());
    }

    run_git(
        repo,
        &[
            "log",
            "--no-color",
            "--pretty=format:%H|%ct|%s",
            "-n",
            &max.to_string(),
            "--",
            path,
        ],
    )
}

/// --- FS fallback helpers (for plain working trees) ---

fn normalize_rel_path(p: &Path) -> Option<String> {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_start_matches("./").to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn list_files_recursive_fs(root: &Path) -> Result<Vec<String>> {
    fn skip_dir(name: &str) -> bool {
        matches!(
            name,
            ".git"
                | "node_modules"
                | "target"
                | ".next"
                | ".turbo"
                | ".idea"
                | ".vscode"
                | ".DS_Store"
        )
    }

    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };

        for ent in rd {
            let ent = match ent {
                Ok(e) => e,
                Err(_) => continue,
            };

            let ft = match ent.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };

            // avoid symlink cycles / oddities
            if ft.is_symlink() {
                continue;
            }

            let path = ent.path();

            if ft.is_dir() {
                let name = ent.file_name().to_string_lossy().to_string();
                if skip_dir(&name) {
                    continue;
                }
                stack.push(path);
            } else if ft.is_file() {
                if let Ok(rel) = path.strip_prefix(root) {
                    if let Some(s) = normalize_rel_path(rel) {
                        out.push(s);
                    }
                }
            }
        }
    }

    out.sort();
    out.dedup();
    Ok(out)
}

/// List all repo-relative file paths from the working tree:
/// - If git repo: tracked + untracked (respecting .gitignore)
/// - Otherwise: filesystem walk fallback
pub fn list_worktree_files(repo: &Path) -> Result<Vec<String>> {
    if ensure_git_installed().is_ok() && ensure_git_repo(repo).is_ok() {
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

        out.sort();
        out.dedup();
        return Ok(out);
    }

    // plain folder fallback
    list_files_recursive_fs(repo)
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
/// NOTE: Diffs require git (and a git repo). This will error in plain working trees (intended).
pub fn diff_file_between(
    repo: &Path,
    from_ref: &str,
    to_ref: &str,
    path: &str,
) -> Result<Vec<u8>> {
    ensure_git_installed()?;
    ensure_git_repo(repo)?;

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
// Minimal FS operations (used by ChangeSet applier)
// -----------------------------------------------------------------------------

pub fn delete_worktree_path(repo: &Path, rel_path: &str) -> Result<()> {
    let p = safe_join_repo_path(repo, rel_path)?;
    if !p.exists() {
        return Ok(());
    }
    let md = std::fs::metadata(&p).with_context(|| format!("failed to stat {}", p.display()))?;
    if md.is_dir() {
        std::fs::remove_dir_all(&p)
            .with_context(|| format!("failed to remove dir {}", p.display()))?;
    } else {
        std::fs::remove_file(&p).with_context(|| format!("failed to remove file {}", p.display()))?;
    }
    Ok(())
}

pub fn move_worktree_path(repo: &Path, from: &str, to: &str) -> Result<()> {
    let src = safe_join_repo_path(repo, from)?;
    let dst = safe_join_repo_path(repo, to)?;
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create dirs for {}", parent.display()))?;
    }
    std::fs::rename(&src, &dst)
        .with_context(|| format!("failed to rename {} -> {}", src.display(), dst.display()))?;
    Ok(())
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

pub fn export_repo_context(
    repo: &Path,
    out_path: &Path,
    opts: ContextExportOptions<'_>,
) -> Result<()> {
    // Only require git when exporting a git ref.
    if opts.git_ref != WORKTREE_REF {
        ensure_git_installed()?;
        ensure_git_repo(repo)?;
    }

    let files: Vec<String> = if let Some(sel) = opts.include_files {
        sel.to_vec()
    } else if opts.git_ref == WORKTREE_REF {
        list_worktree_files(repo)?
    } else {
        let bytes = run_git(repo, &["ls-tree", "-r", "--name-only", opts.git_ref])?;
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
    std::fs::write(out_path, out)
        .with_context(|| format!("failed to write {}", out_path.display()))?;
    Ok(())
}

// -----------------------------------------------------------------------------
// Minimal extensions for patch-apply + file ops (DO NOT remove anything above)
// -----------------------------------------------------------------------------

pub fn run_git_with_input(repo: &Path, args: &[&str], stdin_bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;

    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn git -C {:?} {}", repo, args.join(" ")))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_bytes)
            .with_context(|| "failed writing stdin to git")?;
    }

    let out = child.wait_with_output().with_context(|| "failed to wait for git")?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(out.stdout)
}

pub fn apply_git_patch(repo: &Path, patch_text: &str) -> Result<()> {
    // Normalize to LF and ensure trailing newline. Do NOT rewrite hunks.
    let mut patch = patch_text.replace("\r\n", "\n");
    if !patch.ends_with('\n') {
        patch.push('\n');
    }

    // Basic stats...
    let len = patch.len();
    let nl_count = patch.as_bytes().iter().filter(|&&b| b == b'\n').count();
    let has_diff_git = patch.contains("diff --git ");
    let has_unified_hunk = patch.contains("\n@@ -") || patch.starts_with("@@ -");
    let has_cr = patch.as_bytes().iter().any(|&b| b == b'\r');

    let debug_path = repo.join(".describe_repo_last_patch.patch");
    if let Err(e) = std::fs::write(&debug_path, patch.as_bytes()) {
        eprintln!("WARNING: failed to write debug patch file {:?}: {}", debug_path, e);
    }

    match run_git_with_input(repo, &["apply", "--whitespace=nowarn", "-"], patch.as_bytes()) {
        Ok(_) => Ok(()),
        Err(e) => {
            let mut preview = patch.chars().take(200).collect::<String>();
            preview = preview.replace('\n', "\\n");
            preview = preview.replace('\r', "\\r");

            let check = Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["apply", "--check", "--whitespace=nowarn"])
                .arg(&debug_path)
                .output();

            let mut check_msg = String::new();
            if let Ok(o) = check {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let stdout = String::from_utf8_lossy(&o.stdout);
                check_msg.push_str(&format!(
                    "git apply --check exit={:?}\nstdout:\n{}\nstderr:\n{}\n",
                    o.status.code(),
                    stdout,
                    stderr
                ));
            }

            bail!(
                "git apply failed.\n\
                 patch_len={len} nl_count={nl_count} has_diff_git={has_diff_git} has_unified_hunk={has_unified_hunk} has_cr={has_cr}\n\
                 debug_patch_file={}\n\
                 preview='{}'\n\
                 underlying_error={:#}\n\
                 {}",
                debug_path.display(),
                preview,
                e,
                check_msg
            );
        }
    }
}
