mod analyze;
mod format;
mod git;
mod model;
mod app;
mod platform;
mod capabilities;

use std::sync::Arc;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions::default();

    eframe::run_native(
        "Repo Analyzer",
        native_options,
        Box::new(|_cc| {
            let platform = Arc::new(platform::native::NativePlatform::new());
            Box::new(app::AppState::new(platform))
        }),
    )
}
