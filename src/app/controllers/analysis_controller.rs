use crate::app::actions::{Action, ExpandCmd};
use crate::app::state::{AppState, WORKTREE_REF};
use crate::capabilities::{CapabilityRequest, CapabilityResponse};

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

        // If you want "new repo => select all by default" but "ref switch => preserve",
        // clear selection here so first analysis run re-initializes it.
        // self.tree.context_selected_files.clear();

        // Default: try HEAD, but if not a git repo we will fall back to WORKTREE.
        self.set_git_ref("HEAD".to_string());

        self.results.result = None;
        self.results.error = None;

        self.refresh_git_refs();

        // Chats/tasks are persisted globally per-repo.
        // Hydrate them as soon as the repo is selected.
        let _ = self.load_repo_task_store();

        self.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
        self.run_analysis();
    }

    pub(crate) fn refresh_git_refs(&mut self) {
        let Some(repo) = self.inputs.repo.clone() else {
            // No folder selected yet: safe defaults
            self.set_git_ref_options(vec!["HEAD".to_string(), WORKTREE_REF.to_string()]);
            if self.inputs.git_ref != "HEAD" {
                self.set_git_ref("HEAD".to_string());
            }
            return;
        };

        // If not a git repo, switch UI into WORKTREE-only mode.
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
                // If listing refs fails, don't brick the app: just go worktree-only.
                self.results.error = Some(format!("{:#}", e));
                self.set_git_ref_options_worktree_only();
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

        // If the user selected a git ref but this folder isn't a git repo,
        // force WORKTREE and keep going.
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
                // Preserve selection across ref switches; drop only files that no longer exist.
                self.reconcile_context_selection(&res);

                self.results.result = Some(res);
                self.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
            }
            Ok(_) => self.results.error = Some("Unexpected response from analysis.".into()),
            Err(e) => self.results.error = Some(format!("{:#}", e)),
        }
    }

    /// Keeps any previously-selected files selected when the git ref changes.
    /// If selected files don't exist in the newly analyzed result (deleted/missing),
    /// they are removed from the selection.
    /// If there is no prior selection (first run), default to selecting all.
    fn reconcile_context_selection(&mut self, res: &crate::model::AnalysisResult) {
        // First run / no prior selection: keep previous behavior (select all).
        if self.tree.context_selected_files.is_empty() {
            self.set_context_selection_all(res);
            return;
        }

        // Collect all file identifiers that exist at this ref.
        // IMPORTANT: `FileRow` does not have `path`; it has `full_path`.
        // We therefore reconcile selection using `full_path`.
        let mut files = Vec::new();
        collect_all_file_full_paths(&res.root, &mut files);
        let exists: HashSet<String> = files.into_iter().collect();

        // Retain only selected paths that still exist.
        // (Assumes `context_selected_files` stores the same string as FileRow.full_path.)
        self.tree
            .context_selected_files
            .retain(|p| exists.contains(p));
    }
}

/// Local helper because the existing `collect_all_files` in `workspace_controller.rs` is private.
/// Uses `FileRow.full_path` (per compiler error fields: name, full_path, loc_display, ext).
fn collect_all_file_full_paths(node: &crate::model::DirNode, out: &mut Vec<String>) {
    for f in &node.files {
        out.push(f.full_path.clone());
    }
    for child in &node.children {
        collect_all_file_full_paths(child, out);
    }
}
