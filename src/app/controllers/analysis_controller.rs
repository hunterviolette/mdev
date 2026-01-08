use anyhow::Result;
use regex::Regex;

use crate::{analyze, git};
use crate::app::actions::{Action, ExpandCmd};
use crate::app::state::{AppState, WORKTREE_REF};

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
    pub(crate) fn compile_excludes(&self) -> Result<Vec<Regex>> {
        let mut compiled = Vec::new();
        for rx in &self.inputs.exclude_regex {
            compiled.push(
                Regex::new(rx).map_err(|e| anyhow::anyhow!("Bad exclude regex '{}': {}", rx, e))?,
            );
        }
        Ok(compiled)
    }

    pub(crate) fn pick_local_repo_and_run(&mut self) {
        let Some(p) = self.platform.pick_folder("Select a LOCAL git repo folder") else {
            return;
        };

        self.inputs.local_repo = Some(p.clone());
        self.inputs.repo = Some(p);

        self.set_git_ref("HEAD".to_string());

        self.results.result = None;
        self.results.error = None;

        self.refresh_git_refs();

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

        match git::list_git_refs_for_dropdown(&repo) {
            Ok(list) => self.set_git_ref_options(list),
            Err(e) => {
                self.results.error = Some(format!("{:#}", e));
                self.set_git_ref_options(vec!["HEAD".to_string(), WORKTREE_REF.to_string()]);
                if self.inputs.git_ref != "HEAD" {
                    self.set_git_ref("HEAD".to_string());
                }
            }
        }
    }

    pub(crate) fn run_analysis(&mut self) {
        self.results.error = None;
        self.results.result = None;

        let repo = match &self.inputs.repo {
            Some(r) => r.clone(),
            None => {
                self.results.error = Some("Select a repo folder first.".into());
                return;
            }
        };

        if let Err(e) = git::ensure_git_repo(&repo) {
            self.results.error = Some(format!("{:#}", e));
            return;
        }

        let compiled = match self.compile_excludes() {
            Ok(c) => c,
            Err(e) => {
                self.results.error = Some(format!("{:#}", e));
                return;
            }
        };

        match analyze::analyze_repo(&repo, &self.inputs.git_ref, &compiled, self.inputs.max_exts) {
            Ok(res) => {
                // Keep old behavior: default context-export selection selects all files.
                self.set_context_selection_all(&res);

                self.results.result = Some(res);
                self.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
            }
            Err(e) => self.results.error = Some(format!("{:#}", e)),
        }
    }
}
