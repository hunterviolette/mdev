// src/app/controllers/changeset_loop_controller.rs
// Execute Loop controller (conversation runner)
// - Conversation + ChangeSet modes
// - Dynamic context injection (same generator as Context Exporter, in-memory)
// - Review cycle + auto apply/postprocess handled in UI
// - Non-blocking OpenAI calls via background thread + channel

use std::path::PathBuf;
use std::sync::mpsc;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::{AppState, ExecuteLoopMessage, ExecuteLoopMode};

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        // Backwards compatible: RunOnce behaves like Send
        Action::ExecuteLoopRunOnce { loop_id } => {
            send_turn(state, *loop_id);
            true
        }

        Action::ExecuteLoopSend { loop_id } => {
            send_turn(state, *loop_id);
            true
        }

        Action::ExecuteLoopRunPostprocess { loop_id } => {
            start_postprocess(state, *loop_id);
            true
        }

        Action::ExecuteLoopSetMode { loop_id, mode } => {
            if let Some(st) = state.execute_loops.get_mut(loop_id) {
                st.mode = *mode;
                st.awaiting_review = false;
                st.last_status = Some(match mode {
                    ExecuteLoopMode::Conversation => "Mode: Conversation".to_string(),
                    ExecuteLoopMode::ChangeSet => "Mode: ChangeSet".to_string(),
                });

                // Strong default: in ChangeSet mode, include context on the next send.
                if *mode == ExecuteLoopMode::ChangeSet {
                    st.include_context_next = true;
                }
            }
            true
        }

        Action::ExecuteLoopInjectContext { loop_id } => {

            let ctx_text = match state.generate_current_context_text() {
                Ok(t) => t,
                Err(e) => {
                    if let Some(st) = state.execute_loops.get_mut(loop_id) {
                        st.last_status = Some(format!("Context generation failed: {:#}", e));
                    }
                    return true;
                }
            };

            if let Some(st) = state.execute_loops.get_mut(loop_id) {
                if st.draft.trim().is_empty() {
                    st.draft = format!("CONTEXT UPDATE:\n{}\n", ctx_text);
                } else {
                    st.draft = format!("CONTEXT UPDATE:\n{}\n\n{}", ctx_text, st.draft);
                }
                st.last_status = Some("Context prepared in draft (user message).".to_string());
            }
            true
        }

        Action::ExecuteLoopClearChat { loop_id } => {
            use std::time::{SystemTime, UNIX_EPOCH};

            let now_ms: u64 = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            // Build the canonical cleared transcript (system-only) from the loop's instruction.
            let sys_content = state
                .execute_loops
                .get(loop_id)
                .map(|st| st.instruction.clone())
                .unwrap_or_default();

            let cleared_messages = vec![ExecuteLoopMessage {
                role: "system".to_string(),
                content: sys_content,
            }];

            // 1) Clear the loop view state.
            if let Some(st) = state.execute_loops.get_mut(loop_id) {
                st.messages = cleared_messages.clone();
                st.draft.clear();
                st.awaiting_review = false;
                st.pending = false;
                st.pending_rx = None;

                st.conversation_id = None;

                st.postprocess_pending = false;
                st.postprocess_rx = None;

                st.last_auto_applier_id = None;
                st.last_auto_applier_status = None;
                st.awaiting_apply_result = false;

                st.last_status = Some("Chat cleared.".to_string());
            }

            let bound_task_id = state
                .tasks
                .iter()
                .find_map(|(tid, t)| (t.bound_execute_loop == Some(*loop_id)).then_some(*tid));

            if let Some(tid) = bound_task_id {
                let active_cid = state.tasks.get(&tid).and_then(|t| t.active_conversation);
                if let Some(cid) = active_cid {
                    if let Some(t) = state.tasks.get_mut(&tid) {
                        if let Some(snap) = t.conversations.get_mut(&cid) {
                            snap.messages = cleared_messages;
                            snap.conversation_id = None;
                            snap.updated_at_ms = now_ms;
                        }
                    }

                    state.task_store_dirty = true;
                    state.save_repo_task_store();
                }
            }

            true
        }

        Action::ExecuteLoopMarkReviewed { loop_id } => {
            if let Some(st) = state.execute_loops.get_mut(loop_id) {
                st.awaiting_review = false;
                st.last_status = Some("Reviewed. Ready.".to_string());
            }
            true
        }

        Action::ExecuteLoopClear { loop_id } => {
            if let Some(st) = state.execute_loops.get_mut(loop_id) {
                st.iterations.clear();
                st.last_status = Some("Iterations cleared.".to_string());
            }
            true
        }

        _ => false,
    }
}

fn send_turn(state: &mut AppState, loop_id: ComponentId) {
    // Don't allow sending while already waiting.
    let busy = state
        .execute_loops
        .get(&loop_id)
        .map(|st| st.pending || st.postprocess_pending)
        .unwrap_or(false);
    if busy {
        if let Some(st) = state.execute_loops.get_mut(&loop_id) {
            st.last_status = Some("Already waiting…".to_string());
        }
        return;
    }

    let (model, mode, instruction, draft, _existing_msgs, include_context_next, conversation_id) = {
        let Some(st) = state.execute_loops.get(&loop_id) else {
            return;
        };
        (
            st.model.clone(),
            st.mode,
            st.instruction.clone(),
            st.draft.clone(),
            st.messages.clone(),
            st.include_context_next,
            st.conversation_id.clone(),
        )
    };

    if draft.trim().is_empty() {
        if let Some(st) = state.execute_loops.get_mut(&loop_id) {
            st.last_status = Some("Nothing to send (draft is empty).".to_string());
        }
        return;
    }

    // Best-effort model list fetch once.
    let fetched_models = {
        let need_fetch = state
            .execute_loops
            .get(&loop_id)
            .map(|ls| ls.model_options.is_empty())
            .unwrap_or(false);
        if need_fetch {
            state.openai.list_models().ok()
        } else {
            None
        }
    };

    // Seed repo context only when starting a new conversation. After that, do not inject system items.
    let is_new_conversation = conversation_id.is_none();

    let ctx_text_opt = if is_new_conversation && include_context_next {
        match state.generate_current_context_text() {
            Ok(t) => Some(t),
            Err(e) => {
                if let Some(st) = state.execute_loops.get_mut(&loop_id) {
                    st.last_status = Some(format!("Context generation failed: {:#}", e));
                }
                None
            }
        }
    } else {
        None
    };

    let seed_items_if_new: Vec<(String, String)> = if is_new_conversation {
        let schema = crate::app::ui::changeset_applier::CHANGESET_SCHEMA_EXAMPLE;
        let mut sys = String::new();
        if !instruction.trim().is_empty() {
            sys.push_str(&instruction);
        }

        if let Some(ctx_text) = &ctx_text_opt {
            sys.push_str("\n\nREPO CONTEXT (generated):\n");
            sys.push_str(ctx_text);
        }

        sys.push_str("\n\nCHANGESET MODE CONTRACT:\n");
        sys.push_str("- When the user sends MODE: changeset, return ONLY ONE valid JSON object matching the ChangeSet schema.\n");
        sys.push_str("\nSCHEMA EXAMPLE (copy this structure exactly):\n");
        sys.push_str(schema);

        vec![
          (
            "system".to_string(),
            sys
          )
        ]
    } else {
        Vec::new()
    };

    let mode_header = match mode {
        ExecuteLoopMode::Conversation => "Conversation mode: please discuss coding design and do not provide any changeset payloads",
        ExecuteLoopMode::ChangeSet => "Changeset mode: please provide only strict JSON changeset format and do not waste any token's inserting comments into the code",
    };

    let user_payload = format!("{}\n\n{}", mode_header, draft.trim());

    let mut turn_items: Vec<(String, String)> = Vec::new();
    turn_items.push(("user".to_string(), user_payload));

    let seed_items_for_api = seed_items_if_new.clone();

    let openai = state.openai.clone();
    let (tx, rx) = mpsc::channel::<Result<crate::app::state::ExecuteLoopTurnResult, String>>();

    std::thread::spawn(move || {
        let res = openai
            .chat_in_conversation(&model, conversation_id, seed_items_for_api, turn_items)
            .map(|(text, conv_id)| crate::app::state::ExecuteLoopTurnResult {
                text,
                conversation_id: conv_id,
            })
            .map_err(|e| format!("{:#}", e));
        let _ = tx.send(res);
    });

    if is_new_conversation {
        if let Some(st) = state.execute_loops.get_mut(&loop_id) {
            if let Some((_, sys_content)) = seed_items_if_new.first() {
                if st.messages.is_empty() {
                    st.messages.push(ExecuteLoopMessage {
                        role: "system".to_string(),
                        content: sys_content.clone(),
                    });
                } else {
                    // If there's already a system message at the top, replace it.
                    if st.messages[0].role == "system" {
                        st.messages[0].content = sys_content.clone();
                    } else {
                        // Otherwise insert the system prompt at the beginning so ordering is:
                        // system -> user -> assistant
                        st.messages.insert(
                            0,
                            ExecuteLoopMessage {
                                role: "system".to_string(),
                                content: sys_content.clone(),
                            },
                        );
                    }
                }
            }
            st.include_context_next = false;
        }
    }

    let mut _did_mutate = false;


    {
        let Some(st) = state.execute_loops.get_mut(&loop_id) else {
            return;
        };

        if let Some(mut ms) = fetched_models {
            ms.sort();
            ms.dedup();
            st.model_options = ms;
            if !st.model_options.is_empty() && !st.model_options.iter().any(|m| m == &st.model) {
                st.model = st.model_options[0].clone();
            }
        }

        st.messages.push(ExecuteLoopMessage {
            role: "user".to_string(),
            content: st.draft.trim().to_string(),
        });
        st.draft.clear();

        st.include_context_next = false;

        st.pending = true;
        st.pending_rx = Some(rx);

        st.last_status = Some("Waiting for response…".to_string());
        _did_mutate = true;
    }

    if _did_mutate {
        // Write-through persistence for chats
        state.task_store_dirty = true;
        state.save_repo_task_store();
    }
}

fn start_postprocess(state: &mut AppState, loop_id: ComponentId) {
    let (cmd, cwd, already_pending) = {
        let Some(st) = state.execute_loops.get(&loop_id) else {
            return;
        };
        let pending = st.postprocess_pending;
        let cmd = st.postprocess_cmd.trim().to_string();
        let cwd = state
            .inputs
            .local_repo
            .clone()
            .or_else(|| state.inputs.repo.clone())
            .unwrap_or_else(|| PathBuf::from("."));
        (cmd, cwd, pending)
    };

    if already_pending {
        if let Some(st) = state.execute_loops.get_mut(&loop_id) {
            st.last_status = Some("Postprocess already running…".to_string());
        }
        return;
    }

    if cmd.is_empty() {
        if let Some(st) = state.execute_loops.get_mut(&loop_id) {
            st.last_status = Some("Postprocess command is empty.".to_string());
        }
        return;
    }

    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    let cmd_for_thread = cmd.clone();

    std::thread::spawn(move || {
        let out = run_command_best_effort(&cmd_for_thread, &cwd);
        let _ = tx.send(out);
    });

    if let Some(st) = state.execute_loops.get_mut(&loop_id) {
        st.postprocess_pending = true;
        st.postprocess_rx = Some(rx);
        st.last_status = Some(format!("Postprocess running: {}", cmd));
    }
}

fn run_command_best_effort(cmd: &str, cwd: &PathBuf) -> Result<String, String> {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd")
            .arg("/C")
            .arg(cmd)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("Failed to spawn cmd: {:#}", e))?;

        let mut s = String::new();
        s.push_str(&String::from_utf8_lossy(&output.stdout));
        if !output.stderr.is_empty() {
            if !s.is_empty() {
                s.push_str("\n");
            }
            s.push_str(&String::from_utf8_lossy(&output.stderr));
        }

        if output.status.success() {
            Ok(s)
        } else {
            Err(s)
        }
    }

    #[cfg(not(windows))]
    {
        let output = std::process::Command::new("sh")
            .arg("-lc")
            .arg(cmd)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("Failed to spawn sh: {:#}", e))?;

        let mut s = String::new();
        s.push_str(&String::from_utf8_lossy(&output.stdout));
        if !output.stderr.is_empty() {
            if !s.is_empty() {
                s.push_str("\n");
            }
            s.push_str(&String::from_utf8_lossy(&output.stderr));
        }

        if output.status.success() {
            Ok(s)
        } else {
            Err(s)
        }
    }
}
