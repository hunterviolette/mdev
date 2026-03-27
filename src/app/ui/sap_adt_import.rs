use eframe::egui;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::{AppState, SapAdtObjectOperationRow};

fn lookup<'a>(rows: &'a [SapAdtObjectOperationRow], key: &str) -> (&'a str, &'a str) {
    if let Some(row) = rows.iter().rev().find(|row| row.action == "import" && row.key == key) {
        return (row.state.as_str(), row.message.as_str());
    }
    ("idle", "")
}

fn status_color(state: &str) -> egui::Color32 {
    match state {
        "error" | "syntax_error" => egui::Color32::LIGHT_RED,
        "imported" | "exported" | "activated" | "saved" | "ok" => egui::Color32::LIGHT_GREEN,
        "running" | "queued" | "reading" | "saving" | "activating" | "importing" | "exporting" => egui::Color32::LIGHT_BLUE,
        _ => egui::Color32::LIGHT_GRAY,
    }
}

pub fn sap_adt_import_panel(
    _ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    sap_adt_id: ComponentId,
) -> Vec<Action> {
    let mut actions = Vec::new();

    let Some(st) = state.sap_adts.get_mut(&sap_adt_id) else {
        ui.label("Missing SAP ADT state.");
        return actions;
    };

    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Package");
            ui.add_sized([180.0, 24.0], egui::TextEdit::singleline(&mut st.package_query));
            ui.checkbox(&mut st.include_subpackages, "Include subpackages");
            ui.checkbox(&mut st.import_include_xml_artifacts, "Include XML artifacts");
            if ui.button("Load").clicked() {
                actions.push(Action::SapAdtLoadPackage { sap_adt_id });
            }
            let import_enabled = !st.import_job.is_pending();
            if ui.add_enabled(import_enabled, egui::Button::new("Import selected")).clicked() {
                actions.push(Action::SapAdtImportSelectedPackageObjects { sap_adt_id });
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        egui::Grid::new(("sap_adt_import_grid", sap_adt_id))
            .striped(true)
            .min_col_width(80.0)
            .show(ui, |ui| {
                ui.strong("");
                ui.strong("Object");
                ui.strong("Type");
                ui.strong("Status");
                ui.end_row();

                for object in st.package_objects.iter() {
                    let mut selected = st.import_selected_object_uris.contains(&object.uri);
                    if ui.checkbox(&mut selected, "").clicked() {
                        actions.push(Action::SapAdtToggleImportObjectSelection {
                            sap_adt_id,
                            object_uri: object.uri.clone(),
                        });
                    }

                    ui.label(&object.name);
                    ui.label(&object.object_type);
                    let (status_text, hover_message) = lookup(&st.object_operations, &object.uri);
                    let status = ui.colored_label(status_color(status_text), status_text);
                    if !hover_message.trim().is_empty() {
                        status.on_hover_text(hover_message);
                    }
                    ui.end_row();
                }
            });
    });

    actions
}
