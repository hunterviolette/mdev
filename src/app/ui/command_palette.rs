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
        "context_exporter" => Some(ComponentKind::ContextExporter),
        "changeset_applier" => Some(ComponentKind::ChangeSetApplier),
        "source_control" => Some(ComponentKind::SourceControl),
        "diff_viewer" => Some(ComponentKind::DiffViewer),
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
                        return names
                            .into_iter()
                            .map(|n| format!("workspace/save/{n}"))
                            .collect();
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
                        return names
                            .into_iter()
                            .map(|n| format!("workspace/load/{n}"))
                            .collect();
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
                    "component/context_exporter".into(),
                    "component/changeset_applier".into(),
                    "component/source_control".into(),
                    "component/diff_viewer".into()
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

// ---------------------------
// Fuzzy search helpers
// ---------------------------

fn normalize_q(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    let q = normalize_q(query);
    if q.is_empty() {
        return Some(0);
    }

    let c = candidate.to_ascii_lowercase();

    // Token-based "all tokens must appear" scoring.
    let tokens: Vec<&str> = q.split_whitespace().filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return Some(0);
    }

    let mut score: i32 = 0;
    for t in tokens {
        if let Some(pos) = c.find(t) {
            // Earlier match is better.
            score += 100 - (pos as i32).min(100);
        } else {
            return None;
        }
    }

    // Prefer shorter candidates slightly.
    score -= (c.len() as i32).min(80);
    Some(score)
}

fn fuzzy_filter_sort(query: &str, candidates: &[String], limit: usize) -> Vec<String> {
    let mut scored: Vec<(i32, String)> = candidates
        .iter()
        .filter_map(|c| fuzzy_score(query, c).map(|s| (s, c.clone())))
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, s)| s).collect()
}

fn all_commands(state: &AppState) -> Vec<String> {
    let mut out: Vec<String> = vec![
        "workspace/save".into(),
        "workspace/load".into(),
        "component/file_viewer".into(),
        "component/tree".into(),
        "component/summary".into(),
        "component/terminal".into(),
        "component/context_exporter".into(),
        "component/changeset_applier".into(),
        "component/source_control".into(),
    ];

    let mut names = state.list_workspaces();
    names.sort();
    for n in names {
        out.push(format!("workspace/save/{n}"));
        out.push(format!("workspace/load/{n}"));
    }

    out.sort();
    out.dedup();
    out
}

fn detect_command_lane(query: &str) -> Option<&'static str> {
    // If the user starts a lane prefix, constrain suggestions to that lane.
    // This prevents accidents like typing workspace/load/... and seeing workspace/save/... results.
    let q = query.trim().to_ascii_lowercase();
    if q.starts_with("workspace/load") {
        return Some("workspace/load");
    }
    if q.starts_with("workspace/save") {
        return Some("workspace/save");
    }
    if q.starts_with("component/") {
        return Some("component/");
    }
    if q.starts_with("workspace/") {
        return Some("workspace/");
    }
    None
}

fn constrain_to_lane(mut sugg: Vec<String>, lane: Option<&str>) -> Vec<String> {
    let Some(lane) = lane else {
        return sugg;
    };

    if lane == "workspace/load" {
        sugg.retain(|s| s == "workspace/load" || s.starts_with("workspace/load/"));
        return sugg;
    }

    if lane == "workspace/save" {
        sugg.retain(|s| s == "workspace/save" || s.starts_with("workspace/save/"));
        return sugg;
    }

    if lane == "component/" {
        sugg.retain(|s| s.starts_with("component/"));
        return sugg;
    }

    if lane == "workspace/" {
        sugg.retain(|s| s.starts_with("workspace/"));
        return sugg;
    }

    sugg
}

/// Command palette UI.
/// Returns actions to dispatch this frame.
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
                    .hint_text(
                        "workspace/load/foo | workspace/save/my_layout | component/terminal | component/context_exporter | component/changeset_applier",
                    ),
            );
            resp.request_focus();

            // Fuzzy search over ALL commands. Arrow keys navigate; Enter executes selection.

            // If the query changes via typing, reset selection.
            if resp.changed() {
                state.palette.selected = 0;
            }

            let all = all_commands(state);
            let lane = detect_command_lane(&state.palette.query);

            let mut sugg = if state.palette.query.trim().is_empty() {
                // When empty, show a stable "top" list (not the entire command universe).
                all.into_iter().take(30).collect::<Vec<_>>()
            } else {
                // Pull more, then constrain + truncate.
                fuzzy_filter_sort(&state.palette.query, &all, 60)
            };

            sugg = constrain_to_lane(sugg, lane);
            if sugg.len() > 30 {
                sugg.truncate(30);
            }

            // Keyboard navigation
            let down = ctx.input(|i| i.key_pressed(egui::Key::ArrowDown));
            let up = ctx.input(|i| i.key_pressed(egui::Key::ArrowUp));
            let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));

            if sugg.is_empty() {
                state.palette.selected = 0;
            } else if state.palette.selected >= sugg.len() {
                state.palette.selected = 0;
            }

            if down && !sugg.is_empty() {
                state.palette.selected = (state.palette.selected + 1).min(sugg.len() - 1);
            }
            if up && !sugg.is_empty() {
                state.palette.selected = state.palette.selected.saturating_sub(1);
            }

            ui.separator();
            ui.label("Suggestions:");

            egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                for (i, s) in sugg.iter().enumerate() {
                    let selected = i == state.palette.selected;

                    let row = ui.selectable_label(selected, s);

                    // When navigating with arrow keys, keep the selected row in view.
                    if selected {
                        ui.scroll_to_rect(row.rect, Some(egui::Align::Center));
                    }

                    if row.clicked() {
                        state.palette.selected = i;
                        state.palette.query = s.to_string();
                    }
                }
            });

            // On Enter, promote selected suggestion into the query before parsing.
            if enter {
                if let Some(chosen) = sugg.get(state.palette.selected).cloned() {
                    state.palette.query = chosen;
                }
            }

            let segments = split_segments(&state.palette.query);
            let (cmd, name_arg) = parse_command(&segments);

            ui.separator();

            if cmd.is_some() {
                ui.colored_label(egui::Color32::LIGHT_GREEN, "✓ Valid command (press Enter)");
            } else {
                ui.colored_label(egui::Color32::LIGHT_RED, "… Incomplete/invalid command");
            }

            if enter {
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
