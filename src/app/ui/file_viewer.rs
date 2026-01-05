// src/app/ui/file_viewer.rs
use eframe::egui;
use egui_extras::syntax_highlighting::highlight;

use super::code_editor;
use super::helpers::language_hint_for_path;
use super::super::actions::{Action, ComponentId};
use super::super::state::FileViewAt;
use super::super::state::{AppState, WORKTREE_REF};

pub fn file_viewer(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    viewer_id: ComponentId,
) -> Vec<Action> {
    let mut actions = vec![];

    let Some(v_ro) = state.file_viewers.get(&viewer_id) else {
        ui.label("Missing file viewer state.");
        return actions;
    };

    let Some(path) = v_ro.selected_file.clone() else {
        ui.label("Click a file in the tree to view its full contents.");
        return actions;
    };

    let v = state.file_viewers.get(&viewer_id).unwrap();

    let top_bar_label = if state.inputs.git_ref == WORKTREE_REF {
        "Working tree".to_string()
    } else {
        state.inputs.git_ref.clone()
    };

    ui.horizontal(|ui| {
        ui.label("File:");
        ui.monospace(&path);

        ui.separator();

        ui.label("View at:");

        let selected_text = if let Some(h) = &v.selected_commit {
            let short = &h[..std::cmp::min(10, h.len())];
            format!("{short} (commit)")
        } else {
            match v.view_at {
                FileViewAt::FollowTopBar => format!("Top bar: {top_bar_label}"),
                FileViewAt::WorkingTree => "Working tree".to_string(),
                FileViewAt::Commit => format!("Top bar: {top_bar_label}"),
            }
        };

        egui::ComboBox::from_id_source(("view_at_combo", viewer_id, &path))
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                let follow_selected =
                    v.selected_commit.is_none() && v.view_at == FileViewAt::FollowTopBar;
                let wt_selected =
                    v.selected_commit.is_none() && v.view_at == FileViewAt::WorkingTree;

                if ui
                    .selectable_label(follow_selected, format!("Follow top bar ({top_bar_label})"))
                    .clicked()
                {
                    actions.push(Action::SetViewerViewAt {
                        viewer_id,
                        view_at: FileViewAt::FollowTopBar,
                    });
                    actions.push(Action::SelectCommit { viewer_id, sel: None });
                }

                if ui.selectable_label(wt_selected, "Working tree").clicked() {
                    actions.push(Action::SetViewerViewAt {
                        viewer_id,
                        view_at: FileViewAt::WorkingTree,
                    });
                    actions.push(Action::SelectCommit { viewer_id, sel: None });
                }

                ui.separator();

                for c in v.file_commits.iter() {
                    let short = &c.hash[..std::cmp::min(10, c.hash.len())];
                    let label = format!("{short}  {}  {}", c.date, c.summary);
                    if ui
                        .selectable_label(v.selected_commit.as_deref() == Some(&c.hash), label)
                        .clicked()
                    {
                        actions.push(Action::SelectCommit {
                            viewer_id,
                            sel: Some(c.hash.clone()),
                        });
                    }
                }
            });

        ui.separator();

        let is_editing = v.edit_working_tree;
        if ui.selectable_label(is_editing, "Edit working tree").clicked() {
            actions.push(Action::ToggleEditWorkingTree { viewer_id });
        }

        let can_save = is_editing && state.inputs.repo.is_some();
        if ui.add_enabled(can_save, egui::Button::new("Save")).clicked() {
            actions.push(Action::SaveWorkingTreeFile { viewer_id });
        }

        ui.separator();

        if ui.button("Refresh").clicked() {
            actions.push(Action::RefreshFile { viewer_id });
        }

        ui.separator();

        if ui.button(if v.show_diff { "Hide Diff" } else { "Show Diff" }).clicked() {
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
    if let Some(msg) = &v.edit_status {
        ui.label(msg);
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

            let v = state.file_viewers.get_mut(&viewer_id).unwrap();

            if v.edit_working_tree {
                // Unique focus id per editor instance:
                let editor_id_source = format!("viewer:{:?}|path:{}", viewer_id, path);

                let _changed = code_editor::code_editor(
                    ctx,
                    ui,
                    &state.theme.code_theme,
                    &editor_id_source,
                    &path, //  real path drives language highlighting
                    &mut v.edit_buffer,
                    &mut v.editor,
                );
            } else {
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
            }
        });
    });

    actions
}
