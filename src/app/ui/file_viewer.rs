use eframe::egui;
use egui_extras::syntax_highlighting::highlight;

use super::helpers::language_hint_for_path;

use super::super::actions::{Action, ComponentId};
use super::super::state::AppState;

pub fn file_viewer(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    viewer_id: ComponentId,
) -> Vec<Action> {
    let mut actions = vec![];

    let Some(v) = state.file_viewers.get(&viewer_id) else {
        ui.label("Missing file viewer state.");
        return actions;
    };

    let Some(path) = v.selected_file.clone() else {
        ui.label("Click a file in the tree to view its full contents.");
        return actions;
    };

    let v = state.file_viewers.get(&viewer_id).unwrap();

    // Header row
    ui.horizontal(|ui| {
        ui.label("File:");
        ui.monospace(&path);

        ui.separator();

        ui.label("View at:");
        let current_label = if let Some(h) = &v.selected_commit {
            let short = &h[..std::cmp::min(10, h.len())];
            format!("{short} (commit)")
        } else {
            format!("{} (ref)", state.inputs.git_ref)
        };

        egui::ComboBox::from_id_source(("commit_picker", viewer_id, &path))
            .selected_text(current_label)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(
                        v.selected_commit.is_none(),
                        format!("{} (ref)", state.inputs.git_ref),
                    )
                    .clicked()
                {
                    actions.push(Action::SelectCommit {
                        viewer_id,
                        sel: None,
                    });
                }

                for c in v.file_commits.iter() {
                    let short = &c.hash[..std::cmp::min(10, c.hash.len())];

                    let label = format!("{short} {} — {}", c.date, c.summary);
                    let is_sel = v.selected_commit.as_deref() == Some(c.hash.as_str());
                    if ui.selectable_label(is_sel, label).clicked() {
                        actions.push(Action::SelectCommit {
                            viewer_id,
                            sel: Some(c.hash.clone()),
                        });
                    }
                }
            });

        if ui.button("Refresh").clicked() {
            actions.push(Action::RefreshFile { viewer_id });
        }

        if ui.button("Diff…").clicked() {
            actions.push(Action::ToggleDiff { viewer_id });
        }
    });

    // Diff picker window
    let mut open_diff = state
        .file_viewers
        .get(&viewer_id)
        .map(|v| v.show_diff)
        .unwrap_or(false);

    egui::Window::new("Diff")
        .id(egui::Id::new(("diff_window", viewer_id, &path)))
        .open(&mut open_diff)
        .collapsible(false)
        .resizable(true)
        .show(ctx, |ui| {
            ui.label("Show changes for this file between two commits/refs.");
            ui.add_space(6.0);

            // Base
            ui.horizontal(|ui| {
                ui.label("From:");
                let v = state.file_viewers.get(&viewer_id).unwrap();
                let cur = v
                    .diff_base
                    .as_deref()
                    .map(|h| {
                        let short = &h[..std::cmp::min(10, h.len())];
                        format!("{short} (commit)")
                    })
                    .unwrap_or_else(|| format!("{} (ref)", state.inputs.git_ref));

                egui::ComboBox::from_id_source(("diff_base_picker", viewer_id, &path))
                    .selected_text(cur)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                v.diff_base.is_none(),
                                format!("{} (ref)", state.inputs.git_ref),
                            )
                            .clicked()
                        {
                            actions.push(Action::SetDiffBase {
                                viewer_id,
                                sel: None,
                            });
                        }

                        for c in v.file_commits.iter() {
                            let short = &c.hash[..std::cmp::min(10, c.hash.len())];
                            let label = format!("{short} {} — {}", c.date, c.summary);
                            let is_sel = v.diff_base.as_deref() == Some(c.hash.as_str());
                            if ui.selectable_label(is_sel, label).clicked() {
                                actions.push(Action::SetDiffBase {
                                    viewer_id,
                                    sel: Some(c.hash.clone()),
                                });
                            }
                        }
                    });
            });

            // Target
            ui.horizontal(|ui| {
                ui.label("To:");
                let v = state.file_viewers.get(&viewer_id).unwrap();
                let cur = v
                    .diff_target
                    .as_deref()
                    .map(|h| {
                        let short = &h[..std::cmp::min(10, h.len())];
                        format!("{short} (commit)")
                    })
                    .unwrap_or_else(|| format!("{} (ref)", state.inputs.git_ref));

                egui::ComboBox::from_id_source(("diff_target_picker", viewer_id, &path))
                    .selected_text(cur)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                v.diff_target.is_none(),
                                format!("{} (ref)", state.inputs.git_ref),
                            )
                            .clicked()
                        {
                            actions.push(Action::SetDiffTarget {
                                viewer_id,
                                sel: None,
                            });
                        }

                        for c in v.file_commits.iter() {
                            let short = &c.hash[..std::cmp::min(10, c.hash.len())];
                            let label = format!("{short} {} — {}", c.date, c.summary);
                            let is_sel = v.diff_target.as_deref() == Some(c.hash.as_str());
                            if ui.selectable_label(is_sel, label).clicked() {
                                actions.push(Action::SetDiffTarget {
                                    viewer_id,
                                    sel: Some(c.hash.clone()),
                                });
                            }
                        }
                    });
            });

            ui.add_space(8.0);

            ui.horizontal(|ui| {
                if ui.button("Generate diff").clicked() {
                    actions.push(Action::RefreshDiff { viewer_id });
                }

                if ui.button("Close").clicked() {
                    actions.push(Action::ToggleDiff { viewer_id });
                }
            });

            let v = state.file_viewers.get(&viewer_id).unwrap();
            if let Some(err) = &v.diff_err {
                ui.add_space(6.0);
                ui.colored_label(egui::Color32::LIGHT_RED, err);
            }
        });

    // Sync close button with state
    if open_diff != v.show_diff {
        actions.push(Action::ToggleDiff { viewer_id });
    }

    // File content error
    let v = state.file_viewers.get(&viewer_id).unwrap();
    if let Some(err) = &v.file_content_err {
        ui.colored_label(egui::Color32::LIGHT_RED, err);
    }

    ui.add_space(6.0);

    let remaining_h = ui.available_height().max(120.0);
    let available_w = ui.available_width();

    let code_rect = ui
        .allocate_exact_size(egui::vec2(available_w, remaining_h), egui::Sense::hover())
        .0;

    ui.allocate_ui_at_rect(code_rect, |ui| {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_size(ui.available_size());

            let v = state.file_viewers.get(&viewer_id).unwrap();

            let (text, language): (&str, &str) =
                if v.show_diff && (!v.diff_text.is_empty() || v.diff_err.is_some()) {
                    (v.diff_text.as_str(), "diff")
                } else {
                    (v.file_content.as_str(), language_hint_for_path(&path))
                };

            let job = highlight(ctx, &state.theme.code_theme, text, language);

            egui::ScrollArea::both()
                .id_source(("file_content_scroll", viewer_id, &path))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.label(job);
                });
        });
    });

    actions
}
