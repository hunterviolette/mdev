use crate::app::actions::{Action, ComponentId};
use crate::app::state::{AppState, ContextExportMode};
use crate::app::workflow_api::{ensure_run_for_repo, WorkflowApiClient};

use anyhow::{Context, Result};
use serde_json::json;

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
        Action::ContextToggleSkipBinary { exporter_id } => {
            if let Some(ex) = state.context_exporters.get_mut(exporter_id) {
                ex.skip_binary = !ex.skip_binary;
            }
            true
        }
        Action::ContextToggleSkipGitignore { exporter_id } => {
            if let Some(ex) = state.context_exporters.get_mut(exporter_id) {
                ex.skip_gitignore = !ex.skip_gitignore;
            }
            true
        }
        Action::ContextToggleIncludeStagedDiff { exporter_id } => {
            if let Some(ex) = state.context_exporters.get_mut(exporter_id) {
                ex.include_staged_diff = !ex.include_staged_diff;
            }
            true
        }
        Action::ContextToggleIncludeUnstagedDiff { exporter_id } => {
            if let Some(ex) = state.context_exporters.get_mut(exporter_id) {
                ex.include_unstaged_diff = !ex.include_unstaged_diff;
            }
            true
        }
        Action::ContextRestoreSelectionDefaults { exporter_id } => {
            let defaults = match state.context_exporters.get(exporter_id) {
                Some(ex) => ex.selection_defaults.clone(),
                None => return false,
            };
            state.tree.context_selected_files = defaults.clone();
            let key = state.inputs.git_ref.clone();
            state.tree.context_selected_by_ref.insert(key, defaults);
            true
        }
        Action::ContextGenerate { exporter_id } => {
            start_context_export_via_api(state, *exporter_id);
            true
        }
        _ => false,
    }
}

impl AppState {
    pub(crate) fn generate_current_context_text(&mut self) -> Result<String> {
        let repo = self.inputs.repo.clone().context("No repo selected.")?;
        let include_files = if self.tree.context_selected_files.is_empty() {
            None
        } else {
            let mut v: Vec<String> = self.tree.context_selected_files.iter().cloned().collect();
            v.sort();
            Some(v)
        };

        let out_path = {
            use std::time::{SystemTime, UNIX_EPOCH};
            let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
            let mut p = std::env::temp_dir();
            p.push(format!("repo_context_{ts}.txt"));
            p
        };

        let api = build_api_client();
        let run_id = ensure_run_for_repo(&api, &repo, "Generated context text")?;
        api.invoke_context_export(
            &run_id,
            Some("context_export"),
            json!({
                "repo_ref": repo.to_string_lossy().replace('\\', "/"),
                "git_ref": self.inputs.git_ref,
                "exclude_regex": self.inputs.exclude_regex,
                "skip_binary": true,
                "skip_gitignore": true,
                "include_staged_diff": false,
                "include_unstaged_diff": false,
                "include_files": include_files,
                "save_path": out_path.to_string_lossy().replace('\\', "/")
            }),
        )?;

        let text = std::fs::read_to_string(&out_path)
            .with_context(|| format!("Failed to read temp context file {}", out_path.display()))?;
        let _ = std::fs::remove_file(&out_path);
        Ok(text)
    }
}

fn build_api_client() -> WorkflowApiClient {
    let base = std::env::var("MDEV_WORKFLOW_API_BASE").unwrap_or_else(|_| "http://127.0.0.1:8787".to_string());
    WorkflowApiClient::new(base)
}

fn set_status(state: &mut AppState, exporter_id: ComponentId, status: impl Into<String>) {
    if let Some(ex) = state.context_exporters.get_mut(&exporter_id) {
        ex.status = Some(status.into());
    }
}

fn start_context_export_via_api(state: &mut AppState, exporter_id: ComponentId) {
    let Some(repo) = state.inputs.repo.clone() else {
        set_status(state, exporter_id, "No repo selected.");
        return;
    };

    let (mode, skip_binary, skip_gitignore, include_staged_diff, include_unstaged_diff, save_path) =
        match state.context_exporters.get(&exporter_id) {
            Some(ex) => (
                ex.mode,
                ex.skip_binary,
                ex.skip_gitignore,
                ex.include_staged_diff,
                ex.include_unstaged_diff,
                ex.save_path.clone(),
            ),
            None => return,
        };

    let include_files = match mode {
        ContextExportMode::EntireRepo => None,
        ContextExportMode::TreeSelect => Some(state.tree.context_selected_files.iter().cloned().collect::<Vec<_>>()),
    };

    let out_path = match save_path {
        Some(p) => p,
        None => {
            set_status(state, exporter_id, "Choose a save path first.");
            return;
        }
    };

    let api = build_api_client();
    let result: Result<String> = (|| {
        let run_id = ensure_run_for_repo(&api, &repo, "Context export")?;
        let response = api.invoke_context_export(
            &run_id,
            Some("context_export"),
            json!({
                "repo_ref": repo.to_string_lossy().replace('\\', "/"),
                "git_ref": state.inputs.git_ref,
                "exclude_regex": state.inputs.exclude_regex,
                "skip_binary": skip_binary,
                "skip_gitignore": skip_gitignore,
                "include_staged_diff": include_staged_diff,
                "include_unstaged_diff": include_unstaged_diff,
                "include_files": include_files,
                "save_path": out_path.to_string_lossy().replace('\\', "/")
            }),
        )?;

        let output_path = response.get("output_path").and_then(|v| v.as_str()).unwrap_or_default();
        Ok(format!("Context export completed. run_id={} path={}", run_id, output_path))
    })();

    match result {
        Ok(msg) => set_status(state, exporter_id, msg),
        Err(err) => set_status(state, exporter_id, format!("Context export failed: {err:#}")),
    }
}
