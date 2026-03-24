use eframe::egui;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::{AppState, SapAdtExportRow, SapAdtObjectOperationRow};

fn lookup<'a>(rows: &'a [SapAdtObjectOperationRow], key: &str) -> (&'a str, &'a str) {
    if let Some(row) = rows.iter().rev().find(|row| row.action == "export" && row.key == key) {
        return (row.state.as_str(), row.message.as_str());
    }
    ("idle", "")
}

fn fallback_state(row: &SapAdtExportRow) -> (&str, &str) {
    if row.activation_ok {
        return ("activated", row.message.as_str());
    }
    if row.syntax_ok || row.pushed_files > 0 {
        return ("saved", row.message.as_str());
    }
    if !row.message.trim().is_empty() {
        return ("error", row.message.as_str());
    }
    if row.changed_files > 0 {
        return ("changed", "");
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

pub fn sap_adt_export_panel(
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
            ui.label("Transport");
            ui.add_sized([140.0, 24.0], egui::TextEdit::singleline(&mut st.corr_nr));
            if ui.button("Scan").clicked() {
                actions.push(Action::SapAdtScanExportObjects { sap_adt_id });
            }
            if ui.button("Export selected").clicked() {
                actions.push(Action::SapAdtExportSelectedWorktreeObjects { sap_adt_id });
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        egui::Grid::new(("sap_adt_export_grid", sap_adt_id))
            .striped(true)
            .min_col_width(72.0)
            .show(ui, |ui| {
                ui.strong("");
                ui.strong("Object");
                ui.strong("Type");
                ui.strong("Changed");
                ui.strong("Pushed");
                ui.strong("Status");
                ui.end_row();

                for row in st.export_results_scan.iter() {
                    let mut selected = st.export_selected_manifest_paths.contains(&row.manifest_path);
                    if ui.checkbox(&mut selected, "").clicked() {
                        actions.push(Action::SapAdtToggleExportManifestSelection {
                            sap_adt_id,
                            manifest_path: row.manifest_path.clone(),
                        });
                    }

                    ui.label(&row.object_name);
                    ui.label(&row.object_type);
                    ui.label(row.changed_files.to_string());
                    ui.label(row.pushed_files.to_string());

                    let (status_text, hover_message) = {
                        let from_activity = lookup(&st.object_operations, &row.manifest_path);
                        if from_activity.0 == "idle" {
                            fallback_state(row)
                        } else {
                            from_activity
                        }
                    };

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
