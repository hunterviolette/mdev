use crate::app::actions::Action;
use crate::app::state::AppState;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::ToggleCommandPalette => {
            state.palette.open = !state.palette.open;
            if state.palette.open {
                state.palette.query.clear();
                state.palette.selected = 0;
            }
            true
        }
        _ => false,
    }
}
