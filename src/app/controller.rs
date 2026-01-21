use crate::{format};

use super::actions::Action;
use super::state::AppState;

use super::controllers::{
    analysis_controller, changeset_controller, context_exporter_controller, diff_viewer_controller,
    file_viewer_controller, layout_controller, palette_controller, source_control_controller,
    terminal_controller, tree_controller, canvas_tint, workspace_controller,
};

impl AppState {
    pub fn apply_action(&mut self, action: Action) {
        // Keep ordering stable (global -> domain -> layout/workspace)
        if palette_controller::handle(self, &action) {
            return;
        }
        if canvas_tint::handle(self, &action) {
            return;
        }
        if analysis_controller::handle(self, &action) {
            return;
        }
        if changeset_controller::handle(self, &action) {
            return;
        }
        if tree_controller::handle(self, &action) {
            return;
        }
        if file_viewer_controller::handle(self, &action) {
            return;
        }
        if terminal_controller::handle(self, &action) {
            return;
        }
        if context_exporter_controller::handle(self, &action) {
            return;
        }
        if source_control_controller::handle(self, &action) {
            return;
        }
        if diff_viewer_controller::handle(self, &action) {
            return;
        }
        if layout_controller::handle(self, &action) {
            return;
        }
        if workspace_controller::handle(self, &action) {
            return;
        }
    }

    pub fn finalize_frame(&mut self) {
        // Deferred effects (open file, select commit, refresh viewer)
        file_viewer_controller::finalize_frame(self);
    }

    // helpers used by UI (left here to avoid churn)
    pub fn excludes_joined(&self) -> String {
        format::join_excludes(&self.inputs.exclude_regex)
    }

    pub fn set_excludes_from_joined(&mut self, joined: &str) {
        self.inputs.exclude_regex = format::parse_excludes(joined);
    }
}
