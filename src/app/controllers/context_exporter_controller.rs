use crate::app::actions::{Action, ComponentId};
use crate::app::state::AppState;
use crate::capabilities::{CapabilityRequest, CapabilityResponse, ContextExportReq};

use anyhow::{Context, Result};

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::ContextPickSavePath { exporter_id } => {
            let default_name = "repo_context.txt";
            if let Some(path) = state.platform.save_file("Save context file", default_name) {
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

        let include_files: Option<Vec<String>> = match mode {
            crate::app::state::ContextExportMode::EntireRepo => None,
            crate::app::state::ContextExportMode::TreeSelect => {
                let mut v: Vec<String> = self.tree.context_selected_files.iter().cloned().collect();
                v.sort();
                Some(v)
            }
        };

        let req = ContextExportReq {
            repo,
            out_path,
            git_ref: self.inputs.git_ref.clone(),
            exclude_regex: self.inputs.exclude_regex.clone(),
            max_bytes_per_file,
            skip_binary,
            include_files,
        };

        match self.broker.exec(CapabilityRequest::ExportContext(req)) {
            Ok(CapabilityResponse::Unit) => {
                if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                    ex.status = Some("Wrote context file successfully.".into());
                }
            }
            Ok(_) => {
                if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                    ex.status = Some("Unexpected response exporting context.".into());
                }
            }
            Err(e) => {
                if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                    ex.status = Some(format!("{:#}", e));
                }
            }
        }
    }

    /// Generate the *current* context text using the same ExportContext capability as the
    /// Context Exporter component, but writing to a temp file and reading it back.
    ///
    /// - If the Tree has context-selected files, we export only those.
    /// - Otherwise we export the full repo for the currently selected git_ref.
    ///
    /// This is intended for ExecuteLoop autonomy (no user-provided path needed).
    pub(crate) fn generate_current_context_text(&mut self) -> Result<String> {
        let repo = self
            .inputs
            .repo
            .clone()
            .context("No repo selected.")?;

        // If the user selected files in Tree, honor that selection.
        let include_files: Option<Vec<String>> = if self.tree.context_selected_files.is_empty() {
            None
        } else {
            let mut v: Vec<String> = self.tree.context_selected_files.iter().cloned().collect();
            v.sort();
            Some(v)
        };

        // Temp output file
        let out_path = {
            use std::time::{SystemTime, UNIX_EPOCH};
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let mut p = std::env::temp_dir();
            p.push(format!("repo_context_{ts}.txt"));
            p
        };

        // Default to the same-ish defaults as the Context Exporter component state.
        // (If you later want ExecuteLoop to expose these, thread them from its state.)
        let req = ContextExportReq {
            repo,
            out_path: out_path.clone(),
            git_ref: self.inputs.git_ref.clone(),
            exclude_regex: self.inputs.exclude_regex.clone(),
            max_bytes_per_file: 200_000,
            skip_binary: true,
            include_files,
        };

        match self.broker.exec(CapabilityRequest::ExportContext(req)) {
            Ok(CapabilityResponse::Unit) => {
                let text = std::fs::read_to_string(&out_path)
                    .with_context(|| format!("Failed to read temp context file {}", out_path.display()))?;

                // Best-effort cleanup.
                let _ = std::fs::remove_file(&out_path);

                Ok(text)
            }
            Ok(_) => anyhow::bail!("Unexpected response exporting context."),
            Err(e) => Err(e).context("ExportContext failed"),
        }
    }
}
