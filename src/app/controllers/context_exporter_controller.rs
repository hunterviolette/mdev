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
            state.start_context_export_async(*exporter_id);
            true
        }

        _ => false,
    }
}

impl AppState {
    pub(crate) fn start_context_export_async(&mut self, exporter_id: ComponentId) {
        let Some(repo) = self.inputs.repo.clone() else {
            if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                ex.status = Some("No repo selected.".into());
            }
            return;
        };

        let (mode, out_path, skip_binary, skip_gitignore, include_staged_diff, include_unstaged_diff, already_pending) =
            match self.context_exporters.get(&exporter_id) {
                Some(ex) => (
                    ex.mode,
                    ex.save_path.clone(),
                    ex.skip_binary,
                    ex.skip_gitignore,
                    ex.include_staged_diff,
                    ex.include_unstaged_diff,
                    ex.export_pending,
                ),
                None => return,
            };

        if already_pending {
            if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                ex.status = Some("Generating…".into());
            }
            return;
        }

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

        let git_ref = self.inputs.git_ref.clone();
        let exclude_regex = self.inputs.exclude_regex.clone();

        let (tx, rx) = std::sync::mpsc::channel::<Result<u128, String>>();

        std::thread::spawn(move || {
            use regex::Regex;
            use std::time::{SystemTime, UNIX_EPOCH};

            let compiled: Result<Vec<Regex>, String> = exclude_regex
                .into_iter()
                .map(|p| Regex::new(&p).map_err(|e| format!("Bad exclude regex '{p}': {e}")))
                .collect();

            let res: Result<u128, String> = match compiled {
                Ok(compiled) => {
                    let opts = crate::git::ContextExportOptions {
                        git_ref: &git_ref,
                        exclude: &compiled,
                        skip_binary,
                        skip_gitignore,
                        include_staged_diff,
            include_unstaged_diff,
                        include_files: include_files.as_deref(),
                    };

                    match crate::git::export_repo_context(&repo, &out_path, opts) {
                        Ok(_) => {
                            let now_ms = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis();
                            Ok(now_ms)
                        }
                        Err(e) => Err(format!("{:#}", e)),
                    }
                }
                Err(e) => Err(e),
            };

            let _ = tx.send(res);
        });

        if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
            ex.export_pending = true;
            ex.export_rx = Some(rx);
            ex.status = Some("Generating…".into());
        }
    }

    fn generate_context_file(&mut self, exporter_id: ComponentId) {
        let Some(repo) = self.inputs.repo.clone() else {
            if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                ex.status = Some("No repo selected.".into());
            }
            return;
        };

        let (mode, out_path, skip_binary, skip_gitignore, include_staged_diff, include_unstaged_diff) =
            match self.context_exporters.get(&exporter_id) {
                Some(ex) => (
                    ex.mode,
                    ex.save_path.clone(),
                    ex.skip_binary,
                    ex.skip_gitignore,
                    ex.include_staged_diff,
                    ex.include_unstaged_diff,
                ),
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

        let env_included = {
            let planned: Vec<String> = if let Some(sel) = include_files.as_ref() {
                sel.clone()
            } else {
                let files: Vec<String> = if self.inputs.git_ref == crate::app::state::WORKTREE_REF {
                    crate::git::list_worktree_files(&repo).unwrap_or_default()
                } else {
                    let ls = crate::git::run_git(&repo, &["ls-tree", "-r", "--name-only", &self.inputs.git_ref]).unwrap_or_default();
                    String::from_utf8_lossy(&ls)
                        .lines()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                };
                files
            };

            planned.into_iter().any(|p| {
                let p = p.replace('\\', "/");
                p == ".env" || p.ends_with("/.env") || p.contains("/.env.")
            })
        };

        if env_included {
            if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                ex.status = Some("WARNING: One or more .env files are included in the export selection. Review before sharing.".into());
            }
        }

        let req = ContextExportReq {
            repo,
            out_path,
            git_ref: self.inputs.git_ref.clone(),
            exclude_regex: self.inputs.exclude_regex.clone(),
            skip_binary,
            skip_gitignore,
            include_staged_diff,
            include_unstaged_diff,
            include_files,
        };

        match self.broker.exec(CapabilityRequest::ExportContext(req)) {
            Ok(CapabilityResponse::Unit) => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let now_ms: u128 = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                if let Some(ex) = self.context_exporters.get_mut(&exporter_id) {
                    use time::format_description::well_known::Rfc3339;
                    use time::OffsetDateTime;

                    let nanos = (now_ms as i128) * 1_000_000;
                    let ts = OffsetDateTime::from_unix_timestamp_nanos(nanos)
                        .ok()
                        .and_then(|dt| dt.format(&Rfc3339).ok())
                        .unwrap_or_else(|| now_ms.to_string());

                    ex.status = Some(format!("generated at {}", ts));
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

    pub(crate) fn generate_current_context_text(&mut self) -> Result<String> {
        let repo = self
            .inputs
            .repo
            .clone()
            .context("No repo selected.")?;

        let include_files: Option<Vec<String>> = if self.tree.context_selected_files.is_empty() {
            None
        } else {
            let mut v: Vec<String> = self.tree.context_selected_files.iter().cloned().collect();
            v.sort();
            Some(v)
        };

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

        let include_staged_diff = false;
        let include_unstaged_diff = false;

        let req = ContextExportReq {
            repo,
            out_path: out_path.clone(),
            git_ref: self.inputs.git_ref.clone(),
            exclude_regex: self.inputs.exclude_regex.clone(),
            skip_binary: true,
            skip_gitignore: true,
            include_staged_diff,
            include_unstaged_diff,
            include_files,
        };

        match self.broker.exec(CapabilityRequest::ExportContext(req)) {
            Ok(CapabilityResponse::Unit) => {
                let text = std::fs::read_to_string(&out_path)
                    .with_context(|| format!("Failed to read temp context file {}", out_path.display()))?;

                let _ = std::fs::remove_file(&out_path);

                Ok(text)
            }
            Ok(_) => anyhow::bail!("Unexpected response exporting context."),
            Err(e) => Err(e).context("ExportContext failed"),
        }
    }
}
