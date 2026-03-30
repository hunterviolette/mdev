use sqlx::SqlitePool;

use crate::migration::egui_bridge::EguiBridge;

#[allow(dead_code)]
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    #[allow(dead_code)]
    pub egui_bridge: EguiBridge,
}

impl AppState {
    pub fn new(db: SqlitePool) -> Self {
        Self {
            db,
            egui_bridge: EguiBridge::default(),
        }
    }
}
