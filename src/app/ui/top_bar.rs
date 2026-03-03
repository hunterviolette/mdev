use eframe::egui;

use super::super::actions::Action;
use super::super::state::{AppState, WORKTREE_REF};

fn canvas_shortcut_label(index: usize) -> &'static str {
    match index {
        0 => "Ctrl+1",
        1 => "Ctrl+2",
        2 => "Ctrl+3",
        3 => "Ctrl+4",
        4 => "Ctrl+5",
        5 => "Ctrl+6",
        6 => "Ctrl+7",
        7 => "Ctrl+8",
        8 => "Ctrl+9",
        9 => "Ctrl+0",
        _ => "Ctrl+?",
    }
}

fn canvas_number_label(index: usize) -> &'static str {
    match index {
        0 => "1",
        1 => "2",
        2 => "3",
        3 => "4",
        4 => "5",
        5 => "6",
        6 => "7",
        7 => "8",
        8 => "9",
        9 => "0",
        _ => "?",
    }
}


pub fn top_bar(_ctx: &egui::Context, ui: &mut egui::Ui, state: &mut AppState) -> Vec<Action> {
    let mut actions = vec![];

    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            {
                let label = "Select repository";
                let font_id = ui.style().text_styles[&egui::TextStyle::Button].clone();
                let text_w = ui
                    .fonts(|f| f.layout_no_wrap(label.to_owned(), font_id, ui.visuals().text_color()).size().x);

                let w = (text_w + 18.0).ceil();
                let h = ui.spacing().interact_size.y;

                if ui.add_sized([w, h], egui::Button::new(label)).clicked() {
                    actions.push(Action::PickRepo);
                }
            }

            ui.separator();

            ui.label("Ref:");

            let has_opts = !state.inputs.git_ref_options.is_empty();
            if has_opts {
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

            if ui.button("Command (Ctrl+Shift+E)").clicked() {
                actions.push(Action::ToggleCommandPalette);
            }
        });

        {
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.label("Canvases:");

                if ui.button("+").on_hover_text("Add canvas").clicked() {
                    actions.push(Action::CanvasAdd);
                }

                ui.add_space(8.0);

                egui::ScrollArea::horizontal()
                    .id_source("canvas_tabs_scroll")
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for i in 0..state.canvases.len() {
                            let is_active = state.active_canvas == i;
                            let is_renaming = state.ui.canvas_rename_index == Some(i);
                            let mut switched = false;

                            if i > 0 {
                                ui.add_space(6.0);
                                ui.separator();
                                ui.add_space(6.0);
                            }

                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    let title = format!("{} · {}", canvas_number_label(i), state.canvases[i].name);

                                    let resp = ui
                                        .selectable_label(is_active, egui::RichText::new(title).strong())
                                        .on_hover_text(format!(
                                            "Left-click to select\nRight-click for actions\nShortcut: {}",
                                            canvas_shortcut_label(i)
                                        ));

                                    if resp.clicked() {
                                        actions.push(Action::CanvasSelect { index: i });
                                        switched = true;
                                    }

                                    resp.context_menu(|ui| {
                                        if ui.button("New canvas").clicked() {
                                            actions.push(Action::CanvasAdd);
                                            ui.close_menu();
                                        }

                                        if ui.button("Rename").clicked() {
                                            state.ui.canvas_rename_index = Some(i);
                                            state.ui.canvas_rename_draft = state.canvases[i].name.clone();
                                            ui.close_menu();
                                        }

                                        let can_delete = state.canvases.len() > 1;
                                        if ui
                                            .add_enabled(can_delete, egui::Button::new("Delete"))
                                            .on_hover_text(if can_delete {
                                                "Delete this canvas"
                                            } else {
                                                "Cannot delete the last canvas"
                                            })
                                            .clicked()
                                        {
                                            actions.push(Action::CanvasDelete { index: i });
                                            ui.close_menu();
                                        }
                                    });
                                });

                                if is_renaming {
                                    ui.add_space(4.0);

                                    let resp = ui.add(
                                        egui::TextEdit::singleline(&mut state.ui.canvas_rename_draft)
                                            .desired_width(220.0)
                                            .hint_text("Canvas name"),
                                    );

                                    let enter_pressed = ui.input(|inp| inp.key_pressed(egui::Key::Enter));
                                    let esc_pressed = ui.input(|inp| inp.key_pressed(egui::Key::Escape));
                                    let lost_focus = resp.lost_focus();

                                    if esc_pressed {
                                        state.ui.canvas_rename_index = None;
                                        state.ui.canvas_rename_draft.clear();
                                    } else if enter_pressed || lost_focus {
                                        let name = state.ui.canvas_rename_draft.trim().to_string();
                                        if !name.is_empty() {
                                            actions.push(Action::CanvasRename { index: i, name });
                                        }
                                        state.ui.canvas_rename_index = None;
                                        state.ui.canvas_rename_draft.clear();
                                    }
                                }
                            });

                            if switched && state.ui.canvas_rename_index.is_some() {
                                state.ui.canvas_rename_index = None;
                                state.ui.canvas_rename_draft.clear();
                            }
                        }
                    });
            });
        }
    });

    actions
}
