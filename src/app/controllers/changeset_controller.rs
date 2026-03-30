use crate::app::actions::{Action, ComponentId, ComponentKind};
use crate::app::state::AppState;
use crate::gateway_model::{self, GatewayMode, SyncMode};

use anyhow::Result;
use serde_json::json;

use crate::app::workflow_api::{ensure_run_for_repo, WorkflowApiClient};

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
            invoke_payload_gateway(state, *applier_id);
            true
        }
        Action::ApplyChangeSet { applier_id } => {
            invoke_payload_gateway(state, *applier_id);
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

        self.changeset_appliers.retain(|id, _| ids.binary_search(id).is_ok());

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

fn build_api_client() -> WorkflowApiClient {
    let base = std::env::var("MDEV_WORKFLOW_API_BASE").unwrap_or_else(|_| "http://127.0.0.1:8787".to_string());
    WorkflowApiClient::new(base)
}

fn set_applier_status(state: &mut AppState, applier_id: ComponentId, status: String) {
    if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
        st.status = Some(status);
    }
}

fn invoke_payload_gateway(state: &mut AppState, applier_id: ComponentId) {
    let Some(repo) = state.inputs.repo.clone() else {
        set_applier_status(state, applier_id, "No repo selected. Pick a folder first.".to_string());
        return;
    };

    let (mode, sync_mode, payload_text, sync_skip_binary, sync_skip_gitignore) =
        match state.changeset_appliers.get(&applier_id) {
            Some(st) => (
                st.mode,
                st.sync_mode,
                st.payload.clone(),
                st.sync_skip_binary,
                st.sync_skip_gitignore,
            ),
            None => return,
        };

    let api = build_api_client();
    let result: Result<serde_json::Value> = (|| {
        let run_id = ensure_run_for_repo(&api, &repo, "Payload gateway")?;

        let mode_str = match mode {
            GatewayMode::ChangeSet => "changeset_apply",
            GatewayMode::Sync => "sync_generate",
        };

        let sync_mode_str = match sync_mode {
            SyncMode::Entire => "entire",
            SyncMode::Tree => "tree",
            SyncMode::Diff => "diff",
        };

        let response = api.invoke_payload_gateway(
            &run_id,
            Some("payload_gateway"),
            json!({
                "repo_ref": repo.to_string_lossy().replace('\\', "/"),
                "git_ref": state.inputs.git_ref,
                "exclude_regex": state.inputs.exclude_regex,
                "mode": mode_str,
                "sync_mode": sync_mode_str,
                "payload_text": payload_text,
                "tree_selection": state.tree.context_selected_files.iter().cloned().collect::<Vec<_>>(),
                "sync_skip_binary": sync_skip_binary,
                "sync_skip_gitignore": sync_skip_gitignore
            }),
        )?;

        Ok(json!({
            "run_id": run_id,
            "response": response,
            "mode": mode_str,
        }))
    })();

    match result {
        Ok(bundle) => {
            let run_id = bundle.get("run_id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let response = bundle.get("response").cloned().unwrap_or_else(|| json!({}));

            match mode {
                GatewayMode::Sync => {
                    let payload_text = response.get("payload_text").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                    if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
                        st.sync_payload = payload_text;
                        st.status = Some(format!("Sync payload generated via workflow API. run_id={}", run_id));
                    }
                }
                GatewayMode::ChangeSet => {
                    let summary = response.get("summary").and_then(|v| v.as_str()).unwrap_or("Payload gateway completed").to_string();
                    let normalized = response.get("normalized_payload").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                    let lines = response
                        .get("lines")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("\n"))
                        .unwrap_or_default();
                    let failed = response
                        .get("failed")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect::<Vec<_>>())
                        .unwrap_or_default();
                    let attempted = response
                        .get("attempted")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect::<Vec<_>>())
                        .unwrap_or_default();

                    if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
                        if !normalized.is_empty() {
                            st.payload = normalized.clone();
                            st.last_changeset_payload = normalized;
                        }
                        st.last_attempted_paths = attempted;
                        st.last_failed_paths = failed;
                        st.status = Some(format!("{} run_id={}", summary, run_id));
                        st.result_payload = if lines.is_empty() { summary.clone() } else { format!("{}\n\n{}", summary, lines) };
                        st.changeset_show_result = true;
                    }
                    if state.inputs.git_ref == crate::app::state::WORKTREE_REF {
                        state.refresh_tree_git_status();
                    }
                }
            }
        }
        Err(err) => {
            set_applier_status(state, applier_id, format!("Payload gateway failed: {err:#}"));
        }
    }
}
