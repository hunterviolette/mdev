use eframe::egui;

use crate::app::actions::{Action, ComponentKind};
use crate::app::state::AppState;

fn split_segments(s: &str) -> Vec<String> {
    s.split('/')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

fn component_from_str(s: &str) -> Option<ComponentKind> {
    match s {
        "tree" => Some(ComponentKind::Tree),
        "file_viewer" | "fileviewer" | "viewer" => Some(ComponentKind::FileViewer),
        "summary" => Some(ComponentKind::Summary),
        "terminal" | "term" => Some(ComponentKind::Terminal),
        _ => None,
    }
}

fn suggestions_for(state: &AppState, segments: &[String]) -> Vec<String> {
    if segments.is_empty() {
        return vec!["workspace".into(), "component".into()];
    }

    match segments[0].as_str() {
        "workspace" => {
            let mut names = state.list_workspaces();
            names.sort();

            if segments.len() == 1 {
                return vec!["workspace/save".into(), "workspace/load".into()];
            }

            match segments[1].as_str() {
                "save" => {
                    if names.is_empty() {
                        return vec!["workspace/save".into()];
                    }

                    if segments.len() == 2 {
                        return names.into_iter().map(|n| format!("workspace/save/{n}")).collect();
                    }

                    let typed = segments.get(2).map(|s| s.as_str()).unwrap_or("");
                    let typed_lc = typed.to_lowercase();

                    let out: Vec<String> = names
                        .into_iter()
                        .filter(|n| typed.is_empty() || n.to_lowercase().starts_with(&typed_lc))
                        .map(|n| format!("workspace/save/{n}"))
                        .collect();

                    if out.is_empty() {
                        vec!["workspace/save".into()]
                    } else {
                        out
                    }
                }

                "load" => {
                    if names.is_empty() {
                        return vec!["workspace/load".into()];
                    }

                    if segments.len() == 2 {
                        return names.into_iter().map(|n| format!("workspace/load/{n}")).collect();
                    }

                    let typed = segments.get(2).map(|s| s.as_str()).unwrap_or("");
                    let typed_lc = typed.to_lowercase();

                    let out: Vec<String> = names
                        .into_iter()
                        .filter(|n| typed.is_empty() || n.to_lowercase().starts_with(&typed_lc))
                        .map(|n| format!("workspace/load/{n}"))
                        .collect();

                    if out.is_empty() {
                        vec!["workspace/load".into()]
                    } else {
                        out
                    }
                }

                _ => vec!["workspace/save".into(), "workspace/load".into()],
            }
        }

        "component" => {
            if segments.len() == 1 {
                vec![
                    "component/file_viewer".into(),
                    "component/tree".into(),
                    "component/summary".into(),
                    "component/terminal".into(),
                ]
            } else {
                vec![]
            }
        }

        _ => vec!["workspace".into(), "component".into()],
    }
}

fn parse_command(segments: &[String]) -> (Option<Action>, Option<String>) {
    if segments.is_empty() {
        return (None, None);
    }

    match segments[0].as_str() {
        "component" => {
            if segments.len() < 2 {
                return (None, None);
            }
            let kind = match component_from_str(&segments[1]) {
                Some(k) => k,
                None => return (None, None),
            };
            (Some(Action::AddComponent { kind }), None)
        }

        "workspace" => {
            if segments.len() < 2 {
                return (None, None);
            }
            match segments[1].as_str() {
                "save" => {
                    let name = segments.get(2).cloned();
                    (
                        Some(Action::SaveWorkspace {
                            canvas_size: [1.0, 1.0], // patched at call site
                            viewport_outer_pos: None,
                            viewport_inner_size: None,
                        }),
                        name,
                    )
                }
                "load" => {
                    let name = segments.get(2).cloned();
                    (Some(Action::LoadWorkspace), name)
                }
                _ => (None, None),
            }
        }

        _ => (None, None),
    }
}

pub fn command_palette(
    ctx: &egui::Context,
    state: &mut AppState,
    canvas_size: [f32; 2],
    viewport_outer_pos: Option<[f32; 2]>,
    viewport_inner_size: Option<[f32; 2]>,
) -> Vec<Action> {
    let mut actions = vec![];

    if !state.palette.open {
        return actions;
    }

    let mut open = true;

    egui::Window::new("Command Palette")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(600.0);

            let search_id = ui.make_persistent_id("palette_search");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut state.palette.query)
                    .id(search_id)
                    .hint_text("workspace/load/foo | workspace/save/my_layout | component/terminal"),
            );
            resp.request_focus();

            let segments = split_segments(&state.palette.query);

            ui.separator();
            ui.label("Suggestions:");
            let mut sugg = suggestions_for(state, &segments);

            if !state.palette.query.trim().is_empty() && sugg.is_empty() {
                sugg = vec![
                    "workspace/save".into(),
                    "workspace/load".into(),
                    "component/file_viewer".into(),
                    "component/terminal".into(),
                ];
            }

            egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                for s in &sugg {
                    if ui.link(s).clicked() {
                        state.palette.query = s.trim_end_matches('/').to_string();
                    }
                }
            });

            ui.separator();

            let (cmd, name_arg) = parse_command(&segments);

            if cmd.is_some() {
                ui.colored_label(egui::Color32::LIGHT_GREEN, "✓ Valid command (press Enter)");
            } else {
                ui.colored_label(egui::Color32::LIGHT_RED, "… Incomplete/invalid command");
            }

            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if let Some(mut act) = cmd {
                    if let Action::SaveWorkspace { .. } = act {
                        act = Action::SaveWorkspace {
                            canvas_size,
                            viewport_outer_pos,
                            viewport_inner_size,
                        };
                    }

                    state.palette_last_name = name_arg;

                    actions.push(act);

                    state.palette.open = false;
                    state.palette.query.clear();
                    state.palette.selected = 0;
                }
            }
        });

    if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.palette.open = false;
        state.palette.query.clear();
        state.palette.selected = 0;
    }

    actions
}
