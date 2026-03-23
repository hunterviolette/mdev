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
        ui.label("Package or object:");
        ui.add(
            egui::TextEdit::singleline(&mut st.package_query)
                .desired_width(220.0)
                .hint_text("Z_PACKAGE"),
        );
        ui.checkbox(&mut st.include_subpackages, "Include subpackages");

        if ui.button("Browse").clicked() {
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
                    ui.label("Package tree / search results");
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_source(("sap_adt_package_objects_scroll", sap_adt_id))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for obj in &st.package_objects {
                                    let selected = st.selected_object_name.as_deref() == Some(obj.name.as_str())
                                        && st.selected_object_type.as_deref() == Some(obj.object_type.as_str());
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
                        if let Some(manifest) = &st.selected_manifest {
                            if let Some(resource) = manifest.selected_resource(st.selected_resource_id.as_deref()) {
                                if let Some(content_type) = &resource.content_type {
                                    ui.separator();
                                    ui.small(format!("Content-Type: {}", content_type));
                                }
                            }
                        }
                    });

                    ui.horizontal_wrapped(|ui| {
                        ui.label("Local path:");
                        ui.add(
                            egui::TextEdit::singleline(&mut st.clone_target_path)
                                .desired_width(360.0)
                                .hint_text("sap_adt/package__type__object"),
                        );
                        let can_clone = st.selected_manifest.is_some();
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

                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut st.debug_http_enabled, "HTTP debug");
                        let debug_uri = st
                            .selected_object_metadata_uri
                            .clone()
                            .or_else(|| st.selected_object_uri.clone())
                            .unwrap_or_default();
                        let can_debug = !debug_uri.trim().is_empty();
                        if ui
                            .add_enabled(can_debug, egui::Button::new("Run Accept matrix"))
                            .clicked()
                        {
                            actions.push(Action::SapAdtDebugAcceptMatrix {
                                sap_adt_id,
                                object_uri: debug_uri.clone(),
                            });
                        }
                        if ui.button("Clear debug").clicked() {
                            actions.push(Action::SapAdtClearHttpTrace { sap_adt_id });
                        }
                    });

                    if !st.accept_probe_results.is_empty() {
                        ui.separator();
                        ui.label("Accept matrix");
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_source(("sap_adt_accept_matrix_scroll", sap_adt_id))
                                .auto_shrink([false, false])
                                .max_height(180.0)
                                .show(ui, |ui| {
                                    for probe in st.accept_probe_results.iter() {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.monospace(format!("Accept: {}", probe.accept));
                                            if let Some(status) = probe.status {
                                                ui.separator();
                                                ui.small(format!("Status: {}", status));
                                            }
                                            if let Some(content_type) = &probe.content_type {
                                                ui.separator();
                                                ui.small(format!("Content-Type: {}", content_type));
                                            }
                                        });
                                        if let Some(error) = &probe.error {
                                            let mut text = error.clone();
                                            ui.add(
                                                egui::TextEdit::multiline(&mut text)
                                                    .font(egui::TextStyle::Monospace)
                                                    .desired_width(f32::INFINITY)
                                                    .desired_rows(3)
                                                    .interactive(false),
                                            );
                                        } else if !probe.response_preview.is_empty() {
                                            let mut text = probe.response_preview.clone();
                                            ui.add(
                                                egui::TextEdit::multiline(&mut text)
                                                    .font(egui::TextStyle::Monospace)
                                                    .desired_width(f32::INFINITY)
                                                    .desired_rows(4)
                                                    .interactive(false),
                                            );
                                        }
                                        ui.separator();
                                    }
                                });
                        });
                    }

                    if let Some(trace) = st.last_http_trace.as_ref() {
                        ui.separator();
                        ui.label("Last ADT HTTP trace");
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.small(format!("Label: {}", trace.label));
                                ui.separator();
                                ui.small(format!("Method: {}", trace.method));
                                if let Some(status) = trace.response_status {
                                    ui.separator();
                                    ui.small(format!("Status: {}", status));
                                }
                            });
                            let mut url = trace.url.clone();
                            ui.add(
                                egui::TextEdit::singleline(&mut url)
                                    .desired_width(f32::INFINITY)
                                    .interactive(false),
                            );

                            let mut request_headers = trace
                                .request_headers
                                .iter()
                                .map(|(k, v)| format!("{}: {}", k, v))
                                .collect::<Vec<_>>()
                                .join("\n");
                            ui.label("Request headers");
                            ui.add(
                                egui::TextEdit::multiline(&mut request_headers)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(4)
                                    .interactive(false),
                            );

                            let mut response_headers = trace
                                .response_headers
                                .iter()
                                .map(|(k, v)| format!("{}: {}", k, v))
                                .collect::<Vec<_>>()
                                .join("\n");
                            ui.label("Response headers");
                            ui.add(
                                egui::TextEdit::multiline(&mut response_headers)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(4)
                                    .interactive(false),
                            );

                            if !trace.request_body.is_empty() {
                                let mut request_body = trace.request_body.clone();
                                ui.label("Request body");
                                ui.add(
                                    egui::TextEdit::multiline(&mut request_body)
                                        .font(egui::TextStyle::Monospace)
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(6)
                                        .interactive(false),
                                );
                            }

                            if let Some(error) = &trace.error {
                                let mut error_text = error.clone();
                                ui.label("Error");
                                ui.add(
                                    egui::TextEdit::multiline(&mut error_text)
                                        .font(egui::TextStyle::Monospace)
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(6)
                                        .interactive(false),
                                );
                            } else {
                                let mut response_body = trace.response_body.clone();
                                ui.label("Response body");
                                ui.add(
                                    egui::TextEdit::multiline(&mut response_body)
                                        .font(egui::TextStyle::Monospace)
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(8)
                                        .interactive(false),
                                );
                            }
                        });
                    }

                    if let Some(manifest) = st.selected_manifest.as_ref() {
                        ui.horizontal_wrapped(|ui| {
                            ui.small(format!("Metadata: {}", manifest.metadata_uri));
                            ui.separator();
                            ui.small(format!("Resources: {}", manifest.resources.len()));
                            ui.separator();
                            ui.small(format!("Documents: {}", manifest.documents.len()));
                        });

                        ui.columns(2, |columns| {
                            columns[0].vertical(|ui| {
                                ui.label("Discovered resources");
                                egui::Frame::group(ui.style()).show(ui, |ui| {
                                    egui::ScrollArea::vertical()
                                        .id_source(("sap_adt_resource_list_scroll", sap_adt_id))
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            for resource in manifest.resources.iter() {
                                                let selected = st.selected_resource_id.as_deref() == Some(resource.id.as_str());
                                                let label = format!("{} [{}{}{}]", resource.path, resource.role, if resource.editable { ", editable" } else { "" }, if resource.readable { ", readable" } else { "" });
                                                if ui.selectable_label(selected, label).clicked() {
                                                    st.selected_resource_id = Some(resource.id.clone());
                                                }
                                            }
                                        });
                                });
                            });

                            columns[1].vertical(|ui| {
                                ui.label("Resource content");
                                let mut body = manifest
                                    .selected_resource(st.selected_resource_id.as_deref())
                                    .map(|r| r.body.clone())
                                    .unwrap_or_default();
                                egui::Frame::group(ui.style()).show(ui, |ui| {
                                    egui::ScrollArea::both()
                                        .id_source(("sap_adt_object_content_scroll", sap_adt_id))
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            ui.add(
                                                egui::TextEdit::multiline(&mut body)
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
                    } else {
                        ui.label("No SAP ADT manifest loaded.");
                    }
                });
            });
        },
    );

    actions
}
