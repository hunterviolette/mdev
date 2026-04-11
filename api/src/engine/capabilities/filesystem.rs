use std::{fs, io::ErrorKind, path::{Component, Path, PathBuf}};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FilesystemTarget {
    pub repo_ref: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WriteFileRequest {
    pub repo_ref: String,
    pub path: String,
    pub contents: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateFileRequest {
    pub repo_ref: String,
    pub path: String,
    #[serde(default)]
    pub contents: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateFolderRequest {
    pub repo_ref: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileStat {
    pub path: String,
    pub kind: String,
    pub bytes: u64,
}

pub fn read_text_file(repo_ref: &str, path: &str) -> Result<String> {
    let full = resolve_workspace_path(repo_ref, path)?;
    let bytes = fs::read(&full).with_context(|| format!("failed to read {}", full.display()))?;
    String::from_utf8(bytes).with_context(|| format!("{} is not valid UTF-8 text", path))
}

pub fn write_text_file(repo_ref: &str, path: &str, contents: &str) -> Result<FileStat> {
    let full = resolve_workspace_path(repo_ref, path)?;
    let parent = full.parent().ok_or_else(|| anyhow!("path has no parent: {}", full.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create parent dir {}", parent.display()))?;
    fs::write(&full, contents.as_bytes()).with_context(|| format!("failed to write {}", full.display()))?;
    Ok(stat_for_path(repo_ref, path, &full)?)
}

pub fn create_file(repo_ref: &str, path: &str, contents: &str) -> Result<FileStat> {
    let full = resolve_workspace_path(repo_ref, path)?;
    if full.exists() {
        bail!("path already exists: {}", path);
    }
    let parent = full.parent().ok_or_else(|| anyhow!("path has no parent: {}", full.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create parent dir {}", parent.display()))?;
    fs::write(&full, contents.as_bytes()).with_context(|| format!("failed to create file {}", full.display()))?;
    Ok(stat_for_path(repo_ref, path, &full)?)
}

pub fn create_dir(repo_ref: &str, path: &str) -> Result<FileStat> {
    let full = resolve_workspace_path(repo_ref, path)?;
    if full.exists() {
        bail!("path already exists: {}", path);
    }
    fs::create_dir_all(&full).with_context(|| format!("failed to create directory {}", full.display()))?;
    Ok(stat_for_path(repo_ref, path, &full)?)
}

pub fn delete_path(repo_ref: &str, path: &str) -> Result<()> {
    let normalized = normalize_rel_path(path)?;
    if normalized.is_empty() {
        bail!("refusing to delete workspace root");
    }
    let full = resolve_workspace_path(repo_ref, &normalized)?;
    let metadata = fs::metadata(&full).with_context(|| format!("failed to stat {}", full.display()))?;
    if metadata.is_dir() {
        fs::remove_dir_all(&full).with_context(|| format!("failed to delete directory {}", full.display()))?;
    } else {
        fs::remove_file(&full).with_context(|| format!("failed to delete file {}", full.display()))?;
    }
    Ok(())
}

pub fn normalize_rel_path(path: &str) -> Result<String> {
    let trimmed = path.trim().replace('\\', "/");
    let mut out = PathBuf::new();
    for component in Path::new(&trimmed).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => out.push(value),
            Component::ParentDir => bail!("path may not escape workspace root: {}", path),
            Component::RootDir | Component::Prefix(_) => bail!("absolute paths are not allowed: {}", path),
        }
    }
    Ok(out.to_string_lossy().replace('\\', "/"))
}

fn resolve_workspace_path(repo_ref: &str, path: &str) -> Result<PathBuf> {
    let repo_root = PathBuf::from(repo_ref);
    if repo_root.as_os_str().is_empty() {
        bail!("repo_ref is required");
    }
    let normalized = normalize_rel_path(path)?;
    if normalized.is_empty() {
        bail!("path is required");
    }
    let full = repo_root.join(&normalized);
    ensure_within_root(&repo_root, &full)?;
    Ok(full)
}

fn ensure_within_root(root: &Path, full: &Path) -> Result<()> {
    let root_canon = fs::canonicalize(root).or_else(|err| {
        if err.kind() == ErrorKind::NotFound {
            bail!("workspace root does not exist: {}", root.display())
        } else {
            Err(err).with_context(|| format!("failed to resolve workspace root {}", root.display()))
        }
    })?;

    let existing_ancestor = full
        .ancestors()
        .find(|candidate| candidate.exists())
        .ok_or_else(|| anyhow!("no existing ancestor for {}", full.display()))?;
    let ancestor_canon = fs::canonicalize(existing_ancestor)
        .with_context(|| format!("failed to resolve ancestor {}", existing_ancestor.display()))?;

    if !ancestor_canon.starts_with(&root_canon) {
        bail!("path escapes workspace root: {}", full.display());
    }
    Ok(())
}

fn stat_for_path(repo_ref: &str, rel_path: &str, full: &Path) -> Result<FileStat> {
    let metadata = fs::metadata(full).with_context(|| format!("failed to stat {}", full.display()))?;
    let path = normalize_rel_path(rel_path)?;
    Ok(FileStat {
        path,
        kind: if metadata.is_dir() { "dir".to_string() } else { "file".to_string() },
        bytes: if metadata.is_dir() { 0 } else { metadata.len() },
    })
}
