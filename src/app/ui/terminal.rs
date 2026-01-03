use eframe::egui;

use super::super::actions::{Action, ComponentId, TerminalShell};
use super::super::state::AppState;

fn shell_label(s: &TerminalShell) -> &'static str {
    match s {
        TerminalShell::Auto => "Auto",
        TerminalShell::PowerShell => "PowerShell",
        TerminalShell::Cmd => "cmd",
        TerminalShell::Bash => "bash",
        TerminalShell::Zsh => "zsh",
        TerminalShell::Sh => "sh",
    }
}

pub fn terminal_panel(
    _ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    terminal_id: ComponentId,
) -> Vec<Action> {
    let mut actions = vec![];

    // Snapshot small values without holding a mutable borrow across UI closures.
    let (current_shell, last_status) = match state.terminals.get(&terminal_id) {
        Some(t) => (t.shell.clone(), t.last_status),
        None => {
            ui.label("Missing terminal state.");
            ui.label("Tip: close this window and re-add a Terminal via the command palette.");
            return actions;
        }
    };

    // Header row
    ui.horizontal(|ui| {
        ui.label("Shell:");

        egui::ComboBox::from_id_source(("terminal_shell", terminal_id))
            .selected_text(shell_label(&current_shell))
            .show_ui(ui, |ui| {
                let shells = [
                    TerminalShell::Auto,
                    TerminalShell::PowerShell,
                    TerminalShell::Cmd,
                    TerminalShell::Bash,
                    TerminalShell::Zsh,
                    TerminalShell::Sh,
                ];

                for s in shells {
                    let selected = current_shell == s;
                    if ui.selectable_label(selected, shell_label(&s)).clicked() {
                        actions.push(Action::SetTerminalShell {
                            terminal_id,
                            shell: s,
                        });
                    }
                }
            });

        ui.separator();

        if ui.button("Clear").clicked() {
            actions.push(Action::ClearTerminal { terminal_id });
        }

        if let Some(code) = last_status {
            ui.separator();
            ui.label(format!("last exit: {}", code));
        }
    });

    ui.add_space(6.0);

    // Keep input row visible (don’t let output eat all height).
    let input_row_h = 34.0;
    let output_h = (ui.available_height() - input_row_h).max(120.0);

    // Use current window width (NOT infinity) to avoid horizontal layout weirdness.
    let full_w = ui.available_width().max(10.0);

    // Output area
    {
        let Some(t) = state.terminals.get_mut(&terminal_id) else {
            return actions;
        };

        let (rect, _) = ui.allocate_exact_size(egui::vec2(full_w, output_h), egui::Sense::hover());
        ui.allocate_ui_at_rect(rect, |ui| {
            ui.set_min_size(egui::vec2(full_w, output_h));

            egui::ScrollArea::both()
                .id_source(("terminal_output_scroll", terminal_id))
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    let w = ui.available_width().max(10.0);

                    ui.add(
                        egui::TextEdit::multiline(&mut t.output)
                            .desired_width(w)
                            .font(egui::TextStyle::Monospace)
                            .interactive(false),
                    );
                });
        });
    }

    ui.add_space(6.0);

    // Input row (Enter support)
    let mut fire = false;
    let mut cmd_to_run: Option<String> = None;

    ui.horizontal(|ui| {
        let Some(t) = state.terminals.get_mut(&terminal_id) else {
            ui.label("Missing terminal state.");
            return;
        };

        let run_w = 52.0;
        let input_w = (ui.available_width() - run_w - 8.0).max(50.0);

        let resp = ui.add(
            egui::TextEdit::singleline(&mut t.input)
                .desired_width(input_w)
                .hint_text("type a command and press Enter")
                .font(egui::TextStyle::Monospace),
        );

        // When Enter is pressed, the TextEdit loses focus.
        let enter_pressed = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        if ui.button("Run").clicked() || enter_pressed {
            let cmd = t.input.trim().to_string();
            if !cmd.is_empty() {
                cmd_to_run = Some(cmd);
                fire = true;
                t.input.clear();
            }
            resp.request_focus();
        }
    });

    if fire {
        if let Some(cmd) = cmd_to_run {
            actions.push(Action::RunTerminalCommand { terminal_id, cmd });
        }
    }

    actions
}
