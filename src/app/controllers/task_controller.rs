use crate::app::actions::Action;
use crate::app::state::AppState;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::ExecuteLoopsDelete { loop_ids } => {
            // Reuse single-delete semantics.
            for loop_id in loop_ids.iter().copied() {
                // Inline-dispatch by calling ourselves via an Action.
                // (No recursion risk: this arm calls the single-delete arm only.)
                let _ = handle(state, &Action::ExecuteLoopDelete { loop_id });
            }
            true
        }

        Action::ExecuteLoopsSetPaused { loop_ids, paused } => {
            // Pause/resume loops directly (loop-level), independent of Task paused.
            // Update both live state and persisted snapshot.
            for loop_id in loop_ids.iter().copied() {
                if let Some(st) = state.execute_loops.get_mut(&loop_id) {
                    st.paused = *paused;
                }
                if let Some(snap) = state.execute_loop_store.get_mut(&loop_id) {
                    snap.paused = *paused;
                }
            }

            // Persist (RepoTaskStoreFile contains execute_loops + tasks).
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

            // Close + remove the loop window/component from layout.
            if let Some(w) = state.layout.get_window_mut(*loop_id) {
                w.open = false;
            }
            state.layout.windows.remove(loop_id);
            state.layout.components.retain(|c| {
                !(c.kind == ComponentKind::ExecuteLoop && c.id == *loop_id)
            });
            state.layout_epoch = state.layout_epoch.wrapping_add(1);

            // Remove live loop state (if loaded) and persisted snapshot.
            state.execute_loops.remove(loop_id);
            state.execute_loop_store.remove(loop_id);

            // Unbind any tasks bound to this loop.
            for (_tid, t) in state.tasks.iter_mut() {
                // Remove from the task's conversation list regardless of whether it's currently bound.
                t.execute_loop_ids.retain(|x| *x != *loop_id);

                if t.bound_execute_loop == Some(*loop_id) {
                    t.bound_execute_loop = None;
                    // Consistent with reset-on-(re)bind semantics: deletion clears pause.
                    t.paused = false;
                    t.updated_at_ms = now_ms;
                }
            }

            // Persist (RepoTaskStoreFile contains both execute_loops + tasks).
            state.task_store_dirty = true;
            state.save_repo_task_store();
            true
        }

        Action::TaskSetPaused { task_id, paused } => {
            if let Some(t) = state.tasks.get_mut(task_id) {
                t.paused = *paused;

                // Mirror pause onto the bound Execute Loop.
                if let Some(loop_id) = t.bound_execute_loop {
                    if let Some(ls) = state.execute_loops.get_mut(&loop_id) {
                        ls.paused = *paused;
                    }
                }

                // Write-through persistence
                state.task_store_dirty = true;
                state.save_repo_task_store();
            }
            true
        }

        Action::TaskCreateConversationAndOpen { task_id } => {
            use crate::app::layout::ExecuteLoopSnapshot;
            use crate::app::state::ExecuteLoopState;

            // Step 1: mutate task + create conversation snapshot; extract what we need, then drop the task borrow.
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

                let st = ExecuteLoopState::new();
                t.conversations.insert(
                    cid,
                    ExecuteLoopSnapshot {
                        model: st.model,
                        instruction: st.instruction,
                        mode: st.mode,
                        include_context_next: st.include_context_next,
                        auto_fill_first_changeset_applier: st.auto_fill_first_changeset_applier,
                        messages: st.messages,
                        conversation_id: st.conversation_id,
                        paused: t.paused,
                        // IMPORTANT: created/updated are per-conversation and must be set here.
                        created_at_ms: now_ms,
                        updated_at_ms: now_ms,
                        changeset_auto: st.changeset_auto,
                        postprocess_cmd: st.postprocess_cmd,
                        changesets_total: st.changesets_total,
                        changesets_ok: st.changesets_ok,
                        changesets_err: st.changesets_err,
                        postprocess_ok: st.postprocess_ok,
                        postprocess_err: st.postprocess_err,
                    },
                );

                let snap_clone = t.conversations.get(&cid).cloned().unwrap();
                (cid, t.bound_execute_loop, snap_clone)
            };

            // Step 2: if no loop bound yet, create one WITHOUT holding a mutable borrow into tasks.
            if loop_id_opt.is_none() {
                let new_id = state.new_execute_loop_component();
                state.persist_execute_loop_snapshot(new_id);
                loop_id_opt = Some(new_id);

                // write back bound loop id
                if let Some(t) = state.tasks.get_mut(task_id) {
                    t.bound_execute_loop = Some(new_id);
                }
            }

            let loop_id = loop_id_opt.unwrap();

            // Step 3: open + hydrate from the conversation snapshot.
            state.ensure_execute_loop_component_open(loop_id);
            state.apply_execute_loop_snapshot(loop_id, &snap_clone);
            if let Some(w) = state.layout.get_window_mut(loop_id) {
                w.open = true;
            }

            state.task_store_dirty = true;
            state.save_repo_task_store();
            true
        }

        Action::TaskOpenConversation { task_id, conversation_id } => {
            // Step 1: set active conversation and grab snapshot clone + bound loop id, then drop task borrow.
            let (mut loop_id_opt, snap_opt) = {
                let t = state.tasks.entry(*task_id).or_default();
                t.active_conversation = Some(*conversation_id);
                (t.bound_execute_loop, t.conversations.get(conversation_id).cloned())
            };

            // Step 2: ensure a loop window exists (create if needed) without holding the task borrow.
            if loop_id_opt.is_none() {
                let new_id = state.new_execute_loop_component();
                state.persist_execute_loop_snapshot(new_id);
                loop_id_opt = Some(new_id);

                if let Some(t) = state.tasks.get_mut(task_id) {
                    t.bound_execute_loop = Some(new_id);
                }
            }

            let loop_id = loop_id_opt.unwrap();

            // Step 3: open + hydrate.
            state.ensure_execute_loop_component_open(loop_id);
            if let Some(snap) = snap_opt {
                state.apply_execute_loop_snapshot(loop_id, &snap);
            } else {
                state.ensure_execute_loop_state_loaded(loop_id);
            }
            if let Some(w) = state.layout.get_window_mut(loop_id) {
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

                // Track this loop under the task (idempotent).
                if !t.execute_loop_ids.iter().any(|x| *x == *loop_id) {
                    t.execute_loop_ids.push(*loop_id);
                }


                // Mirror current task pause onto the loop immediately.
                if let Some(ls) = state.execute_loops.get_mut(loop_id) {
                    ls.paused = t.paused;
                }

                // Write-through persistence
                state.task_store_dirty = true;
                state.save_repo_task_store();
            }
            true
        }

        Action::TaskCreateAndBindExecuteLoop { task_id } => {
            // Create a new chat thread (Execute Loop), bind it to the task, and open it.
            let new_id = state.new_execute_loop_component();

            // Baseline snapshot so the loop appears in the Task panel immediately.
            // (created_at/updated_at can remain 0 until first assistant/OAI message, per persist logic.)
            state.persist_execute_loop_snapshot(new_id);

            if let Some(t) = state.tasks.get_mut(task_id) {
                t.bound_execute_loop = Some(new_id);

                // Track this loop under the task (idempotent).
                if !t.execute_loop_ids.iter().any(|x| *x == new_id) {
                    t.execute_loop_ids.push(new_id);
                }


                // Mirror task pause to the new loop.
                if let Some(ls) = state.execute_loops.get_mut(&new_id) {
                    ls.paused = t.paused;
                }

                // Write-through persistence
                state.task_store_dirty = true;
                state.save_repo_task_store();
            }

            if let Some(w) = state.layout.get_window_mut(new_id) {
                w.open = true;
            }

            true
        }

        Action::TaskOpenExecuteLoop { task_id } => {
            // If bound, open and hydrate the ExecuteLoop view on-demand.
            // If not bound, create a new ExecuteLoop, bind, persist snapshot, and open.
            let bound = state
                .tasks
                .get(task_id)
                .and_then(|t| t.bound_execute_loop);

            let loop_id = match bound {
                Some(id) => id,
                None => {
                    let new_id = state.new_execute_loop_component();

                    // Baseline snapshot so the loop appears in the Task panel immediately.
                    state.persist_execute_loop_snapshot(new_id);

                    if let Some(t) = state.tasks.get_mut(task_id) {
                        t.bound_execute_loop = Some(new_id);
                        if let Some(ls) = state.execute_loops.get_mut(&new_id) {
                            ls.paused = t.paused;
                        }

                        // Write-through persistence
                        state.task_store_dirty = true;
                        state.save_repo_task_store();
                    }
                    new_id
                }
            };

            // Ensure layout exists.
            state.ensure_execute_loop_component_open(loop_id);

            // Task-owned: hydrate the ExecuteLoop view strictly from the Task's ACTIVE conversation snapshot.
            // ExecuteLoops do not own OpenAI conversation identity.
            let snap_opt = state
                .tasks
                .get(task_id)
                .and_then(|t| t.active_conversation)
                .and_then(|cid| state.tasks.get(task_id).and_then(|t| t.conversations.get(&cid).cloned()));

            if let Some(snap) = snap_opt {
                state.apply_execute_loop_snapshot(loop_id, &snap);
            }

            if let Some(w) = state.layout.get_window_mut(loop_id) {
                w.open = true;
            }

            true
        }

        _ => false,
    }
}
