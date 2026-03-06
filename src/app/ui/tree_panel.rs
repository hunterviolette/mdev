use eframe::egui;

use crate::format;
use crate::model::{AnalysisResult, DirNode, FileRow};

use super::super::actions::{Action, ComponentId, ComponentKind, ExpandCmd};
use super::super::state::AppState;

fn overlay_bounds(ui: &egui::Ui) -> egui::Rect {
    ui.clip_rect().shrink(6.0)
}

fn centered_overlay_rect(
    bounds: egui::Rect,
    w_frac: f32,
    h_frac: f32,
    min: [f32; 2],
    max: [f32; 2],
) -> egui::Rect {
    let pad = 8.0;
    let max_w_by_bounds = (bounds.width() - pad * 2.0).max(1.0);
    let max_h_by_bounds = (bounds.height() - pad * 2.0).max(1.0);

    let min_w = min[0].min(max_w_by_bounds);
    let min_h = min[1].min(max_h_by_bounds);

    let max_w = max[0].min(max_w_by_bounds);
    let max_h = max[1].min(max_h_by_bounds);

    let w = (bounds.width() * w_frac).clamp(min_w, max_w);
    let h = (bounds.height() * h_frac).clamp(min_h, max_h);

    let rect = egui::Rect::from_center_size(bounds.center(), egui::vec2(w, h));
    rect.intersect(bounds)
}

fn modal_blocker(ctx: &egui::Context, bounds: egui::Rect, id: egui::Id, popup_rect: egui::Rect) -> bool {
    let mut clicked_outside = false;

    let popup_rect = popup_rect.intersect(bounds);

    egui::Area::new(id)
        .order(egui::Order::Middle)
        .fixed_pos(bounds.min)
        .constrain_to(bounds)
        .show(ctx, |ui| {
            ui.set_min_size(bounds.size());
            ui.set_max_size(bounds.size());

            ui.painter().rect_filled(
                bounds,
                0.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 90),
            );

            let top = egui::Rect::from_min_max(bounds.min, egui::pos2(bounds.max.x, popup_rect.min.y));
            let bottom = egui::Rect::from_min_max(egui::pos2(bounds.min.x, popup_rect.max.y), bounds.max);
            let left = egui::Rect::from_min_max(
                egui::pos2(bounds.min.x, popup_rect.min.y),
                egui::pos2(popup_rect.min.x, popup_rect.max.y),
            );
            let right = egui::Rect::from_min_max(
                egui::pos2(popup_rect.max.x, popup_rect.min.y),
                egui::pos2(bounds.max.x, popup_rect.max.y),
            );

            for r in [top, bottom, left, right] {
                let r = r.intersect(bounds);
                if r.is_positive() {
                    let resp = ui.allocate_rect(r, egui::Sense::click());
                    if resp.clicked() {
                        clicked_outside = true;
                    }
                }
            }
        });

    clicked_outside
}

fn popup_overlay<R>(
    ctx: &egui::Context,
    bounds: egui::Rect,
    id: egui::Id,
    title: &str,
    rect: egui::Rect,
    mut add_contents: impl FnMut(&mut egui::Ui) -> R,
) -> bool {
    let mut open = true;

    egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .constrain_to(bounds)
        .interactable(true)
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            ui.set_enabled(true);

            egui::Frame::popup(ui.style())
                .rounding(egui::Rounding::same(10.0))
                .shadow(ui.style().visuals.popup_shadow)
                .show(ui, |ui| {
                    ui.set_min_size(rect.size());
                    ui.set_max_size(rect.size());
                    ui.set_enabled(true);

                    ui.horizontal(|ui| {
                        ui.heading(title);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✕").clicked() {
                                open = false;
                            }
                        });
                    });

                    ui.add_space(8.0);
                    let _ = add_contents(ui);
                });
        });

    open
}


fn file_viewer_choices(state: &AppState) -> Vec<(ComponentId, String)> {
    let mut list: Vec<(ComponentId, String)> = state
        .active_layout()
        .components
        .iter()
        .filter(|c| c.kind == ComponentKind::FileViewer)
        .filter(|c| {
            state
                .active_layout()
                .get_window(c.id)
                .map(|w| w.open)
                .unwrap_or(false)
        })
        .map(|c| (c.id, c.title.clone()))
        .collect();

    list.sort_by(|a, b| a.1.cmp(&b.1));
    list
}

fn collect_files_under_dir(node: &DirNode, out: &mut Vec<String>) {
    for f in &node.files {
        out.push(f.full_path.clone());
    }
    for c in &node.children {
        collect_files_under_dir(c, out);
    }
}

fn collect_all_files(res: &AnalysisResult) -> Vec<String> {
    let mut out = Vec::new();
    for f in &res.root.files {
        out.push(f.full_path.clone());
    }
    for d in &res.root.children {
        collect_files_under_dir(d, &mut out);
    }
    out
}

fn dir_selection_state(state: &AppState, node: &DirNode) -> (bool, bool) {
    let mut files = Vec::new();
    collect_files_under_dir(node, &mut files);

    if files.is_empty() {
        return (true, true);
    }

    let mut any = false;
    let mut all = true;
    for f in files {
        let sel = state.tree.context_selected_files.contains(&f);
        any |= sel;
        all &= sel;
    }
    (all, any)
}

fn set_dir_selected(state: &mut AppState, node: &DirNode, selected: bool) {
    let mut files = Vec::new();
    collect_files_under_dir(node, &mut files);

    if selected {
        for f in files {
            state.tree.context_selected_files.insert(f);
        }
    } else {
        for f in files {
            state.tree.context_selected_files.remove(&f);
        }
    }
}


fn file_row(ui: &mut egui::Ui, state: &mut AppState, actions: &mut Vec<Action>, f: &FileRow) {
    let viewers = file_viewer_choices(state);
    let popup_id = egui::Id::new(("open_in_popup", f.full_path.as_str()));

    let mut checked = state.tree.context_selected_files.contains(&f.full_path);

    let link_resp = ui
        .horizontal(|ui| {
            if ui.checkbox(&mut checked, "").clicked() {
                if checked {
                    state.tree.context_selected_files.insert(f.full_path.clone());
                } else {
                    state.tree.context_selected_files.remove(&f.full_path);
                }
            }

            ui.monospace(format!("{:>6}", f.loc_display));

            let st = state.tree.git_status_by_path.get(&f.full_path);
            let code = if let Some(e) = st {
                if e.untracked {
                    "??".to_string()
                } else {
                    format!("{}{}", e.index_status.trim(), e.worktree_status.trim())
                }
            } else {
                "".to_string()
            };
            ui.monospace(format!("{:>2}", code));

            let mut link_color = ui.visuals().hyperlink_color;
            if state.tree.untracked_paths.contains(&f.full_path) {
                link_color = egui::Color32::from_rgb(220, 160, 60);
            } else if state.tree.modified_paths.contains(&f.full_path) {
                link_color = egui::Color32::from_rgb(220, 120, 120);
            } else if state.tree.staged_paths.contains(&f.full_path) {
                link_color = egui::Color32::from_rgb(120, 220, 120);
            }

            let link_text = egui::RichText::new(&f.name).color(link_color);

            let resp = ui
                .add(egui::Link::new(link_text))
                .on_hover_text(&f.full_path)
                .on_hover_cursor(egui::CursorIcon::PointingHand);

            resp
        })
        .inner;


    link_resp.context_menu(|ui| {
        let can_edit = state.inputs.git_ref == crate::app::state::WORKTREE_REF;

        if ui.add_enabled(can_edit, egui::Button::new("Rename"))
            .on_hover_text("Rename this file")
            .clicked()
        {
            state.tree.create_parent = None;
            state.tree.create_draft.clear();
            state.tree.confirm_delete_target = None;

            state.tree.rename_target = Some(f.full_path.clone());
            state.tree.modal_focus_request = true;
            state.tree.rename_draft = f.name.clone();
            ui.close_menu();
        }

        if ui.add_enabled(can_edit, egui::Button::new("Delete"))
            .on_hover_text("Delete this file")
            .clicked()
        {
            state.tree.rename_target = None;
            state.tree.rename_draft.clear();
            state.tree.create_parent = None;
            state.tree.create_draft.clear();

            state.tree.confirm_delete_target = Some(f.full_path.clone());
            state.tree.modal_focus_request = true;
            ui.close_menu();
        }
    });
    if link_resp.clicked() {
        link_resp.request_focus();

        if viewers.is_empty() {
            state.results.error = Some(
                "No File Viewer windows. Create one with command palette: component/file_viewer"
                    .to_string(),
            );
        } else if viewers.len() == 1 {
            state.set_active_file_viewer_id(Some(viewers[0].0));
            actions.push(Action::OpenFile(f.full_path.clone()));
        } else {
            ui.memory_mut(|m| m.open_popup(popup_id));
        }
    }

    egui::popup::popup_below_widget(ui, popup_id, &link_resp, |ui| {
        ui.set_min_width(260.0);

        ui.label("Open in:");
        ui.separator();

        let default_id = state
            .active_file_viewer_id()
            .filter(|id| viewers.iter().any(|(vid, _)| vid == id))
            .unwrap_or(viewers[0].0);

        for (viewer_id, title) in viewers.iter() {
            let selected = *viewer_id == default_id;

            if ui.selectable_label(selected, title).clicked() {
                state.set_active_file_viewer_id(Some(*viewer_id));
                actions.push(Action::FocusFileViewer(*viewer_id));
                actions.push(Action::OpenFile(f.full_path.clone()));
                ui.close_menu();
            }
        }
    });
}

fn show_dir(
    ui: &mut egui::Ui,
    state: &mut AppState,
    node: &DirNode,
    filter: &str,
    show_stats_badge: bool,
    max_exts: usize,
    expand_cmd: Option<ExpandCmd>,
    actions: &mut Vec<Action>,
) {
    let id = egui::Id::new(("dir", node.full_path.as_str()));

    let badge = if show_stats_badge {
        format!(" [{}]", format::format_top_stats(&node.stats, max_exts))
    } else {
        "".to_string()
    };
    let label = format!("{}/{}", node.name, badge);

    let mut st =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);

    if let Some(cmd) = expand_cmd {
        match cmd {
            ExpandCmd::ExpandAll => st.set_open(true),
            ExpandCmd::CollapseAll => st.set_open(false),
        }
    }

    st.show_header(ui, |ui| {

        let (all, _any) = dir_selection_state(state, node);
        let mut desired = all;

        if ui.checkbox(&mut desired, "").clicked() {
            set_dir_selected(state, node, desired);
        }

        let label_resp = ui.add(egui::Label::new(label).wrap(true));
        let can_edit = state.inputs.git_ref == crate::app::state::WORKTREE_REF;
        label_resp.context_menu(|ui| {
            if ui.add_enabled(can_edit, egui::Button::new("New file"))
                .on_hover_text("Create a new file in this folder")
                .clicked()
            {
                state.tree.rename_target = None;
                state.tree.rename_draft.clear();
                state.tree.confirm_delete_target = None;

                state.tree.create_parent = Some(node.full_path.clone());
                state.tree.modal_focus_request = true;
                state.tree.create_draft.clear();
                state.tree.create_is_dir = false;
                ui.close_menu();
            }

            if ui.add_enabled(can_edit, egui::Button::new("New folder"))
                .on_hover_text("Create a new folder in this folder")
                .clicked()
            {
                state.tree.rename_target = None;
                state.tree.rename_draft.clear();
                state.tree.confirm_delete_target = None;

                state.tree.create_parent = Some(node.full_path.clone());
                state.tree.create_draft.clear();
                state.tree.create_is_dir = true;
                state.tree.modal_focus_request = true;
                ui.close_menu();
            }

            if ui.add_enabled(can_edit, egui::Button::new("Rename"))
                .on_hover_text("Rename this folder")
                .clicked()
            {
                state.tree.create_parent = None;
                state.tree.create_draft.clear();
                state.tree.confirm_delete_target = None;

                state.tree.rename_target = Some(node.full_path.clone());
                state.tree.modal_focus_request = true;
                state.tree.rename_draft = node.name.clone();
                ui.close_menu();
            }

            if ui.add_enabled(can_edit, egui::Button::new("Delete"))
                .on_hover_text("Delete this folder")
                .clicked()
            {
                state.tree.rename_target = None;
                state.tree.rename_draft.clear();
                state.tree.create_parent = None;
                state.tree.create_draft.clear();

                state.tree.confirm_delete_target = Some(node.full_path.clone());
                state.tree.modal_focus_request = true;
                ui.close_menu();
            }
        });
    })
    .body(|ui| {
        for f in &node.files {
            if !format::contains_case_insensitive(&f.full_path, filter) {
                continue;
            }
            file_row(ui, state, actions, f);
        }

        for c in &node.children {
            show_dir(ui, state, c, filter, show_stats_badge, max_exts, expand_cmd, actions);
        }
    });
}

pub fn tree_panel(
    _ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    res: &AnalysisResult,
) -> Vec<Action> {
    let mut actions = vec![];

    ui.set_enabled(true);


    let filter_text = state.ui.filter_text.clone();
    let max_exts = state.inputs.max_exts;
    let expand_cmd = state.tree.expand_cmd.take();

    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut state.ui.show_top_level_stats, "Badges");
        let can_edit = state.inputs.git_ref == crate::app::state::WORKTREE_REF;
        if ui.add_enabled(can_edit, egui::Button::new("New file"))
            .on_hover_text("Create a new file at repo root")
            .clicked()
        {
            state.tree.rename_target = None;
            state.tree.rename_draft.clear();
            state.tree.confirm_delete_target = None;

            state.tree.create_parent = Some("".to_string());
            state.tree.modal_focus_request = true;
            state.tree.create_draft.clear();
            state.tree.create_is_dir = false;
        }
        if ui.add_enabled(can_edit, egui::Button::new("New folder"))
            .on_hover_text("Create a new folder at repo root")
            .clicked()
        {
            state.tree.rename_target = None;
            state.tree.rename_draft.clear();
            state.tree.confirm_delete_target = None;

            state.tree.create_parent = Some("".to_string());
            state.tree.create_draft.clear();
            state.tree.create_is_dir = true;
            state.tree.modal_focus_request = true;
        }


        ui.separator();

        if ui.button("Expand all").clicked() {
            actions.push(Action::ExpandAll);
        }
        if ui.button("Collapse all").clicked() {
            actions.push(Action::CollapseAll);
        }

        ui.separator();

        if ui.button("All context").clicked() {
            state.tree.context_selected_files = collect_all_files(res).into_iter().collect();
        }
        if ui.button("No context").clicked() {
            state.tree.context_selected_files.clear();
        }
    });

    ui.add_space(6.0);

    ui.push_id("tree_panel", |ui| {
        egui::ScrollArea::both()
            .id_source("tree_scroll_both")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let show_badges = state.ui.show_top_level_stats;

                let badge = if show_badges {
                    format!(" [{}]", format::format_top_stats(&res.root.stats, max_exts))
                } else {
                    "".to_string()
                };
                let root_label = format!("./{}", badge);

                let root_id = egui::Id::new(("dir", "root"));
                let mut root_state =
                    egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        root_id,
                        true,
                    );

                if let Some(cmd) = expand_cmd {
                    match cmd {
                        ExpandCmd::ExpandAll => root_state.set_open(true),
                        ExpandCmd::CollapseAll => root_state.set_open(false),
                    }
                }

                root_state
                    .show_header(ui, |ui| {
                        let all_files = collect_all_files(res);
                        let mut all = !all_files.is_empty()
                            && all_files
                                .iter()
                                .all(|p| state.tree.context_selected_files.contains(p));

                        if ui.checkbox(&mut all, "").clicked() {
                            if all {
                                state.tree.context_selected_files = all_files.into_iter().collect();
                            } else {
                                state.tree.context_selected_files.clear();
                            }
                        }

                        ui.add(egui::Label::new(root_label).wrap(true));
                    })
                    .body(|ui| {
                        for f in &res.root.files {
                            if !format::contains_case_insensitive(&f.full_path, &filter_text) {
                                continue;
                            }
                            file_row(ui, state, &mut actions, f);
                        }

                        for d in &res.root.children {
                            show_dir(
                                ui,
                                state,
                                d,
                                &filter_text,
                                show_badges,
                                max_exts,
                                expand_cmd,
                                &mut actions,
                            );
                        }
                    });
            });
    });


    {
        let ctx = ui.ctx();
        let bounds = overlay_bounds(ui);

        if state.tree.rename_target.is_some() || state.tree.create_parent.is_some() || state.tree.confirm_delete_target.is_some() {
            let mut popup_rect = centered_overlay_rect(bounds, 0.55, 0.26, [360.0, 150.0], [620.0, 240.0]);

            if state.tree.confirm_delete_target.is_some() {
                popup_rect = centered_overlay_rect(bounds, 0.50, 0.22, [340.0, 140.0], [560.0, 210.0]);
            }

            let clicked_outside = modal_blocker(ctx, bounds, egui::Id::new("tree_modal_blocker"), popup_rect);
            if clicked_outside {
                state.tree.rename_target = None;
                state.tree.rename_draft.clear();
                state.tree.create_parent = None;
                state.tree.create_draft.clear();
                state.tree.confirm_delete_target = None;
                state.tree.modal_focus_request = false;
            }

            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                state.tree.rename_target = None;
                state.tree.rename_draft.clear();
                state.tree.create_parent = None;
                state.tree.create_draft.clear();
                state.tree.confirm_delete_target = None;
                state.tree.modal_focus_request = false;
            }
        }

        if let Some(target) = state.tree.rename_target.clone() {
            let rect = centered_overlay_rect(bounds, 0.55, 0.26, [360.0, 150.0], [620.0, 240.0]);

            let mut submit = false;
            let mut cancel = false;

            let open_now = popup_overlay(
                ctx,
                bounds,
                egui::Id::new("tree_rename_overlay"),
                "Rename",
                rect,
                |ui| {
                    ui.label(target.as_str());
                    ui.add_space(8.0);

                    let text_id = ui.make_persistent_id("tree_rename_text");
                    let _resp = ui.add(
                        egui::TextEdit::singleline(&mut state.tree.rename_draft)
                            .id(text_id)
                            .desired_width(f32::INFINITY)
                            .hint_text("New name (or path)")
                            .lock_focus(true),
                    );

                    if state.tree.modal_focus_request {
                        ui.memory_mut(|m| m.request_focus(text_id));
                        state.tree.modal_focus_request = false;
                    }

                    let enter_pressed = ui.input(|inp| inp.key_pressed(egui::Key::Enter));
                    if enter_pressed {
                        submit = true;
                    }

                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Rename").clicked() {
                                submit = true;
                            }
                        });
                    });
                },
            );

            if cancel || !open_now {
                state.tree.rename_target = None;
                state.tree.rename_draft.clear();
            } else if submit {
                let to = state.tree.rename_draft.trim().to_string();
                if !to.is_empty() {
                    actions.push(Action::TreeRenamePath { from: target.clone(), to });
                }
                state.tree.rename_target = None;
                state.tree.rename_draft.clear();
            }
        }

        if let Some(parent) = state.tree.create_parent.clone() {
            let rect = centered_overlay_rect(bounds, 0.55, 0.26, [360.0, 150.0], [620.0, 240.0]);

            let mut submit = false;
            let mut cancel = false;

            let title = if state.tree.create_is_dir { "New folder" } else { "New file" };

            let open_now = popup_overlay(
                ctx,
                bounds,
                egui::Id::new("tree_create_overlay"),
                title,
                rect,
                |ui| {
                    ui.label(if parent.is_empty() { "./" } else { parent.as_str() });
                    ui.add_space(8.0);

                    let text_id = ui.make_persistent_id("tree_create_text");
                    let _resp = ui.add(
                        egui::TextEdit::singleline(&mut state.tree.create_draft)
                            .id(text_id)
                            .desired_width(f32::INFINITY)
                            .hint_text("Name")
                            .lock_focus(true),
                    );

                    if state.tree.modal_focus_request {
                        ui.memory_mut(|m| m.request_focus(text_id));
                        state.tree.modal_focus_request = false;
                    }

                    let enter_pressed = ui.input(|inp| inp.key_pressed(egui::Key::Enter));
                    if enter_pressed {
                        submit = true;
                    }

                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Create").clicked() {
                                submit = true;
                            }
                        });
                    });
                },
            );

            if cancel || !open_now {
                state.tree.create_parent = None;
                state.tree.create_draft.clear();
            } else if submit {
                let name = state.tree.create_draft.trim().replace('\\', "/");
                if !name.is_empty() {
                    let full = if parent.is_empty() { name } else { format!("{}/{}", parent, name) };
                    if state.tree.create_is_dir {
                        actions.push(Action::TreeCreateFolder { path: full });
                    } else {
                        actions.push(Action::TreeCreateFile { path: full });
                    }
                }
                state.tree.create_parent = None;
                state.tree.create_draft.clear();
            }
        }

        if let Some(target) = state.tree.confirm_delete_target.clone() {
            let rect = centered_overlay_rect(bounds, 0.50, 0.22, [340.0, 140.0], [560.0, 210.0]);

            let mut confirm = false;
            let mut cancel = false;

            let open_now = popup_overlay(
                ctx,
                bounds,
                egui::Id::new("tree_delete_overlay"),
                "Delete",
                rect,
                |ui| {
                    ui.label("This cannot be undone.");
                    ui.add_space(10.0);
                    ui.monospace(target.as_str());
                    ui.add_space(12.0);

                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Delete").clicked() {
                                confirm = true;
                            }
                        });
                    });
                },
            );

            if cancel || !open_now {
                state.tree.confirm_delete_target = None;
            } else if confirm {
                actions.push(Action::TreeDeletePath { path: target.clone() });
                state.tree.confirm_delete_target = None;
            }
            state.tree.modal_focus_request = false;
            if state.tree.confirm_delete_target.is_none() {
                state.tree.modal_focus_request = false;
            }
        }
    }
    let key = state.inputs.git_ref.clone();
    state.tree
        .context_selected_by_ref
        .insert(key, state.tree.context_selected_files.clone());

    actions
}
