// src/app/ui/task_panel.rs
use eframe::egui;

use crate::app::actions::{Action, ComponentId, ConversationId};
use crate::app::state::AppState;
use std::collections::BTreeSet;

fn status_string(loop_st: &crate::app::state::ExecuteLoopState) -> String {
    if loop_st.paused {
        return "paused".to_string();
    }
    if loop_st.pending {
        return "waiting for response".to_string();
    }
    if loop_st.awaiting_review {
        return "waiting for review".to_string();
    }
    if loop_st.postprocess_pending {
        return "compiling".to_string();
    }
    if loop_st.awaiting_apply_result {
        return "applying changesets".to_string();
    }
    if loop_st
        .last_status
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase()
        .contains("error")
    {
        return "error".to_string();
    }
    "idle".to_string()
}

fn status_string_from_snapshot(snap: &crate::app::layout::ExecuteLoopSnapshot) -> String {
    if snap.paused {
        "paused".to_string()
    } else {
        "idle".to_string()
    }
}

fn fmt_dt(ms: u64) -> String {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    if ms == 0 {
        return "-".to_string();
    }

    let nanos = (ms as i128) * 1_000_000;
    match OffsetDateTime::from_unix_timestamp_nanos(nanos) {
        Ok(dt) => dt.format(&Rfc3339).unwrap_or_else(|_| ms.to_string()),
        Err(_) => ms.to_string(),
    }
}

fn oai_id_tail_last_10(oai_id: &str) -> String {
    // Char-safe tail (OpenAI ids are ASCII-ish, but keep this correct for Unicode anyway).
    let tail: String = oai_id.chars().rev().take(10).collect::<String>().chars().rev().collect();
    if oai_id.chars().count() > 10 {
        format!("…{}", tail)
    } else {
        tail
    }
}

pub fn task_panel(
    _ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    task_id: ComponentId,
) -> Vec<Action> {
    let mut actions: Vec<Action> = Vec::new();

    // Local (non-persisted) multi-select state per task.
    // Stored in AppState.ui to avoid `static mut` (and to keep selection stable across frames).
    let selected_conversations: &mut BTreeSet<ConversationId> = {
        let map = state.ui.task_panel_selected_loops_mut();
        map.entry(task_id).or_insert_with(BTreeSet::new)
    };

    // Avoid holding a mutable borrow of `state.tasks` across UI closures.
    let (_task_paused, _task_bound_execute_loop, task_active_conversation) = {
        let task = state.tasks.entry(task_id).or_default();
        (task.paused, task.bound_execute_loop, task.active_conversation)
    };

    ui.horizontal(|ui| {
        ui.heading(format!("Task {}", task_id));
        ui.add_space(8.0);

        if ui
            .button("Delete selected")
            .on_hover_text("Delete all selected conversations")
            .clicked()
        {
            let ids: Vec<ConversationId> = selected_conversations.iter().copied().collect();
            if !ids.is_empty() {
                actions.push(Action::TaskConversationsDelete {
                    task_id,
                    conversation_ids: ids,
                });
                selected_conversations.clear();
            }
        }

        if ui.button("Pause selected").clicked() {
            let ids: Vec<ConversationId> = selected_conversations.iter().copied().collect();
            if !ids.is_empty() {
                actions.push(Action::TaskConversationsSetPaused {
                    task_id,
                    conversation_ids: ids,
                    paused: true,
                });
            }
        }

        if ui.button("Resume selected").clicked() {
            let ids: Vec<ConversationId> = selected_conversations.iter().copied().collect();
            if !ids.is_empty() {
                actions.push(Action::TaskConversationsSetPaused {
                    task_id,
                    conversation_ids: ids,
                    paused: false,
                });
            }
        }

        ui.add_space(8.0);

        if ui
            .button("New chat")
            .on_hover_text("Create a new conversation and open it")
            .clicked()
        {
            actions.push(Action::TaskCreateConversationAndOpen { task_id });
        }
    });

    ui.add_space(8.0);
    ui.add_space(8.0);

    ui.group(|ui| {
        ui.label("Chats (Conversations) — status / stats");

        // List Task-owned durable conversations.
        let mut ids: Vec<ConversationId> = state
            .tasks
            .get(&task_id)
            .map(|t| t.conversations.keys().copied().collect())
            .unwrap_or_default();
        ids.sort();

        egui::Grid::new(("conversation_stats_grid", task_id))
            .striped(true)
            .show(ui, |ui| {
                ui.strong("Sel");
                ui.strong("Conversation");
                ui.strong("Status");
                ui.strong("Mode");
                ui.strong("Msgs");
                ui.strong("Apply ok/total");
                ui.strong("Post ok/total");
                ui.strong("Updated");
                ui.end_row();

                for conversation_id in ids {
                    let snap = match state
                        .tasks
                        .get(&task_id)
                        .and_then(|t| t.conversations.get(&conversation_id))
                    {
                        Some(s) => s,
                        None => continue,
                    };

                    let mode = format!("{:?}", snap.mode);
                    let msgs = snap.messages.len();

                    let apply_total = snap.changesets_total;
                    let apply_ok = snap.changesets_ok;

                    let post_total = snap.postprocess_ok + snap.postprocess_err;
                    let post_ok = snap.postprocess_ok;

                    // Snapshot status (task-scoped durable view).
                    let status = status_string_from_snapshot(snap);

                    // Selection checkbox
                    let mut is_sel = selected_conversations.contains(&conversation_id);
                    if ui.checkbox(&mut is_sel, "").changed() {
                        if is_sel {
                            selected_conversations.insert(conversation_id);
                        } else {
                            selected_conversations.remove(&conversation_id);
                        }
                    }

                    ui.horizontal(|ui| {
                        let selected = task_active_conversation == Some(conversation_id);

                        // Display: last 10 chars of the OpenAI conversation id (conv_...), if present.
                        // Hover: show full OpenAI id and internal ConversationId.
                        let (display, hover) = match snap.conversation_id.as_deref() {
                            Some(full) if !full.trim().is_empty() => {
                                let tail = oai_id_tail_last_10(full);
                                (tail, format!("OpenAI: {}\nInternal: Conversation {}", full, conversation_id))
                            }
                            _ => (
                                format!("Conversation {}", conversation_id),
                                format!("OpenAI: (none yet)\nInternal: Conversation {}", conversation_id),
                            ),
                        };

                        let resp = ui.selectable_label(selected, display);

                        // Hover: show full OpenAI conversation id + usage hint.
                        let resp = if let Some(full) = snap.conversation_id.as_deref().filter(|s| !s.trim().is_empty()) {
                            let hover = format!("{}\n\nLeft-click: open\nRight-click: copy id", full);
                            resp.on_hover_text(hover)
                        } else {
                            resp.on_hover_text("(no OpenAI conversation id yet)\n\nLeft-click: open\nRight-click: copy id")
                        };

                        // Right-click: copy full OpenAI conversation id to clipboard.
                        if resp.clicked_by(eframe::egui::PointerButton::Secondary) {
                            if let Some(full) = snap.conversation_id.as_deref().filter(|s| !s.trim().is_empty()) {
                                ui.output_mut(|o| o.copied_text = full.to_string());
                            }
                        }

                        // Left-click: open (existing behavior).
                        if resp.clicked() {
                            actions.push(Action::TaskOpenConversation {
                                task_id,
                                conversation_id,
                            });
                        }
                    });

                    ui.monospace(status);
                    ui.monospace(mode);
                    ui.monospace(msgs.to_string());
                    ui.monospace(format!("{}/{}", apply_ok, apply_total));
                    ui.monospace(format!("{}/{}", post_ok, post_total));

                    ui.add(
                        egui::Label::new(egui::RichText::new(fmt_dt(snap.updated_at_ms)).monospace())
                            .sense(egui::Sense::hover()),
                    )
                    .on_hover_text(format!(
                        "Created: {}",
                        fmt_dt(if snap.created_at_ms == 0 {
                            snap.updated_at_ms
                        } else {
                            snap.created_at_ms
                        })
                    ));

                    ui.end_row();
                }
            });
    });

    actions
}
