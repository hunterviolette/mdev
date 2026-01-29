// src/app/ui/top_bar.rs
use eframe::egui;

use super::super::actions::Action;
use super::super::state::{AppState, WORKTREE_REF};

pub fn top_bar(_ctx: &egui::Context, ui: &mut egui::Ui, state: &mut AppState) -> Vec<Action> {
    let mut actions = vec![];

    ui.horizontal(|ui| {
        // ----- Repository picker -----
        {
            let label = "Select repository";
            let font_id = ui.style().text_styles[&egui::TextStyle::Button].clone();
            let text_w = ui
                .fonts(|f| f.layout_no_wrap(label.to_owned(), font_id, ui.visuals().text_color()).size().x);

            let w = (text_w + 18.0).ceil();
            let h = ui.spacing().interact_size.y;

            if ui
                .add_sized([w, h], egui::Button::new(label))
                .clicked()
            {
                actions.push(Action::PickRepo);
            }
        }

        ui.separator();

        // ----- Ref dropdown -----
        ui.label("Ref:");

        let has_opts = !state.inputs.git_ref_options.is_empty();
        if has_opts {
            // Display "Working tree" when WORKTREE_REF is selected
            let selected_label = if state.inputs.git_ref == WORKTREE_REF {
                "Working tree".to_string()
            } else {
                state.inputs.git_ref.clone()
            };

            egui::ComboBox::from_id_source("git_ref_combo")
                .selected_text(selected_label)
                .width(220.0)
                .show_ui(ui, |ui| {
                    for r in state.inputs.git_ref_options.iter() {
                        let label = if r == WORKTREE_REF { "Working tree" } else { r.as_str() };

                        if ui.selectable_label(&state.inputs.git_ref == r, label).clicked() {
                            actions.push(Action::SetGitRef(r.clone()));
                        }
                    }
                });
        } else {
            ui.text_edit_singleline(&mut state.inputs.git_ref);
        }

        if ui.button("↻").clicked() {
            actions.push(Action::RefreshGitRefs);
        }

        ui.separator();

        // ----- Command palette -----
        if ui.button("Command (Ctrl+Shift+E)").clicked() {
            actions.push(Action::ToggleCommandPalette);
        }
    });

    actions
}
