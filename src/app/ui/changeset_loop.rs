use eframe::egui;

use crate::app::actions::{Action, ComponentId};
use crate::app::browser_bridge::{browser_model_options, runtime_timeout_remaining_secs, set_runtime_timeout_secs, timeout_runtime_now};
use crate::app::state::{
    AppState,
    BrowserBridgeStatus,
    ExecuteLoopMessage,
    ExecuteLoopMode,
    ExecuteLoopStageAutomation,
    ExecuteLoopTransport,
    ExecuteLoopWorkflowStage,
};

pub fn changeset_loop_panel(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    loop_id: ComponentId,
) -> Vec<Action> {
    let mut actions: Vec<Action> = Vec::new();

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

    if let Some(p) = bound_paused {
        st.paused = p;
    }

    let mut did_mutate = false;
    st.ensure_default_changeset_workflow();


    let before_model = st.model.clone();
    let before_instruction = st.instruction.clone();
    let before_manual_fragments = st.manual_fragments.clone();
    let before_automatic_fragments = st.automatic_fragments.clone();
    let before_fragment_overrides = st.fragment_overrides.clone();
    let before_include_ctx = st.include_context_next;
    let before_auto_fill = st.auto_fill_first_changeset_applier;
    let before_browser_target_url = st.browser_target_url.clone();


    if !st.auto_fill_first_changeset_applier {
        st.auto_fill_first_changeset_applier = true;
        did_mutate = true;
    }
    let before_postprocess_cmd = st.postprocess_cmd.clone();
    let before_workflow_stages = st.workflow_stages.clone();
    let before_workflow_active_stage = st.workflow_active_stage;
    let before_browser_response_timeout_ms = st.browser_response_timeout_ms;
    let before_browser_response_poll_ms = st.browser_response_poll_ms;

    if st.transport == ExecuteLoopTransport::Api && !st.history_sync_pending {
        if let Some(cid) = st.conversation_id.clone() {
            if cid.starts_with("conv") {
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
            } else {
                st.history_synced_conversation_id = Some(cid);
                st.history_sync_pending = false;
                st.history_sync_rx = None;
            }
        }
    } else if st.transport == ExecuteLoopTransport::BrowserBridge {
        st.history_sync_pending = false;
        st.history_sync_rx = None;
    }

    if st.history_sync_pending {
        if let Some(rx) = &st.history_sync_rx {
            match rx.try_recv() {
                Ok(Ok(mut msgs)) => {

                    if let Some(sys_idx) = msgs.iter().position(|m| m.role == "system") {
                        let sys = msgs.remove(sys_idx);
                        msgs.insert(0, sys);
                    }

                    if msgs.len() >= 3 {
                        let tail = &msgs[1..];
                        let mut first_non = None;
                        let mut second_non = None;
                        for m in tail {
                            if m.role == "system" {
                                continue;
                            }
                            if first_non.is_none() {
                                first_non = Some(m.role.as_str());
                            } else {
                                second_non = Some(m.role.as_str());
                                break;
                            }
                        }

                        if first_non == Some("assistant") && second_non == Some("user") {
                            let head = msgs.remove(0);
                            msgs.reverse();
                            msgs.insert(0, head);
                        }
                    }

                    st.messages = msgs;
                    st.history_synced_conversation_id = st.conversation_id.clone();
                    st.history_sync_pending = false;
                    st.history_sync_rx = None;
                }
                Ok(Err(err)) => {
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

    if st.pending {
        if let Some(rx) = &st.pending_rx {
            match rx.try_recv() {
                Ok(Ok(out)) => {
                    if let Some(conversation_id) = out.conversation_id.clone() {
                        st.conversation_id = Some(conversation_id);
                    }
                    if let Some(browser_session_id) = out.browser_session_id.clone() {
                        st.browser_session_id = Some(browser_session_id);
                        st.browser_status = BrowserBridgeStatus::Ready;
                        st.browser_attached = true;
                    }

                    if let Some(sys_idx) = st.messages.iter().position(|m| m.role == "system") {
                        if sys_idx != 0 {
                            let sys = st.messages.remove(sys_idx);
                            st.messages.insert(0, sys);
                        }
                    }

                    st.messages.push(ExecuteLoopMessage {
                        role: "assistant".to_string(),
                        content: out.text.clone(),
                    });
                    did_mutate = true;

                    st.pending = false;
                    st.pending_rx = None;
                    st.active_browser_runtime_key = None;

                    if st.effective_mode() == ExecuteLoopMode::ChangeSet {
                        if let Some((applier_id, ap)) = state.changeset_appliers.iter_mut().next() {
                            match crate::app::controllers::changeset_controller::normalize_and_validate_changeset_payload_text(&out.text) {
                                Ok(normalized) => {
                                    ap.payload = normalized;
                                    ap.status = Some(format!("Validated ChangeSet from Execute Loop {}", loop_id));

                                    st.last_auto_applier_id = Some(*applier_id);
                                    st.last_auto_applier_status = ap.status.clone();
                                    st.changesets_total = st.changesets_total.saturating_add(1);

                                    st.workflow_set_active_stage(ExecuteLoopWorkflowStage::Code);
                                    st.awaiting_apply_result = true;
                                    actions.push(Action::ApplyChangeSet { applier_id: *applier_id });
                                    st.last_status = Some("Valid ChangeSet received in Code stage: applying…".to_string());
                                    st.awaiting_review = false;
                                    st.automatic_fragments.changeset_validation_error = None;
                                }
                                Err(err) => {
                                    ap.payload = out.text.clone();
                                    ap.status = Some(format!("Rejected auto-apply: {}", err));

                                    st.last_auto_applier_id = Some(*applier_id);
                                    st.last_auto_applier_status = ap.status.clone();
                                    st.awaiting_apply_result = false;
                                    st.awaiting_review = true;
                                    st.last_status = Some(format!(
                                        "Assistant response was not a valid final ChangeSet. Not applied: {}",
                                        err
                                    ));

                                    let validation_prompt = format!(
                                        "Your previous response was not a valid ChangeSet JSON payload. Return only a valid ChangeSet JSON object, version 1, with at least one operation.\n\nVALIDATION ERROR:\n{}",
                                        err
                                    );
                                    if st.workflow_stage_is_auto(ExecuteLoopWorkflowStage::Code) {
                                        st.automatic_fragments.changeset_validation_error = Some(validation_prompt);
                                    } else {
                                        st.draft = validation_prompt;
                                    }
                                }
                            }
                        } else {
                            st.last_status = Some(
                                "ChangeSet received, but no ChangeSet Applier exists.".to_string(),
                            );
                        }
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
                    st.active_browser_runtime_key = None;
                    st.last_status = Some("Request failed.".to_string());
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    st.pending = false;
                    st.pending_rx = None;
                    st.active_browser_runtime_key = None;
                    st.last_status = Some("Request channel disconnected.".to_string());
                    did_mutate = true;
                }
            }
        } else {
            st.pending = false;
        }

        ctx.request_repaint();
    }

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

    if st.awaiting_apply_result {
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

                            st.workflow_set_active_stage(ExecuteLoopWorkflowStage::Code);
                            if st.workflow_stage_is_auto(ExecuteLoopWorkflowStage::Code) {
                                st.automatic_fragments.apply_error = Some(prompt.clone());
                                st.include_context_next = st.manual_fragments.include_repo_context;
                                st.last_status = Some(
                                    "Apply failed: requesting follow-up ChangeSet…".to_string(),
                                );
                                actions.push(Action::ExecuteLoopSend { loop_id });
                            } else {
                                st.awaiting_review = true;
                                st.automatic_fragments.apply_error = Some(prompt.clone());
                                st.draft = prompt.clone();
                                st.last_status = Some(
                                    "Apply failed (manual): review error then Run stage for follow-up.".to_string(),
                                );
                            }
                        } else {
                            st.last_status = Some("Apply succeeded.".to_string());
                            st.workflow_set_active_stage(ExecuteLoopWorkflowStage::Code);
                            let next = st.workflow_next_stage(ExecuteLoopWorkflowStage::Code);
                            if let Some(ExecuteLoopWorkflowStage::Compile) = next {
                                st.workflow_set_active_stage(ExecuteLoopWorkflowStage::Compile);
                                if st.workflow_stage_is_auto(ExecuteLoopWorkflowStage::Compile) {
                                    st.awaiting_review = false;
                                    actions.push(Action::ExecuteLoopRunPostprocess { loop_id });
                                } else {
                                    st.awaiting_review = true;
                                    st.last_status = Some("Apply succeeded. Compile stage is ready for manual run.".to_string());
                                }
                            } else {
                                st.workflow_set_active_stage(ExecuteLoopWorkflowStage::Finished);
                                st.awaiting_review = true;
                                st.last_status = Some("Workflow finished. Ready for review.".to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    if st.postprocess_pending {
        if let Some(rx) = &st.postprocess_rx {
            match rx.try_recv() {
                Ok(Ok(output)) => {
                    st.postprocess_pending = false;
                    st.postprocess_rx = None;

                    st.messages.push(ExecuteLoopMessage {
                        role: "system".to_string(),
                        content: format!("COMPILE OK ({})\n{}", st.postprocess_cmd, output),
                    });
                    did_mutate = true;

                    st.last_status = Some("Compile succeeded. Ready for review.".to_string());

                    st.postprocess_ok = st.postprocess_ok.saturating_add(1);
                    st.workflow_set_active_stage(ExecuteLoopWorkflowStage::Finished);
                    st.awaiting_review = true;
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


                    st.postprocess_err = st.postprocess_err.saturating_add(1);
                    let compile_fragment = format!(
                        "Postprocess command failed after applying the previous ChangeSet.\n\nPOSTPROCESS OUTPUT:\n{}\n\nPlease provide a NEW ChangeSet JSON (version 1) that fixes the errors.",
                        msg
                    );
                    st.workflow_set_active_stage(ExecuteLoopWorkflowStage::Code);
                    if st.workflow_stage_is_auto(ExecuteLoopWorkflowStage::Code) {
                        st.automatic_fragments.compile_error = Some(compile_fragment);
                        st.include_context_next = st.manual_fragments.include_repo_context;
                        st.last_status = Some(
                            "Compile failed: requesting follow-up ChangeSet…".to_string(),
                        );
                        actions.push(Action::ExecuteLoopSend { loop_id });
                    } else {
                        st.awaiting_review = true;
                        st.automatic_fragments.compile_error = Some(compile_fragment.clone());
                        st.draft = compile_fragment;
                        st.last_status = Some(
                            "Compile failed (manual): review output then Run stage for follow-up.".to_string(),
                        );
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
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

    {
        let once_id = egui::Id::new(("execute_loop_models_fetched", loop_id));
        let already = ctx.data(|d| d.get_temp::<bool>(once_id)).unwrap_or(false);
        if !already {
            ctx.data_mut(|d| d.insert_temp(once_id, true));
            if st.model_options.is_empty() {
                match st.transport {
                    ExecuteLoopTransport::Api => {
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
                    ExecuteLoopTransport::BrowserBridge => {
                        st.model_options = browser_model_options();
                        if !st.model_options.is_empty() && !st.model_options.iter().any(|m| m == &st.model) {
                            st.model = st.model_options[0].clone();
                        }
                    }
                }
            }
        }
    }

    let panel_w = ui.available_width();
    let browser_status = match st.transport {
        ExecuteLoopTransport::Api => BrowserBridgeStatus::Ready,
        ExecuteLoopTransport::BrowserBridge => st.browser_status,
    };

    if st.transport == ExecuteLoopTransport::BrowserBridge && st.browser_session_id.is_some() && !st.pending && !st.postprocess_pending {
        let probe_key = egui::Id::new(("execute_loop_browser_auto_probe_at", loop_id));
        let now = ctx.input(|i| i.time);
        let last_probe_at = ctx.data(|d| d.get_temp::<f64>(probe_key)).unwrap_or(0.0);
        if now - last_probe_at >= 1.5 {
            ctx.data_mut(|d| d.insert_temp(probe_key, now));
            actions.push(Action::ExecuteLoopBrowserProbe { loop_id });
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
    let transport_summary = match st.transport {
        ExecuteLoopTransport::Api => format!("API · {}", st.model),
        ExecuteLoopTransport::BrowserBridge => {
            let label = match browser_status {
                BrowserBridgeStatus::Detached => "Browser bridge · detached",
                BrowserBridgeStatus::Attached => "Browser bridge · attached",
                BrowserBridgeStatus::Ready => "Browser bridge · ready",
            };
            label.to_string()
        }
    };

    ui.horizontal(|ui| {
        ui.heading(format!("Execute Loop {}", loop_id));

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
            ui.strong("Awaiting manual action");
        }

        if let Some(s) = &st.last_status {
            ui.separator();
            ui.small(s);
        }
    });

    ui.add_space(6.0);

    ui.horizontal_wrapped(|ui| {
        if st.transport == ExecuteLoopTransport::BrowserBridge {
            let color = match browser_status {
                BrowserBridgeStatus::Detached => egui::Color32::from_rgb(220, 70, 70),
                BrowserBridgeStatus::Attached => egui::Color32::from_rgb(220, 190, 70),
                BrowserBridgeStatus::Ready => egui::Color32::from_rgb(80, 200, 120),
            };
            let desired = egui::vec2(12.0, 12.0);
            let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 5.0, color);
        }
        ui.small(transport_summary);
        if st.transport == ExecuteLoopTransport::BrowserBridge {
            if ui.button("Launch Browser").clicked() {
                actions.push(Action::ExecuteLoopBrowserLaunchAndAttach { loop_id });
            }
            let can_open_tab = st.browser_session_id.is_some() && !st.browser_target_url.trim().is_empty();
            if ui.add_enabled(can_open_tab, egui::Button::new("Open Tab")).clicked() {
                actions.push(Action::ExecuteLoopBrowserOpenUrl { loop_id });
            }
            let can_detach_tab = st.browser_session_id.is_some();
            if ui.add_enabled(can_detach_tab, egui::Button::new("Detach Tab")).clicked() {
                actions.push(Action::ExecuteLoopBrowserDetach { loop_id });
            }
        }
    });

    ui.add_space(6.0);

    if st.transport == ExecuteLoopTransport::BrowserBridge {
        ui.horizontal_wrapped(|ui| {
            ui.label("URL");
            ui.add(
                egui::TextEdit::singleline(&mut st.browser_target_url)
                    .desired_width((panel_w - 180.0).max(220.0))
                    .hint_text("https://...")
            );
        });

        ui.add_space(6.0);
    }
    if st.transport == ExecuteLoopTransport::BrowserBridge {
        let runtime_key = st.active_browser_runtime_key.clone();
        let poll_secs_f = (st.browser_response_poll_ms.max(250) as f64) / 1000.0;
        let mut poll_secs_ui = poll_secs_f;
        let applied_timeout_secs = (st.browser_response_timeout_ms.max(1000) + 999) / 1000;
        let draft_timeout_secs = st.browser_response_timeout_input.trim().parse::<u64>().ok();
        let input_valid = draft_timeout_secs
            .map(|secs| (1..=9999).contains(&secs))
            .unwrap_or(false);
        let remaining_secs = runtime_key
            .as_deref()
            .map(|key| runtime_timeout_remaining_secs(key, applied_timeout_secs))
            .unwrap_or(applied_timeout_secs);
        let elapsed_secs = applied_timeout_secs.saturating_sub(remaining_secs);
        let requires_timeout_confirm = st.pending
            && draft_timeout_secs
                .map(|secs| secs <= elapsed_secs)
                .unwrap_or(false);
        let _can_apply_timeout = input_valid && (!requires_timeout_confirm || st.browser_timeout_confirm_pending);

        ui.horizontal_wrapped(|ui| {
            ui.label("Response timeout (s)");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut st.browser_response_timeout_input)
                    .desired_width(80.0)
                    .hint_text("seconds")
            );
            if resp.changed() {
                st.browser_timeout_confirm_pending = false;
            }
            if ui
                .add_enabled(input_valid, egui::Button::new(if requires_timeout_confirm && !st.browser_timeout_confirm_pending {
                    "Update timeout"
                } else if requires_timeout_confirm {
                    "Confirm timeout update"
                } else {
                    "Update timeout"
                }))
                .clicked()
            {
                if requires_timeout_confirm && !st.browser_timeout_confirm_pending {
                    st.browser_timeout_confirm_pending = true;
                } else if let Some(next_secs) = draft_timeout_secs {
                    let next_secs = next_secs.clamp(1, 9999);
                    let next_ms = next_secs * 1000;
                    st.browser_response_timeout_ms = next_ms;
                    st.browser_response_timeout_input = next_secs.to_string();
                    st.browser_timeout_confirm_pending = false;
                    if let Some(runtime_key) = runtime_key.as_deref() {
                        if next_secs <= elapsed_secs {
                            timeout_runtime_now(runtime_key);
                        } else {
                            set_runtime_timeout_secs(runtime_key, next_secs);
                        }
                    }
                }
            }
            if requires_timeout_confirm && st.browser_timeout_confirm_pending {
                if ui.button("Cancel").clicked() {
                    st.browser_timeout_confirm_pending = false;
                }
            }
            ui.separator();
            ui.small(format!("Applied: {}s", applied_timeout_secs));
            ui.separator();
            ui.label(format!("Time left: {}s", remaining_secs));
            ui.separator();
            ui.label("Poll (s)");
            let poll_resp = ui.add(
                egui::DragValue::new(&mut poll_secs_ui)
                    .speed(0.25)
                    .clamp_range(0.25..=30.0),
            );
            if poll_resp.changed() {
                st.browser_response_poll_ms = (poll_secs_ui.max(0.25) * 1000.0).round() as u64;
            }
        });

        let msg = if st.browser_response_timeout_input.trim().is_empty() {
            "Invalid timeout value".to_string()
        } else {
            match draft_timeout_secs {
                Some(secs) if !(1..=9999).contains(&secs) => "Invalid timeout value".to_string(),
                None => "Invalid timeout value".to_string(),
                Some(_secs) if requires_timeout_confirm && !st.browser_timeout_confirm_pending => {
                    "Are you sure? This will timeout immediately.".to_string()
                }
                Some(secs) if requires_timeout_confirm && st.browser_timeout_confirm_pending => {
                    format!("Confirm timeout update to {}s or cancel", secs)
                }
                Some(_) => String::new(),
            }
        };
        if !msg.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.small(msg);
            });
        }

        ui.add_space(6.0);
    }

    ui.horizontal_wrapped(|ui| {
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
                    .hint_text("model id")
                    .desired_width(260.0),
            );
        }

        if ui
            .button("↻")
            .on_hover_text(match st.transport {
                ExecuteLoopTransport::Api => "Fetch/refresh model list from API",
                ExecuteLoopTransport::BrowserBridge => "Refresh browser model alias",
            })
            .clicked()
        {
            match st.transport {
                ExecuteLoopTransport::Api => {
                    match state.openai.list_models() {
                        Ok(mut ms) => {
                            ms.sort();
                            ms.dedup();
                            st.model_options = ms;
                            if !st.model_options.is_empty() && !st.model_options.iter().any(|m| m == &st.model) {
                                st.model = st.model_options[0].clone();
                            }
                            st.last_status = Some(format!("Fetched {} model(s)", st.model_options.len()));
                        }
                        Err(e) => {
                            st.last_status = Some(format!("Model list fetch failed: {:#}", e));
                        }
                    }
                }
                ExecuteLoopTransport::BrowserBridge => {
                    st.model_options = browser_model_options();
                    if !st.model_options.is_empty() && !st.model_options.iter().any(|m| m == &st.model) {
                        st.model = st.model_options[0].clone();
                    }
                    st.last_status = Some("Browser model alias refreshed.".to_string());
                }
            }

            let once_id = egui::Id::new(("execute_loop_models_fetched", loop_id));
            ctx.data_mut(|d| d.insert_temp(once_id, true));
        }
    });

    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label(format!(
            "Workflow mode: {:?} (stage: {:?})",
            st.effective_mode(),
            st.workflow_active_stage
        ));
    });

    ui.label("Message fragments");
    ui.horizontal_wrapped(|ui| {
        if ui.checkbox(&mut st.manual_fragments.include_system_instruction, "System instructions").changed() {
            did_mutate = true;
        }
        if ui.checkbox(&mut st.manual_fragments.include_repo_context, "Repo context").changed() {
            st.include_context_next = st.manual_fragments.include_repo_context;
            did_mutate = true;
        }
        if ui.checkbox(&mut st.manual_fragments.include_changeset_schema, "ChangeSet schema").changed() {
            did_mutate = true;
        }
    });

    if st.manual_fragments.include_system_instruction {
        ui.horizontal_wrapped(|ui| {
            ui.label("System instructions source");
            let mut use_override = st.fragment_overrides.system_instruction.is_some();
            egui::ComboBox::from_id_source(("system_instruction_override", loop_id))
                .selected_text(if use_override { "Override" } else { "Default" })
                .show_ui(ui, |ui| {
                    if ui.selectable_label(!use_override, "Default").clicked() {
                        st.fragment_overrides.system_instruction = None;
                        use_override = false;
                    }
                    if ui.selectable_label(use_override, "Override").clicked() {
                        let default_text = st.effective_system_instruction_fragment();
                        st.fragment_overrides.system_instruction = Some(default_text);
                        use_override = true;
                    }
                });
            if ui.button("Reset to default").clicked() {
                st.fragment_overrides.system_instruction = None;
            }
        });
        if let Some(text) = st.fragment_overrides.system_instruction.as_mut() {
            ui.add(
                egui::TextEdit::multiline(text)
                    .desired_rows(4)
                    .desired_width(panel_w),
            );
        }
    }

    if st.manual_fragments.include_changeset_schema {
        ui.horizontal_wrapped(|ui| {
            ui.label("ChangeSet schema source");
            let mut use_override = st.fragment_overrides.changeset_schema.is_some();
            egui::ComboBox::from_id_source(("changeset_schema_override", loop_id))
                .selected_text(if use_override { "Override" } else { "Default" })
                .show_ui(ui, |ui| {
                    if ui.selectable_label(!use_override, "Default").clicked() {
                        st.fragment_overrides.changeset_schema = None;
                        use_override = false;
                    }
                    if ui.selectable_label(use_override, "Override").clicked() {
                        let default_text = st.effective_changeset_schema_fragment();
                        st.fragment_overrides.changeset_schema = Some(default_text);
                        use_override = true;
                    }
                });
            if ui.button("Reset to default").clicked() {
                st.fragment_overrides.changeset_schema = None;
            }
        });
        if let Some(text) = st.fragment_overrides.changeset_schema.as_mut() {
            ui.add(
                egui::TextEdit::multiline(text)
                    .desired_rows(6)
                    .desired_width(panel_w),
            );
        }
    }

    let mut auto_labels: Vec<String> = Vec::new();
    if st.automatic_fragments.apply_error.is_some() {
        auto_labels.push("Apply error".to_string());
    }
    if st.automatic_fragments.changeset_validation_error.is_some() {
        auto_labels.push("ChangeSet validation error".to_string());
    }
    if st.automatic_fragments.compile_error.is_some() {
        auto_labels.push("Compile error".to_string());
    }
    if !auto_labels.is_empty() {
        ui.small(format!("Automatic for next turn: {}", auto_labels.join(", ")));
    }
    ui.add_space(8.0);

    ui.add_space(8.0);

    ui.separator();
    ui.label("Workflow");
    ui.small(format!("Current stage: {:?} (mode: {:?})", st.workflow_active_stage, st.effective_mode()));
    ui.add_space(4.0);

    for idx in 0..st.workflow_stages.len() {
        let stage = st.workflow_stages[idx].stage;
        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                let is_finished = stage == ExecuteLoopWorkflowStage::Finished;
                let enabled_ref = &mut st.workflow_stages[idx].enabled;
                if is_finished {
                    *enabled_ref = true;
                    ui.add_enabled(false, egui::Checkbox::new(enabled_ref, format!("{:?}", stage)));
                } else {
                    ui.checkbox(enabled_ref, format!("{:?}", stage));
                }

                let auto_supported = matches!(stage, ExecuteLoopWorkflowStage::Code | ExecuteLoopWorkflowStage::Compile);
                egui::ComboBox::from_id_source(("workflow_stage_auto", loop_id, idx))
                    .selected_text(match st.workflow_stages[idx].automation {
                        ExecuteLoopStageAutomation::Manual => "Manual",
                        ExecuteLoopStageAutomation::Auto => "Auto",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut st.workflow_stages[idx].automation,
                            ExecuteLoopStageAutomation::Manual,
                            "Manual",
                        );
                        if auto_supported {
                            ui.selectable_value(
                                &mut st.workflow_stages[idx].automation,
                                ExecuteLoopStageAutomation::Auto,
                                "Auto",
                            );
                        }
                    });

                if ui.small_button("Go").clicked() {
                    actions.push(Action::ExecuteLoopWorkflowJumpToStage { loop_id, stage });
                }
            });

            if stage == ExecuteLoopWorkflowStage::Compile {
                let mut joined = st.workflow_stages[idx].commands.join("\n");
                ui.label("Compile commands (one per line)");
                if ui
                    .add(
                        egui::TextEdit::multiline(&mut joined)
                            .desired_rows(3)
                            .desired_width(panel_w)
                            .hint_text("cd bridge && npm run build\ncargo run"),
                    )
                    .changed()
                {
                    st.workflow_stages[idx].commands = joined
                        .lines()
                        .map(|line| line.trim())
                        .filter(|line| !line.is_empty())
                        .map(|line| line.to_string())
                        .collect();
                }
            }
        });
        ui.add_space(4.0);
    }

    st.ensure_default_changeset_workflow();
    ui.add_space(8.0);

    let reserved_bottom = 360.0;
    let chat_max_h = (ui.available_height() - reserved_bottom).max(120.0);

    ui.label("Conversation");

    let mut force_open_all: Option<bool> = None;
    ui.horizontal(|ui| {
        if ui.small_button("Expand all").clicked() {
            force_open_all = Some(true);
        }
        if ui.small_button("Collapse all").clicked() {
            force_open_all = Some(false);
        }
    });

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
                let header = format!("#{}  {}", i + 1, m.role);

                let id = ui.make_persistent_id(("execute_loop_msg", loop_id, i));
                let default_open = m.role != "system";
                let mut cs = egui::collapsing_header::CollapsingState::load_with_default_open(
                    ctx,
                    id,
                    default_open,
                );

                if let Some(force) = force_open_all {
                    cs.set_open(force);
                }

                cs.show_header(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(&header);

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if m.role == "assistant" {
                                if ui.small_button("Copy").clicked() {
                                    ctx.output_mut(|o| o.copied_text = m.content.clone());
                                }
                            }
                        });
                    });
                })
                .body(|ui| {
                    ui.add_space(4.0);
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

    let browser_blocked = st.transport == ExecuteLoopTransport::BrowserBridge && browser_status != BrowserBridgeStatus::Ready;
    if browser_blocked {
        let status_text = match browser_status {
            BrowserBridgeStatus::Detached => "Attach a browser session before sending messages or running the loop.",
            BrowserBridgeStatus::Attached => "Browser session is attached but chat is not operable yet. Probe the page or open a valid chat tab.",
            BrowserBridgeStatus::Ready => "",
        };
        if !status_text.is_empty() {
            ui.small(status_text);
            ui.add_space(4.0);
        }
    } else {
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

            ui.separator();

            let advance_label = match st.workflow_active_stage {
                ExecuteLoopWorkflowStage::Design | ExecuteLoopWorkflowStage::Code => "Run stage",
                ExecuteLoopWorkflowStage::Compile => "Run compile",
                _ => "Advance",
            };
            let can_advance = !st.pending && !st.postprocess_pending && !st.awaiting_apply_result;
            if ui
                .add_enabled(can_advance, egui::Button::new(advance_label))
                .on_hover_text("Run or advance the active workflow stage")
                .clicked()
            {
                actions.push(Action::ExecuteLoopWorkflowAdvance { loop_id });
            }

            if st.pending {
                ui.separator();
                ui.small("Waiting for response…");
            } else if st.postprocess_pending {
                ui.separator();
                ui.small("Running compile stage…");
            } else if st.awaiting_review {
                ui.separator();
                ui.small("Paused — workflow is waiting for manual review/intervention.");
            }
        });
    }

    let ui_changed = st.model != before_model
        || st.instruction != before_instruction
        || st.manual_fragments != before_manual_fragments
        || st.automatic_fragments != before_automatic_fragments
        || st.fragment_overrides != before_fragment_overrides
        || st.include_context_next != before_include_ctx
        || st.auto_fill_first_changeset_applier != before_auto_fill
        || st.browser_target_url != before_browser_target_url
        || st.postprocess_cmd != before_postprocess_cmd
        || st.workflow_stages != before_workflow_stages
        || st.workflow_active_stage != before_workflow_active_stage
        || st.browser_response_timeout_ms != before_browser_response_timeout_ms
        || st.browser_response_poll_ms != before_browser_response_poll_ms;

    if did_mutate || ui_changed {
        state.persist_execute_loop_snapshot(loop_id);
        state.task_store_dirty = true;
        state.save_repo_task_store();
    }

    actions
}
