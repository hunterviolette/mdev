use eframe::egui;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::AppState;

pub fn sap_adt_panel(
    _ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    sap_adt_id: ComponentId,
) -> Vec<Action> {
    let mut actions = vec![];

    let Some(st) = state.sap_adts.get_mut(&sap_adt_id) else {
        ui.label("Missing SAP ADT state.");
        return actions;
    };

    ui.horizontal(|ui| {
        let color = if st.connected {
            egui::Color32::LIGHT_GREEN
        } else {
            egui::Color32::LIGHT_RED
        };
        ui.colored_label(color, "●");
        ui.label(if st.connected { "Enabled" } else { "Not connected" });

        if ui.button("Connect to SAP").clicked() {
            actions.push(Action::SapAdtConnect { sap_adt_id });
        }

        if let Some(msg) = &st.last_status {
            ui.separator();
            ui.small(msg);
        }
    });

    if let Some(err) = &st.last_error {
        ui.colored_label(egui::Color32::LIGHT_RED, err);
    }

    if let Some(url) = &st.discovery_url {
        ui.horizontal(|ui| {
            ui.small("Discovery:");
            ui.monospace(url);
        });
    }

    ui.separator();

    if !st.connected {
        return actions;
    }

    ui.horizontal(|ui| {
        ui.label("Object/Package:");
        ui.add(
            egui::TextEdit::singleline(&mut st.package_query)
                .desired_width(220.0)
                .hint_text("Z_PACKAGE"),
        );
        ui.checkbox(&mut st.include_subpackages, "Include subpackages");

        if ui.button("Load package").clicked() {
            actions.push(Action::SapAdtLoadPackage { sap_adt_id });
        }

        ui.separator();
        ui.small(format!("Objects: {}", st.package_objects.len()));

        if let Some(name) = &st.selected_object_name {
            ui.separator();
            ui.small(format!("Selected: {}", name));
        }
    });

    ui.separator();

    let available = ui.available_size();
    ui.allocate_ui_with_layout(
        available,
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.columns(2, |columns| {
                columns[0].vertical(|ui| {
                    ui.label("Package contents");
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_source(("sap_adt_package_objects_scroll", sap_adt_id))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for obj in &st.package_objects {
                                    let selected = st.selected_object_uri.as_deref() == Some(obj.uri.as_str())
                                        || st.selected_object_uri.as_deref() == obj.source_uri.as_deref();
                                    let label = format!("[{}] {}", obj.object_type, obj.name);
                                    if ui.selectable_label(selected, label).clicked() {
                                        actions.push(Action::SapAdtReadObject {
                                            sap_adt_id,
                                            object_uri: obj
                                                .source_uri
                                                .clone()
                                                .unwrap_or_else(|| obj.uri.clone()),
                                        });
                                    }

                                    if let Some(desc) = &obj.description {
                                        if !desc.trim().is_empty() {
                                            ui.small(desc);
                                        }
                                    }

                                    if let Some(package_name) = &obj.package_name {
                                        if !package_name.trim().is_empty() {
                                            ui.small(format!("Package: {}", package_name));
                                        }
                                    }

                                    ui.small(obj.source_uri.as_deref().unwrap_or(obj.uri.as_str()));
                                    ui.separator();
                                }
                            });
                    });
                });

                columns[1].vertical(|ui| {
                    ui.label("Object contents");

                    ui.horizontal_wrapped(|ui| {
                        if let Some(name) = &st.selected_object_name {
                            ui.strong(name);
                        }
                        if let Some(kind) = &st.selected_object_type {
                            ui.separator();
                            ui.small(format!("Type: {}", kind));
                        }
                        if let Some(content_type) = &st.selected_object_content_type {
                            ui.separator();
                            ui.small(format!("Content-Type: {}", content_type));
                        }
                    });

                    ui.horizontal_wrapped(|ui| {
                        ui.label("Local path:");
                        ui.add(
                            egui::TextEdit::singleline(&mut st.clone_target_path)
                                .desired_width(360.0)
                                .hint_text("sap_adt/package__type__object.abap"),
                        );
                        let can_clone = st.selected_object_uri.is_some() && !st.selected_object_content.is_empty();
                        if ui
                            .add_enabled(can_clone, egui::Button::new("Clone to worktree"))
                            .clicked()
                        {
                            actions.push(Action::SapAdtCloneSelectedToWorktree { sap_adt_id });
                        }
                    });

                    ui.horizontal_wrapped(|ui| {
                        ui.label("Transport:");
                        ui.add(
                            egui::TextEdit::singleline(&mut st.corr_nr)
                                .desired_width(220.0)
                                .hint_text("Transport"),
                        );
                    });

                    ui.horizontal_wrapped(|ui| {
                        let local_path = st.clone_target_path.trim().to_string();
                        let can_sync = !local_path.is_empty();
                        if ui
                            .add_enabled(can_sync, egui::Button::new("Push local changes"))
                            .clicked()
                        {
                            actions.push(Action::SapAdtPushWorktreeToAdt {
                                sap_adt_id,
                                path: local_path.clone(),
                            });
                        }
                        if ui
                            .add_enabled(can_sync, egui::Button::new("Activate object"))
                            .clicked()
                        {
                            actions.push(Action::SapAdtActivateWorktreeObject {
                                sap_adt_id,
                                path: local_path,
                            });
                        }
                    });

                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        egui::ScrollArea::both()
                            .id_source(("sap_adt_object_content_scroll", sap_adt_id))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut st.selected_object_content)
                                        .id_source(("sap_adt_object_content_text", sap_adt_id))
                                        .font(egui::TextStyle::Monospace)
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(32)
                                        .interactive(false),
                                );
                            });
                    });
                });
            });
        },
    );

    actions
}
