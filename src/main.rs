mod analyze;
mod format;
mod git;
mod model;
mod app;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Repo Analyzer",
        native_options,
        Box::new(|_cc| Box::new(app::AppState::default())),
    )
}
