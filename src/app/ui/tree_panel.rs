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

    // Stable order
    list.sort_by(|a, b| a.1.cmp(&b.1));
    list
}

/// Render one file row. If there's only one FileViewer instance, click opens immediately.
/// If there are multiple, click opens a small popup menu anchored to this row.
fn file_row(ui: &mut egui::Ui, state: &mut AppState, actions: &mut Vec<Action>, f: &FileRow) {
    let viewers = file_viewer_choices(state);

    // IMPORTANT: use a stable ID that does NOT depend on ui id-stack.
    let popup_id = egui::Id::new(("open_in_popup", f.full_path.as_str()));

    // Row UI: LOC + clickable blue "link" label (no wrapping so we get horizontal scrolling)
    let link_resp = ui
        .horizontal(|ui| {
            ui.monospace(format!("{:>6}", f.loc_display));

            // egui::Link has no `.wrap(...)`. Use a Label styled like a hyperlink + click sense.
            let link_text = egui::RichText::new(&f.name).color(ui.visuals().hyperlink_color);

            ui.add(
                egui::Label::new(link_text)
                    .wrap(false)
                    .sense(egui::Sense::click()),
            )
            .on_hover_text(&f.full_path)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
        })
        .inner;

    // On click:
    // - 0 viewers: show error
    // - 1 viewer: open directly
    // - >1 viewers: open popup anchored at the clicked row
    if link_resp.clicked() {
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

    // Show the popup menu if open (anchored near this row).
    egui::popup::popup_below_widget(ui, popup_id, &link_resp, |ui| {
        ui.set_min_width(260.0);

        ui.label("Open in:");
        ui.separator();

        // Default selection preference:
        // - active viewer (if exists)
        // - otherwise first viewer
        let default_id = state
            .active_file_viewer
            .filter(|id| viewers.iter().any(|(vid, _)| vid == id))
            .unwrap_or(viewers[0].0);

        for (viewer_id, title) in viewers.iter() {
            let selected = *viewer_id == default_id;

            if ui.selectable_label(selected, title).clicked() {
                state.active_file_viewer = Some(*viewer_id);

                // Optional: bring that viewer to front (only if your canvas supports it).
                actions.push(Action::FocusFileViewer(*viewer_id));

                // Actually open the file
                actions.push(Action::OpenFile(f.full_path.clone()));

                ui.close_menu(); // closes popup immediately
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
    // IMPORTANT: stable ID independent of ui id-stack (prevents expansion reset on layout changes).
    let id = egui::Id::new(("dir", node.full_path.as_str()));

    let badge = if show_stats_badge {
        format!(" [{}]", format::format_top_stats(&node.stats, max_exts))
    } else {
        "".to_string()
    };
    let label = format!("{}/{}", node.name, badge);

    let mut st = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);

    // Apply expand/collapse ONCE (tree_panel clears it after rendering)
    if let Some(cmd) = expand_cmd {
        match cmd {
            ExpandCmd::ExpandAll => st.set_open(true),
            ExpandCmd::CollapseAll => st.set_open(false),
        }
    }

    st.show_header(ui, |ui| {
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
    let expand_cmd = state.tree.expand_cmd;

    // Tree-local controls (moved from top panel)
    ui.horizontal(|ui| {
        ui.checkbox(&mut state.ui.show_top_level_stats, "Badges");

        ui.separator();

        if ui.button("Expand all").clicked() {
            actions.push(Action::ExpandAll);
        }
        if ui.button("Collapse all").clicked() {
            actions.push(Action::CollapseAll);
        }
    });

    ui.add_space(6.0);

    ui.push_id("tree_panel", |ui| {
        // No “shape restrictions”: always allow both scrollbars
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

                // IMPORTANT: stable ID independent of ui id-stack.
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

    // Consume one-shot expand/collapse so it doesn't keep forcing open/close every frame.
    state.tree.expand_cmd = None;

    actions
}
