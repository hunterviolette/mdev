use crate::app::actions::{Action, ComponentId};
use crate::app::state::AppState;
use crate::git;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::ContextPickSavePath { exporter_id } => {
            let default_name = "repo_context.txt";
            if let Some(path) = state
                .platform
                .save_file("Save context file", default_name)
            {
                if let Some(ex) = state.context_exporters.get_mut(exporter_id) {
                    ex.save_path = Some(path);
                    ex.status = None;
                }
            }
            true
        }

        Action::ContextSetMaxBytes { exporter_id, max } => {
            if let Some(ex) = state.context_exporters.get_mut(exporter_id) {
                ex.max_bytes_per_file = (*max).max(1_000);
            }
            true
        }

        Action::ContextToggleSkipBinary { exporter_id } => {
            if let Some(ex) = state.context_exporters.get_mut(exporter_id) {
                ex.skip_binary = !ex.skip_binary;
            }
            true
        }

        Action::ContextGenerate { exporter_id } => {
            state.generate_context_file(*exporter_id);
            true
        }

        _ => false,
    }
}

impl AppState {
    fn generate_context_file(&mut self, exporter_id: ComponentId) {
        let Some(repo) = self.inputs.repo.clone() else {
            if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                ex.status = Some("No repo selected.".into());
            }
            return;
        };

        let (mode, out_path, max_bytes_per_file, skip_binary) =
            match self.context_exporters.get(&exporter_id) {
                Some(ex) => (ex.mode, ex.save_path.clone(), ex.max_bytes_per_file, ex.skip_binary),
                None => return,
            };

        let Some(out_path) = out_path else {
            if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                ex.status = Some("Pick a save path first.".into());
            }
            return;
        };

        let compiled = match self.compile_excludes() {
            Ok(c) => c,
            Err(e) => {
                if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                    ex.status = Some(format!("{:#}", e));
                }
                return;
            }
        };

        let include_files: Option<Vec<String>> = match mode {
            crate::app::state::ContextExportMode::EntireRepo => None,
            crate::app::state::ContextExportMode::TreeSelect => {
                let mut v: Vec<String> = self.tree.context_selected_files.iter().cloned().collect();
                v.sort();
                Some(v)
            }
        };

        let opts = git::ContextExportOptions {
            git_ref: &self.inputs.git_ref,
            exclude: &compiled,
            max_bytes_per_file,
            skip_binary,
            include_files: include_files.as_deref(),
        };

        match git::export_repo_context(&repo, &out_path, opts) {
            Ok(()) => {
                if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                    ex.status = Some(format!("Wrote {}", out_path.display()));
                }
            }
            Err(e) => {
                if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                    ex.status = Some(format!("{:#}", e));
                }
            }
        }
    }
}
