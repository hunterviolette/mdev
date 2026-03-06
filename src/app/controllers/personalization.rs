
use crate::app::actions::Action;
use crate::app::state::AppState;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::OpenCanvasTintPopup => {
            state.ui.canvas_tint_popup_open = true;

            if state.ui.canvas_tint_draft.is_none() {
                state.ui.canvas_tint_draft = Some(state.ui.canvas_bg_tint.unwrap_or([0, 128, 255, 18]));
            }

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
        Action::SaveStartupLayoutOverride {
            canvas_size,
            viewport_outer_pos,
            viewport_inner_size,
            pixels_per_point,
        } => {
            state.save_startup_layout_override_to_appdata(
                *canvas_size,
                *viewport_outer_pos,
                *viewport_inner_size,
                *pixels_per_point,
            );
            true
        }
        Action::ClearStartupLayoutOverride => {
            state.clear_startup_layout_override_from_appdata();
            true
        }
        Action::ExportBuiltInStartupLayout {
            canvas_size,
            viewport_outer_pos,
            viewport_inner_size,
            pixels_per_point,
        } => {
            state.export_built_in_startup_layout_to_repo_file(
                *canvas_size,
                *viewport_outer_pos,
                *viewport_inner_size,
                *pixels_per_point,
            );
            true
        }
        
        _ => false,
    }
}
