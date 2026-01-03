use eframe::egui;

use super::super::actions::{Action, ComponentId};
use super::super::state::{AppState, ContextExportMode};

pub fn context_exporter(
    ui: &mut egui::Ui,
    state: &mut AppState,
    exporter_id: ComponentId,
) -> Vec<Action> {
    let mut actions = vec![];

    let Some(ex) = state.context_exporters.get_mut(&exporter_id) else {
        ui.label("Missing context exporter state.");
        return actions;
    };

    ui.heading("AI Context Export");
    ui.add_space(6.0);

    if state.inputs.repo.is_none() {
        ui.colored_label(egui::Color32::LIGHT_RED, "No repo selected. Pick a repo first.");
        return actions;
    }

    ui.label("Exports tracked file tree + file contents (at the current git ref) into one text file.");
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.label("Git ref:");
        ui.monospace(&state.inputs.git_ref);
    });

    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label("Save to:");
        let path_txt = ex
            .save_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(not set)".to_string());
        ui.monospace(path_txt);

        if ui.button("Choose…").clicked() {
            actions.push(Action::ContextPickSavePath { exporter_id });
        }
    });

    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.label("Max bytes/file:");
        let mut s = ex.max_bytes_per_file.to_string();
        if ui.text_edit_singleline(&mut s).changed() {
            if let Ok(v) = s.trim().parse::<usize>() {
                actions.push(Action::ContextSetMaxBytes { exporter_id, max: v });
            }
        }

        let mut skip = ex.skip_binary;
        if ui.checkbox(&mut skip, "Skip binary").clicked() {
            actions.push(Action::ContextToggleSkipBinary { exporter_id });
        }
    });

    ui.add_space(10.0);

    ui.horizontal(|ui| {
        ui.label("Mode:");
        let cur = match ex.mode {
            ContextExportMode::EntireRepo => "ENTIRE REPO",
            ContextExportMode::TreeSelect => "TREE SELECT",
        };

        egui::ComboBox::from_id_source(("context_mode", exporter_id))
            .selected_text(cur)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(ex.mode == ContextExportMode::EntireRepo, "ENTIRE REPO")
                    .clicked()
                {
                    ex.mode = ContextExportMode::EntireRepo;
                }
                if ui
                    .selectable_label(ex.mode == ContextExportMode::TreeSelect, "TREE SELECT")
                    .clicked()
                {
                    ex.mode = ContextExportMode::TreeSelect;
                }
            });
    });

    ui.add_space(10.0);

    let can_generate = ex.save_path.is_some();
    if ui
        .add_enabled(can_generate, egui::Button::new("Generate context file"))
        .clicked()
    {
        actions.push(Action::ContextGenerate { exporter_id });
    }
    if !can_generate {
        ui.add_space(4.0);
        ui.label("Choose an output path to enable generation.");
    }

    if let Some(msg) = &ex.status {
        ui.add_space(10.0);
        ui.separator();
        ui.label(msg);
    }

    actions
}
