// src/app/controllers/ui_prefs_controller.rs

use crate::app::actions::Action;
use crate::app::state::AppState;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::OpenCanvasTintPopup => {
            state.ui.canvas_tint_popup_open = true;

            // Initialize a stable in-progress value for the picker UI.
            // This avoids selector drift/jumps while dragging and makes the popup
            // resilient to the timing of action application.
            if state.ui.canvas_tint_draft.is_none() {
                state.ui.canvas_tint_draft = Some(state.ui.canvas_bg_tint.unwrap_or([0, 128, 255, 18]));
            }

            // If opened via palette, this makes it feel modal-ish.
            state.palette.open = false;
            true
        }
        Action::CloseCanvasTintPopup => {
            state.ui.canvas_tint_popup_open = false;
            state.ui.canvas_tint_draft = None;
            true
        }
        Action::SetCanvasBgTint { rgba } => {
            state.ui.canvas_bg_tint = *rgba;
            true
        }
        // Keep draft in sync if tint changes via non-popup routes (future-proofing).
        // We only overwrite the draft when the popup is not open; while open, the
        // popup owns the draft value.
        
        _ => false,
    }
}
