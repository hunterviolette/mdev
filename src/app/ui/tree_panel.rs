// src/app/ui/tree_panel.rs
use eframe::egui;

use crate::format;
use crate::model::{AnalysisResult, DirNode, FileRow};

use super::super::actions::{Action, ComponentId, ComponentKind, ExpandCmd};
use super::super::state::AppState;

fn file_viewer_choices(state: &AppState) -> Vec<(ComponentId, String)> {
    let mut list: Vec<(ComponentId, String)> = state
        .layout
        .components
        .iter()
        .filter(|c| c.kind == ComponentKind::FileViewer)
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

/// Render one file row:
/// - checkbox gates context export selection
/// - filename still opens file viewer (existing behavior)
fn file_row(ui: &mut egui::Ui, state: &mut AppState, actions: &mut Vec<Action>, f: &FileRow) {
    let viewers = file_viewer_choices(state);
    let popup_id = egui::Id::new(("open_in_popup", f.full_path.as_str()));

    let mut checked = state.tree.context_selected_files.contains(&f.full_path);

    // NOTE: use a *focusable* widget for the filename so keyboard focus can leave the editor.
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

            let link_text = egui::RichText::new(&f.name).color(ui.visuals().hyperlink_color);

            // Link is focusable (unlike Label + Sense::click)
            let resp = ui
                .add(egui::Link::new(link_text))
                .on_hover_text(&f.full_path)
                .on_hover_cursor(egui::CursorIcon::PointingHand);

            resp
        })
        .inner;

    if link_resp.clicked() {
        // Explicitly move keyboard focus away from the code editor
        link_resp.request_focus();

        if viewers.is_empty() {
            state.results.error = Some(
                "No File Viewer windows. Create one with command palette: component/file_viewer"
                    .to_string(),
            );
        } else if viewers.len() == 1 {
            state.active_file_viewer = Some(viewers[0].0);
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
            .active_file_viewer
            .filter(|id| viewers.iter().any(|(vid, _)| vid == id))
            .unwrap_or(viewers[0].0);

        for (viewer_id, title) in viewers.iter() {
            let selected = *viewer_id == default_id;

            if ui.selectable_label(selected, title).clicked() {
                state.active_file_viewer = Some(*viewer_id);
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
        // NOTE: egui checkboxes don’t support true tri-state without extra work.
        // We follow your ask: "dir selects all files in it" using a single checkbox.
        // Behavior:
        // - checked => all files under dir are selected
        // - unchecked => none selected (even if previously partial)
        let (all, _any) = dir_selection_state(state, node);
        let mut desired = all;

        if ui.checkbox(&mut desired, "").clicked() {
            set_dir_selected(state, node, desired);
        }

        ui.add(egui::Label::new(label).wrap(false));
    })
    .body(|ui| {
        for f in &node.files {
            if !format::contains_case_insensitive(&f.full_path, filter) {
                continue;
            }
            file_row(ui, state, actions, f);
        }

        for c in &node.children {
            show_dir(ui, state, c, filter, false, max_exts, expand_cmd, actions);
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

    let filter_text = state.ui.filter_text.clone();
    let max_exts = state.inputs.max_exts;

    // One-shot expand/collapse command
    // IMPORTANT: consume it so it doesn't override manual caret toggles every frame.
    let expand_cmd = state.tree.expand_cmd.take();

    ui.horizontal(|ui| {
        ui.checkbox(&mut state.ui.show_top_level_stats, "Badges");

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
                        // Root checkbox controls all files
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

                        ui.add(egui::Label::new(root_label).wrap(false));
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

    // Persist current selection for this ref each frame so reruns of analysis (same ref)
    // do not blow away user selections.
    let key = state.inputs.git_ref.clone();
    state.tree
        .context_selected_by_ref
        .insert(key, state.tree.context_selected_files.clone());

    actions
}
