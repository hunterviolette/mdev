use eframe::egui;
use egui_extras::syntax_highlighting::CodeTheme;

use crate::app::state::AppState;

/// Apply the current theme prefs stored in AppState to egui visuals + code highlighting.
/// Safe to call every frame (idempotent).
///
/// If syntect_theme is empty, this picks a reasonable default based on dark/light.
pub fn apply_from_state(ctx: &egui::Context, state: &mut AppState) {
    // Ensure syntect theme has a default (mutates prefs only when needed).
    if state.theme.prefs.syntect_theme.trim().is_empty() {
        state.theme.prefs.syntect_theme = if state.theme.prefs.dark {
            "SolarizedDark".to_string()
        } else {
            "SolarizedLight".to_string()
        };
    }

    let desired_dark = state.theme.prefs.dark;
    let desired_syntect = state.theme.prefs.syntect_theme.as_str();

    // Fast path: nothing changed since last apply.
    let unchanged = state.theme.last_applied_dark == Some(desired_dark)
        && state
            .theme
            .last_applied_syntect_theme
            .as_deref()
            .is_some_and(|s| s == desired_syntect);

    if unchanged {
        return;
    }

    // Apply visuals only when changed.
    if desired_dark {
        ctx.set_visuals(egui::Visuals::dark());
    } else {
        ctx.set_visuals(egui::Visuals::light());
    }

    // Rebuild/store CodeTheme only when prefs change.
    let json = format!(
        r#"{{\"dark_mode\":{},\"syntect_theme\":\"{}\"}}"#,
        desired_dark,
        desired_syntect
    );

    if let Ok(theme) = serde_json::from_str::<CodeTheme>(&json) {
        theme.clone().store_in_memory(ctx);
        state.theme.code_theme = theme;
    } else {
        state.theme.code_theme = CodeTheme::from_memory(ctx);
    }

    state.theme.last_applied_dark = Some(desired_dark);
    state.theme.last_applied_syntect_theme = Some(desired_syntect.to_string());
}
