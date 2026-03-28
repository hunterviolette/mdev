use crate::{format};

use super::actions::Action;
use super::state::{AppState, GlobalPerfConfig};

use super::controllers::{
    changeset_loop_controller,
    analysis_controller, changeset_controller, context_exporter_controller, diff_viewer_controller,
    file_viewer_controller, layout_controller, palette_controller, sap_adt_controller, source_control_controller,
    terminal_controller, tree_controller, personalization, workspace_controller,
    task_controller,
};

use crate::app::layout::{ExecuteLoopSnapshot, TaskSnapshot};
use crate::app::task_store::{RepoTaskStoreFile, repo_key_for_path, task_store_path};

impl AppState {
    fn task_store_dir(&self) -> anyhow::Result<std::path::PathBuf> {
        let mut dir = self.platform.app_data_dir("devApp")?;
        dir.push("task_store");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn global_perf_config_path(&self) -> anyhow::Result<std::path::PathBuf> {
        let mut dir = self.platform.app_data_dir("devApp")?;
        std::fs::create_dir_all(&dir)?;
        dir.push("global_perf_config.json");
        Ok(dir)
    }

    pub fn load_global_perf_config_from_appdata(&mut self) {
        let path = match self.global_perf_config_path() {
            Ok(path) => path,
            Err(_) => return,
        };

        let perf = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<GlobalPerfConfig>(&text).ok())
            .unwrap_or_default()
            .normalized();

        self.perf = perf;
    }

    pub fn save_global_perf_config_to_appdata(&self) {
        let path = match self.global_perf_config_path() {
            Ok(path) => path,
            Err(_) => return,
        };

        let Ok(text) = serde_json::to_string_pretty(&self.perf.normalized()) else {
            return;
        };

        let _ = Self::atomic_write_json(&path, &text);
    }

    fn repo_task_store_path(&self, repo: &std::path::PathBuf) -> anyhow::Result<std::path::PathBuf> {
        let dir = self.task_store_dir()?;
        Ok(task_store_path(&dir, repo))
    }

    fn atomic_write_json(path: &std::path::Path, text: &str) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn save_repo_task_store(&mut self) {
        let Some(repo) = self.inputs.repo.clone() else {
            self.task_store_dirty = false;
            return;
        };

        let path = match self.repo_task_store_path(&repo) {
            Ok(p) => p,
            Err(_) => return,
        };

        let execute_loops = self.execute_loop_store.clone();

        let mut tasks = std::collections::HashMap::new();
        for (id, t) in self.tasks.iter() {
            tasks.insert(
                *id,
                TaskSnapshot {
                    bound_execute_loop: t.bound_execute_loop,
                    paused: t.paused,
                    execute_loop_ids: t.execute_loop_ids.clone(),
                    created_at_ms: t.created_at_ms,
                    updated_at_ms: t.updated_at_ms,
                    conversations: t.conversations.clone(),
                    active_conversation: t.active_conversation,
                    next_conversation_id: t.next_conversation_id,
                },
            );
        }

        let repo_key = repo_key_for_path(&repo);
        let file = RepoTaskStoreFile {
            version: 1,
            repo_key,
            execute_loops,
            tasks,
        };

        let Ok(text) = serde_json::to_string_pretty(&file) else {
            return;
        };

        let _ = Self::atomic_write_json(&path, &text);
        self.task_store_dirty = false;
    }

    pub fn load_repo_task_store(&mut self) -> bool {
        let Some(repo) = self.inputs.repo.clone() else {
            return false;
        };

        let path = match self.repo_task_store_path(&repo) {
            Ok(p) => p,
            Err(_) => return false,
        };

        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => return false,
        };

        let parsed: RepoTaskStoreFile = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => return false,
        };

        self.execute_loop_store = parsed.execute_loops.clone();

        for (task_id, ts) in parsed.tasks.iter() {
            let exists = self
                .all_layouts()
                .any(|l| l.components.iter().any(|c| {
                    c.kind == crate::app::actions::ComponentKind::Task
                        && self.task_id_for_component_ref(c.id) == *task_id
                }));
            if !exists {
                continue;
            }


            self.tasks.insert(
                *task_id,
                crate::app::state::TaskState {
                    bound_execute_loop: ts.bound_execute_loop,
                    paused: ts.paused,
                    execute_loop_ids: ts.execute_loop_ids.clone(),
                    created_at_ms: ts.created_at_ms,
                    updated_at_ms: ts.updated_at_ms,
                    conversations: ts.conversations.clone(),
                    active_conversation: ts.active_conversation,
                    next_conversation_id: ts.next_conversation_id,
                },
            );

            if let Some(loop_id) = ts.bound_execute_loop {
                if let Some(ls) = self.execute_loops.get_mut(&loop_id) {
                    ls.paused = ts.paused;
                }
            }
        }

        self.task_store_dirty = false;
        true
    }


    pub fn apply_execute_loop_snapshot(
        &mut self,
        loop_id: crate::app::actions::ComponentId,
        snap: &crate::app::layout::ExecuteLoopSnapshot,
    ) {
        use crate::app::state::ExecuteLoopState;

        let st = self.execute_loops.entry(loop_id).or_insert_with(ExecuteLoopState::new);

        st.model = snap.model.clone();
        st.instruction = snap.instruction.clone();
        st.include_context_next = snap.include_context_next;
        st.manual_fragments = snap.manual_fragments.clone();
        st.automatic_fragments = snap.automatic_fragments.clone();
        st.fragment_overrides = snap.fragment_overrides.clone();
        st.automation_policies = snap.automation_policies.clone();
        st.reset_apply_failure_focused_context_runtime();
        st.auto_fill_first_changeset_applier = snap.auto_fill_first_changeset_applier;
        st.messages = snap.messages.clone();
        st.conversation_id = snap.conversation_id.clone();
        st.changesets_total = snap.changesets_total;
        st.changeset_auto = snap.changeset_auto;
        st.postprocess_cmd = snap.postprocess_cmd.clone();
        st.workflow_stages = snap.workflow_stages.clone();
        st.workflow_active_stage = snap.workflow_active_stage;
        st.changesets_ok = snap.changesets_ok;
        st.changesets_err = snap.changesets_err;
        st.postprocess_ok = snap.postprocess_ok;
        st.postprocess_err = snap.postprocess_err;
        st.paused = snap.paused;
        st.transport = snap.transport;
        st.browser_profile = snap.browser_profile.clone();
        st.browser_cdp_url = snap.browser_cdp_url.clone();
        st.browser_page_url_contains = snap.browser_page_url_contains.clone();
        st.browser_target_url = snap.browser_target_url.clone();
        st.browser_session_id = snap.browser_session_id.clone();
        st.browser_status = snap.browser_status;
        st.browser_last_probe = snap.browser_last_probe.clone();
        st.browser_probe_pending = snap.browser_probe_pending;
        st.browser_probe_error = snap.browser_probe_error.clone();
        st.browser_attached = snap.browser_attached;
        st.browser_auto_launch_edge = snap.browser_auto_launch_edge;
        st.browser_response_timeout_ms = snap.browser_response_timeout_ms;
        st.browser_response_poll_ms = snap.browser_response_poll_ms;
        st.browser_response_timeout_input = ((snap.browser_response_timeout_ms.max(1000) + 999) / 1000).to_string();
        st.browser_timeout_confirm_pending = false;
        st.ensure_default_changeset_workflow();
    }

    pub fn ensure_execute_loop_state_loaded(&mut self, loop_id: crate::app::actions::ComponentId) {
        use crate::app::state::ExecuteLoopState;

        let _st = self
            .execute_loops
            .entry(loop_id)
            .or_insert_with(ExecuteLoopState::new);

        if let Some(snap) = self.execute_loop_store.get(&loop_id) {
            let snap = snap.clone();
            self.apply_execute_loop_snapshot(loop_id, &snap);
        }
    }

    pub fn persist_execute_loop_snapshot(&mut self, loop_id: crate::app::actions::ComponentId) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now_ms: u64 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let Some(st) = self.execute_loops.get(&loop_id) else {
            return;
        };

        let mut bump_oai_ts = false;
        if let Some(last) = st.messages.last() {
            let role_dbg = format!("{:?}", last);
            if role_dbg.to_ascii_lowercase().contains("assistant") {
                bump_oai_ts = true;
            }
        }

        let prev_created = self
            .execute_loop_store
            .get(&loop_id)
            .map(|s| s.created_at_ms)
            .unwrap_or(0);
        let prev_updated = self
            .execute_loop_store
            .get(&loop_id)
            .map(|s| s.updated_at_ms)
            .unwrap_or(0);

        let created_at_ms = if prev_created != 0 {
            prev_created
        } else if bump_oai_ts {
            now_ms
        } else {
            0
        };

        let updated_at_ms = if bump_oai_ts { now_ms } else { prev_updated };

        let mut task_conversation_writeback: Option<(
            crate::app::actions::ComponentId,
            crate::app::actions::ConversationId,
        )> = None;
        for (tid, t) in self.tasks.iter() {
            if t.bound_execute_loop == Some(loop_id) {
                if let Some(cid) = t.active_conversation {
                    task_conversation_writeback = Some((*tid, cid));
                    break;
                }
            }
        }

        self.execute_loop_store.insert(
            loop_id,
            ExecuteLoopSnapshot {
                model: st.model.clone(),
                instruction: st.instruction.clone(),
                include_context_next: st.include_context_next,
                manual_fragments: st.manual_fragments.clone(),
                automatic_fragments: st.automatic_fragments.clone(),
                fragment_overrides: st.fragment_overrides.clone(),
                automation_policies: st.automation_policies.clone(),
                auto_fill_first_changeset_applier: st.auto_fill_first_changeset_applier,
                messages: st.messages.clone(),
                conversation_id: None,
                paused: st.paused,
                created_at_ms,
                updated_at_ms,
                changeset_auto: st.changeset_auto,
                postprocess_cmd: st.postprocess_cmd.clone(),
                workflow_stages: st.workflow_stages.clone(),
                workflow_active_stage: st.workflow_active_stage,
                changesets_total: st.changesets_total,
                changesets_ok: st.changesets_ok,
                changesets_err: st.changesets_err,
                postprocess_ok: st.postprocess_ok,
                postprocess_err: st.postprocess_err,
                transport: st.transport,
                browser_profile: st.browser_profile.clone(),
                browser_bridge_dir: String::new(),
                browser_cdp_url: st.browser_cdp_url.clone(),
                browser_page_url_contains: st.browser_page_url_contains.clone(),
                browser_target_url: st.browser_target_url.clone(),
                browser_edge_executable: String::new(),
                browser_user_data_dir: String::new(),
                browser_session_id: st.browser_session_id.clone(),
                browser_status: st.browser_status,
                browser_last_probe: st.browser_last_probe.clone(),
                browser_probe_pending: st.browser_probe_pending,
                browser_probe_error: st.browser_probe_error.clone(),
                browser_attached: st.browser_attached,
                browser_auto_launch_edge: st.browser_auto_launch_edge,
                browser_response_timeout_ms: st.browser_response_timeout_ms,
                browser_response_poll_ms: st.browser_response_poll_ms,
            },
        );

        if let Some((tid, cid)) = task_conversation_writeback {
            if let Some(t) = self.tasks.get_mut(&tid) {
                let prev_created = t
                    .conversations
                    .get(&cid)
                    .map(|s| s.created_at_ms)
                    .unwrap_or(0);

                let c_created_at_ms = if prev_created != 0 { prev_created } else { now_ms };

                t.conversations.insert(
                    cid,
                    ExecuteLoopSnapshot {
                        model: st.model.clone(),
                        instruction: st.instruction.clone(),
                        include_context_next: st.include_context_next,
                        manual_fragments: st.manual_fragments.clone(),
                        automatic_fragments: st.automatic_fragments.clone(),
                        fragment_overrides: st.fragment_overrides.clone(),
                        automation_policies: st.automation_policies.clone(),
                        auto_fill_first_changeset_applier: st.auto_fill_first_changeset_applier,
                        messages: st.messages.clone(),
                        conversation_id: st.conversation_id.clone(),
                        paused: st.paused,
                        created_at_ms: c_created_at_ms,
                        updated_at_ms: now_ms,
                        changeset_auto: st.changeset_auto,
                        postprocess_cmd: st.postprocess_cmd.clone(),
                        workflow_stages: st.workflow_stages.clone(),
                        workflow_active_stage: st.workflow_active_stage,
                        changesets_total: st.changesets_total,
                        changesets_ok: st.changesets_ok,
                        changesets_err: st.changesets_err,
                        postprocess_ok: st.postprocess_ok,
                        postprocess_err: st.postprocess_err,
                        transport: st.transport,
                        browser_profile: st.browser_profile.clone(),
                        browser_bridge_dir: String::new(),
                        browser_cdp_url: st.browser_cdp_url.clone(),
                        browser_page_url_contains: st.browser_page_url_contains.clone(),
                        browser_target_url: st.browser_target_url.clone(),
                        browser_edge_executable: String::new(),
                        browser_user_data_dir: String::new(),
                        browser_session_id: st.browser_session_id.clone(),
                        browser_status: st.browser_status,
                        browser_last_probe: st.browser_last_probe.clone(),
                        browser_probe_pending: st.browser_probe_pending,
                        browser_probe_error: st.browser_probe_error.clone(),
                        browser_attached: st.browser_attached,
                        browser_auto_launch_edge: st.browser_auto_launch_edge,
                        browser_response_timeout_ms: st.browser_response_timeout_ms,
                        browser_response_poll_ms: st.browser_response_poll_ms,
                    },
                );

                self.task_store_dirty = true;
                self.save_repo_task_store();
            }
        }
    }

    pub fn ensure_execute_loop_component_open(&mut self, loop_id: crate::app::actions::ComponentId) {
        use crate::app::actions::ComponentKind;
        use crate::app::layout::{ComponentInstance, WindowLayout};

        {
            let active_layout = self.active_layout_mut();
            let exists_here = active_layout
                .components
                .iter()
                .any(|c| c.kind == ComponentKind::ExecuteLoop && c.id == loop_id);

            if exists_here {
                if let Some(w) = active_layout.get_window_mut(loop_id) {
                    w.open = true;
                }
                return;
            }

            let occupied_by_other_kind = active_layout
                .components
                .iter()
                .any(|c| c.id == loop_id && c.kind != ComponentKind::ExecuteLoop);

            if occupied_by_other_kind {
                active_layout.components.retain(|c| c.id != loop_id);
                active_layout.windows.remove(&loop_id);
            }
        }

        self.active_layout_mut().merge_with_defaults();
        self.active_layout_mut().components.push(ComponentInstance {
            id: loop_id,
            kind: ComponentKind::ExecuteLoop,
            title: format!("Execute Loop {}", loop_id),
        });
        self.active_layout_mut().windows.insert(
            loop_id,
            WindowLayout {
                open: true,
                locked: false,
                pos_norm: None,
                size_norm: None,
                pos: [150.0, 150.0],
                size: [860.0, 680.0],
            },
        );
        self.layout_epoch = self.layout_epoch.wrapping_add(1);
    }
    pub fn apply_action(&mut self, action: Action) {
        if palette_controller::handle(self, &action) {
            return;
        }
        if personalization::handle(self, &action) {
            return;
        }
        if analysis_controller::handle(self, &action) {
            return;
        }
        if changeset_controller::handle(self, &action) {
            return;
        }
        if changeset_loop_controller::handle(self, &action) {
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
        if task_controller::handle(self, &action) {
            return;
        }
        if sap_adt_controller::handle(self, &action) {
            return;
        }
        if layout_controller::handle(self, &action) {
            return;
        }
        if workspace_controller::handle(self, &action) {
            return;
        }
    }

    pub fn poll_git_status_refresh(&mut self) -> bool {
        tree_controller::poll_git_status_refresh(self)
    }

    pub fn poll_diff_stats_refresh(&mut self) -> bool {
        tree_controller::poll_diff_stats_refresh(self)
    }

    pub fn finalize_frame(&mut self) {
        file_viewer_controller::finalize_frame(self);
        sap_adt_controller::finalize_frame(self);
        sap_adt_controller::finalize_frame(self);
    }

    pub fn excludes_joined(&self) -> String {
        format::join_excludes(&self.inputs.exclude_regex)
    }

    pub fn set_excludes_from_joined(&mut self, joined: &str) {
        self.inputs.exclude_regex = format::parse_excludes(joined);
    }
}
