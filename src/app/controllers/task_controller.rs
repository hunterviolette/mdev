use crate::app::actions::Action;
use crate::app::state::AppState;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::ExecuteLoopsDelete { loop_ids } => {
            for loop_id in loop_ids.iter().copied() {
                let _ = handle(state, &Action::ExecuteLoopDelete { loop_id });
            }
            true
        }

        Action::ExecuteLoopsSetPaused { loop_ids, paused } => {
            for loop_id in loop_ids.iter().copied() {
                if let Some(st) = state.execute_loops.get_mut(&loop_id) {
                    st.paused = *paused;
                }
                if let Some(snap) = state.execute_loop_store.get_mut(&loop_id) {
                    snap.paused = *paused;
                }
            }

            state.task_store_dirty = true;
            state.save_repo_task_store();
            true
        }

        Action::ExecuteLoopDelete { loop_id } => {
            use crate::app::actions::ComponentKind;
            use std::time::{SystemTime, UNIX_EPOCH};

            let now_ms: u64 = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            for canvas in state.canvases.iter_mut() {
                if let Some(w) = canvas.layout.get_window_mut(*loop_id) {
                    w.open = false;
                }
                canvas.layout.windows.remove(loop_id);
                canvas.layout.components.retain(|c| !(c.kind == ComponentKind::ExecuteLoop && c.id == *loop_id));
            }
            state.layout_epoch = state.layout_epoch.wrapping_add(1);

            state.execute_loops.remove(loop_id);
            state.execute_loop_store.remove(loop_id);

            for (_tid, t) in state.tasks.iter_mut() {
                t.execute_loop_ids.retain(|x| *x != *loop_id);

                if t.bound_execute_loop == Some(*loop_id) {
                    t.bound_execute_loop = None;
                    t.paused = false;
                    t.updated_at_ms = now_ms;
                }
            }

            state.task_store_dirty = true;
            state.save_repo_task_store();
            true
        }

        Action::TaskSetPaused { task_id, paused } => {
            if let Some(t) = state.tasks.get_mut(task_id) {
                t.paused = *paused;

                if let Some(loop_id) = t.bound_execute_loop {
                    if let Some(ls) = state.execute_loops.get_mut(&loop_id) {
                        ls.paused = *paused;
                    }
                }

                state.task_store_dirty = true;
                state.save_repo_task_store();
            }
            true
        }

        Action::TaskCreateConversationAndOpen { task_id, transport } => {
            use crate::app::layout::ExecuteLoopSnapshot;
            use crate::app::state::ExecuteLoopState;

            let (_cid, mut loop_id_opt, snap_clone) = {
                use std::time::{SystemTime, UNIX_EPOCH};
                let now_ms: u64 = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;

                let t = state.tasks.entry(*task_id).or_default();

                let cid = t.next_conversation_id;
                t.next_conversation_id = t.next_conversation_id.saturating_add(1);
                t.active_conversation = Some(cid);

                let mut st = ExecuteLoopState::new();
                st.transport = *transport;
                if st.transport == crate::app::state::ExecuteLoopTransport::BrowserBridge {
                    st.model = "browser-bridge".to_string();
                    st.browser_attached = false;
                    st.browser_session_id = None;
                }

                t.conversations.insert(
                    cid,
                    ExecuteLoopSnapshot {
                        model: st.model,
                        instruction: st.instruction,
                        include_context_next: st.include_context_next,
                        manual_fragments: st.manual_fragments.clone(),
                        automatic_fragments: st.automatic_fragments.clone(),
                        fragment_overrides: st.fragment_overrides.clone(),
                        auto_fill_first_changeset_applier: st.auto_fill_first_changeset_applier,
                        messages: st.messages,
                        conversation_id: st.conversation_id,
                        paused: t.paused,
                        created_at_ms: now_ms,
                        updated_at_ms: now_ms,
                        changeset_auto: st.changeset_auto,
                        postprocess_cmd: st.postprocess_cmd,
                        workflow_stages: st.workflow_stages.clone(),
                        workflow_active_stage: st.workflow_active_stage,
                        changesets_total: st.changesets_total,
                        changesets_ok: st.changesets_ok,
                        changesets_err: st.changesets_err,
                        postprocess_ok: st.postprocess_ok,
                        postprocess_err: st.postprocess_err,
                        transport: st.transport,
                        browser_profile: st.browser_profile,
                        browser_bridge_dir: st.browser_bridge_dir,
                        browser_cdp_url: st.browser_cdp_url,
                        browser_page_url_contains: st.browser_page_url_contains,
                        browser_target_url: st.browser_target_url,
                        browser_edge_executable: st.browser_edge_executable,
                        browser_user_data_dir: st.browser_user_data_dir,
                        browser_session_id: st.browser_session_id,
                        browser_status: st.browser_status,
                        browser_last_probe: st.browser_last_probe,
                        browser_probe_pending: st.browser_probe_pending,
                        browser_probe_error: st.browser_probe_error,
                        browser_attached: st.browser_attached,
                        browser_auto_launch_edge: st.browser_auto_launch_edge,
                        browser_response_timeout_ms: st.browser_response_timeout_ms,
                        browser_response_poll_ms: st.browser_response_poll_ms,
                    },
                );

                let snap_clone = t.conversations.get(&cid).cloned().unwrap();
                (cid, t.bound_execute_loop, snap_clone)
            };

            if loop_id_opt.is_none() {
                let new_id = state.new_execute_loop_component();
                state.persist_execute_loop_snapshot(new_id);
                loop_id_opt = Some(new_id);

                if let Some(t) = state.tasks.get_mut(task_id) {
                    t.bound_execute_loop = Some(new_id);
                }
            }

            let loop_id = loop_id_opt.unwrap();

            state.ensure_execute_loop_component_open(loop_id);
            state.apply_execute_loop_snapshot(loop_id, &snap_clone);
            if let Some(w) = state.active_layout_mut().get_window_mut(loop_id) {
                w.open = true;
            }

            state.task_store_dirty = true;
            state.save_repo_task_store();
            true
        }

        Action::TaskOpenConversation { task_id, conversation_id } => {
            let (mut loop_id_opt, snap_opt) = {
                let t = state.tasks.entry(*task_id).or_default();
                t.active_conversation = Some(*conversation_id);
                (t.bound_execute_loop, t.conversations.get(conversation_id).cloned())
            };

            if loop_id_opt.is_none() {
                let new_id = state.new_execute_loop_component();
                state.persist_execute_loop_snapshot(new_id);
                loop_id_opt = Some(new_id);

                if let Some(t) = state.tasks.get_mut(task_id) {
                    t.bound_execute_loop = Some(new_id);
                }
            }

            let loop_id = loop_id_opt.unwrap();

            state.ensure_execute_loop_component_open(loop_id);
            if let Some(snap) = snap_opt {
                state.apply_execute_loop_snapshot(loop_id, &snap);
            } else {
                state.ensure_execute_loop_state_loaded(loop_id);
            }
            if let Some(w) = state.active_layout_mut().get_window_mut(loop_id) {
                w.open = true;
            }

            state.task_store_dirty = true;
            state.save_repo_task_store();
            true
        }

        Action::TaskConversationsDelete { task_id, conversation_ids } => {
            if let Some(t) = state.tasks.get_mut(task_id) {
                for cid in conversation_ids {
                    t.conversations.remove(cid);
                    if t.active_conversation == Some(*cid) {
                        t.active_conversation = None;
                    }
                }
                state.task_store_dirty = true;
                state.save_repo_task_store();
            }
            true
        }

        Action::TaskConversationsSetPaused { task_id, conversation_ids, paused } => {
            if let Some(t) = state.tasks.get_mut(task_id) {
                for cid in conversation_ids {
                    if let Some(snap) = t.conversations.get_mut(cid) {
                        snap.paused = *paused;
                    }
                }
                state.task_store_dirty = true;
                state.save_repo_task_store();
            }
            true
        }

        Action::TaskBindExecuteLoop { task_id, loop_id } => {
            if let Some(t) = state.tasks.get_mut(task_id) {
                t.bound_execute_loop = Some(*loop_id);

                if !t.execute_loop_ids.iter().any(|x| *x == *loop_id) {
                    t.execute_loop_ids.push(*loop_id);
                }


                if let Some(ls) = state.execute_loops.get_mut(loop_id) {
                    ls.paused = t.paused;
                }

                state.task_store_dirty = true;
                state.save_repo_task_store();
            }
            true
        }

        Action::TaskCreateAndBindExecuteLoop { task_id } => {
            let new_id = state.new_execute_loop_component();

            state.persist_execute_loop_snapshot(new_id);

            if let Some(t) = state.tasks.get_mut(task_id) {
                t.bound_execute_loop = Some(new_id);

                if !t.execute_loop_ids.iter().any(|x| *x == new_id) {
                    t.execute_loop_ids.push(new_id);
                }


                if let Some(ls) = state.execute_loops.get_mut(&new_id) {
                    ls.paused = t.paused;
                }

                state.task_store_dirty = true;
                state.save_repo_task_store();
            }

            if let Some(w) = state.active_layout_mut().get_window_mut(new_id) {
                w.open = true;
            }

            true
        }

        Action::TaskOpenExecuteLoop { task_id } => {
            let bound = state
                .tasks
                .get(task_id)
                .and_then(|t| t.bound_execute_loop);

            let loop_id = match bound {
                Some(id) => id,
                None => {
                    let new_id = state.new_execute_loop_component();

                    state.persist_execute_loop_snapshot(new_id);

                    if let Some(t) = state.tasks.get_mut(task_id) {
                        t.bound_execute_loop = Some(new_id);
                        if let Some(ls) = state.execute_loops.get_mut(&new_id) {
                            ls.paused = t.paused;
                        }

                        state.task_store_dirty = true;
                        state.save_repo_task_store();
                    }
                    new_id
                }
            };

            state.ensure_execute_loop_component_open(loop_id);

            let snap_opt = state
                .tasks
                .get(task_id)
                .and_then(|t| t.active_conversation)
                .and_then(|cid| state.tasks.get(task_id).and_then(|t| t.conversations.get(&cid).cloned()));

            if let Some(snap) = snap_opt {
                state.apply_execute_loop_snapshot(loop_id, &snap);
            }

            if let Some(w) = state.active_layout_mut().get_window_mut(loop_id) {
                w.open = true;
            }

            true
        }

        _ => false,
    }
}
