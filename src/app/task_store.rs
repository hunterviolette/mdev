// src/app/task_store.rs

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::app::actions::ComponentId;
use crate::app::layout::{ExecuteLoopSnapshot, TaskSnapshot};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoTaskStoreFile {
    pub version: u32,
    pub repo_key: String,

    #[serde(default)]
    pub execute_loops: HashMap<ComponentId, ExecuteLoopSnapshot>,

    #[serde(default)]
    pub tasks: HashMap<ComponentId, TaskSnapshot>,
}

impl RepoTaskStoreFile {
    pub fn empty(repo_key: String) -> Self {
        Self {
            version: 1,
            repo_key,
            execute_loops: HashMap::new(),
            tasks: HashMap::new(),
        }
    }
}

/// Stable-ish key for a repo path. We keep it filename-safe and include a hash
/// to avoid collisions.
pub fn repo_key_for_path(repo: &Path) -> String {
    let s = repo
        .canonicalize()
        .unwrap_or_else(|_| repo.to_path_buf())
        .to_string_lossy()
        .to_string();

    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    let hash = h.finish();

    // A readable prefix based on the last path segment (sanitized)
    let leaf = repo
        .file_name()
        .and_then(|x| x.to_str())
        .unwrap_or("repo")
        .to_string();

    let mut safe = String::with_capacity(leaf.len());
    for ch in leaf.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            safe.push(ch);
        } else {
            safe.push('_');
        }
    }
    if safe.trim().is_empty() {
        safe = "repo".to_string();
    }

    format!("{}_{:016x}", safe, hash)
}

pub fn task_store_path(task_store_dir: &Path, repo: &Path) -> PathBuf {
    let key = repo_key_for_path(repo);
    task_store_dir.join(format!("{}.json", key))
}
