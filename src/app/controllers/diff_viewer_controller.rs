use crate::app::actions::{Action, ComponentId, ComponentKind};
use crate::app::state::{AppState, DiffRow, DiffRowKind, DiffViewerState, WORKTREE_REF};
use crate::capabilities::{CapabilityRequest, CapabilityResponse, FileSource};

const INDEX_REF: &str = "INDEX";

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::OpenDiffViewerForPath { path } => {
            state.open_or_attach_diff_viewer(path.clone());
            true
        }
        Action::OpenDiffViewerForPathWithRefs { path, from_ref, to_ref } => {
            state.open_or_attach_diff_viewer_with_refs(path.clone(), from_ref.clone(), to_ref.clone());
            true
        }
        Action::RefreshDiffViewer { viewer_id } => {
            state.refresh_diff_viewer(*viewer_id);
            true
        }
        _ => false,
    }
}

impl AppState {
    pub fn open_or_attach_diff_viewer_with_refs(&mut self, path: String, from_ref: String, to_ref: String) {
        let target_id = if let Some(active) = self.active_diff_viewer {
            active
        } else {
            self.new_diff_viewer_component()
        };

        if let Some(v) = self.diff_viewers.get_mut(&target_id) {
            v.path = Some(path);
            v.from_ref = from_ref;
            v.to_ref = to_ref;
            v.needs_refresh = true;
        }

        self.active_diff_viewer = Some(target_id);
        self.refresh_diff_viewer(target_id);
    }

    /// Open a path in a Diff Viewer.
    /// - If there is an active Diff Viewer and it is open -> reuse it
    /// - Else reuse the most recently open Diff Viewer
    /// - Else create a new Diff Viewer component
    pub fn open_or_attach_diff_viewer(&mut self, path: String) {
        // 1) Prefer currently active diff viewer if open
        let mut target = self
            .active_diff_viewer
            .and_then(|id| self.layout.get_window(id).map(|w| (id, w.open)))
            .and_then(|(id, open)| if open { Some(id) } else { None });

        // 2) Else find the last open DiffViewer in layout order
        if target.is_none() {
            for c in self.layout.components.iter().rev() {
                if c.kind == ComponentKind::DiffViewer {
                    if let Some(w) = self.layout.get_window(c.id) {
                        if w.open {
                            target = Some(c.id);
                            break;
                        }
                    }
                }
            }
        }

        // 3) Else create a new DiffViewer component
        let target_id = match target {
            Some(id) => id,
            None => self.new_diff_viewer_component(),
        };

        self.diff_viewers
            .entry(target_id)
            .or_insert_with(DiffViewerState::new);

        if let Some(v) = self.diff_viewers.get_mut(&target_id) {
            v.path = Some(path);
            v.last_error = None;
            v.needs_refresh = true;
        }

        self.active_diff_viewer = Some(target_id);
        self.refresh_diff_viewer(target_id);
    }

    fn new_diff_viewer_component(&mut self) -> ComponentId {
        self.layout.merge_with_defaults();

        let id = self.layout.next_free_id();
        let title = format!("Diff Viewer {}", id);

        self.layout
            .components
            .push(crate::app::layout::ComponentInstance {
                id,
                kind: ComponentKind::DiffViewer,
                title,
            });

        self.layout.windows.insert(
            id,
            crate::app::layout::WindowLayout {
                open: true,
                locked: false,
                pos: [180.0, 180.0],
                size: [980.0, 720.0],
            },
        );

        self.diff_viewers.insert(id, DiffViewerState::new());
        self.active_diff_viewer = Some(id);
        self.layout_epoch = self.layout_epoch.wrapping_add(1);

        id
    }

    pub fn refresh_diff_viewer(&mut self, viewer_id: ComponentId) {
        let Some(repo) = self.inputs.repo.clone() else {
            if let Some(v) = self.diff_viewers.get_mut(&viewer_id) {
                v.last_error = Some("No repo selected.".into());
                v.rows.clear();
                v.needs_refresh = false;
            }
            return;
        };

        let Some(path) = self
            .diff_viewers
            .get(&viewer_id)
            .and_then(|v| v.path.clone())
        else {
            return;
        };

        let (from_ref, to_ref) = self
            .diff_viewers
            .get(&viewer_id)
            .map(|v| (v.from_ref.clone(), v.to_ref.clone()))
            .unwrap_or_else(|| ("HEAD".to_string(), WORKTREE_REF.to_string()));

        let old_source = if from_ref == WORKTREE_REF {
            FileSource::Worktree
        } else if from_ref == INDEX_REF {
            FileSource::Index
        } else {
            FileSource::GitRef(from_ref.clone())
        };

        let old_text = match self.broker.exec(CapabilityRequest::ReadFile {
            repo: repo.clone(),
            path: path.clone(),
            source: old_source,
        }) {
            Ok(CapabilityResponse::Bytes(bytes)) => String::from_utf8_lossy(&bytes).to_string(),
            // Missing file in old ref => treat as empty
            Err(_) => String::new(),
            Ok(_) => String::new(),
        };

        let new_source = if to_ref == WORKTREE_REF {
            FileSource::Worktree
        } else if to_ref == INDEX_REF {
            FileSource::Index
        } else {
            FileSource::GitRef(to_ref.clone())
        };

        let new_text = match self.broker.exec(CapabilityRequest::ReadFile {
            repo,
            path: path.clone(),
            source: new_source,
        }) {
            Ok(CapabilityResponse::Bytes(bytes)) => String::from_utf8_lossy(&bytes).to_string(),
            Err(_) => String::new(),
            Ok(_) => String::new(),
        };

        let rows = build_side_by_side_rows(&old_text, &new_text);

        if let Some(v) = self.diff_viewers.get_mut(&viewer_id) {
            v.rows = rows;
            v.last_error = None;
            v.needs_refresh = false;
        }
    }

    /// Keep `diff_viewers` in sync with the current layout.
    pub fn rebuild_diff_viewers_from_layout(&mut self) {
        self.diff_viewers.clear();

        let ids: Vec<ComponentId> = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::DiffViewer)
            .map(|c| c.id)
            .collect();

        for id in ids {
            self.diff_viewers.insert(id, DiffViewerState::new());
        }

        if let Some(active) = self.active_diff_viewer {
            if !self.diff_viewers.contains_key(&active) {
                self.active_diff_viewer = None;
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Diff algorithm (LCS-based, correct + simple) -> side-by-side alignment
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Equal,
    Insert,
    Delete,
}

fn build_side_by_side_rows(old_text: &str, new_text: &str) -> Vec<DiffRow> {
    let a: Vec<&str> = old_text.lines().collect();
    let b: Vec<&str> = new_text.lines().collect();

    // Correct op stream (no “whole file delete+add” for small edits)
    let ops = lcs_ops(&a, &b);

    let mut rows: Vec<DiffRow> = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;
    let mut a_ln = 1usize;
    let mut b_ln = 1usize;

    // Coalesce runs of deletes+inserts into Change rows (VS Code-like)
    let mut k = 0usize;
    while k < ops.len() {
        match ops[k] {
            Op::Equal => {
                rows.push(DiffRow {
                    left_no: Some(a_ln),
                    right_no: Some(b_ln),
                    left: Some(a[i].to_string()),
                    right: Some(b[j].to_string()),
                    kind: DiffRowKind::Equal,
                });
                i += 1;
                j += 1;
                a_ln += 1;
                b_ln += 1;
                k += 1;
            }
            Op::Delete => {
                let mut del_count = 0usize;
                while k < ops.len() && ops[k] == Op::Delete {
                    del_count += 1;
                    k += 1;
                }

                let mut ins_count = 0usize;
                while k < ops.len() && ops[k] == Op::Insert {
                    ins_count += 1;
                    k += 1;
                }

                let paired = del_count.min(ins_count);

                for _ in 0..paired {
                    rows.push(DiffRow {
                        left_no: Some(a_ln),
                        right_no: Some(b_ln),
                        left: Some(a[i].to_string()),
                        right: Some(b[j].to_string()),
                        kind: DiffRowKind::Change,
                    });
                    i += 1;
                    j += 1;
                    a_ln += 1;
                    b_ln += 1;
                }

                for _ in paired..del_count {
                    rows.push(DiffRow {
                        left_no: Some(a_ln),
                        right_no: None,
                        left: Some(a[i].to_string()),
                        right: None,
                        kind: DiffRowKind::Delete,
                    });
                    i += 1;
                    a_ln += 1;
                }

                for _ in paired..ins_count {
                    rows.push(DiffRow {
                        left_no: None,
                        right_no: Some(b_ln),
                        left: None,
                        right: Some(b[j].to_string()),
                        kind: DiffRowKind::Add,
                    });
                    j += 1;
                    b_ln += 1;
                }
            }
            Op::Insert => {
                rows.push(DiffRow {
                    left_no: None,
                    right_no: Some(b_ln),
                    left: None,
                    right: Some(b[j].to_string()),
                    kind: DiffRowKind::Add,
                });
                j += 1;
                b_ln += 1;
                k += 1;
            }
        }
    }

    rows
}

/// Build a correct edit script using an LCS DP table.
/// This is O(n*m) but is robust and produces stable, intuitive diffs.
fn lcs_ops(a: &[&str], b: &[&str]) -> Vec<Op> {
    let n = a.len();
    let m = b.len();

    // dp[i][j] = LCS length of a[i..] and b[j..]
    let mut dp = vec![vec![0usize; m + 1]; n + 1];

    for i in (0..n).rev() {
        for j in (0..m).rev() {
            if a[i] == b[j] {
                dp[i][j] = dp[i + 1][j + 1] + 1;
            } else {
                dp[i][j] = dp[i + 1][j].max(dp[i][j + 1]);
            }
        }
    }

    // Reconstruct edit script
    let mut ops = Vec::with_capacity(n + m);
    let mut i = 0usize;
    let mut j = 0usize;

    while i < n && j < m {
        if a[i] == b[j] {
            ops.push(Op::Equal);
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            ops.push(Op::Delete);
            i += 1;
        } else {
            ops.push(Op::Insert);
            j += 1;
        }
    }

    while i < n {
        ops.push(Op::Delete);
        i += 1;
    }

    while j < m {
        ops.push(Op::Insert);
        j += 1;
    }

    ops
}
