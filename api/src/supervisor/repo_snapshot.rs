use std::{fs, path::{Path, PathBuf}};

use anyhow::{anyhow, Context, Result};
use crate::engine::capabilities::context_export::is_gitignored;
use uuid::Uuid;


#[derive(Debug, Clone)]
pub struct SupervisorWorkspace {
    pub root: PathBuf,
    pub snapshot: PathBuf,
    pub integration: PathBuf,

    pub patches: PathBuf,

}

pub fn workspace_for(root_repo_path: &str, supervisor_id: Uuid) -> Result<SupervisorWorkspace> {
    let root_repo = PathBuf::from(root_repo_path);
    let root = root_repo.join(".mdev").join(supervisor_id.to_string());
    Ok(SupervisorWorkspace {
        snapshot: root.join("snapshot"),
        integration: root.join("integration"),

        patches: root.join("patches"),

        root,
    })
}





pub fn materialize_work_unit_workspace(
    root_repo_path: &str,
    supervisor_id: Uuid,
    workspace_id: Uuid,
) -> Result<PathBuf> {
    let workspace = workspace_for(root_repo_path, supervisor_id)?;
    fs::create_dir_all(&workspace.root)?;
    let workspace_path = workspace.root.join(workspace_id.to_string());
    copy_repo_tree(
        Path::new(root_repo_path),
        &workspace_path,
        Some(&workspace.root),
    )?;
    tracing::info!(
        supervisor_id = %supervisor_id,
        root_repo_path = %root_repo_path,
        workspace_id = %workspace_id,
        workspace_path = %workspace_path.display(),
        "materialized supervisor work unit workspace from current worktree"
    );
    Ok(workspace_path)
}

pub fn delete_supervisor_workspace_path(root_repo_path: &str, supervisor_id: Uuid, workspace_path: &str) -> Result<bool> {
    let workspace_path = workspace_path.trim();
    if workspace_path.is_empty() {
        return Ok(false);
    }

    let workspace = workspace_for(root_repo_path, supervisor_id)?;
    let candidate = PathBuf::from(workspace_path);
    let root_repo = fs::canonicalize(root_repo_path)
        .with_context(|| format!("failed to canonicalize root repo {}", root_repo_path))?;
    let supervisor_root = canonicalize_existing_or_parent(&workspace.root)?;
    let candidate_canonical = canonicalize_existing_or_parent(&candidate)?;

    if candidate_canonical == root_repo {
        return Err(anyhow!("refusing to delete root repo as supervisor workspace"));
    }
    if candidate_canonical == supervisor_root {
        return Err(anyhow!("refusing to delete supervisor workspace root"));
    }
    if !candidate_canonical.starts_with(&supervisor_root) {
        return Err(anyhow!("refusing to delete path outside supervisor-owned workspaces: {}", candidate.display()));
    }

    if candidate.exists() {
        fs::remove_dir_all(&candidate).or_else(|_| fs::remove_file(&candidate))
            .with_context(|| format!("failed to delete supervisor workspace {}", candidate.display()))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn canonicalize_existing_or_parent(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path).with_context(|| format!("failed to canonicalize {}", path.display()));
    }

    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("path has no parent: {}", path.display()))?;
    let parent = canonicalize_existing_or_parent(parent)?;
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow!("path has no final component: {}", path.display()))?;

    Ok(parent.join(file_name))
}





fn copy_repo_tree(from: &Path, to: &Path, skip_root: Option<&Path>) -> Result<()> {
    copy_tree_inner(from, to, Some(from), skip_root)
}

fn copy_tree_inner(from: &Path, to: &Path, gitignore_root: Option<&Path>, skip_root: Option<&Path>) -> Result<()> {
    if !from.is_dir() {
        return Err(anyhow!("{} is not a directory", from.display()));
    }
    if to.exists() {
        fs::remove_dir_all(to).with_context(|| format!("failed to remove {}", to.display()))?;
    }
    fs::create_dir_all(to).with_context(|| format!("failed to create {}", to.display()))?;
    tracing::info!(from = %from.display(), to = %to.display(), gitignore_filter = gitignore_root.is_some(), "copying supervisor tree");
    copy_dir_contents(from, to, gitignore_root, skip_root)?;
    tracing::info!(from = %from.display(), to = %to.display(), "copied supervisor tree");
    Ok(())
}

fn copy_dir_contents(from: &Path, to: &Path, gitignore_root: Option<&Path>, skip_root: Option<&Path>) -> Result<()> {
    for entry in fs::read_dir(from).with_context(|| format!("failed to read {}", from.display()))? {
        let entry = entry?;
        let src = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str == ".git" {
            tracing::debug!(path = %src.display(), "skipping git metadata during supervisor tree copy");
            continue;
        }

        if name_str == ".mdev" {
            tracing::debug!(path = %src.display(), "skipping mdev metadata during supervisor tree copy");
            continue;
        }

        if skip_root.is_some_and(|skip| path_is_or_under(&src, skip)) {
            tracing::info!(path = %src.display(), "skipping active supervisor workspace during snapshot copy");
            continue;
        }

        if let Some(repo) = gitignore_root {
            let rel = src
                .strip_prefix(repo)
                .unwrap_or(&src)
                .to_string_lossy()
                .replace('\\', "/");
            if is_gitignored(repo, &rel)? {
                tracing::debug!(path = %rel, "skipping gitignored path during supervisor snapshot copy");
                continue;
            }
        }

        let dst = to.join(&name);
        let meta = entry.metadata()?;
        if meta.is_dir() {
            fs::create_dir_all(&dst)?;
            copy_dir_contents(&src, &dst, gitignore_root, skip_root)?;
        } else if meta.is_file() {
            fs::copy(&src, &dst).with_context(|| format!("failed to copy {} to {}", src.display(), dst.display()))?;
        }
    }
    Ok(())
}

fn path_is_or_under(path: &Path, root: &Path) -> bool {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    path == root || path.starts_with(root)
}

