use eframe::egui;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::{AppState, SapAdtLogEntry};

fn popup_window_frame<R>(
    ctx: &egui::Context,
    title: &str,
    id_suffix: &str,
    open: &mut bool,
    mut add_contents: impl FnMut(&mut egui::Ui) -> R,
) {
    let mut is_open = *open;

    egui::Window::new(title)
        .id(egui::Id::new(("sap_adt_popup", id_suffix)))
        .open(&mut is_open)
        .collapsible(false)
        .resizable(true)
        .default_width(720.0)
        .default_height(360.0)
        .min_width(320.0)
        .min_height(180.0)
        .show(ctx, |ui| {
            egui::ScrollArea::both()
                .id_source(("sap_adt_popup_scroll", id_suffix))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let _ = add_contents(ui);
                });
        });

    *open = is_open;
}

fn status_color(state: &str) -> egui::Color32 {
    match state {
        "error" | "syntax_error" => egui::Color32::LIGHT_RED,
        "imported" | "exported" | "activated" | "saved" | "ok" => egui::Color32::LIGHT_GREEN,
        "running" | "queued" | "reading" | "saving" | "activating" | "importing" | "exporting" => egui::Color32::LIGHT_BLUE,
        _ => egui::Color32::LIGHT_GRAY,
    }
}

fn render_log_row(ui: &mut egui::Ui, row: &SapAdtLogEntry) {
    ui.horizontal_wrapped(|ui| {
        ui.label(&row.object_name);
        if !row.object_type.trim().is_empty() {
            ui.separator();
            ui.small(&row.object_type);
        }
        if !row.action.trim().is_empty() {
            ui.separator();
            ui.small(&row.action);
        }
        ui.separator();
        let status = ui.colored_label(status_color(&row.state), &row.state);
        if !row.message.trim().is_empty() {
            status.on_hover_text(&row.message);
        }
    });
}

fn render_import_popup(
    ctx: &egui::Context,
    state: &mut AppState,
    sap_adt_id: ComponentId,
    actions: &mut Vec<Action>,
) {
    let open_now = state.sap_adts.get(&sap_adt_id).map(|st| st.import_popup_open).unwrap_or(false);
    if !open_now {
        return;
    }

    let mut open = open_now;
    popup_window_frame(ctx, "SAP ADT Import", "import", &mut open, |ui| {
        actions.extend(crate::app::ui::sap_adt_import::sap_adt_import_panel(ctx, ui, state, sap_adt_id));
    });

    if !open {
        actions.push(Action::CloseSapAdtImportPopup { sap_adt_id });
    }
}

fn render_export_popup(
    ctx: &egui::Context,
    state: &mut AppState,
    sap_adt_id: ComponentId,
    actions: &mut Vec<Action>,
) {
    let open_now = state.sap_adts.get(&sap_adt_id).map(|st| st.export_popup_open).unwrap_or(false);
    if !open_now {
        return;
    }

    let mut open = open_now;
    popup_window_frame(ctx, "SAP ADT Export", "export", &mut open, |ui| {
        actions.extend(crate::app::ui::sap_adt_export::sap_adt_export_panel(ctx, ui, state, sap_adt_id));
    });

    if !open {
        actions.push(Action::CloseSapAdtExportPopup { sap_adt_id });
    }
}

fn render_logs_popup(
    ctx: &egui::Context,
    state: &mut AppState,
    sap_adt_id: ComponentId,
    actions: &mut Vec<Action>,
) {
    let open_now = state.sap_adts.get(&sap_adt_id).map(|st| st.logs_popup_open).unwrap_or(false);
    if !open_now {
        return;
    }

    let mut open = open_now;
    popup_window_frame(ctx, "SAP ADT Logs", "logs", &mut open, |ui| {
        let Some(st) = state.sap_adts.get(&sap_adt_id) else {
            ui.label("Missing SAP ADT state.");
            return;
        };

        if st.logs.is_empty() {
            ui.label("No object activity yet.");
        } else {
            for row in st.logs.iter().rev() {
                render_log_row(ui, row);
                ui.separator();
            }
        }
    });

    if !open {
        actions.push(Action::CloseSapAdtLogsPopup { sap_adt_id });
    }
}

pub fn sap_adt_panel(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    sap_adt_id: ComponentId,
) -> Vec<Action> {
    let mut actions = vec![];

    let st = state
        .sap_adts
        .entry(sap_adt_id)
        .or_insert_with(crate::app::state::SapAdtState::new);

    ui.horizontal(|ui| {
        let color = if st.connected { egui::Color32::LIGHT_GREEN } else { egui::Color32::LIGHT_RED };
        ui.colored_label(color, "◻");
        ui.label(if st.connected { "Connected" } else { "Disconnected" });
    });

    ui.separator();

    if st.connected {
        ui.horizontal(|ui| {
            if ui.button("Import...").clicked() {
                actions.push(Action::OpenSapAdtImportPopup { sap_adt_id });
            }
            if ui.button("Export...").clicked() {
                actions.push(Action::OpenSapAdtExportPopup { sap_adt_id });
            }
            if ui.button("Logs...").clicked() {
                actions.push(Action::OpenSapAdtLogsPopup { sap_adt_id });
            }
        });
    } else if ui.button("Connect to SAP").clicked() {
        actions.push(Action::SapAdtConnect { sap_adt_id });
    }

    drop(st);

    render_import_popup(ctx, state, sap_adt_id, &mut actions);
    render_export_popup(ctx, state, sap_adt_id, &mut actions);
    render_logs_popup(ctx, state, sap_adt_id, &mut actions);

    actions
}
