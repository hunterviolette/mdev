use eframe::egui;
use egui_extras::syntax_highlighting::highlight;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::{AppState, FileViewAt, WORKTREE_REF};

use super::code_editor;
use super::helpers::language_hint_for_path;

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn diff_overlay(
    ctx: &egui::Context,
    bounds: egui::Rect,
    state: &mut AppState,
    viewer_id: ComponentId,
    path: &str,
) -> Vec<Action> {
    let mut actions = vec![];

    let panel_w = (bounds.width() * 0.80).clamp(520.0, 920.0);
    let panel_h = (bounds.height() * 0.70).clamp(320.0, 640.0);
    let panel_rect =
        egui::Rect::from_center_size(bounds.center(), egui::vec2(panel_w, panel_h));

    let mut close_clicked = false;

    egui::Area::new(egui::Id::new(("diff_overlay", viewer_id, path)))
        .order(egui::Order::Foreground)
        .fixed_pos(panel_rect.min)
        .constrain_to(bounds)
        .show(ctx, |ui| {
            ui.set_min_size(panel_rect.size());

            egui::Frame::popup(ui.style())
                .rounding(egui::Rounding::same(10.0))
                .shadow(ui.style().visuals.popup_shadow)
                .show(ui, |ui| {
                    ui.set_min_size(panel_rect.size());

                    ui.horizontal(|ui| {
                        ui.heading("Diff");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✕").clicked() {
                                close_clicked = true;
                            }
                        });
                    });

                    ui.add_space(6.0);
                    ui.label("Show changes for this file between two commits/refs.");
                    ui.add_space(8.0);

                    let v = state.file_viewers.get(&viewer_id).unwrap();

                    // Base
                    ui.horizontal(|ui| {
                        ui.label("From:");
                        let cur = v
                            .diff_base
                            .as_deref()
                            .map(|h| {
                                let short = &h[..std::cmp::min(10, h.len())];
                                format!("{short} (commit)")
                            })
                            .unwrap_or_else(|| format!("{} (ref)", state.inputs.git_ref));

                        egui::ComboBox::from_id_source(("diff_base_picker", viewer_id, path))
                            .selected_text(cur)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(
                                        v.diff_base.is_none(),
                                        format!("{} (ref)", state.inputs.git_ref),
                                    )
                                    .clicked()
                                {
                                    actions.push(Action::SetDiffBase { viewer_id, sel: None });
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
                        let cur = v
                            .diff_target
                            .as_deref()
                            .map(|h| {
                                let short = &h[..std::cmp::min(10, h.len())];
                                format!("{short} (commit)")
                            })
                            .unwrap_or_else(|| format!("{} (ref)", state.inputs.git_ref));

                        egui::ComboBox::from_id_source(("diff_target_picker", viewer_id, path))
                            .selected_text(cur)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(
                                        v.diff_target.is_none(),
                                        format!("{} (ref)", state.inputs.git_ref),
                                    )
                                    .clicked()
                                {
                                    actions.push(Action::SetDiffTarget { viewer_id, sel: None });
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

                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        if ui.button("Generate diff").clicked() {
                            actions.push(Action::RefreshDiff { viewer_id });

                            if let Some(v) = state.file_viewers.get_mut(&viewer_id) {
                                v.diff_picker_open = false;
                            }
                        }
                        if ui.button("Close").clicked() {
                            close_clicked = true;
                        }
                    });

                    let v = state.file_viewers.get(&viewer_id).unwrap();
                    if let Some(err) = &v.diff_err {
                        ui.add_space(6.0);
                        ui.colored_label(egui::Color32::LIGHT_RED, err);
                    }
                });
        });

    if close_clicked {
        if let Some(v) = state.file_viewers.get_mut(&viewer_id) {
            v.diff_picker_open = false;
        }
    }

    actions
}

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

    // ─────────────────────────────────────────────────────────────
    // Component title: filename + full path (restored)
    // ─────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.heading(basename(&path));
        ui.add_space(8.0);
        ui.weak(&path);
    });

    ui.add_space(6.0);

    // ─────────────────────────────────────────────────────────────
    // Header toolbar: Top bar ref OR Working tree OR commit list
    // ─────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        let v = state.file_viewers.get(&viewer_id).unwrap();

        ui.label("View:");

        let selected_text = match v.view_at {
            FileViewAt::FollowTopBar => {
                if state.inputs.git_ref == WORKTREE_REF {
                    "Top bar (Working tree)".to_string()
                } else {
                    format!("Top bar ({})", state.inputs.git_ref)
                }
            }
            FileViewAt::WorkingTree => "Working tree".to_string(),
            FileViewAt::Commit => {
                if let Some(h) = v.selected_commit.as_deref() {
                    let short = &h[..std::cmp::min(10, h.len())];
                    format!("{short} (commit)")
                } else {
                    "Commit".to_string()
                }
            }
        };

        egui::ComboBox::from_id_source(("file_view_at_combo", viewer_id, &path))
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                // Top bar ref
                let is_topbar = v.view_at == FileViewAt::FollowTopBar;
                if ui.selectable_label(is_topbar, "Top bar ref").clicked() {
                    actions.push(Action::SetViewerViewAt {
                        viewer_id,
                        view_at: FileViewAt::FollowTopBar,
                    });
                }

                // Working tree (ALWAYS available at file level)
                let is_wt = v.view_at == FileViewAt::WorkingTree;
                if ui.selectable_label(is_wt, "Working tree").clicked() {
                    actions.push(Action::SetViewerViewAt {
                        viewer_id,
                        view_at: FileViewAt::WorkingTree,
                    });
                }

                ui.separator();

                // Commit history (full file version at each commit)
                if v.file_commits.is_empty() {
                    ui.weak("No history loaded for this file yet.");
                } else {
                    for c in v.file_commits.iter() {
                        let short = &c.hash[..std::cmp::min(10, c.hash.len())];
                        let label = format!("{short} {} — {}", c.date, c.summary);
                        let is_sel = v.view_at == FileViewAt::Commit
                            && v.selected_commit.as_deref() == Some(c.hash.as_str());
                        if ui.selectable_label(is_sel, label).clicked() {
                            actions.push(Action::SelectCommit {
                                viewer_id,
                                sel: Some(c.hash.clone()),
                            });
                        }
                    }
                }
            });

        ui.separator();

        // Editing controls
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

        // Diff toggle remains independent
        if ui.button(if v.show_diff { "Hide Diff" } else { "Show Diff" }).clicked() {
            actions.push(Action::ToggleDiff { viewer_id });
        }

        // Reopen picker without turning diff off
        if v.show_diff && ui.small_button("Diff options…").clicked() {
            if let Some(v) = state.file_viewers.get_mut(&viewer_id) {
                v.diff_picker_open = true;
            }
        }
    });

    // Errors/status
    let v = state.file_viewers.get(&viewer_id).unwrap();
    if let Some(err) = &v.file_content_err {
        ui.colored_label(egui::Color32::LIGHT_RED, err);
    }
    if let Some(msg) = &v.edit_status {
        ui.label(msg);
    }

    ui.add_space(6.0);

    // ─────────────────────────────────────────────────────────────
    // Body
    // ─────────────────────────────────────────────────────────────
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
                let editor_id_source = format!("viewer:{:?}|path:{}", viewer_id, path);

                let _changed = code_editor::code_editor(
                    ctx,
                    ui,
                    &state.theme.code_theme,
                    &editor_id_source,
                    &path,
                    &mut v.edit_buffer,
                    &mut v.editor,
                );
            } else {
                // If diff is enabled, show diff output (if any) as highlighted 'diff'
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

    // Diff picker overlay: ONLY when explicitly open
    let show_picker = state
        .file_viewers
        .get(&viewer_id)
        .map(|v| v.diff_picker_open)
        .unwrap_or(false);

    if show_picker {
        let bounds = ui.max_rect();
        actions.extend(diff_overlay(ctx, bounds, state, viewer_id, &path));
    }

    actions
}
