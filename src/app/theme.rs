use eframe::egui;
use egui_extras::syntax_highlighting::CodeTheme;

use crate::app::state::AppState;

/// Apply the current theme prefs stored in AppState to egui visuals + code highlighting.
/// Safe to call every frame (idempotent).
///
/// If syntect_theme is empty, this picks a reasonable default based on dark/light.
pub fn apply_from_state(ctx: &egui::Context, state: &mut AppState) {
    if state.theme.prefs.dark {
        ctx.set_visuals(egui::Visuals::dark());
    } else {
        ctx.set_visuals(egui::Visuals::light());
    }

    if state.theme.prefs.syntect_theme.trim().is_empty() {
        state.theme.prefs.syntect_theme = if state.theme.prefs.dark {
            "SolarizedDark".to_string()
        } else {
            "SolarizedLight".to_string()
        };
    }

    // Keep CodeTheme in sync with prefs.
    let json = format!(
        r#"{{\"dark_mode\":{},\"syntect_theme\":\"{}\"}}"#,
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
