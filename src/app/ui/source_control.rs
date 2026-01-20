use eframe::egui;
use std::time::{Duration, Instant};


use crate::app::actions::{Action, ComponentId};
use crate::app::state::AppState;

pub fn source_control_panel(
    _ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    sc_id: ComponentId,
) -> Vec<Action> {
    let mut actions: Vec<Action> = Vec::new();

    let Some(repo) = state.inputs.repo.as_ref() else {
        ui.label("No repo selected. Pick a folder first.");
        return actions;
    };

    ui.horizontal_wrapped(|ui| {
        ui.label("Repo:");
    });

    let Some(sc) = state.source_controls.get_mut(&sc_id) else {
        ui.label("Source control state missing (try resetting layout/workspace)." );
        return actions;
    };

    // Auto-refresh once when the panel first appears so branch/remote options populate.
    if sc.needs_refresh {
        sc.needs_refresh = false;
        actions.push(Action::RefreshSourceControl { sc_id });
    }

    // Auto-refresh periodically so the panel stays current without manual clicks.
    // Uses egui temp-data so we don't need to add persistent fields to state.
    {
        let now = Instant::now();
        let last_id = egui::Id::new(("sc_last_auto_refresh", sc_id));
        let last = ui
            .ctx()
            .data(|d| d.get_temp::<Instant>(last_id))
            .unwrap_or(now);

        // Refresh at most once every 2 seconds.
        if now.duration_since(last) >= Duration::from_secs(2) {
            actions.push(Action::RefreshSourceControl { sc_id });
            ui.ctx().data_mut(|d| d.insert_temp(last_id, now));
        }

        // Keep repainting often enough for the timer to fire.
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }


    // Compact always-visible summary
    // Use wrapped layout so long remote/branch names never force the window wider.
    ui.horizontal_wrapped(|ui| {
        ui.label("Remote:");
        ui.monospace(if sc.remote.is_empty() { "(none)" } else { sc.remote.as_str() });
        ui.separator();
        ui.label("Branch:");
        ui.monospace(if sc.branch.is_empty() { "(none)" } else { sc.branch.as_str() });
    });

    egui::CollapsingHeader::new("Details")
        .id_source(("sc_header", sc_id))
        .default_open(false)
        .show(ui, |ui| {
            // Details header layout:
            //   line 1: actions (Refresh/Fetch/Pull/Commit)
            //   line 2: Remote picker
            //   line 3: Branch picker

            // Line 1: actions
            ui.horizontal_wrapped(|ui| {
                if ui.button("Refresh").clicked() {
                    actions.push(Action::RefreshSourceControl { sc_id });
                }
                if ui.button("Fetch").clicked() {
                    actions.push(Action::FetchRemote { sc_id });
                    actions.push(Action::RefreshSourceControl { sc_id });
                }
                if ui.button("Pull").clicked() {
                    actions.push(Action::PullRemote { sc_id });
                    actions.push(Action::RefreshSourceControl { sc_id });
                }

                // Commit moved into a modal flow (see below).
                let commit_modal_id = egui::Id::new(("sc_commit_modal", sc_id));
                let commit_open = ui
                    .ctx()
                    .data(|d| d.get_temp::<bool>(commit_modal_id).unwrap_or(false));
                let commit_label = if commit_open { "Commit (open)" } else { "Commit..." };
                if ui.button(commit_label).clicked() {
                    ui.ctx().data_mut(|d| d.insert_temp(commit_modal_id, true));
                }
            });

            ui.add_space(4.0);

            // Line 2: Remote
            ui.horizontal_wrapped(|ui| {
                ui.label("Remote:");
                egui::ComboBox::from_id_source(("sc_remote", sc_id))
                    .selected_text(if sc.remote.is_empty() { "(none)" } else { sc.remote.as_str() })
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        if sc.remote_options.is_empty() {
                            ui.label("(no remotes)");
                        }
                        for r in sc.remote_options.iter() {
                            let is_sel = sc.remote == *r;
                            if ui.selectable_label(is_sel, r).clicked() {
                                actions.push(Action::SetSourceControlRemote {
                                    sc_id,
                                    remote: r.clone(),
                                });
                            }
                        }
                    });
            });

            ui.add_space(2.0);

            // Line 3: Branch
            ui.horizontal_wrapped(|ui| {
                ui.label("Branch:");
                let selected_branch = if sc.branch.is_empty() { "(none)" } else { sc.branch.as_str() };
                egui::ComboBox::from_id_source(("sc_branch", sc_id))
                    .selected_text(selected_branch)
                    .width(320.0)
                    .show_ui(ui, |ui| {
                        if sc.branch_options.is_empty() {
                            ui.label("(no branches)");
                        }
                        for b in sc.branch_options.iter() {
                            let is_sel = sc.branch == *b;
                            if ui.selectable_label(is_sel, b).clicked() {
                                actions.push(Action::SetSourceControlBranch {
                                    sc_id,
                                    branch: b.clone(),
                                });
                            }
                        }
                    });
            });

            // Commit modal (prompt for message + final action)
            let commit_modal_id = egui::Id::new(("sc_commit_modal", sc_id));
            let mut commit_open = ui
                .ctx()
                .data(|d| d.get_temp::<bool>(commit_modal_id).unwrap_or(false));

            if commit_open {
                let mut open = true;
                egui::Window::new("Commit")
                    .id(egui::Id::new(("sc_commit_window", sc_id)))
                    .collapsible(false)
                    .resizable(true)
                    .open(&mut open)
                    .show(ui.ctx(), |ui| {
                        ui.label("Commit message:");
                        ui.add(
                            egui::TextEdit::multiline(&mut sc.commit_message)
                                .desired_rows(6)
                                .hint_text("Enter commit message")
                                .lock_focus(true)
                                .desired_width(f32::INFINITY),
                        );

                        ui.add_space(8.0);

                        let can_commit = !sc.commit_message.trim().is_empty();

                        ui.horizontal(|ui| {
                            if ui.button("Cancel").clicked() {
                                ui.ctx().data_mut(|d| d.insert_temp(commit_modal_id, false));
                            }

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add_enabled(can_commit, egui::Button::new("Commit + Push")).clicked() {
                                    actions.push(Action::CommitStaged { sc_id });

                                    actions.push(Action::RefreshSourceControl { sc_id });
                                    ui.ctx().data_mut(|d| d.insert_temp(commit_modal_id, false));
                                }

                                if ui.add_enabled(can_commit, egui::Button::new("Commit")).clicked() {
                                    actions.push(Action::CommitStaged { sc_id });
                                    actions.push(Action::RefreshSourceControl { sc_id });
                                    ui.ctx().data_mut(|d| d.insert_temp(commit_modal_id, false));
                                }
                            });
                        });
                    });

                if !open {
                    commit_open = false;
                }

                ui.ctx().data_mut(|d| d.insert_temp(commit_modal_id, commit_open));
            }


        });



    // Split into two queues.
    // IMPORTANT: files like AM/MM must appear in BOTH queues.
    // - Staged Changes: anything currently staged (index has content)
    // - Changes: anything with worktree changes (right status) OR untracked

    let mut staged_files: Vec<_> = sc.files.iter().filter(|f| f.staged).collect();

    let mut unstaged_files: Vec<_> = sc
        .files
        .iter()
        .filter(|f| {
            if f.untracked {
                return true;
            }
            let wt = f.worktree_status.as_str();
            // In git porcelain-like status, '.' and ' ' both mean "no change".
            !(wt.is_empty() || wt == " " || wt == ".")
        })
        .collect();

    staged_files.sort_by(|a, b| a.path.cmp(&b.path));
    unstaged_files.sort_by(|a, b| a.path.cmp(&b.path));


    // -------------------------
    // Staged Changes
    // -------------------------
    ui.horizontal(|ui| {
        ui.strong(format!("Staged Changes ({})", staged_files.len()));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Unstage All").clicked() {
                actions.push(Action::UnstageAll { sc_id });
                actions.push(Action::RefreshSourceControl { sc_id });
            }

        });
    });

    egui::ScrollArea::vertical()
        .id_source(("sc_staged_scroll", sc_id))
        .max_height(180.0)
        .show(ui, |ui| {
            if staged_files.is_empty() {
                ui.label("(none)");
                return;
            }

            for f in staged_files.iter() {
                ui.horizontal(|ui| {

                    // Show combined code for context (e.g. AM/MM), but this list is the staged pipeline.
                    let code = format!("{}{}", f.index_status, f.worktree_status);
                    ui.monospace(code);
                    ui.monospace(&f.path);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Unstage").clicked() {
                            actions.push(Action::UnstagePath {
                                sc_id,
                                path: f.path.clone(),
                            });
                            actions.push(Action::RefreshSourceControl { sc_id });
                        }
                    });
                });
            }
        });

    ui.add_space(8.0);

    // -------------------------
    // Changes (unstaged)
    // -------------------------
    ui.horizontal(|ui| {
        ui.strong(format!("Changes ({})", unstaged_files.len()));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Stage All").clicked() {
                actions.push(Action::StageAll { sc_id });
                actions.push(Action::RefreshSourceControl { sc_id });
            }

        });
    });

    egui::ScrollArea::vertical()
        .id_source(("sc_unstaged_scroll", sc_id))
        .max_height(260.0)
        .show(ui, |ui| {
            if unstaged_files.is_empty() {
                ui.label("(clean)");
                return;
            }

            for f in unstaged_files.iter() {
                ui.horizontal(|ui| {

                    let code = format!("{}{}", f.index_status, f.worktree_status);
                    ui.monospace(code);
                    ui.monospace(&f.path);

                    if f.untracked {
                        ui.label("(untracked)");
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Stage").clicked() {
                            actions.push(Action::StagePath {
                                sc_id,
                                path: f.path.clone(),
                            });
                            actions.push(Action::RefreshSourceControl { sc_id });
                        }
                    });
                });
            }
        });

    ui.separator();

    ui.label("Last output:");
    egui::ScrollArea::vertical()
        .id_source(("sc_output_scroll", sc_id))
        .max_height(160.0)
        .show(ui, |ui| {
            if let Some(out) = &sc.last_output {
                ui.monospace(out);
            } else {
                ui.label("(none)");
            }

            if let Some(err) = &sc.last_error {
                ui.separator();
                ui.label("Error:");
                ui.monospace(err);
            }
        });

    actions
}
