use std::{fs, path::{Path, PathBuf}};

use anyhow::{anyhow, Context, Result};
use crate::engine::capabilities::context_export::is_gitignored;
use uuid::Uuid;

use crate::engine::capabilities::planner::FeaturePlanItem;

#[derive(Debug, Clone)]
pub struct SupervisorWorkspace {
    pub root: PathBuf,
    pub snapshot: PathBuf,
    pub integration: PathBuf,
    pub shards: PathBuf,
    pub patches: PathBuf,
    pub logs: PathBuf,
}

pub fn workspace_for(root_repo_path: &str, supervisor_id: Uuid) -> Result<SupervisorWorkspace> {
    let root_repo = PathBuf::from(root_repo_path);
    let root = root_repo.join(".mdev").join(supervisor_id.to_string());
    Ok(SupervisorWorkspace {
        snapshot: root.join("snapshot"),
        integration: root.join("integration"),
        shards: root.clone(),
        patches: root.join("patches"),
        logs: root.join("logs"),
        root,
    })
}

pub fn create_workspace(root_repo_path: &str, supervisor_id: Uuid, items: &[FeaturePlanItem]) -> Result<SupervisorWorkspace> {
    let workspace = workspace_for(root_repo_path, supervisor_id)?;
    tracing::info!(supervisor_id = %supervisor_id, root_repo_path = %root_repo_path, workspace = %workspace.root.display(), item_count = items.len(), "creating supervisor workspace");
    if workspace.root.exists() {
        tracing::info!(supervisor_id = %supervisor_id, path = %workspace.root.display(), "clearing existing supervisor workspace");
        fs::remove_dir_all(&workspace.root).with_context(|| format!("failed to clear {}", workspace.root.display()))?;
    }
    fs::create_dir_all(&workspace.root)?;
    fs::create_dir_all(&workspace.patches)?;
    fs::create_dir_all(&workspace.logs)?;
    tracing::info!(supervisor_id = %supervisor_id, workspace = %workspace.root.display(), item_count = items.len(), "created supervisor workspace root");
    Ok(workspace)
}

pub fn ensure_snapshot_from_worktree(root_repo_path: &str, supervisor_id: Uuid) -> Result<SupervisorWorkspace> {
    let workspace = workspace_for(root_repo_path, supervisor_id)?;
    if !workspace.snapshot.is_dir() {
        fs::create_dir_all(&workspace.root)?;
        copy_repo_tree(Path::new(root_repo_path), &workspace.snapshot, Some(&workspace.root))?;
        tracing::info!(supervisor_id = %supervisor_id, root_repo_path = %root_repo_path, snapshot = %workspace.snapshot.display(), "created supervisor snapshot from current worktree");
    }
    Ok(workspace)
}

pub fn reset_integration_from_snapshot(root_repo_path: &str, supervisor_id: Uuid) -> Result<SupervisorWorkspace> {
    let workspace = ensure_snapshot_from_worktree(root_repo_path, supervisor_id)?;
    copy_repo_tree(&workspace.snapshot, &workspace.integration, None)?;
    tracing::info!(supervisor_id = %supervisor_id, root_repo_path = %root_repo_path, integration = %workspace.integration.display(), snapshot = %workspace.snapshot.display(), "reset supervisor integration workspace from snapshot");
    Ok(workspace)
}

pub fn create_shard_from_snapshot(root_repo_path: &str, supervisor_id: Uuid, shard_id: Uuid) -> Result<PathBuf> {
    let workspace = ensure_snapshot_from_worktree(root_repo_path, supervisor_id)?;
    let shard = shard_path(&workspace, shard_id);
    copy_repo_tree(&workspace.snapshot, &shard, None)?;
    Ok(shard)
}

pub fn refresh_integration_from_worktree(root_repo_path: &str, supervisor_id: Uuid) -> Result<SupervisorWorkspace> {
    let workspace = workspace_for(root_repo_path, supervisor_id)?;
    fs::create_dir_all(&workspace.root)?;
    copy_repo_tree(Path::new(root_repo_path), &workspace.integration, Some(&workspace.root))?;
    tracing::info!(supervisor_id = %supervisor_id, root_repo_path = %root_repo_path, integration = %workspace.integration.display(), "refreshed supervisor integration workspace from current worktree");
    Ok(workspace)
}


fn root_repo_for_workspace(workspace: &SupervisorWorkspace) -> Result<PathBuf> {
    workspace
        .root
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("failed to resolve root repo for supervisor workspace {}", workspace.root.display()))
}

pub fn create_shard_from_workspace_snapshot(workspace: &SupervisorWorkspace, shard_id: Uuid) -> Result<PathBuf> {
    fs::create_dir_all(&workspace.root)?;
    if !workspace.snapshot.is_dir() {
        let root_repo = root_repo_for_workspace(workspace)?;
        copy_repo_tree(&root_repo, &workspace.snapshot, Some(&workspace.root))?;
    }
    let shard = shard_path(workspace, shard_id);
    copy_repo_tree(&workspace.snapshot, &shard, None)?;
    Ok(shard)
}

pub fn refresh_shard_from_worktree(root_repo_path: &str, supervisor_id: Uuid, shard_id: Uuid) -> Result<PathBuf> {
    let workspace = workspace_for(root_repo_path, supervisor_id)?;
    fs::create_dir_all(&workspace.root)?;
    let shard = shard_path(&workspace, shard_id);
    copy_repo_tree(Path::new(root_repo_path), &shard, Some(&workspace.root))?;
    tracing::info!(
        supervisor_id = %supervisor_id,
        root_repo_path = %root_repo_path,
        shard_id = %shard_id,
        shard = %shard.display(),
        "refreshed supervisor child shard from current worktree"
    );
    Ok(shard)
}

pub fn shard_path(workspace: &SupervisorWorkspace, shard_id: Uuid) -> PathBuf {
    workspace.root.join(shard_id.to_string())
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
    let integration_root = canonicalize_existing_or_parent(&workspace.integration)?;
    let candidate_canonical = canonicalize_existing_or_parent(&candidate)?;

    if candidate_canonical == root_repo {
        return Err(anyhow!("refusing to delete root repo as supervisor workspace"));
    }
    if candidate_canonical == supervisor_root {
        return Err(anyhow!("refusing to delete supervisor workspace root"));
    }
    if candidate_canonical == integration_root {
        return Err(anyhow!("refusing to delete integration root directly; archive the integration work unit instead"));
    }
    if !candidate_canonical.starts_with(&supervisor_root) && !candidate_canonical.starts_with(&integration_root) {
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

pub fn sanitize_path_segment(value: &str) -> String {
    let out = value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if out.is_empty() { "execution-item".to_string() } else { out }
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    copy_tree_inner(from, to, None, None)
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

