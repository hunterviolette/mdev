use eframe::egui;
use std::time::{Duration, Instant};


use crate::app::actions::{Action, ComponentId};
use crate::app::state::AppState;
use crate::app::state::WORKTREE_REF;

fn overlay_bounds(ui: &egui::Ui) -> egui::Rect {
    // Constrain overlays to the clipped region of this component so they stay
    // centered and sized within the parent component (not relative to screen).
    ui.clip_rect().shrink(6.0)
}

fn centered_overlay_rect(
    bounds: egui::Rect,
    w_frac: f32,
    h_frac: f32,
    min: [f32; 2],
    max: [f32; 2],
) -> egui::Rect {
    // IMPORTANT: if the parent component is smaller than our nominal minimums,
    // allow the overlay to shrink so it always fits and stays centered.
    let pad = 8.0;
    let max_w_by_bounds = (bounds.width() - pad * 2.0).max(1.0);
    let max_h_by_bounds = (bounds.height() - pad * 2.0).max(1.0);

    let min_w = min[0].min(max_w_by_bounds);
    let min_h = min[1].min(max_h_by_bounds);

    let max_w = max[0].min(max_w_by_bounds);
    let max_h = max[1].min(max_h_by_bounds);

    let w = (bounds.width() * w_frac).clamp(min_w, max_w);
    let h = (bounds.height() * h_frac).clamp(min_h, max_h);

    let rect = egui::Rect::from_center_size(bounds.center(), egui::vec2(w, h));
    rect.intersect(bounds)
}

fn popup_overlay<R>(
    ctx: &egui::Context,
    bounds: egui::Rect,
    id: egui::Id,
    title: &str,
    rect: egui::Rect,
    mut add_contents: impl FnMut(&mut egui::Ui) -> R,
) -> bool {
    let mut open = true;

    egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .constrain_to(bounds)
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());

            egui::Frame::popup(ui.style())
                .rounding(egui::Rounding::same(10.0))
                .shadow(ui.style().visuals.popup_shadow)
                .show(ui, |ui| {
                    ui.set_min_size(rect.size());
                    ui.set_max_size(rect.size());

                    ui.horizontal(|ui| {
                        ui.heading(title);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✕").clicked() {
                                open = false;
                            }
                        });
                    });

                    ui.add_space(8.0);
                    let _ = add_contents(ui);
                });
        });

    open
}

pub fn source_control_panel(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    sc_id: ComponentId,
) -> Vec<Action> {
    let mut actions: Vec<Action> = Vec::new();

    let Some(_repo) = state.inputs.repo.as_ref() else {
        ui.label("No repo selected. Pick a folder first.");
        return actions;
    };


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

        let last_opt = ui.ctx().data(|d| d.get_temp::<Instant>(last_id));
        let last = match last_opt {
            Some(v) => v,
            None => {
                ui.ctx().data_mut(|d| d.insert_temp(last_id, now));
                now
            }
        };

        // Refresh at most once every 2 seconds.
        if now.duration_since(last) >= Duration::from_secs(4) {
            actions.push(Action::RefreshSourceControl { sc_id });
            ui.ctx().data_mut(|d| d.insert_temp(last_id, now));
        }

        // Keep repainting often enough for the timer to fire.
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }


    // --- Header ---
    // Keep header height stable so horizontal resizing does not change the
    // remaining vertical space for the lists.
    // We allocate a fixed-height header region and use wrapped rows inside.
    let row_h = ui.spacing().interact_size.y;
    let header_h = (row_h * 2.0) + (ui.spacing().item_spacing.y * 3.0) + 6.0;

    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), header_h),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            // Line 1: selectors (Remote/Branch)
            ui.horizontal_wrapped(|ui| {
                let remote_open_id = egui::Id::new(("sc_remote_popup_open", sc_id));
                let branch_open_id = egui::Id::new(("sc_branch_popup_open", sc_id));

                let mut remote_open = ui.ctx().data(|d| d.get_temp::<bool>(remote_open_id).unwrap_or(false));
                let mut branch_open = ui.ctx().data(|d| d.get_temp::<bool>(branch_open_id).unwrap_or(false));

                ui.label("Remote:");
                let remote_txt = if sc.remote.is_empty() { "(none)" } else { sc.remote.as_str() };
                if ui.button(remote_txt).clicked() {
                    remote_open = true;
                    branch_open = false;
                }

                ui.separator();

                ui.label("Branch:");
                let branch_txt = if sc.branch.is_empty() { "(none)" } else { sc.branch.as_str() };
                if ui.button(branch_txt).clicked() {
                    branch_open = true;
                    remote_open = false;
                }

                ui.ctx().data_mut(|d| {
                    d.insert_temp(remote_open_id, remote_open);
                    d.insert_temp(branch_open_id, branch_open);
                });
            });

            // Line 2: buttons
            ui.horizontal_wrapped(|ui| {
                let commit_modal_id = egui::Id::new(("sc_commit_modal", sc_id));
                let commit_open = ui.ctx().data(|d| d.get_temp::<bool>(commit_modal_id).unwrap_or(false));

                if ui.button("Fetch").clicked() {
                    actions.push(Action::FetchRemote { sc_id });
                    actions.push(Action::RefreshSourceControl { sc_id });
                }
                if ui.button("Pull").clicked() {
                    actions.push(Action::PullRemote { sc_id });
                    actions.push(Action::RefreshSourceControl { sc_id });
                }

                let commit_label = if commit_open { "Commit (open)" } else { "Commit..." };
                if ui.button(commit_label).clicked() {
                    ui.ctx().data_mut(|d| d.insert_temp(commit_modal_id, true));
                }
            });
        },
    );

    ui.add_space(6.0);

    // --- Remote popup (component-centered overlay) ---
    {
        let bounds = overlay_bounds(ui);
        let remote_open_id = egui::Id::new(("sc_remote_popup_open", sc_id));
        let mut remote_open = ui.ctx().data(|d| d.get_temp::<bool>(remote_open_id).unwrap_or(false));

        if remote_open {
            let rect = centered_overlay_rect(bounds, 0.60, 0.55, [360.0, 220.0], [720.0, 520.0]);
            let open_now = popup_overlay(
                ctx,
                bounds,
                egui::Id::new(("sc_remote_popup_overlay", sc_id)),
                "Select Remote",
                rect,
                |ui| {
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
                            actions.push(Action::RefreshSourceControl { sc_id });
                            remote_open = false;
                        }
                    }
                },
            );

            remote_open = open_now && remote_open;
            ui.ctx().data_mut(|d| d.insert_temp(remote_open_id, remote_open));
        }
    }

    // --- Branch popup (component-centered overlay; includes create new branch) ---
    {
        let bounds = ui.max_rect();
        let branch_open_id = egui::Id::new(("sc_branch_popup_open", sc_id));
        let mut branch_open = ui.ctx().data(|d| d.get_temp::<bool>(branch_open_id).unwrap_or(false));

        if branch_open {
            let rect = centered_overlay_rect(bounds, 0.70, 0.70, [420.0, 260.0], [860.0, 640.0]);
            let open_now = popup_overlay(
                ctx,
                bounds,
                egui::Id::new(("sc_branch_popup_overlay", sc_id)),
                "Select Branch",
                rect,
                |ui| {
                    ui.label("Branches:");
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
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
                                actions.push(Action::RefreshSourceControl { sc_id });
                            }
                        }
                    });

                    ui.separator();

                    ui.horizontal(|ui| {
                        if ui.button("Checkout selected").clicked() {
                            actions.push(Action::CheckoutBranch {
                                sc_id,
                                create_if_missing: false,
                            });
                            actions.push(Action::RefreshSourceControl { sc_id });
                            branch_open = false;
                        }
                    });

                    ui.separator();

                    ui.label("Create new branch:");
                    let new_branch_id = egui::Id::new(("sc_new_branch", sc_id));
                    let mut new_branch = ui
                        .ctx()
                        .data(|d| d.get_temp::<String>(new_branch_id).unwrap_or_default());

                    ui.add(egui::TextEdit::singleline(&mut new_branch).hint_text("new-branch-name"));

                    let can_create = !new_branch.trim().is_empty();
                    ui.horizontal(|ui| {
                        if ui.add_enabled(can_create, egui::Button::new("Create & Checkout")).clicked() {
                            let nb = new_branch.trim().to_string();
                            actions.push(Action::SetSourceControlBranch {
                                sc_id,
                                branch: nb,
                            });
                            actions.push(Action::CheckoutBranch {
                                sc_id,
                                create_if_missing: true,
                            });
                            actions.push(Action::RefreshSourceControl { sc_id });
                            ui.ctx().data_mut(|d| d.insert_temp(new_branch_id, String::new()));
                            branch_open = false;
                        }

                        if ui.button("Clear").clicked() {
                            new_branch.clear();
                            ui.ctx().data_mut(|d| d.insert_temp(new_branch_id, String::new()));
                        }
                    });

                    ui.ctx().data_mut(|d| d.insert_temp(new_branch_id, new_branch));
                },
            );

            branch_open = open_now && branch_open;
            ui.ctx().data_mut(|d| d.insert_temp(branch_open_id, branch_open));
        }
    }

    // --- Commit modal (component-centered overlay) ---
    {
        let bounds = ui.max_rect();
        let commit_modal_id = egui::Id::new(("sc_commit_modal", sc_id));
        let mut commit_open = ui
            .ctx()
            .data(|d| d.get_temp::<bool>(commit_modal_id).unwrap_or(false));

        if commit_open {
            let rect = centered_overlay_rect(bounds, 0.72, 0.62, [460.0, 260.0], [920.0, 640.0]);
            let open_now = popup_overlay(
                ctx,
                bounds,
                egui::Id::new(("sc_commit_overlay", sc_id)),
                "Commit",
                rect,
                |ui| {
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
                            commit_open = false;
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add_enabled(can_commit, egui::Button::new("Commit + Push")).clicked() {
                                actions.push(Action::CommitAndPush { sc_id });
                                actions.push(Action::RefreshSourceControl { sc_id });
                                commit_open = false;
                            }

                            if ui.add_enabled(can_commit, egui::Button::new("Commit")).clicked() {
                                actions.push(Action::CommitStaged { sc_id });
                                actions.push(Action::RefreshSourceControl { sc_id });
                                commit_open = false;
                            }
                        });
                    });
                },
            );

            commit_open = open_now && commit_open;
            ui.ctx().data_mut(|d| d.insert_temp(commit_modal_id, commit_open));
        }
    }


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

    // --- Dynamic sizing (lists first) ---
    // Give nearly all available height to Staged/Changes lists.
    // Last output is rendered as a single-line row and should not grow with panel height.
    let avail_h = ui.available_height().max(1.0);
    let output_row_h = 24.0;

    // Reserve for headings/buttons/separators etc.
    let reserve_h = 70.0;

    // Remaining height goes to the file lists (split evenly).
    let lists_h_total = (avail_h - output_row_h - reserve_h).max(260.0);
    let list_h = (lists_h_total * 0.50).clamp(160.0, 4000.0);

    let pending_discard_id = egui::Id::new(("sc_pending_discard", sc_id));
    let mut pending_discard = ui
        .ctx()
        .data(|d| d.get_temp::<String>(pending_discard_id).unwrap_or_default());


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
        .auto_shrink([false, false])
        .max_height(list_h)
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

                    // Clickable filename -> open/attach Diff Viewer
                    // STAGED list (like VS Code): INDEX (staged) vs WORKTREE
                    if ui
                        .add(egui::Link::new(egui::RichText::new(&f.path).monospace()))
                        .clicked()
                    {
                        actions.push(Action::OpenDiffViewerForPathWithRefs {
                            path: f.path.clone(),
                            from_ref: "HEAD".to_string(),
                            to_ref: "INDEX".to_string(),
                        });
                    }

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
        .auto_shrink([false, false])
        .max_height(list_h)
        .show(ui, |ui| {
            if unstaged_files.is_empty() {
                ui.label("(clean)");
                return;
            }

            for f in unstaged_files.iter() {
                ui.horizontal(|ui| {

                    let code = format!("{}{}", f.index_status, f.worktree_status);
                    ui.monospace(code);

                    // Clickable filename -> open/attach Diff Viewer
                    // UNSTAGED list (like VS Code): HEAD vs WORKTREE
                    if ui
                        .add(egui::Link::new(egui::RichText::new(&f.path).monospace()))
                        .clicked()
                    {
                        actions.push(Action::OpenDiffViewerForPathWithRefs {
                            path: f.path.clone(),
                            from_ref: "INDEX".to_string(),
                            to_ref: WORKTREE_REF.to_string(),
                        });
                    }

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

                        if ui.button("Discard").clicked() {
                            pending_discard = f.path.clone();
                            ui.ctx().data_mut(|d| d.insert_temp(pending_discard_id, pending_discard.clone()));
                        }
                    });
                });
            }
        });

    // --- Discard confirmation overlay (component-centered) ---
    {
        let bounds = ui.max_rect();
        if !pending_discard.is_empty() {
            let rect = centered_overlay_rect(bounds, 0.62, 0.40, [420.0, 180.0], [820.0, 360.0]);

            let untracked = sc
                .files
                .iter()
                .find(|x| x.path == pending_discard)
                .map(|x| x.untracked)
                .unwrap_or(false);

            let open_now = popup_overlay(
                ctx,
                bounds,
                egui::Id::new(("sc_discard_confirm_overlay", sc_id)),
                "Discard changes?",
                rect,
                |ui| {
                    ui.label("This will revert the file to the last committed state.");
                    if untracked {
                        ui.label("This file is untracked, so it will be deleted.");
                    }
                    ui.add_space(10.0);
                    ui.monospace(&pending_discard);
                    ui.add_space(12.0);

                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            pending_discard.clear();
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Discard").clicked() {
                                actions.push(Action::DiscardPath {
                                    sc_id,
                                    path: pending_discard.clone(),
                                    untracked,
                                });
                                actions.push(Action::RefreshSourceControl { sc_id });
                                pending_discard.clear();
                            }
                        });
                    });
                },
            );

            if !open_now {
                pending_discard.clear();
            }

            ui.ctx().data_mut(|d| d.insert_temp(pending_discard_id, pending_discard));
        }
    }

    ui.separator();

    // --- Last output (single line; click to copy full output) ---
    ui.horizontal(|ui| {
        ui.label("Last output:");

        let full = sc
            .last_output
            .clone()
            .unwrap_or_else(|| "(none)".to_string());

        // Display only the first line, truncated.
        let mut display = full.lines().next().unwrap_or("").to_string();
        if display.is_empty() {
            display = "(none)".to_string();
        }

        const MAX_CHARS: usize = 140;
        if display.chars().count() > MAX_CHARS {
            display = display.chars().take(MAX_CHARS).collect::<String>() + "…";
        }
        if full.contains('\n') && !display.ends_with('…') {
            display.push('…');
        }

        let resp = ui
            .add(
                egui::Label::new(egui::RichText::new(display).monospace())
                    .sense(egui::Sense::click()),
            )
            .on_hover_text(full.clone());

        if resp.clicked() {
            ui.output_mut(|o| o.copied_text = full.clone());
        }

        if let Some(err) = &sc.last_error {
            let err_resp = ui.add(
                egui::Label::new(egui::RichText::new("⚠ Error")).sense(egui::Sense::hover()),
            );
            err_resp.on_hover_text(err.clone());
        }
    });

    actions
}
