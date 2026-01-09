use crate::capabilities::{CapabilityRequest, CapabilityResponse};
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

        match self
            .broker
            .exec(CapabilityRequest::ListGitRefs { repo })
        {
            Ok(CapabilityResponse::GitRefs(list)) => self.set_git_ref_options(list),
            Ok(_) => {
                self.results.error = Some("Unexpected response listing git refs.".into());
            }
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

        if let Err(e) = self
            .broker
            .exec(CapabilityRequest::EnsureGitRepo { repo: repo.clone() })
        {
            self.results.error = Some(format!("{:#}", e));
            return;
        }

        let exclude = self.compile_excludes_raw();

        match self.broker.exec(CapabilityRequest::AnalyzeRepo {
            repo,
            git_ref: self.inputs.git_ref.clone(),
            exclude_regex: exclude,
            max_exts: self.inputs.max_exts,
        }) {
            Ok(CapabilityResponse::Analysis(res)) => {
                // Keep old behavior: default context-export selection selects all files.
                self.set_context_selection_all(&res);

                self.results.result = Some(res);
                self.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
            }
            Ok(_) => self.results.error = Some("Unexpected response from analysis.".into()),
            Err(e) => self.results.error = Some(format!("{:#}", e)),
        }
    }
}
