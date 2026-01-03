use crate::app::actions::{Action, ExpandCmd};
use crate::app::state::AppState;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::ExpandAll => {
            state.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
            true
        }
        Action::CollapseAll => {
            state.tree.expand_cmd = Some(ExpandCmd::CollapseAll);
            true
        }
        Action::OpenFile(path) => {
            state.deferred.open_file = Some(path.clone());
            state.deferred.open_file_target_viewer = state.active_file_viewer;
            true
        }
        _ => false,
    }
}
