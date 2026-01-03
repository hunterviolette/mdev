use anyhow::Result;
use regex::Regex;
use rfd::FileDialog;

use crate::{analyze, git};
use crate::app::actions::{Action, ExpandCmd};
use crate::app::state::AppState;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::PickRepo => {
            state.pick_repo_and_run();
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
    // pub(crate) so other controllers (workspace/context exporter) can call these
    pub(crate) fn compile_excludes(&self) -> Result<Vec<Regex>> {
        let mut compiled = Vec::new();
        for rx in &self.inputs.exclude_regex {
            compiled.push(
                Regex::new(rx).map_err(|e| anyhow::anyhow!("Bad exclude regex '{}': {}", rx, e))?,
            );
        }
        Ok(compiled)
    }

    pub(crate) fn pick_repo_and_run(&mut self) {
        if let Some(p) = FileDialog::new()
            .set_title("Select a git repo folder")
            .pick_folder()
        {
            self.inputs.repo = Some(p);
            self.results.result = None;
            self.results.error = None;
            self.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
            self.run_analysis();
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
                // IMPORTANT: keep old behavior for ContextExporter TreeSelect defaults
                self.set_context_selection_all(&res);

                self.results.result = Some(res);
                self.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
            }
            Err(e) => self.results.error = Some(format!("{:#}", e)),
        }
    }
}
