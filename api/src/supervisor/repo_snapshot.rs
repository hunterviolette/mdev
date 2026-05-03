use std::{fs, path::{Path, PathBuf}};

use anyhow::{anyhow, Context, Result};
use uuid::Uuid;

use crate::supervisor::models::FeaturePlanItem;

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
    if workspace.root.exists() {
        fs::remove_dir_all(&workspace.root).with_context(|| format!("failed to clear {}", workspace.root.display()))?;
    }
    fs::create_dir_all(&workspace.root)?;
    fs::create_dir_all(&workspace.shards)?;
    fs::create_dir_all(&workspace.patches)?;
    fs::create_dir_all(&workspace.logs)?;
    copy_tree(Path::new(root_repo_path), &workspace.snapshot)?;
    copy_tree(&workspace.snapshot, &workspace.integration)?;
    for item in items {
        copy_tree(&workspace.snapshot, &workspace.shards.join(sanitize_path_segment(&item.id)))?;
    }
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
    if !from.is_dir() {
        return Err(anyhow!("{} is not a directory", from.display()));
    }
    if to.exists() {
        fs::remove_dir_all(to).with_context(|| format!("failed to remove {}", to.display()))?;
    }
    fs::create_dir_all(to).with_context(|| format!("failed to create {}", to.display()))?;
    copy_dir_contents(from, to)
}

fn copy_dir_contents(from: &Path, to: &Path) -> Result<()> {
    for entry in fs::read_dir(from).with_context(|| format!("failed to read {}", from.display()))? {
        let entry = entry?;
        let src = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if should_skip(name_str.as_ref()) {
            continue;
        }
        let dst = to.join(&name);
        let meta = entry.metadata()?;
        if meta.is_dir() {
            fs::create_dir_all(&dst)?;
            copy_dir_contents(&src, &dst)?;
        } else if meta.is_file() {
            fs::copy(&src, &dst).with_context(|| format!("failed to copy {} to {}", src.display(), dst.display()))?;
        }
    }
    Ok(())
}

fn should_skip(name: &str) -> bool {
    matches!(name, ".git" | "target" | "node_modules" | "dist" | "build")
}
