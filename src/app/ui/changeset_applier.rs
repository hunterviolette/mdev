use eframe::egui;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::AppState;
use crate::gateway_model::{GatewayMode, SyncMode, CHANGESET_SCHEMA_EXAMPLE};

pub fn changeset_applier_panel(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    applier_id: ComponentId,
) -> Vec<Action> {
    let mut actions = vec![];

    let git_ref = state.inputs.git_ref.clone();
    let has_repo = state.inputs.repo.is_some();

    let Some(st) = state.changeset_appliers.get_mut(&applier_id) else {
        ui.label("Missing ChangeSetApplier state.");
        return actions;
    };

    ui.push_id(("changeset_applier", applier_id), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("Payload:");
            if let Some(status) = st.status.as_deref() {
                ui.separator();
                ui.monospace(status);
            }

            let cur_payload = match st.mode {
                GatewayMode::ChangeSet => "CHANGESET",
                GatewayMode::Sync => "SYNC",
            };

            egui::ComboBox::from_id_source(("changeset_payload_type", applier_id))
                .selected_text(cur_payload)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(st.mode == GatewayMode::ChangeSet, "CHANGESET")
                        .clicked()
                    {
                        actions.push(Action::SetChangeSetGatewayMode {
                            applier_id,
                            mode: GatewayMode::ChangeSet,
                        });
                    }
                    if ui
                        .selectable_label(st.mode == GatewayMode::Sync, "SYNC")
                        .clicked()
                    {
                        actions.push(Action::SetChangeSetGatewayMode {
                            applier_id,
                            mode: GatewayMode::Sync,
                        });
                    }
                });

            if st.mode == GatewayMode::Sync {
                ui.separator();
                ui.label("Mode:");

                let cur_mode = match st.sync_mode {
                    SyncMode::Entire => "ENTIRE REPO",
                    SyncMode::Tree => "TREE SELECT",
                    SyncMode::Diff => "DIFF",
                };

                egui::ComboBox::from_id_source(("changeset_sync_mode", applier_id))
                    .selected_text(cur_mode)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(st.sync_mode == SyncMode::Entire, "ENTIRE REPO")
                            .clicked()
                        {
                            actions.push(Action::SetChangeSetSyncMode {
                                applier_id,
                                mode: SyncMode::Entire,
                            });
                        }
                        if ui
                            .selectable_label(st.sync_mode == SyncMode::Tree, "TREE SELECT")
                            .clicked()
                        {
                            actions.push(Action::SetChangeSetSyncMode {
                                applier_id,
                                mode: SyncMode::Tree,
                            });
                        }
                        if ui
                            .selectable_label(st.sync_mode == SyncMode::Diff, "DIFF")
                            .clicked()
                        {
                            actions.push(Action::SetChangeSetSyncMode {
                                applier_id,
                                mode: SyncMode::Diff,
                            });
                        }
                    });

                ui.separator();
                ui.label("Ref:");
                ui.monospace(&git_ref);

                let mut skip_binary = st.sync_skip_binary;
                if ui.checkbox(&mut skip_binary, "Skip binary").clicked() {
                    actions.push(Action::SetChangeSetSyncSkipBinary {
                        applier_id,
                        value: skip_binary,
                    });
                }

                let mut skip_gitignore = st.sync_skip_gitignore;
                if ui.checkbox(&mut skip_gitignore, "Skip .gitignore").clicked() {
                    actions.push(Action::SetChangeSetSyncSkipGitignore {
                        applier_id,
                        value: skip_gitignore,
                    });
                }

                if git_ref != crate::app::state::WORKTREE_REF {
                    ui.separator();
                    ui.weak("(diff mode requires Ref=WORKTREE)");
                }
            }
        });

        ui.add_space(8.0);

        if st.mode == GatewayMode::ChangeSet {
            if st.changeset_show_result {
                ui.label("The main box is showing the last apply result. Use Copy last changeset to reuse the payload.");
            } else {
                ui.label("Paste a JSON changeset payload and apply it to the repo working tree.");
            }
        } else {
            ui.label("Generate a Sync payload from the current selection. The main box shows the generated artifact.");
        }

        ui.add_space(8.0);

        let visible_text = if st.mode == GatewayMode::ChangeSet {
            if st.changeset_show_result {
                st.result_payload.clone()
            } else {
                st.payload.clone()
            }
        } else {
            st.sync_payload.clone()
        };
        let has_visible_text = !visible_text.trim().is_empty();
        let has_last_changeset = !st.last_changeset_payload.trim().is_empty();

        ui.push_id("header_buttons", |ui| {
            ui.horizontal_wrapped(|ui| {
                if st.mode == GatewayMode::ChangeSet && !st.changeset_show_result {
                    if ui.button("Copy changeset schema").clicked() {
                        let schema = CHANGESET_SCHEMA_EXAMPLE.to_string();
                        ctx.output_mut(|o| o.copied_text = schema.clone());
                        st.payload = schema;
                        st.changeset_show_result = false;
                    }
                }

                if ui
                    .add_enabled(has_visible_text, egui::Button::new("Copy visible"))
                    .clicked()
                {
                    ctx.output_mut(|o| o.copied_text = visible_text.clone());
                }

                if st.mode == GatewayMode::ChangeSet && st.changeset_show_result {
                    if ui
                        .add_enabled(has_last_changeset, egui::Button::new("Copy last changeset"))
                        .clicked()
                    {
                        ctx.output_mut(|o| o.copied_text = st.last_changeset_payload.clone());
                    }
                }

                if st.mode == GatewayMode::Sync {
                    if ui.button("Generate payload").clicked() {
                        actions.push(Action::GenerateSyncPayload { applier_id });
                    }
                }

                if st.mode == GatewayMode::ChangeSet && !st.changeset_show_result {
                    let can_apply = has_repo && !st.payload.trim().is_empty();
                    if ui
                        .add_enabled(can_apply, egui::Button::new("Apply"))
                        .clicked()
                    {
                        actions.push(Action::ApplyChangeSet { applier_id });
                    }
                }

                if st.mode == GatewayMode::ChangeSet && st.changeset_show_result {
                    if ui.button("New changeset").clicked() {
                        actions.push(Action::ClearChangeSet { applier_id });
                    }
                } else if ui.button("Clear").clicked() {
                    actions.push(Action::ClearChangeSet { applier_id });
                }
            });
        });

        ui.add_space(8.0);

        let pane_w = ui.available_width().max(1.0);

        let is_changeset_result = st.mode == GatewayMode::ChangeSet && st.changeset_show_result;

        let pane_label = if st.mode == GatewayMode::ChangeSet {
            if is_changeset_result {
                "Output"
            } else {
                "ChangeSet"
            }
        } else {
            "Generated artifact"
        };

        ui.push_id("payload_pane", |ui| {
            ui.label(pane_label);
            ui.add_space(4.0);

            let pane_h = (ui.max_rect().bottom() - ui.cursor().top()).max(1.0);
            let pane_size = egui::vec2(pane_w, pane_h);
            let (pane_rect, _) = ui.allocate_exact_size(pane_size, egui::Sense::hover());

            let frame = egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(6.0));
            frame.paint(pane_rect);

            let inner_rect = frame.inner_margin.shrink_rect(pane_rect);
            let mut inner_ui = ui.child_ui(inner_rect, *ui.layout());

            egui::ScrollArea::both()
                .id_source(("changeset_applier_scroll", applier_id))
                .auto_shrink([false, false])
                .show(&mut inner_ui, |ui| {
                    if st.mode == GatewayMode::ChangeSet && !st.changeset_show_result {
                        ui.add_sized(
                            ui.available_size(),
                            egui::TextEdit::multiline(&mut st.payload)
                                .id_source(("changeset_applier_payload_text", applier_id))
                                .font(egui::TextStyle::Monospace),
                        );
                    } else if st.mode == GatewayMode::ChangeSet {
                        ui.add_sized(
                            ui.available_size(),
                            egui::TextEdit::multiline(&mut st.result_payload)
                                .id_source(("changeset_applier_payload_text", applier_id))
                                .font(egui::TextStyle::Monospace)
                                .interactive(false)
                                .desired_width(f32::INFINITY),
                        );
                    } else {
                        ui.add_sized(
                            ui.available_size(),
                            egui::TextEdit::multiline(&mut st.sync_payload)
                                .id_source(("changeset_applier_payload_text", applier_id))
                                .font(egui::TextStyle::Monospace)
                                .interactive(false)
                                .desired_width(f32::INFINITY),
                        );
                    }
                });
        });


    });

    actions
}