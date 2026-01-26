// src/app/ui/changeset_loop.rs
use eframe::egui;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::{AppState, ExecuteLoopMessage, ExecuteLoopMode};

pub fn changeset_loop_panel(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    loop_id: ComponentId,
) -> Vec<Action> {
    let mut actions: Vec<Action> = Vec::new();

    // ExecuteLoop UI is a view over the global Task store.
    // Pause/resume is written through Tasks; TaskStore is the source of truth.
    let mut bound_tasks: Vec<ComponentId> = state
        .tasks
        .iter()
        .filter_map(|(tid, t)| (t.bound_execute_loop == Some(loop_id)).then_some(*tid))
        .collect();
    bound_tasks.sort();

    let bound_paused: Option<bool> = bound_tasks
        .first()
        .and_then(|tid| state.tasks.get(tid))
        .map(|t| t.paused);

    let Some(st) = state.execute_loops.get_mut(&loop_id) else {
        ui.label("Execute loop state missing (try resetting layout/workspace).");
        return actions;
    };

    // Mirror pause from Task -> ExecuteLoop (view-only).
    if let Some(p) = bound_paused {
        st.paused = p;
    }

    // Track whether we should persist to RepoTaskStore.
    let mut did_mutate = false;

    // Snapshot fields so we can detect UI edits and save.
    let before_model = st.model.clone();
    let before_instruction = st.instruction.clone();
    let before_mode = st.mode;
    let before_include_ctx = st.include_context_next;
    let before_auto_fill = st.auto_fill_first_changeset_applier;
    let before_changeset_auto = st.changeset_auto;
    let before_postprocess_cmd = st.postprocess_cmd.clone();

    // ------------------------------------------------------------
    // Fetch & hydrate server-side conversation history when resuming
    // ------------------------------------------------------------
    // If we have a conversation_id but haven't synced its items into st.messages yet,
    // fetch /v1/conversations/{id}/items on a background thread and replace st.messages.
    // This makes "Open Execute Loop" show the historical transcript and ensures local state
    // matches the persistent conversation.
    if !st.history_sync_pending {
        if let Some(cid) = st.conversation_id.clone() {
            let already_synced = st
                .history_synced_conversation_id
                .as_deref()
                .map(|s| s == cid)
                .unwrap_or(false);

            if !already_synced {
                let openai = state.openai.clone();
                let (tx, rx) = std::sync::mpsc::channel::<
                    Result<Vec<crate::app::state::ExecuteLoopMessage>, String>,
                >();

                st.history_sync_pending = true;
                st.history_sync_rx = Some(rx);

                std::thread::spawn(move || {
                    let res = openai
                        .list_conversation_messages(&cid)
                        .map(|pairs| {
                            pairs
                                .into_iter()
                                .map(|(role, content)| crate::app::state::ExecuteLoopMessage {
                                    role,
                                    content,
                                })
                                .collect::<Vec<_>>()
                        })
                        .map_err(|e| format!("{:#}", e));
                    let _ = tx.send(res);
                });
            }
        }
    }

    if st.history_sync_pending {
        if let Some(rx) = &st.history_sync_rx {
            match rx.try_recv() {
                Ok(Ok(msgs)) => {
                    // Replace local transcript with server transcript.
                    st.messages = msgs;
                    st.history_synced_conversation_id = st.conversation_id.clone();
                    st.history_sync_pending = false;
                    st.history_sync_rx = None;
                                    }
                Ok(Err(err)) => {
                    // Non-fatal: keep local messages; show status.
                    st.last_status = Some(format!("History sync failed: {}", err));
                    st.history_sync_pending = false;
                    st.history_sync_rx = None;
                    did_mutate = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint();
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    st.last_status = Some("History sync channel disconnected.".to_string());
                    st.history_sync_pending = false;
                    st.history_sync_rx = None;
                    did_mutate = true;
                }
            }
        } else {
            st.history_sync_pending = false;
        }
    }

    // ------------------------------------------------------------
    // Poll in-flight OpenAI request (non-blocking)
    // ------------------------------------------------------------
    if st.pending {
        if let Some(rx) = &st.pending_rx {
            match rx.try_recv() {
                Ok(Ok(out)) => {
                    // Persist OpenAI conversation id so subsequent turns can send only deltas.
                    st.conversation_id = Some(out.conversation_id.clone());

                    st.messages.push(ExecuteLoopMessage {
                        role: "assistant".to_string(),
                        content: out.text.clone(),
                    });
                    did_mutate = true;

                    st.pending = false;
                    st.pending_rx = None;

                    if st.mode == ExecuteLoopMode::ChangeSet {
                        // Auto-fill and auto-apply
                        if st.auto_fill_first_changeset_applier {
                            if let Some((applier_id, ap)) = state.changeset_appliers.iter_mut().next() {
                                ap.payload = out.text.clone();
                                ap.status = Some(format!("Auto-filled from Execute Loop {}", loop_id));

                                // Log the attempt into the chat so it’s visible.
                                st.messages.push(ExecuteLoopMessage {
                                    role: "system".to_string(),
                                    content: format!(
                                        "CHANGESET AUTO-APPLY: sending payload to ChangeSetApplier {} (then applying)",
                                        applier_id
                                    ),
                                });
                                did_mutate = true;

                                // Track this applier so we can log its result status changes.
                                st.last_auto_applier_id = Some(*applier_id);
                                st.last_auto_applier_status = ap.status.clone();
                                // Mark that we are waiting on the apply result to decide next step.
                                st.awaiting_apply_result = true;

                                // Stats: count an auto-apply attempt.
                                st.changesets_total = st.changesets_total.saturating_add(1);

                                actions.push(Action::ApplyChangeSet { applier_id: *applier_id });
                                st.last_status = Some("ChangeSet received: auto-applying…".to_string());
                            } else {
                                st.last_status = Some(
                                    "ChangeSet received, but no ChangeSet Applier exists.".to_string(),
                                );
                            }
                        } else {
                            st.last_status = Some("ChangeSet received (auto-fill disabled).".to_string());
                        }

                        // Pause only if manual stepping.
                        st.awaiting_review = !st.changeset_auto;
                    } else {
                        st.last_status = Some("Response received.".to_string());
                    }
                }
                Ok(Err(err)) => {
                    st.messages.push(ExecuteLoopMessage {
                        role: "assistant".to_string(),
                        content: format!("[error]\n{}", err),
                    });
                    did_mutate = true;

                    st.pending = false;
                    st.pending_rx = None;
                    st.last_status = Some("Request failed.".to_string());
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // still waiting
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    st.pending = false;
                    st.pending_rx = None;
                    st.last_status = Some("Request channel disconnected.".to_string());
                    did_mutate = true;
                }
            }
        } else {
            st.pending = false;
        }

        // keep repainting so we notice completion quickly
        ctx.request_repaint();
    }

    // ------------------------------------------------------------
    // Poll auto-apply result (ChangeSetApplier.status) and log into chat
    // ------------------------------------------------------------
    if let Some(applier_id) = st.last_auto_applier_id {
        if let Some(ap) = state.changeset_appliers.get(&applier_id) {
            let cur = ap.status.clone();
            if cur != st.last_auto_applier_status {
                if let Some(s) = cur.clone() {
                    st.messages.push(ExecuteLoopMessage {
                        role: "system".to_string(),
                        content: format!("CHANGESET APPLY RESULT (applier {}): {}", applier_id, s),
                    });
                    did_mutate = true;
                }
                st.last_auto_applier_status = cur;
            }
        }
    }

    // If we are in ChangeSet mode and an auto-apply failed, feed the error back to the model.
    // Heuristic: treat statuses containing "error" or "invalid" or "failed" as failures.
    if st.mode == ExecuteLoopMode::ChangeSet && st.awaiting_apply_result {
        if let Some(applier_id) = st.last_auto_applier_id {
            if let Some(ap) = state.changeset_appliers.get(&applier_id) {
                if let Some(status) = ap.status.clone() {
                    let s_lc = status.to_lowercase();
                    let is_fail =
                        s_lc.contains("error") || s_lc.contains("invalid") || s_lc.contains("failed");
                    let is_done = is_fail
                        || s_lc.contains("ok")
                        || s_lc.contains("applied")
                        || s_lc.contains("success");

                    if is_done {
                        st.awaiting_apply_result = false;

                        // Stats: record apply result.
                        if is_fail {
                            st.changesets_err = st.changesets_err.saturating_add(1);
                        } else {
                            st.changesets_ok = st.changesets_ok.saturating_add(1);
                        }
                        did_mutate = true;

                        if is_fail {
                            let prompt = format!(
                                "The previous ChangeSet was applied and FAILED. Please return a NEW ChangeSet JSON (version 1) that fixes the issue.\n\nAPPLY ERROR:\n{}",
                                status
                            );

                            if st.changeset_auto {
                                // Auto: immediately ask the model for a corrected ChangeSet.
                                st.draft = prompt;
                                st.include_context_next = true;
                                st.last_status = Some(
                                    "Apply failed: requesting follow-up ChangeSet…".to_string(),
                                );
                                actions.push(Action::ExecuteLoopSend { loop_id });
                            } else {
                                // Manual: pause and let user review / edit the prompt.
                                st.awaiting_review = true;
                                st.draft = prompt;
                                st.last_status = Some(
                                    "Apply failed (manual): review error then Send for follow-up.".to_string(),
                                );
                            }
                        } else {
                            // Success: optionally run postprocess automatically in auto mode.
                            st.last_status = Some("Apply succeeded.".to_string());
                            if st.changeset_auto {
                                // Kick off postprocess automatically.
                                actions.push(Action::ExecuteLoopRunPostprocess { loop_id });
                            }
                        }
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------
    // Poll postprocess command (non-blocking)
    // ------------------------------------------------------------
    if st.postprocess_pending {
        if let Some(rx) = &st.postprocess_rx {
            match rx.try_recv() {
                Ok(Ok(output)) => {
                    st.postprocess_pending = false;
                    st.postprocess_rx = None;

                    st.messages.push(ExecuteLoopMessage {
                        role: "system".to_string(),
                        content: format!("POSTPROCESS OK ({})\n{}", st.postprocess_cmd, output),
                    });
                    did_mutate = true;

                    st.last_status = Some("Postprocess OK.".to_string());

                    // Stats: record postprocess success.
                    st.postprocess_ok = st.postprocess_ok.saturating_add(1);
                }
                Ok(Err(output)) => {
                    st.postprocess_pending = false;
                    st.postprocess_rx = None;

                    let msg = format!("POSTPROCESS FAILED ({})\n{}", st.postprocess_cmd, output);
                    st.messages.push(ExecuteLoopMessage {
                        role: "system".to_string(),
                        content: msg.clone(),
                    });
                    did_mutate = true;

                    // If we're in ChangeSet mode, feed the failure output back to the model.

                    // Stats: record postprocess failure.
                    st.postprocess_err = st.postprocess_err.saturating_add(1);
                    if st.mode == ExecuteLoopMode::ChangeSet {
                        if st.changeset_auto {
                            st.draft = format!(
                                "Postprocess command failed after applying the previous ChangeSet.\n\nPOSTPROCESS OUTPUT:\n{}\n\nPlease provide a NEW ChangeSet JSON (version 1) that fixes the errors.",
                                msg
                            );
                            st.include_context_next = true;
                            st.last_status = Some(
                                "Postprocess failed: requesting follow-up ChangeSet…".to_string(),
                            );
                            actions.push(Action::ExecuteLoopSend { loop_id });
                        } else {
                            st.awaiting_review = true;
                            st.last_status = Some(
                                "Postprocess failed (manual): review output then Send for follow-up.".to_string(),
                            );
                        }
                    } else {
                        st.last_status = Some("Postprocess failed.".to_string());
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // still running
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    st.postprocess_pending = false;
                    st.postprocess_rx = None;
                    st.last_status = Some("Postprocess channel disconnected.".to_string());
                    did_mutate = true;
                }
            }
        } else {
            st.postprocess_pending = false;
        }

        ctx.request_repaint();
    }

    // One-shot best-effort model list fetch so the dropdown populates.
    {
        let once_id = egui::Id::new(("execute_loop_models_fetched", loop_id));
        let already = ctx.data(|d| d.get_temp::<bool>(once_id)).unwrap_or(false);
        if !already {
            ctx.data_mut(|d| d.insert_temp(once_id, true));
            if st.model_options.is_empty() {
                match state.openai.list_models() {
                    Ok(mut ms) => {
                        ms.sort();
                        ms.dedup();
                        st.model_options = ms;
                        if !st.model_options.is_empty()
                            && !st.model_options.iter().any(|m| m == &st.model)
                        {
                            st.model = st.model_options[0].clone();
                        }
                    }
                    Err(e) => {
                        st.last_status = Some(format!("Model list fetch failed: {:#}", e));
                    }
                }
            }
        }
    }

    // Header row
    // Keep all child widgets constrained to the panel width.
    // Some widgets (especially singleline text edits inside horizontals) can request very large widths
    // and cause the component/window to expand horizontally.
    let panel_w = ui.available_width();

    ui.horizontal(|ui| {
        ui.heading(format!("Execute Loop {}", loop_id));

        // Pause is controlled via bound Tasks (TaskStore source of truth).
        if !bound_tasks.is_empty() {
            ui.separator();
            ui.label("Task(s)");
            ui.monospace(
                bound_tasks
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            );

            let paused_now = bound_paused.unwrap_or(false);
            let label = if paused_now { "Resume" } else { "Pause" };
            if ui.button(label).clicked() {
                for tid in bound_tasks.iter().copied() {
                    actions.push(Action::TaskSetPaused {
                        task_id: tid,
                        paused: !paused_now,
                    });
                }
            }
        }

        if ui.button("Clear chat").clicked() {
            actions.push(Action::ExecuteLoopClearChat { loop_id });
        }

        if ui
            .button("Inject context")
            .on_hover_text("Generate + inject repo context as a system message now")
            .clicked()
        {
            actions.push(Action::ExecuteLoopInjectContext { loop_id });
        }

        if st.pending {
            ui.separator();
            ui.small("Waiting…");
            ui.add(egui::Spinner::new());
        }

        if st.postprocess_pending {
            ui.separator();
            ui.small("Postprocess…");
            ui.add(egui::Spinner::new());
        }

        if st.awaiting_review {
            ui.separator();
            ui.strong("Awaiting review");
            if ui.button("Mark reviewed").clicked() {
                actions.push(Action::ExecuteLoopMarkReviewed { loop_id });
            }
        }

        if let Some(s) = &st.last_status {
            ui.separator();
            ui.small(s);
        }
    });

    ui.add_space(6.0);

    // Model dropdown + refresh
    ui.horizontal(|ui| {
        ui.label("Model");

        if !st.model_options.is_empty() {
            egui::ComboBox::from_id_source(("execute_loop_model_combo", loop_id))
                .selected_text(st.model.clone())
                .width(260.0)
                .show_ui(ui, |ui| {
                    for m in st.model_options.iter() {
                        ui.selectable_value(&mut st.model, m.clone(), m);
                    }
                });
        } else {
            ui.add(
                egui::TextEdit::singleline(&mut st.model)
                    .hint_text("model id (click ↻ to fetch)")
                    .desired_width(260.0),
            );
        }

        if ui
            .button("↻")
            .on_hover_text("Fetch/refresh model list from API")
            .clicked()
        {
            match state.openai.list_models() {
                Ok(mut ms) => {
                    ms.sort();
                    ms.dedup();
                    st.model_options = ms;
                    if !st.model_options.is_empty() && !st.model_options.iter().any(|m| m == &st.model)
                    {
                        st.model = st.model_options[0].clone();
                    }
                    st.last_status = Some(format!("Fetched {} model(s)", st.model_options.len()));
                }
                Err(e) => {
                    st.last_status = Some(format!("Model list fetch failed: {:#}", e));
                }
            }

            let once_id = egui::Id::new(("execute_loop_models_fetched", loop_id));
            ctx.data_mut(|d| d.insert_temp(once_id, true));
        }
    });

    ui.add_space(6.0);

    // Mode + toggles
    ui.horizontal(|ui| {
        ui.label("Mode");

        let mut mode = st.mode;
        ui.radio_value(&mut mode, ExecuteLoopMode::Conversation, "Conversation");
        ui.radio_value(&mut mode, ExecuteLoopMode::ChangeSet, "ChangeSet");
        if mode != st.mode {
            actions.push(Action::ExecuteLoopSetMode { loop_id, mode });
        }

        ui.separator();
        ui.checkbox(&mut st.include_context_next, "Include context on next send");

        if st.mode == ExecuteLoopMode::ChangeSet {
            ui.separator();
            ui.checkbox(&mut st.changeset_auto, "Auto");
            ui.small(if st.changeset_auto { "(won't pause)" } else { "(pause each step)" });
        }

        ui.separator();
        ui.checkbox(&mut st.auto_fill_first_changeset_applier, "Auto-fill + auto-apply ChangeSet");
    });

    ui.add_space(8.0);

    // System instruction
    ui.label("System instruction");
    ui.add(
        egui::TextEdit::multiline(&mut st.instruction)
            .desired_rows(3)
            .desired_width(panel_w),
    );

    ui.add_space(8.0);

    // Postprocess (ChangeSet mode)
    if st.mode == ExecuteLoopMode::ChangeSet {
        ui.label("Postprocess command");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut st.postprocess_cmd)
                    .desired_width((panel_w - 80.0).max(120.0))
                    .hint_text("e.g. cargo check"),
            );

            let can_run = !st.postprocess_pending && !st.pending;
            if ui.add_enabled(can_run, egui::Button::new("Run")).clicked() {
                actions.push(Action::ExecuteLoopRunPostprocess { loop_id });
            }
        });
        ui.add(
            egui::Label::new(
                "Run this after the ChangeSet is applied. If it fails, the output will be sent back to the model for a follow-up ChangeSet (auto mode), or paused for review (manual).",
            )
            .wrap(true),
        );
        ui.add_space(8.0);
    }

    // IMPORTANT: cap transcript height so the component doesn't expand.
    let reserved_bottom = if st.mode == ExecuteLoopMode::ChangeSet { 260.0 } else { 200.0 };
    let chat_max_h = (ui.available_height() - reserved_bottom).max(120.0);

    // Conversation transcript
    ui.label("Conversation");
    egui::ScrollArea::both()
        .id_source(("execute_loop_chat_scroll", loop_id))
        .auto_shrink([false, false])
        .max_height(chat_max_h)
        .show(ui, |ui| {
            if st.messages.is_empty() {
                ui.label("(no messages yet)");
                return;
            }

            for (i, m) in st.messages.iter().enumerate() {
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::same(6.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!("#{}", i + 1));
                            ui.separator();
                            ui.monospace(&m.role);
                        });
                        ui.add_space(4.0);
                        // Use a selectable label so users can highlight/copy transcript text.
                        // (Non-interactive TextEdit prevents selection.)
                        ui.add(
                            egui::Label::new(egui::RichText::new(m.content.clone()).monospace())
                                .selectable(true)
                                .wrap(false),
                        );
                    });
                ui.add_space(6.0);
            }
        });

    ui.add_space(10.0);

    // Draft input
    ui.label("Your message");
    ui.add(
        egui::TextEdit::multiline(&mut st.draft)
            .desired_rows(4)
            .desired_width(panel_w)
            .hint_text("Type a message…"),
    );

    ui.horizontal(|ui| {
        let can_send = !st.awaiting_review && !st.pending && !st.postprocess_pending;
        if ui.add_enabled(can_send, egui::Button::new("Send")).clicked() {
            actions.push(Action::ExecuteLoopSend { loop_id });
        }

        if st.pending {
            ui.separator();
            ui.small("Waiting for response…");
        } else if st.postprocess_pending {
            ui.separator();
            ui.small("Running postprocess…");
        } else if st.awaiting_review {
            ui.separator();
            ui.small("Paused for review — click 'Mark reviewed' to continue.");
        }

        if st.mode == ExecuteLoopMode::ChangeSet {
            ui.separator();
            ui.small(if st.changeset_auto { "Auto" } else { "Manual" });
        }
    });

    // Persist ExecuteLoop UI edits + transcript mutations to RepoTaskStore.
    // NOTE: pause/resume itself is persisted via Task snapshots.
    let ui_changed = st.model != before_model
        || st.instruction != before_instruction
        || st.mode != before_mode
        || st.include_context_next != before_include_ctx
        || st.auto_fill_first_changeset_applier != before_auto_fill
        || st.changeset_auto != before_changeset_auto
        || st.postprocess_cmd != before_postprocess_cmd;

    if did_mutate || ui_changed {
        // Write-through to snapshot store (source of truth), then flush to disk.
        state.persist_execute_loop_snapshot(loop_id);
        state.task_store_dirty = true;
        state.save_repo_task_store();
    }

    actions
}
