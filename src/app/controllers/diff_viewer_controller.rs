use crate::app::actions::{Action, ComponentId, ComponentKind};
use crate::app::state::{AppState, DiffRow, DiffRowKind, DiffViewerPendingSection, DiffViewerState, WORKTREE_REF};
use crate::capabilities::{CapabilityRequest, CapabilityResponse, FileSource};

const INDEX_REF: &str = "INDEX";

fn staged_paths_for_source_control(state: &AppState, sc_id: ComponentId) -> Vec<String> {
    let Some(sc) = state.source_controls.get(&sc_id) else {
        return vec![];
    };

    let mut paths: Vec<String> = sc
        .files
        .iter()
        .filter(|f| f.staged)
        .map(|f| f.path.clone())
        .collect();

    paths.sort();
    paths.dedup();
    paths
}

fn unstaged_paths_for_source_control(state: &AppState, sc_id: ComponentId) -> Vec<String> {
    let Some(sc) = state.source_controls.get(&sc_id) else {
        return vec![];
    };

    let mut paths: Vec<String> = sc
        .files
        .iter()
        .filter(|f| {
            if f.untracked {
                return true;
            }
            let wt = f.worktree_status.as_str();
            !(wt.is_empty() || wt == " " || wt == ".")
        })
        .map(|f| f.path.clone())
        .collect();

    paths.sort();
    paths.dedup();
    paths
}

fn load_diff_rows_for_refs_with_broker(
    broker: &crate::capabilities::CapabilityBroker,
    repo: &std::path::Path,
    path: &str,
    from_ref: &str,
    to_ref: &str,
) -> Result<Vec<DiffRow>, String> {
    let old_source = if from_ref == WORKTREE_REF {
        FileSource::Worktree
    } else if from_ref == INDEX_REF {
        FileSource::Index
    } else {
        FileSource::GitRef(from_ref.to_string())
    };

    let old_text = match broker.exec(CapabilityRequest::ReadFile {
        repo: repo.to_path_buf(),
        path: path.to_string(),
        source: old_source,
    }) {
        Ok(CapabilityResponse::Bytes(bytes)) => String::from_utf8_lossy(&bytes).to_string(),
        Err(_) => String::new(),
        Ok(_) => String::new(),
    };

    let new_source = if to_ref == WORKTREE_REF {
        FileSource::Worktree
    } else if to_ref == INDEX_REF {
        FileSource::Index
    } else {
        FileSource::GitRef(to_ref.to_string())
    };

    let new_text = match broker.exec(CapabilityRequest::ReadFile {
        repo: repo.to_path_buf(),
        path: path.to_string(),
        source: new_source,
    }) {
        Ok(CapabilityResponse::Bytes(bytes)) => String::from_utf8_lossy(&bytes).to_string(),
        Err(_) => String::new(),
        Ok(_) => String::new(),
    };

    Ok(build_side_by_side_rows(&old_text, &new_text))
}

impl AppState {
    fn schedule_grouped_diff_viewer_load(
        &mut self,
        target_id: ComponentId,
        paths: Vec<String>,
        from_ref: &str,
        to_ref: &str,
        title: &str,
    ) {
        let pending_sections: Vec<DiffViewerPendingSection> = paths
            .into_iter()
            .map(|path| DiffViewerPendingSection {
                path,
                from_ref: from_ref.to_string(),
                to_ref: to_ref.to_string(),
            })
            .collect();

        if let Some(v) = self.diff_viewers.get_mut(&target_id) {
            v.path = None;
            v.from_ref = from_ref.to_string();
            v.to_ref = to_ref.to_string();
            v.rows.clear();
            v.file_sections.clear();
            v.aggregate_title = Some(title.to_string());
            v.last_error = None;
            v.needs_refresh = false;
            v.loading = !pending_sections.is_empty();
            v.loaded_files = 0;
            v.total_files = pending_sections.len();
        }

        self.diff_viewer_jobs
            .entry(target_id)
            .or_default()
            .start_queue(pending_sections);
    }

    pub fn poll_diff_viewer_loads(&mut self) -> bool {
        let viewer_ids: Vec<ComponentId> = self.diff_viewers.keys().copied().collect();
        let mut changed = false;
        let mut finished_jobs = Vec::new();

        let Some(repo) = self.inputs.repo.clone() else {
            for viewer_id in viewer_ids {
                if self.diff_viewer_jobs.remove(&viewer_id).is_some() {
                    if let Some(v) = self.diff_viewers.get_mut(&viewer_id) {
                        v.loading = false;
                        if v.last_error.is_none() {
                            v.last_error = Some("No repo selected.".to_string());
                        }
                    }
                    changed = true;
                }
            }
            return changed;
        };

        let broker = self.broker.clone();

        for viewer_id in viewer_ids {
            let polled = {
                let Some(job) = self.diff_viewer_jobs.get_mut(&viewer_id) else {
                    continue;
                };

                let broker = broker.clone();
                let repo = repo.clone();
                let result = job.poll_with(move |pending: DiffViewerPendingSection| {
                    let rows = load_diff_rows_for_refs_with_broker(
                        &broker,
                        &repo,
                        &pending.path,
                        &pending.from_ref,
                        &pending.to_ref,
                    )?;
                    Ok(crate::app::state::DiffViewerFileSection {
                        path: pending.path,
                        from_ref: pending.from_ref,
                        to_ref: pending.to_ref,
                        rows,
                    })
                });
                let still_pending = job.is_pending();
                result.map(|(loaded, total, res)| (loaded, total, res, still_pending))
            };

            let Some((loaded, total, res, still_pending)) = polled else {
                continue;
            };

            let Some(v) = self.diff_viewers.get_mut(&viewer_id) else {
                continue;
            };

            match res {
                Ok(section) => {
                    v.file_sections.push(section);
                }
                Err(err) => {
                    if v.last_error.is_none() {
                        v.last_error = Some(err);
                    }
                }
            }

            v.loaded_files = loaded;
            v.total_files = total;
            v.loading = still_pending;
            changed = true;

            if !still_pending {
                finished_jobs.push(viewer_id);
            }
        }

        for viewer_id in finished_jobs {
            self.diff_viewer_jobs.remove(&viewer_id);
        }

        changed
    }
}

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
        Action::OpenDiffViewerForStaged { sc_id } => {
            let paths = staged_paths_for_source_control(state, *sc_id);
            state.open_or_attach_staged_diff_viewer(paths);
            true
        }
        Action::OpenDiffViewerForUnstaged { sc_id } => {
            let paths = unstaged_paths_for_source_control(state, *sc_id);
            state.open_or_attach_unstaged_diff_viewer(paths);
            true
        }
        Action::RefreshDiffViewer { viewer_id } => {
            state.refresh_diff_viewer(*viewer_id);
            true
        }
                Action::DiffViewerRevertPatch { viewer_id, patch } => {
            let Some(repo) = state.inputs.repo.clone() else {
                if let Some(v) = state.diff_viewers.get_mut(viewer_id) {
                    v.last_error = Some("No repo selected.".into());
                }
                return true;
            };

            match state.broker.exec(crate::capabilities::CapabilityRequest::ApplyGitPatchReverse {
                repo,
                patch: patch.clone(),
            }) {
                Ok(_) => {
                    if let Some(v) = state.diff_viewers.get_mut(viewer_id) {
                        v.last_error = None;
                        v.needs_refresh = true;
                    }
                }
                Err(e) => {
                    if let Some(v) = state.diff_viewers.get_mut(viewer_id) {
                        v.last_error = Some(format!("{:#}", e));
                    }
                }
            }
            true
        }
_ => false,
    }
}

impl AppState {
    pub fn load_diff_rows_for_refs(&self, path: &str, from_ref: &str, to_ref: &str) -> Result<Vec<DiffRow>, String> {
        let Some(repo) = self.inputs.repo.clone() else {
            return Err("No repo selected.".to_string());
        };

        let old_source = if from_ref == WORKTREE_REF {
            FileSource::Worktree
        } else if from_ref == INDEX_REF {
            FileSource::Index
        } else {
            FileSource::GitRef(from_ref.to_string())
        };

        let old_text = match self.broker.exec(CapabilityRequest::ReadFile {
            repo: repo.clone(),
            path: path.to_string(),
            source: old_source,
        }) {
            Ok(CapabilityResponse::Bytes(bytes)) => String::from_utf8_lossy(&bytes).to_string(),
            Err(_) => String::new(),
            Ok(_) => String::new(),
        };

        let new_source = if to_ref == WORKTREE_REF {
            FileSource::Worktree
        } else if to_ref == INDEX_REF {
            FileSource::Index
        } else {
            FileSource::GitRef(to_ref.to_string())
        };

        let new_text = match self.broker.exec(CapabilityRequest::ReadFile {
            repo,
            path: path.to_string(),
            source: new_source,
        }) {
            Ok(CapabilityResponse::Bytes(bytes)) => String::from_utf8_lossy(&bytes).to_string(),
            Err(_) => String::new(),
            Ok(_) => String::new(),
        };

        Ok(build_side_by_side_rows(&old_text, &new_text))
    }

    fn open_diff_viewer_in_active_canvas(&mut self) -> ComponentId {
        let mut target = self
            .active_diff_viewer_id()
            .and_then(|id| self.active_layout().get_window(id).map(|w| (id, w.open)))
            .and_then(|(id, open)| if open { Some(id) } else { None });

        if target.is_none() {
            for c in self.active_layout().components.iter().rev() {
                if c.kind == ComponentKind::DiffViewer {
                    if let Some(w) = self.active_layout().get_window(c.id) {
                        if w.open {
                            target = Some(c.id);
                            break;
                        }
                    }
                }
            }
        }

        match target {
            Some(id) => id,
            None => self.new_diff_viewer_component(),
        }
    }

    pub fn open_or_attach_staged_diff_viewer(&mut self, paths: Vec<String>) {
        let target_id = self.open_diff_viewer_in_active_canvas();
        self.schedule_grouped_diff_viewer_load(target_id, paths, "HEAD", "INDEX", "Staged Diff");
        self.set_active_diff_viewer_id(Some(target_id));
    }

    pub fn open_or_attach_unstaged_diff_viewer(&mut self, paths: Vec<String>) {
        let target_id = self.open_diff_viewer_in_active_canvas();
        self.schedule_grouped_diff_viewer_load(target_id, paths, "INDEX", WORKTREE_REF, "Unstaged Diff");
        self.set_active_diff_viewer_id(Some(target_id));
    }

    pub fn open_or_attach_diff_viewer_with_refs(&mut self, path: String, from_ref: String, to_ref: String) {
        let target_id = self.open_diff_viewer_in_active_canvas();

        if let Some(v) = self.diff_viewers.get_mut(&target_id) {
            v.path = Some(path);
            v.from_ref = from_ref;
            v.to_ref = to_ref;
            v.rows.clear();
            v.file_sections.clear();
            v.aggregate_title = None;
            v.last_error = None;
            v.needs_refresh = true;
            v.loading = false;
            v.loaded_files = 0;
            v.total_files = 0;
        }

        self.diff_viewer_jobs.remove(&target_id);
        self.set_active_diff_viewer_id(Some(target_id));
        self.refresh_diff_viewer(target_id);
    }

    pub fn open_or_attach_diff_viewer(&mut self, path: String) {
        let mut target = self
            .active_diff_viewer_id()
            .and_then(|id| self.active_layout().get_window(id).map(|w| (id, w.open)))
            .and_then(|(id, open)| if open { Some(id) } else { None });

        if target.is_none() {
            for c in self.active_layout().components.iter().rev() {
                if c.kind == ComponentKind::DiffViewer {
                    if let Some(w) = self.active_layout().get_window(c.id) {
                        if w.open {
                            target = Some(c.id);
                            break;
                        }
                    }
                }
            }
        }

        let target_id = match target {
            Some(id) => id,
            None => self.new_diff_viewer_component(),
        };

        self.diff_viewers
            .entry(target_id)
            .or_insert_with(DiffViewerState::new);

        if let Some(v) = self.diff_viewers.get_mut(&target_id) {
            v.path = Some(path);
            v.rows.clear();
            v.file_sections.clear();
            v.aggregate_title = None;
            v.last_error = None;
            v.needs_refresh = true;
            v.loading = false;
            v.loaded_files = 0;
            v.total_files = 0;
        }

        self.diff_viewer_jobs.remove(&target_id);
        self.set_active_diff_viewer_id(Some(target_id));
        self.refresh_diff_viewer(target_id);
    }

    fn new_diff_viewer_component(&mut self) -> ComponentId {
        self.active_layout_mut().merge_with_defaults();

        let id = self.alloc_component_id();
        let title = format!("Diff Viewer {}", id);

        self.active_layout_mut()
            .components
            .push(crate::app::layout::ComponentInstance {
                id,
                kind: ComponentKind::DiffViewer,
                title,
            });

        self.active_layout_mut().windows.insert(
            id,
            crate::app::layout::WindowLayout {
                open: true,
                locked: false,
                pos_norm: None,
                size_norm: None,
                pos: [180.0, 180.0],
                size: [980.0, 720.0],
            },
        );

        self.diff_viewers.insert(id, DiffViewerState::new());
        self.set_active_diff_viewer_id(Some(id));
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

    pub fn rebuild_diff_viewers_from_layout(&mut self) {
        self.diff_viewers.clear();
        self.diff_viewer_jobs.clear();

        let mut ids: Vec<ComponentId> = self
            .all_layouts()
            .flat_map(|l| l.components.iter())
            .filter(|c| c.kind == ComponentKind::DiffViewer)
            .map(|c| c.id)
            .collect();

        ids.sort_unstable();
        ids.dedup();

        for id in ids {
            self.diff_viewers.insert(id, DiffViewerState::new());
        }

        for canvas in self.canvases.iter_mut() {
            if let Some(active) = canvas.active_diff_viewer {
                let exists = canvas
                    .layout
                    .components
                    .iter()
                    .any(|c| c.kind == ComponentKind::DiffViewer && c.id == active);
                if !exists {
                    canvas.active_diff_viewer = None;
                }
            }
        }
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Equal,
    Insert,
    Delete,
}

fn build_side_by_side_rows(old_text: &str, new_text: &str) -> Vec<DiffRow> {
    let a: Vec<&str> = old_text.lines().collect();
    let b: Vec<&str> = new_text.lines().collect();

    let ops = lcs_ops(&a, &b);

    let mut rows: Vec<DiffRow> = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;
    let mut a_ln = 1usize;
    let mut b_ln = 1usize;

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

fn lcs_ops(a: &[&str], b: &[&str]) -> Vec<Op> {
    let n = a.len();
    let m = b.len();

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
