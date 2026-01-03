use eframe::egui;
use egui_extras::syntax_highlighting::CodeTheme;

pub fn seed_solarized_dark_once(ctx: &egui::Context) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let has_stored = ctx.data_mut(|d| {
            d.get_persisted::<CodeTheme>(egui::Id::new("dark")).is_some()
                || d.get_persisted::<CodeTheme>(egui::Id::new("light")).is_some()
        });

        if !has_stored {
            // Matches egui_extras::syntax_highlighting::CodeTheme with syntect enabled in 0.27
            let json = r#"{"dark_mode":true,"syntect_theme":"SolarizedDark"}"#;
            if let Ok(theme) = serde_json::from_str::<CodeTheme>(json) {
                theme.store_in_memory(ctx);
            }
        }
    });
}
