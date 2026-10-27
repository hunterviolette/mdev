use std::{fs, path::{Path, PathBuf}, process::Command};

use anyhow::{anyhow, Context, Result};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SupervisorWorkspace {
    pub root: PathBuf,
}

pub fn workspace_for(root_repo_path: &str) -> Result<SupervisorWorkspace> {
    Ok(SupervisorWorkspace {
        root: PathBuf::from(root_repo_path).join(".mdev").join("supervisors").join("shards"),
    })
}

fn git(root: &Path, args: &[&str], index: Option<&Path>) -> Result<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = command.output().with_context(|| format!("failed to execute git in {}", root.display()))?;
    if !output.status.success() {
        return Err(anyhow!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn source_tree(root: &Path) -> Result<String> {
    let index = std::env::temp_dir().join(format!("mdev-snapshot-{}.index", Uuid::new_v4()));
    let result = (|| -> Result<String> {
        git(root, &["read-tree", "HEAD"], Some(&index))?;
        git(root, &["add", "-A"], Some(&index))?;
        git(root, &["write-tree"], Some(&index))
    })();
    let _ = fs::remove_file(&index);
    result
}

pub fn source_tree_oid(root_repo_path: &str) -> Result<String> {
    source_tree(Path::new(root_repo_path))
}

fn snapshot_commit(root: &Path) -> Result<String> {
    let tree = source_tree(root)?;
    git(root, &[
        "-c", "user.name=mdev supervisor",
        "-c", "user.email=mdev-supervisor@example.invalid",
        "commit-tree", &tree, "-m", "supervisor source snapshot",
    ], None)
}

pub fn materialize_work_unit_workspace(
    root_repo_path: &str,
    supervisor_id: Uuid,
    workspace_id: Uuid,
) -> Result<PathBuf> {
    let workspace = workspace_for(root_repo_path)?;
    fs::create_dir_all(&workspace.root)?;
    let workspace_path = workspace.root.join(workspace_id.to_string());
    if workspace_path.exists() {
        return Err(anyhow!("supervisor workspace already exists: {}", workspace_path.display()));
    }
    let root = Path::new(root_repo_path);
    let commit = snapshot_commit(root)?;
    git(root, &["worktree", "add", "--detach", "--", workspace_path.to_string_lossy().as_ref(), &commit], None)?;
    tracing::info!(supervisor_id = %supervisor_id, workspace_id = %workspace_id, snapshot_commit = %commit, workspace_path = %workspace_path.display(), "materialized supervisor Git worktree");
    Ok(workspace_path)
}

fn canonicalize_existing_or_parent(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path).with_context(|| format!("failed to canonicalize {}", path.display()));
    }
    let parent = path.parent().ok_or_else(|| anyhow!("path has no parent: {}", path.display()))?;
    let parent = canonicalize_existing_or_parent(parent)?;
    let name = path.file_name().ok_or_else(|| anyhow!("path has no final component: {}", path.display()))?;
    Ok(parent.join(name))
}

fn validate_workspace(root_repo_path: &str, workspace_path: &str) -> Result<PathBuf> {
    let workspace = workspace_for(root_repo_path)?;
    let root = fs::canonicalize(root_repo_path)?;
    let shards_root = canonicalize_existing_or_parent(&workspace.root)?;
    let candidate = canonicalize_existing_or_parent(Path::new(workspace_path))?;
    if candidate == root || candidate == shards_root || candidate.parent() != Some(shards_root.as_path()) {
        return Err(anyhow!("invalid supervisor workspace path: {}", workspace_path));
    }
    let name = candidate.file_name().and_then(|value| value.to_str()).ok_or_else(|| anyhow!("missing workspace identifier"))?;
    Uuid::parse_str(name)?;
    Ok(candidate)
}

fn detach_workspace_links(directory: &Path) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        let mut is_link = metadata.file_type().is_symlink();
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            is_link |= metadata.file_attributes() & 0x400 != 0;
        }
        if is_link {
            if fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(metadata.is_dir()) {
                fs::remove_dir(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
        } else if metadata.is_dir() {
            detach_workspace_links(&path)?;
        }
    }
    Ok(())
}

pub fn delete_supervisor_workspace_path(root_repo_path: &str, _supervisor_id: Uuid, workspace_path: &str) -> Result<bool> {
    if workspace_path.trim().is_empty() {
        return Ok(false);
    }
    let candidate = validate_workspace(root_repo_path, workspace_path)?;
    if !candidate.exists() {
        return Ok(false);
    }
    if candidate.join(".git").is_file() {
        let root = Path::new(root_repo_path);
        let common = git(&candidate, &["rev-parse", "--path-format=absolute", "--git-common-dir"], None)?;
        let expected = git(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"], None)?;
        if fs::canonicalize(common)? != fs::canonicalize(expected)? {
            return Err(anyhow!("workspace is attached to another Git repository"));
        }
        detach_workspace_links(&candidate)?;
        fs::remove_dir_all(&candidate)?;
        git(root, &["worktree", "prune", "--expire", "now"], None)?;
    } else if candidate.join(".git").is_dir() {
        let repository_root = git(&candidate, &["rev-parse", "--show-toplevel"], None)?;
        if fs::canonicalize(repository_root)? != candidate {
            return Err(anyhow!("legacy supervisor workspace is not its own Git root"));
        }
        detach_workspace_links(&candidate)?;
        fs::remove_dir_all(&candidate)?;
    } else {
        detach_workspace_links(&candidate)?;
        fs::remove_dir_all(&candidate)?;
        git(Path::new(root_repo_path), &["worktree", "prune", "--expire", "now"], None)?;
    }
    Ok(true)
}

pub fn integration_baseline_matches(root_repo_path: &str, workspace_path: &str, source_oid: &str) -> Result<bool> {
    let candidate = validate_workspace(root_repo_path, workspace_path)?;
    if !candidate.join(".git").is_file() {
        return Ok(false);
    }
    let root = Path::new(root_repo_path);
    let common = git(&candidate, &["rev-parse", "--path-format=absolute", "--git-common-dir"], None)?;
    let expected = git(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"], None)?;
    if fs::canonicalize(common)? != fs::canonicalize(expected)? {
        return Ok(false);
    }
    Ok(git(&candidate, &["rev-parse", "HEAD^{tree}"], None)? == source_oid)
}

pub fn reset_integration_workspace(root_repo_path: &str, supervisor_id: Uuid, workspace_path: &str) -> Result<()> {
    let candidate = validate_workspace(root_repo_path, workspace_path)?;
    if !candidate.join(".git").is_file() {
        return Err(anyhow!("integration workspace is not a linked Git worktree"));
    }
    git(&candidate, &["reset", "--hard", "HEAD"], None)?;
    git(&candidate, &["clean", "-fd", "--", "."], None)?;
    tracing::info!(supervisor_id = %supervisor_id, workspace_path = %candidate.display(), "restored integration baseline");
    Ok(())
}
