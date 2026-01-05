// src/app/ui/top_bar.rs
use eframe::egui;
use egui_extras::syntax_highlighting::CodeTheme;

use crate::format;

use super::super::actions::Action;
use super::super::state::{AppState, WORKTREE_REF};

fn canvas_rect_id() -> egui::Id {
    egui::Id::new("canvas_rect_after_top_panel")
}

fn last_canvas_size(ctx: &egui::Context) -> [f32; 2] {
    let r = ctx
        .data_mut(|d| d.get_persisted::<egui::Rect>(canvas_rect_id()))
        .unwrap_or_else(|| ctx.available_rect());

    [r.width().max(1.0), r.height().max(1.0)]
}

fn viewport_outer_pos(ctx: &egui::Context) -> Option<[f32; 2]> {
    ctx.input(|i| i.viewport().outer_rect.map(|r| [r.min.x, r.min.y]))
}

fn viewport_inner_size(ctx: &egui::Context) -> Option<[f32; 2]> {
    ctx.input(|i| i.viewport().inner_rect.map(|r| [r.width(), r.height()]))
}

fn apply_theme(ctx: &egui::Context, state: &mut AppState) {
    if state.theme.prefs.dark {
        ctx.set_visuals(egui::Visuals::dark());
    } else {
        ctx.set_visuals(egui::Visuals::light());
    }

    let json = format!(
        r#"{{"dark_mode":{},"syntect_theme":"{}"}}"#,
        state.theme.prefs.dark,
        state.theme.prefs.syntect_theme
    );

    if let Ok(theme) = serde_json::from_str::<CodeTheme>(&json) {
        theme.clone().store_in_memory(ctx);
        state.theme.code_theme = theme;
    } else {
        state.theme.code_theme = CodeTheme::from_memory(ctx);
    }
}

pub fn top_bar(ctx: &egui::Context, ui: &mut egui::Ui, state: &mut AppState) -> Vec<Action> {
    let mut actions = vec![];

    ui.horizontal(|ui| {
        // ----- Local repo picker only -----
        if ui.button("Pick Local Repo…").clicked() {
            actions.push(Action::PickRepo);
        }

        ui.separator();

        // ----- Ref dropdown -----
        ui.label("Ref:");

        let has_opts = !state.inputs.git_ref_options.is_empty();
        if has_opts {
            // Display "Working tree" when WORKTREE_REF is selected
            let selected_label = if state.inputs.git_ref == WORKTREE_REF {
                "Working tree".to_string()
            } else {
                state.inputs.git_ref.clone()
            };

            egui::ComboBox::from_id_source("git_ref_combo")
                .selected_text(selected_label)
                .width(220.0)
                .show_ui(ui, |ui| {
                    for r in state.inputs.git_ref_options.iter() {
                        let label = if r == WORKTREE_REF { "Working tree" } else { r.as_str() };

                        if ui
                            .selectable_label(&state.inputs.git_ref == r, label)
                            .clicked()
                        {
                            actions.push(Action::SetGitRef(r.clone()));
                        }
                    }
                });
        } else {
            ui.text_edit_singleline(&mut state.inputs.git_ref);
        }

        if ui.button("↻").clicked() {
            actions.push(Action::RefreshGitRefs);
        }

        ui.separator();

        // ----- Filter -----
        ui.label("Filter:");
        ui.text_edit_singleline(&mut state.ui.filter_text);

        ui.separator();

        if ui.button("Palette (Ctrl+Shift+E)").clicked() {
            actions.push(Action::ToggleCommandPalette);
        }

        ui.separator();

        // Theme toggle
        let was_dark = state.theme.prefs.dark;

        if ui.selectable_label(state.theme.prefs.dark, "Dark").clicked() {
            state.theme.prefs.dark = true;
            if !was_dark {
                state.theme.prefs.syntect_theme = "SolarizedDark".to_string();
            }
            apply_theme(ctx, state);
        }

        if ui.selectable_label(!state.theme.prefs.dark, "Light").clicked() {
            state.theme.prefs.dark = false;
            if was_dark {
                state.theme.prefs.syntect_theme = "SolarizedLight".to_string();
            }
            apply_theme(ctx, state);
        }

        ui.separator();

        if ui.button("Run").clicked() {
            actions.push(Action::RunAnalysis);
        }
    });

    ui.horizontal(|ui| {
        ui.label("Exclude regex:");
        let mut joined = format::join_excludes(&state.inputs.exclude_regex);
        if ui.text_edit_singleline(&mut joined).changed() {
            state.inputs.exclude_regex = format::parse_excludes(&joined);
        }

        ui.label("Max exts:");
        //  FIX: Rust inclusive range is ..=
        ui.add(egui::DragValue::new(&mut state.inputs.max_exts).clamp_range(1..=20));
    });

    // Status lines
    if let Some(repo) = &state.inputs.repo {
        ui.label(format!("Repo: {:?}", repo));
    } else {
        ui.label("Repo: (none selected)");
    }

    // Workspace save/load helpers (unchanged behavior)
    ui.horizontal(|ui| {
        let canvas = last_canvas_size(ctx);
        let outer = viewport_outer_pos(ctx);
        let inner = viewport_inner_size(ctx);

        if ui.button("Save workspace").clicked() {
            actions.push(Action::SaveWorkspace {
                canvas_size: canvas,
                viewport_outer_pos: outer,
                viewport_inner_size: inner,
            });
        }
        if ui.button("Load workspace").clicked() {
            actions.push(Action::LoadWorkspace);
        }
    });

    actions
}
