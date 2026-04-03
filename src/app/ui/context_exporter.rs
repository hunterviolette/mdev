use eframe::egui;

use super::super::actions::{Action, ComponentId};
use super::super::state::{AppState, ContextExportMode};

pub fn context_exporter(
    ui: &mut egui::Ui,
    state: &mut AppState,
    exporter_id: ComponentId,
) -> Vec<Action> {
    let mut actions = vec![];

    let Some(ex) = state.context_exporters.get_mut(&exporter_id) else {
        ui.label("Missing context exporter state.");
        return actions;
    };

    ex.export_pending = false;
    ex.export_rx = None;

    ui.add_space(6.0);

    let has_repo = state.inputs.repo.is_some();
    if !has_repo {
        ui.colored_label(egui::Color32::LIGHT_RED, "No repo selected. Pick a repo first.");
    }

    let home_dir = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf());
    let format_path = |p: &std::path::PathBuf| -> String {
        let raw = p.display().to_string();
        let Some(h) = home_dir.as_ref() else { return raw; };
        if p.starts_with(h) {
            let rel = p.strip_prefix(h).ok();
            if let Some(rel) = rel {
                let rel_s = rel.display().to_string().replace('\\', "/");
                if rel_s.is_empty() {
                    return "~/".to_string();
                }
                return format!("~/{}", rel_s.trim_start_matches('/'));
            }
        }
        raw
    };

    let env_selected = {
        let planned: Vec<String> = match ex.mode {
            ContextExportMode::TreeSelect => state.tree.context_selected_files.iter().cloned().collect(),
            ContextExportMode::EntireRepo => match state.inputs.repo.as_ref() {
                None => Vec::new(),
                Some(repo) => {
                    if state.inputs.git_ref == crate::app::state::WORKTREE_REF {
                        crate::git::list_worktree_files(repo).unwrap_or_default()
                    } else {
                        let ls = crate::git::run_git(
                            repo,
                            &["ls-tree", "-r", "--name-only", &state.inputs.git_ref],
                        )
                        .unwrap_or_default();
                        String::from_utf8_lossy(&ls)
                            .lines()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    }
                }
            }
        };

        planned.into_iter().any(|p| {
            let p = p.replace('\\', "/");
            p == ".env" || p.ends_with("/.env") || p.contains("/.env.")
        })
    };

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !has_repo {
                return;
            }

            
            ui.add_space(8.0);


            ui.horizontal_wrapped(|ui| {
                let mut skip_bin = ex.skip_binary;
                if ui.checkbox(&mut skip_bin, "Skip binary").clicked() {
                    actions.push(Action::ContextToggleSkipBinary { exporter_id });
                }

                let mut skip_gitignore = ex.skip_gitignore;
                if ui.checkbox(&mut skip_gitignore, "Skip .gitignore").clicked() {
                    actions.push(Action::ContextToggleSkipGitignore { exporter_id });
                }

                let is_worktree = state.inputs.git_ref == crate::app::state::WORKTREE_REF;

                let mut staged_diff = ex.include_staged_diff;
                if ui
                    .add_enabled(is_worktree, egui::Checkbox::new(&mut staged_diff, "Staged diff"))
                    .clicked()
                {
                    actions.push(Action::ContextToggleIncludeStagedDiff { exporter_id });
                }

                let mut unstaged_diff = ex.include_unstaged_diff;
                if ui
                    .add_enabled(is_worktree, egui::Checkbox::new(&mut unstaged_diff, "Unstaged diff"))
                    .clicked()
                {
                    actions.push(Action::ContextToggleIncludeUnstagedDiff { exporter_id });
                }
            });


            ui.add_space(10.0);

            ui.horizontal_wrapped(|ui| {
                ui.label("Save to:");
                let path_txt = ex
                    .save_path
                    .as_ref()
                    .map(|p| format_path(p))
                    .unwrap_or_else(|| "(not set)".to_string());
                ui.monospace(path_txt);

                if ui.button("Choose…").clicked() {
                    actions.push(Action::ContextPickSavePath { exporter_id });
                }
            });

            ui.add_space(8.0);

            ui.horizontal_wrapped(|ui| {
                ui.label("Mode:");
                let cur = match ex.mode {
                    ContextExportMode::EntireRepo => "ENTIRE REPO",
                    ContextExportMode::TreeSelect => "TREE SELECT",
                };

                egui::ComboBox::from_id_source(("context_mode", exporter_id))
                    .selected_text(cur)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(ex.mode == ContextExportMode::EntireRepo, "ENTIRE REPO").clicked() {
                            ex.mode = ContextExportMode::EntireRepo;
                        }
                        if ui.selectable_label(ex.mode == ContextExportMode::TreeSelect, "TREE SELECT").clicked() {
                            ex.mode = ContextExportMode::TreeSelect;
                        }
                    });

                if ui
                    .add_enabled(
                        !ex.selection_defaults.is_empty(),
                        egui::Button::new("Restore selection defaults"),
                    )
                    .on_hover_text("restores workspace defaults selections for context exporter to file tree")
                    .clicked()
                {
                    actions.push(Action::ContextRestoreSelectionDefaults { exporter_id });
                }

                ui.separator();

                ui.label("Ref:");
                ui.monospace(&state.inputs.git_ref);

                if state.inputs.git_ref != crate::app::state::WORKTREE_REF {
                    ui.add_space(6.0);
                    ui.weak("(diffs require Ref=WORKTREE)");
                }
            });


            ui.add_space(12.0);

            ui.separator();

            if env_selected {
                ui.add_space(6.0);
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    "WARNING: .env file(s) are currently included in the selection.",
                );
                ui.add_space(6.0);
            }

            let can_generate = ex.save_path.is_some();

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(can_generate, egui::Button::new("Generate context file"))
                    .clicked()
                {
                    actions.push(Action::ContextGenerate { exporter_id });
                }

                if let Some(msg) = &ex.status {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.label(msg);
                }
            });

            if !can_generate {
                ui.add_space(4.0);
                ui.label("Choose an output path to enable generation.");
            }
        });

    actions
}


