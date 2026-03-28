mod analyze;
mod format;
mod git;
mod model;
mod app;
mod platform;
mod capabilities;
mod gateway_model;

use std::sync::Arc;

fn init_tracing() {

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_level(true)
        .try_init();
}

fn main() -> eframe::Result<()> {
    eprintln!("[startup] Repo Analyzer starting (stderr)");

    init_tracing();
    tracing::info!(target: "workspace_geom", event = "startup");

    let _ = dotenvy::dotenv();

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
