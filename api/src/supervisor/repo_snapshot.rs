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
    let root = root_repo.join(".mdev").join("supervisors").join(supervisor_id.to_string());
    Ok(SupervisorWorkspace {
        snapshot: root.join("snapshot"),
        integration: root.join("integration"),
        shards: root.join("shards"),
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
    fs::create_dir_all(&workspace.shards)?;
    fs::create_dir_all(&workspace.patches)?;
    fs::create_dir_all(&workspace.logs)?;
    copy_repo_tree(Path::new(root_repo_path), &workspace.snapshot, Some(&workspace.root))?;
    copy_tree(&workspace.snapshot, &workspace.integration)?;
    for item in items {
        copy_tree(&workspace.snapshot, &workspace.shards.join(sanitize_path_segment(&item.id)))?;
    }
    tracing::info!(supervisor_id = %supervisor_id, snapshot = %workspace.snapshot.display(), integration = %workspace.integration.display(), "created supervisor workspace");
    Ok(workspace)
}

pub fn refresh_integration_from_worktree(root_repo_path: &str, supervisor_id: Uuid) -> Result<SupervisorWorkspace> {
    let workspace = workspace_for(root_repo_path, supervisor_id)?;
    fs::create_dir_all(&workspace.root)?;
    fs::create_dir_all(&workspace.shards)?;
    fs::create_dir_all(&workspace.patches)?;
    fs::create_dir_all(&workspace.logs)?;
    copy_repo_tree(Path::new(root_repo_path), &workspace.integration, Some(&workspace.root))?;
    tracing::info!(supervisor_id = %supervisor_id, root_repo_path = %root_repo_path, integration = %workspace.integration.display(), "refreshed supervisor integration shard from current worktree");
    Ok(workspace)
}


pub fn shard_path(workspace: &SupervisorWorkspace, execution_item_id: &str) -> PathBuf {
    workspace.shards.join(sanitize_path_segment(execution_item_id))
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

