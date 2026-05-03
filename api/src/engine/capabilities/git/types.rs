#[derive(Clone, Debug)]
pub struct GitStatusEntry {
    pub path: String,
    pub index_status: String,
    pub worktree_status: String,
    pub staged: bool,
    pub untracked: bool,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct GitStatusResult {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<GitStatusEntry>,
}
