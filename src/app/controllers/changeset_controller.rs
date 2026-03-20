
use crate::app::actions::{Action, ComponentId, ComponentKind};
use crate::app::state::AppState;
use crate::gateway_model::{self, GatewayMode, SyncMode};

pub fn normalize_and_validate_changeset_payload_text(raw: &str) -> anyhow::Result<String> {
    gateway_model::changeset::normalize_and_validate_payload_text(raw)
}


pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::SetChangeSetGatewayMode { applier_id, mode } => {
            if let Some(st) = state.changeset_appliers.get_mut(applier_id) {
                st.mode = *mode;
                st.status = None;
                st.result_payload.clear();
                st.changeset_show_result = false;
            }
            true
        }
        Action::SetChangeSetSyncMode { applier_id, mode } => {
            if let Some(st) = state.changeset_appliers.get_mut(applier_id) {
                st.sync_mode = *mode;
                st.status = None;
                st.sync_payload.clear();
            }
            true
        }
        Action::SetChangeSetSyncSkipBinary { applier_id, value } => {
            if let Some(st) = state.changeset_appliers.get_mut(applier_id) {
                st.sync_skip_binary = *value;
                st.status = None;
            }
            true
        }
        Action::SetChangeSetSyncSkipGitignore { applier_id, value } => {
            if let Some(st) = state.changeset_appliers.get_mut(applier_id) {
                st.sync_skip_gitignore = *value;
                st.status = None;
            }
            true
        }
        Action::GenerateSyncPayload { applier_id } => {
            gateway_model::sync::generate_payload(state, *applier_id);
            true
        }
        Action::ApplyChangeSet { applier_id } => {
            apply_changeset(state, *applier_id);
            true
        }
        Action::ClearChangeSet { applier_id } => {
            if let Some(st) = state.changeset_appliers.get_mut(applier_id) {
                st.payload.clear();
                st.sync_payload.clear();
                st.last_changeset_payload.clear();
                st.result_payload.clear();
                st.changeset_show_result = false;
                st.status = None;
                st.last_attempted_paths.clear();
                st.last_failed_paths.clear();
            }
            true
        }
        _ => false,
    }
}

impl AppState {
    pub fn rebuild_changeset_appliers_from_layout(&mut self) {
        use crate::app::state::ChangeSetApplierState;

        let mut ids: Vec<ComponentId> = self
            .all_layouts()
            .flat_map(|l| l.components.iter())
            .filter(|c| c.kind == ComponentKind::ChangeSetApplier)
            .map(|c| c.id)
            .collect();

        ids.sort_unstable();
        ids.dedup();

        self.changeset_appliers
            .retain(|id, _| ids.binary_search(id).is_ok());

        for id in ids {
            self.changeset_appliers.entry(id).or_insert(ChangeSetApplierState {
                mode: GatewayMode::ChangeSet,
                sync_mode: SyncMode::Tree,
                payload: String::new(),
                sync_payload: String::new(),
                sync_skip_binary: true,
                sync_skip_gitignore: true,
                status: None,
                last_changeset_payload: String::new(),
                result_payload: String::new(),
                changeset_show_result: false,
                last_attempted_paths: Vec::new(),
                last_failed_paths: Vec::new(),
            });
        }
    }
}

fn apply_changeset(state: &mut AppState, applier_id: ComponentId) {
    gateway_model::changeset::apply(state, applier_id)
}

fn set_applier_status(state: &mut AppState, applier_id: ComponentId, status: String) {
    if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
        st.status = Some(status);
    }
}

