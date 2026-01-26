// src/app/ui/task_panel.rs
use eframe::egui;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::AppState;

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

fn pct(n: u32, d: u32) -> String {
    if d == 0 {
        "-".to_string()
    } else {
        format!("{:.0}%", (n as f32) * 100.0 / (d as f32))
    }
}

pub fn task_panel(
    _ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    task_id: ComponentId,
) -> Vec<Action> {
    let mut actions: Vec<Action> = Vec::new();

    // Avoid holding a mutable borrow of `state.tasks` across UI closures.
    let (task_paused, task_bound_execute_loop) = {
        let task = state.tasks.entry(task_id).or_default();
        (task.paused, task.bound_execute_loop)
    };

    ui.horizontal(|ui| {
        ui.heading(format!("Task {}", task_id));
        ui.add_space(8.0);

        let pause_label = if task_paused { "Resume" } else { "Pause" };
        if ui.button(pause_label).clicked() {
            actions.push(Action::TaskSetPaused {
                task_id,
                paused: !task_paused,
            });
        }

        if ui.button("New chat").on_hover_text("Create a new chat thread (Execute Loop) and bind it to this Task").clicked() {
            actions.push(Action::TaskCreateAndBindExecuteLoop { task_id });
        }
    });

    ui.add_space(8.0);


    ui.add_space(8.0);

    ui.group(|ui| {
        ui.label("Chats (Execute Loops) — status / stats");

        // Map loop -> bound tasks (for display)
        let mut loop_to_tasks: std::collections::HashMap<ComponentId, Vec<ComponentId>> =
            Default::default();
        for (tid, t) in state.tasks.iter() {
            if let Some(lid) = t.bound_execute_loop {
                loop_to_tasks.entry(lid).or_default().push(*tid);
            }
        }
        for v in loop_to_tasks.values_mut() {
            v.sort();
        }

        let mut ids: Vec<ComponentId> = state.execute_loop_store.keys().copied().collect();
        ids.sort();

        egui::Grid::new(("execute_loop_stats_grid", task_id))
            .striped(true)
            .show(ui, |ui| {
                ui.strong("Loop");
                ui.strong("Bound tasks");
                ui.strong("Mode");
                ui.strong("Paused");
                ui.strong("Msgs");
                ui.strong("Apply ok/total");
                ui.strong("Apply %");
                ui.strong("Post ok/total");
                ui.strong("Post %");
                ui.strong("Status");
                ui.end_row();

                for loop_id in ids {
                    let snap = match state.execute_loop_store.get(&loop_id) {
                        Some(s) => s,
                        None => continue,
                    };

                    let bound_tasks = loop_to_tasks
                        .get(&loop_id)
                        .map(|v| {
                            v.iter()
                                .map(|x| x.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_else(|| "-".to_string());

                    let mode = format!("{:?}", snap.mode);
                    let paused = if snap.paused { "yes" } else { "no" };
                    let msgs = snap.messages.len();

                    let apply_total = snap.changesets_total;
                    let apply_ok = snap.changesets_ok;
                    let apply_pct = pct(apply_ok, apply_total);

                    let post_total = snap.postprocess_ok + snap.postprocess_err;
                    let post_ok = snap.postprocess_ok;
                    let post_pct = pct(post_ok, post_total);

                    // Prefer richer live status if loop is loaded/open, else snapshot-only.
                    let status = if let Some(loop_st) = state.execute_loops.get(&loop_id) {
                        status_string(loop_st)
                    } else {
                        status_string_from_snapshot(snap)
                    };

                    let selected = task_bound_execute_loop == Some(loop_id);
                    if ui
                        .selectable_label(selected, format!("Execute Loop {}", loop_id))
                        .clicked()
                    {
                        // Single click: bind + open.
                        actions.push(Action::TaskBindExecuteLoop { task_id, loop_id });
                        actions.push(Action::TaskOpenExecuteLoop { task_id });
                    }

                    ui.monospace(bound_tasks);
                    ui.monospace(mode);
                    ui.monospace(paused);
                    ui.monospace(msgs.to_string());
                    ui.monospace(format!("{}/{}", apply_ok, apply_total));
                    ui.monospace(apply_pct);
                    ui.monospace(format!("{}/{}", post_ok, post_total));
                    ui.monospace(post_pct);
                    ui.monospace(status);
                    ui.end_row();
                }
            });
    });

    actions
}
