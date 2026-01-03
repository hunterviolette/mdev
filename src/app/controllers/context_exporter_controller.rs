use rfd::FileDialog;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::{AppState, ContextExportMode, ContextExporterState};
use crate::git;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::ContextPickSavePath { exporter_id } => {
            let default_name = "repo_context.txt";
            if let Some(path) = FileDialog::new()
                .set_title("Save context file")
                .set_file_name(default_name)
                .save_file()
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
        // repo must exist
        let Some(repo) = self.inputs.repo.clone() else {
            if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                ex.status = Some("No repo selected.".into());
            }
            return;
        };

        // Snapshot exporter state so we can mut-borrow later
        let (mode, out_path, max_bytes_per_file, skip_binary) =
            match self.context_exporters.get(&exporter_id) {
                Some(ex) => (
                    ex.mode,
                    ex.save_path.clone(),
                    ex.max_bytes_per_file,
                    ex.skip_binary,
                ),
                None => return,
            };

        // Need save path
        let Some(out_path) = out_path else {
            if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                ex.status = Some("Pick a save path first.".into());
            }
            return;
        };

        // Compile excludes (owned by analysis controller)
        let compiled = match self.compile_excludes() {
            Ok(c) => c,
            Err(e) => {
                if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                    ex.status = Some(format!("Bad exclude regex: {:#}", e));
                }
                return;
            }
        };

        // Include list only in TreeSelect mode
        let include_files: Option<Vec<String>> = match mode {
            ContextExportMode::EntireRepo => None,
            ContextExportMode::TreeSelect => {
                if self.results.result.is_none() {
                    if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                        ex.status =
                            Some("Run analysis first (tree selection requires analysis).".into());
                    }
                    return;
                }

                let mut v: Vec<String> = self.tree.context_selected_files.iter().cloned().collect();

                // IMPORTANT: empty selection must NOT fall back to entire repo.
                if v.is_empty() {
                    if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                        ex.status = Some("No files selected in tree.".into());
                    }
                    return;
                }

                v.sort();
                Some(v)
            }
        };

        if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
            ex.status = Some("Generating…".into());
        }

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
                    ex.status = Some(format!("Wrote: {}", out_path.display()));
                }
            }
            Err(e) => {
                if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                    ex.status = Some(format!("Export failed: {:#}", e));
                }
            }
        }
    }

    // Ephemeral restore helper (used by layout reset + workspace load)
    pub(crate) fn rebuild_context_exporters_from_layout(&mut self) {
        use crate::app::actions::ComponentKind;

        self.context_exporters.clear();

        let ids: Vec<ComponentId> = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::ContextExporter)
            .map(|c| c.id)
            .collect();

        for id in ids {
            self.context_exporters.insert(
                id,
                ContextExporterState {
                    save_path: None,
                    max_bytes_per_file: 200_000,
                    skip_binary: true,
                    mode: ContextExportMode::EntireRepo,
                    status: None,
                },
            );
        }
    }

    // Keep behavior from old controller: after running analysis, select-all for context export
    pub(crate) fn set_context_selection_all(&mut self, res: &crate::model::AnalysisResult) {
        let mut files = Vec::new();
        Self::collect_all_files(&res.root, &mut files);
        self.tree.context_selected_files = files.into_iter().collect();
    }

    fn collect_all_files(node: &crate::model::DirNode, out: &mut Vec<String>) {
        for f in &node.files {
            out.push(f.full_path.clone());
        }
        for c in &node.children {
            Self::collect_all_files(c, out);
        }
    }
}
