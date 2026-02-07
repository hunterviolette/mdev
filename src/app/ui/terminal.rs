use eframe::egui;
use std::sync::mpsc::TryRecvError;
use egui::Key;

use super::super::actions::{Action, ComponentId, TerminalShell};
use super::super::state::AppState;

fn prompt_text(shell: &TerminalShell, cwd: &Option<std::path::PathBuf>) -> String {
    match (shell, cwd) {
        (TerminalShell::PowerShell, Some(dir)) => format!("PS {}> ", dir.display()),
        (TerminalShell::Cmd, Some(dir)) => format!("{}> ", dir.display()),
        (_, Some(dir)) => format!("{}$ ", dir.display()),
        (TerminalShell::PowerShell, None) => "PS > ".to_string(),
        (TerminalShell::Cmd, None) => "> ".to_string(),
        (_, None) => "$ ".to_string(),
    }
}

fn push_bounded(out: &mut String, s: &str, max_bytes: usize) {
    out.push_str(s);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    // Keep only the tail to avoid UI blowups on huge command output.
    if out.len() > max_bytes {
        let keep = max_bytes;
        let start = out.len().saturating_sub(keep);
        // Try to start on a char boundary.
        let mut start2 = start;
        while start2 < out.len() && !out.is_char_boundary(start2) {
            start2 += 1;
        }
        *out = out[start2..].to_string();
    }
}


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

        const MAX_OUT_BYTES: usize = 2 * 1024 * 1024; // 2MB tail
        let mut finished = false;
        if let Some(rx) = t.pending_rx.as_ref() {
            loop {
                match rx.try_recv() {
                    Ok(ev) => match ev {
                        crate::app::state::TerminalEvent::Stdout(s) => {
                            push_bounded(&mut t.output, &s, MAX_OUT_BYTES);
                        }
                        crate::app::state::TerminalEvent::Stderr(s) => {
                            push_bounded(&mut t.output, &s, MAX_OUT_BYTES);
                        }
                        crate::app::state::TerminalEvent::Error(s) => {
                            push_bounded(&mut t.output, &s, MAX_OUT_BYTES);
                        }
                        crate::app::state::TerminalEvent::Exit(code) => {
                            t.last_status = Some(code);
                            push_bounded(&mut t.output, &format!("[exit: {}]", code), MAX_OUT_BYTES);
                            finished = true;
                        }
                    },
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
        }

        if finished {
            t.running = false;
            t.pending_rx = None;
            t.child = None;
        }

        let (rect, _) = ui.allocate_exact_size(egui::vec2(full_w, output_h), egui::Sense::hover());
        ui.allocate_ui_at_rect(rect, |ui| {
            ui.set_min_size(egui::vec2(full_w, output_h));

            ui.set_width(full_w);
            ui.set_max_width(full_w);

            egui::ScrollArea::both()
                .id_source(("terminal_output_scroll", terminal_id))
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {

                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(&t.output).monospace())
                                .selectable(true)
                                .wrap(false),
                        );
                    });
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

        let ctrl_c = ui.input(|i| i.modifiers.ctrl && i.key_pressed(Key::C));
        if ctrl_c && t.running {
            actions.push(Action::InterruptTerminal { terminal_id });
        }

        let prompt = prompt_text(&t.shell, &t.cwd);
        ui.label(egui::RichText::new(prompt).monospace());

        let run_w = 72.0;
        let stop_w = 72.0;
        let input_w = (ui.available_width() - run_w - stop_w - 16.0).max(50.0);

        let resp = ui.add(
            egui::TextEdit::singleline(&mut t.input)
                .desired_width(input_w)
                .hint_text("type a command and press Enter")
                .font(egui::TextStyle::Monospace),
        );

        // When Enter is pressed, the TextEdit loses focus.
        let enter_pressed = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        let run_clicked = ui
            .add_enabled(!t.running, egui::Button::new("Run"))
            .clicked();

        if run_clicked || (!t.running && enter_pressed) {
            let cmd = t.input.trim().to_string();
            if !cmd.is_empty() {
                cmd_to_run = Some(cmd);
                fire = true;
                t.input.clear();
            }
            resp.request_focus();
        }

        let stop_clicked = ui
            .add_enabled(t.running, egui::Button::new("Stop"))
            .clicked();
        if stop_clicked {
            actions.push(Action::InterruptTerminal { terminal_id });
        }

        if t.running {
            ui.add_space(6.0);
            ui.label("running...");
        }
    });

    if fire {
        if let Some(cmd) = cmd_to_run {
            actions.push(Action::RunTerminalCommand { terminal_id, cmd });
        }
    }

    actions
}
