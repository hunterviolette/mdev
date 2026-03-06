use crate::app::actions::{Action, ExpandCmd};
use crate::app::state::{AppState, WORKTREE_REF};
use crate::capabilities::{CapabilityRequest, CapabilityResponse};
use regex::Regex;

use std::collections::HashSet;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::PickRepo => {
            state.pick_local_repo_and_run();
            true
        }
        Action::RefreshGitRefs => {
            state.refresh_git_refs();
            true
        }
        Action::SetGitRef(r) => {
            state.set_git_ref(r.clone());
            state.run_analysis();
            true
        }
        Action::RunAnalysis => {
            state.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
            state.run_analysis();
            true
        }
        _ => false,
    }
}

impl AppState {
    pub(crate) fn compile_excludes_raw(&self) -> Vec<String> {
        self.inputs.exclude_regex.clone()
    }

    pub(crate) fn pick_local_repo_and_run(&mut self) {
        let Some(p) = self
            .platform
            .pick_folder("Select a folder (git repo OR plain working tree)")
        else {
            return;
        };

        self.inputs.local_repo = Some(p.clone());
        self.inputs.repo = Some(p.clone());

        self.set_git_ref("HEAD".to_string());

        self.results.result = None;
        self.results.error = None;

        self.refresh_git_refs();

        let _ = self.load_repo_task_store();

        self.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
        self.run_analysis();
    }

    pub(crate) fn refresh_git_refs(&mut self) {
        let Some(repo) = self.inputs.repo.clone() else {
            self.set_git_ref_options(vec!["HEAD".to_string(), WORKTREE_REF.to_string()]);
            if self.inputs.git_ref != "HEAD" {
                self.set_git_ref("HEAD".to_string());
            }
            return;
        };

        if self
            .broker
            .exec(CapabilityRequest::EnsureGitRepo { repo: repo.clone() })
            .is_err()
        {
            self.set_git_ref_options_worktree_only();
            return;
        }

        match self.broker.exec(CapabilityRequest::ListGitRefs { repo }) {
            Ok(CapabilityResponse::GitRefs(list)) => self.set_git_ref_options(list),
            Ok(_) => {
                self.results.error = Some("Unexpected response listing git refs.".into());
            }
            Err(e) => {
                self.results.error = Some(format!("{:#}", e));
                self.set_git_ref_options_worktree_only();
            }
        }
    }

    pub fn start_analysis_refresh_async(&mut self) {
        if self.tree.analysis_refresh_pending {
            return;
        }
        let Some(repo) = self.inputs.repo.clone() else {
            return;
        };
        if self.inputs.git_ref != WORKTREE_REF {
            return;
        }

        let git_ref = self.inputs.git_ref.clone();
        let exclude = self.compile_excludes_raw();
        let max_exts = self.inputs.max_exts;

        let (tx, rx) = std::sync::mpsc::channel::<Result<crate::model::AnalysisResult, String>>();
        self.tree.analysis_refresh_pending = true;
        self.tree.analysis_refresh_rx = Some(rx);

        std::thread::spawn(move || {
            let compiled: Result<Vec<Regex>, String> = exclude
                .into_iter()
                .map(|p| Regex::new(&p).map_err(|e| format!("Bad exclude regex '{p}': {e}")))
                .collect();

            let res = match compiled {
                Ok(c) => crate::analyze::analyze_repo(&repo, &git_ref, &c, max_exts)
                    .map_err(|e| format!("{:#}", e)),
                Err(e) => Err(e),
            };

            let _ = tx.send(res);
        });
    }

    pub fn poll_analysis_refresh(&mut self) -> bool {
        if !self.tree.analysis_refresh_pending {
            return false;
        }
        let Some(rx) = &self.tree.analysis_refresh_rx else {
            self.tree.analysis_refresh_pending = false;
            return false;
        };

        match rx.try_recv() {
            Ok(Ok(res)) => {
                self.reconcile_context_selection(&res);
                self.results.result = Some(res);
                self.results.error = None;
                self.tree.analysis_refresh_pending = false;
                self.tree.analysis_refresh_rx = None;
                if self.inputs.git_ref == WORKTREE_REF {
                    self.refresh_tree_git_status();
                }
                true
            }
            Ok(Err(err)) => {
                self.results.error = Some(err);
                self.tree.analysis_refresh_pending = false;
                self.tree.analysis_refresh_rx = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.results.error = Some("Auto-refresh channel disconnected.".to_string());
                self.tree.analysis_refresh_pending = false;
                self.tree.analysis_refresh_rx = None;
                true
            }
        }
    }

    pub(crate) fn run_analysis(&mut self) {
        self.results.error = None;
        self.results.result = None;

        let repo = match &self.inputs.repo {
            Some(r) => r.clone(),
            None => {
                self.results.error = Some("Select a folder first.".into());
                return;
            }
        };

        if self.inputs.git_ref != WORKTREE_REF {
            if self
                .broker
                .exec(CapabilityRequest::EnsureGitRepo { repo: repo.clone() })
                .is_err()
            {
                self.set_git_ref_options_worktree_only();
            }
        }

        let exclude = self.compile_excludes_raw();

        match self.broker.exec(CapabilityRequest::AnalyzeRepo {
            repo,
            git_ref: self.inputs.git_ref.clone(),
            exclude_regex: exclude,
            max_exts: self.inputs.max_exts,
        }) {
            Ok(CapabilityResponse::Analysis(res)) => {
                self.reconcile_context_selection(&res);

                self.results.result = Some(res);
                self.tree.expand_cmd = Some(ExpandCmd::ExpandAll);

                if self.inputs.git_ref == WORKTREE_REF {
                    self.refresh_tree_git_status();
                }
            }
            Ok(_) => self.results.error = Some("Unexpected response from analysis.".into()),
            Err(e) => self.results.error = Some(format!("{:#}", e)),
        }
    }

    pub fn refresh_tree_git_status(&mut self) {
        let Some(repo) = self.inputs.repo.clone() else {
            self.tree.modified_paths.clear();
            self.tree.staged_paths.clear();
            self.tree.untracked_paths.clear();
            return;
        };

        if self.inputs.git_ref != WORKTREE_REF {
            self.tree.modified_paths.clear();
            self.tree.staged_paths.clear();
            self.tree.untracked_paths.clear();
            return;
        }

        let st = match self.broker.exec(CapabilityRequest::GitStatus { repo }) {
            Ok(CapabilityResponse::GitStatus(v)) => v,
            _ => {
                self.tree.modified_paths.clear();
                self.tree.staged_paths.clear();
                self.tree.untracked_paths.clear();
                self.tree.git_status_by_path.clear();
                return;
            }
        };

        self.tree.modified_paths.clear();
        self.tree.staged_paths.clear();
        self.tree.untracked_paths.clear();
        self.tree.git_status_by_path.clear();

        for f in st.files {
            self.tree.git_status_by_path.insert(f.path.clone(), f.clone());

            if f.untracked {
                self.tree.untracked_paths.insert(f.path.clone());
                self.tree.modified_paths.insert(f.path);
                continue;
            }
            if f.staged {
                self.tree.staged_paths.insert(f.path.clone());
            }
            if f.worktree_status.trim() != "" || f.index_status.trim() != "" {
                self.tree.modified_paths.insert(f.path);
            }
        }
    }

    fn reconcile_context_selection(&mut self, res: &crate::model::AnalysisResult) {
        if !self.tree.context_initialized {
            self.set_context_selection_all(res);
            self.tree.context_initialized = true;
            return;
        }

        let mut files = Vec::new();
        collect_all_file_full_paths(&res.root, &mut files);
        let exists: HashSet<String> = files.into_iter().collect();

        self.tree
            .context_selected_files
            .retain(|p| exists.contains(p));
    }
}

fn collect_all_file_full_paths(node: &crate::model::DirNode, out: &mut Vec<String>) {
    for f in &node.files {
        out.push(f.full_path.clone());
    }
    for child in &node.children {
        collect_all_file_full_paths(child, out);
    }
}
