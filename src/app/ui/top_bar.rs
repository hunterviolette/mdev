use eframe::egui;
use egui_extras::syntax_highlighting::CodeTheme;

use crate::format;

use super::super::actions::Action;
use super::super::state::AppState;

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
    // Apply egui visuals (UI chrome)
    if state.theme.prefs.dark {
        ctx.set_visuals(egui::Visuals::dark());
    } else {
        ctx.set_visuals(egui::Visuals::light());
    }

    // Apply code highlighting theme
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
        if ui.button("Select Repo…").clicked() {
            actions.push(Action::PickRepo);
        }

        ui.label("Ref:");
        ui.text_edit_singleline(&mut state.inputs.git_ref);

        ui.separator();

        ui.label("Filter:");
        ui.text_edit_singleline(&mut state.ui.filter_text);

        ui.separator();

        if ui.button("Palette (Ctrl+Shift+E)").clicked() {
            actions.push(Action::ToggleCommandPalette);
        }

        ui.separator();

        // Theme toggle (do not overwrite theme each frame)
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
        ui.add(egui::DragValue::new(&mut state.inputs.max_exts).clamp_range(1..=20));
    });

    if let Some(repo) = &state.inputs.repo {
        ui.label(format!("Repo: {}", repo.display()));
    } else {
        ui.label("Repo: (none selected)");
    }

    if let Some(err) = &state.results.error {
        ui.colored_label(egui::Color32::LIGHT_RED, err);
    }

    // If you still use these helpers elsewhere:
    let _ = last_canvas_size(ctx);
    let _ = viewport_outer_pos(ctx);
    let _ = viewport_inner_size(ctx);

    actions
}
