use eframe::egui;
use egui::text::LayoutJob;
use egui::{Color32, FontId, Key, TextFormat};

use super::super::actions::{Action, ComponentId, TerminalShell};
use super::super::state::AppState;
use super::super::state::TerminalEvent;

fn is_scroll_input(ui: &egui::Ui) -> bool {
    ui.input(|i| {
        let a = i.raw_scroll_delta;
        let b = i.smooth_scroll_delta;
        a != egui::Vec2::ZERO || b != egui::Vec2::ZERO
    })
}


fn calc_rows_cols(ui: &egui::Ui, rect: egui::Rect) -> (u16, u16) {
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let (row_h, col_w) = ui.fonts(|f| {
        let rh = f.row_height(&font_id);
        let cw = f.glyph_width(&font_id, 'W');
        (rh.max(1.0), cw.max(1.0))
    });

    let rows = (rect.height() / row_h).floor().max(2.0) as u16;
    let cols = (rect.width() / col_w).floor().max(10.0) as u16;
    (rows, cols)
}

fn key_to_ansi(key: egui::Key) -> Option<&'static [u8]> {
    match key {
        egui::Key::ArrowUp => Some(b"\x1b[A"),
        egui::Key::ArrowDown => Some(b"\x1b[B"),
        egui::Key::ArrowRight => Some(b"\x1b[C"),
        egui::Key::ArrowLeft => Some(b"\x1b[D"),
        egui::Key::Home => Some(b"\x1b[H"),
        egui::Key::End => Some(b"\x1b[F"),
        egui::Key::PageUp => Some(b"\x1b[5~"),
        egui::Key::PageDown => Some(b"\x1b[6~"),
        _ => None,
    }
}

fn vt_layout_with_cursor(
    ui: &egui::Ui,
    text: &str,
    cols: u16,
    cursor_row: u16,
    cursor_col: u16,
) -> LayoutJob {
    let font_id: FontId = egui::TextStyle::Monospace.resolve(ui.style());

    let mut job = LayoutJob::default();

    let row = cursor_row as usize;
    let col = cursor_col.min(cols.saturating_sub(1)) as usize;

    let normal_fmt = TextFormat {
        font_id: font_id.clone(),
        color: ui.visuals().text_color(),
        ..Default::default()
    };

    let cursor_fmt = TextFormat {
        font_id,
        color: Color32::BLACK,
        background: Color32::YELLOW,
        ..Default::default()
    };

    let mut lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
    if lines.is_empty() {
        lines.push(String::new());
    }

    while lines.len() <= row {
        lines.push(String::new());
    }

    let line = &mut lines[row];
    let cur_len = line.chars().count();
    if cur_len <= col {
        line.extend(std::iter::repeat(' ').take(col + 1 - cur_len));
    }

    let padded_text = lines.join("\n");

    let mut idx: usize = 0;
    for (r, l) in lines.iter().enumerate() {
        if r == row {
            break;
        }
        idx += l.len();
        idx += 1; // '\n'
    }

    let mut byte_in_line: usize = 0;
    let mut ch_i: usize = 0;
    for (b, ch) in lines[row].char_indices() {
        if ch_i == col {
            byte_in_line = b;
            break;
        }
        ch_i += 1;
        byte_in_line = b + ch.len_utf8();
    }
    idx += byte_in_line;

    if idx >= padded_text.len() {
        job.append(&padded_text, 0.0, normal_fmt);
        return job;
    }

    let mut next = idx;
    if let Some(ch) = padded_text[idx..].chars().next() {
        next = idx + ch.len_utf8();
    }

    if idx > 0 {
        job.append(&padded_text[..idx], 0.0, normal_fmt.clone());
    }

    let cell = &padded_text[idx..next.min(padded_text.len())];
    let mut cursor_glyph = cell;
    if cursor_glyph == "\n" || cursor_glyph.trim().is_empty() {
        cursor_glyph = "█";
    }
    job.append(cursor_glyph, 0.0, cursor_fmt);

    if next < padded_text.len() {
        job.append(&padded_text[next..], 0.0, normal_fmt);
    }

    job
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

fn push_bounded(out: &mut String, s: &str, max_bytes: usize) {
    out.push_str(s);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if out.len() > max_bytes {
        let start = out.len().saturating_sub(max_bytes);
        let mut start2 = start;
        while start2 < out.len() && !out.is_char_boundary(start2) {
            start2 += 1;
        }
        *out = out[start2..].to_string();
    }
}

pub fn terminal_panel(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    terminal_id: ComponentId,
) -> Vec<Action> {
    let mut actions = vec![];

    let surface_id = egui::Id::new(("terminal_surface", terminal_id));


    let (current_shell, _cwd_display) = match state.terminals.get(&terminal_id) {
        Some(t) => {
            let cwd_display = t
                .cwd
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(no cwd)".to_string());
            (t.shell.clone(), cwd_display)
        }
        None => {
            ui.label("Missing terminal state.");
            ui.label("Tip: close this window and re-add a Terminal via the command palette.");
            return actions;
        }
    };

    ui.scope(|ui| {
        let mut style = ui.style().as_ref().clone();
        style.visuals.extreme_bg_color = Color32::BLACK;
        style.visuals.window_fill = Color32::BLACK;
        style.visuals.panel_fill = Color32::BLACK;
        style.visuals.widgets.noninteractive.bg_fill = Color32::BLACK;
        style.visuals.widgets.inactive.bg_fill = Color32::BLACK;
        style.visuals.widgets.hovered.bg_fill = Color32::BLACK;
        style.visuals.widgets.active.bg_fill = Color32::BLACK;
        style.visuals.widgets.open.bg_fill = Color32::BLACK;
        ui.set_style(std::sync::Arc::new(style));
        ui.visuals_mut().override_text_color = Some(Color32::from_rgb(220, 220, 220));

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

                        ui.label("Scrollback:");
            if let Some(t) = state.terminals.get_mut(&terminal_id) {
                let mut v = t.scrollback_max_lines as i32;
                ui.add(egui::DragValue::new(&mut v).clamp_range(100..=20000));
                let v2 = (v.max(100) as usize).min(20000);
                if v2 != t.scrollback_max_lines {
                    t.scrollback_max_lines = v2;
                }
            }
            ui.separator();

ui.separator();

            if ui.button("Clear").clicked() {
                actions.push(Action::ClearTerminal { terminal_id });
            }


        });

        ui.add_space(6.0);

        let full_w = ui.available_width().max(10.0);
        let output_h = ui.available_height().max(120.0);

        {
            let Some(t) = state.terminals.get_mut(&terminal_id) else {
                ui.label("Missing terminal state.");
                return;
            };

            const MAX_OUT_BYTES: usize = 2 * 1024 * 1024;
            let mut _finished = false;

            let drained: Vec<TerminalEvent> = {
                let mut v: Vec<TerminalEvent> = Vec::new();
                if let Some(rx) = t.pending_rx.as_ref() {
                    while let Ok(ev) = rx.try_recv() {
                        v.push(ev);
                    }
                }
                v
            };

            if !drained.is_empty() {
                for ev in drained {
                    match ev {
                        TerminalEvent::Stdout(s) | TerminalEvent::Stderr(s) => {
                            if let Some(vt) = t.vt.as_mut() {
                                vt.process(s.as_bytes());
                            }
                        }
                        TerminalEvent::Error(e) => {
                            let msg = format!("\n[pty] {e}\n");
                            if let Some(vt) = t.vt.as_mut() {
                                vt.process(msg.as_bytes());
                            }
                        }
                    }
                }

                if let Some(vt) = t.vt.as_mut() {
                    if t.follow_output {
                        vt.screen_mut().set_scrollback(0);
                    }
                    t.rendered_output = crate::app::controllers::terminal_controller::vt_screen_to_string(vt);
                }
            }


            let (rect, out_resp) = ui.allocate_exact_size(
                egui::vec2(full_w, output_h),
                egui::Sense::click(),
            );

            let surface_resp = ui.interact(rect, surface_id, egui::Sense::click());
            if out_resp.clicked() || surface_resp.clicked() {
                surface_resp.request_focus();
            }

            if ui
                .input(|i| i.pointer.hover_pos().map(|p| rect.contains(p)).unwrap_or(false))
                && is_scroll_input(ui)
                && !ui.input(|i| i.pointer.primary_down())
            {
                t.follow_output = false;
                if let Some(vt) = t.vt.as_mut() {
                    let dy = ui.input(|i| i.raw_scroll_delta.y + i.smooth_scroll_delta.y);
                    let row_h = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
                    let lines = (-dy / row_h).round() as i32;
                    if lines != 0 {
                        let cur = vt.screen().scrollback() as i32;
                        let next = (cur + lines).max(0) as usize;
                        vt.screen_mut().set_scrollback(next);
                        t.rendered_output = crate::app::controllers::terminal_controller::vt_screen_to_string(vt);
                    }
                }
            }
            let (rows, cols) = calc_rows_cols(ui, rect);
            if t.pty_in.is_none() || t.pty_master.is_none() {
                actions.push(Action::StartTerminalSession {
                    terminal_id,
                    rows,
                    cols,
                });
            } else if t.pty_size != Some((rows, cols)) {
                actions.push(Action::ResizeTerminal {
                    terminal_id,
                    rows,
                    cols,
                });
            }

            let mut want_terminal_focus = false;

            let surface_has_focus = ui.memory(|m| m.has_focus(surface_id));

            ui.allocate_ui_at_rect(rect, |ui| {
                ui.set_width(full_w);
                ui.set_max_width(full_w);

                if ui.input(|i| i.pointer.primary_down() && i.pointer.delta() != egui::Vec2::ZERO) {
                    t.follow_output = false;
                }

let text = &t.rendered_output;

                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    if t.vt.is_some() {
                        if surface_has_focus {
                            let (_rows, cols) = t.pty_size.unwrap_or((30, 120));
                            let (cur_row, cur_col) = t
                                .vt
                                .as_ref()
                                .map(|p| p.screen().cursor_position())
                                .unwrap_or((0, 0));

                            let job = vt_layout_with_cursor(ui, text, cols, cur_row, cur_col);
                            let resp = ui.add(egui::Label::new(job).selectable(true).wrap(false));
                            if resp.clicked() || resp.drag_started() {
                                want_terminal_focus = true;
                            }
                        } else {
                            let resp = ui.add(
                                egui::Label::new(egui::RichText::new(text).monospace())
                                    .selectable(true)
                                    .wrap(false),
                            );
                            if resp.clicked() || resp.drag_started() {
                                want_terminal_focus = true;
                            }
                        }
                    } else {
                        let resp = ui.add(
                            egui::Label::new(egui::RichText::new(text).monospace())
                                .selectable(true)
                                .wrap(false),
                        );
                        if resp.clicked() || resp.drag_started() {
                            want_terminal_focus = true;
                        }
                    }
                });
            });

            if want_terminal_focus {
                ui.memory_mut(|m| m.request_focus(surface_id));
            }
        }

        {
            let Some(_t) = state.terminals.get_mut(&terminal_id) else {
                return;
            };

            let surface_has_focus = ui.memory(|m| m.has_focus(surface_id));

            if surface_has_focus {

                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        surface_id,
                        egui::EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: true,
                        },
                    );
                });

                let mut bytes_to_send: Vec<u8> = Vec::new();

                ctx.input_mut(|i| {

                    i.events.retain(|e| {
                        match e {
                            egui::Event::Key {
                                key: Key::C,
                                pressed: true,
                                modifiers,
                                ..
                            } if (modifiers.ctrl && modifiers.shift) || (modifiers.command && modifiers.shift) => {
                                bytes_to_send.push(0x03);
                                false
                            }
                            egui::Event::Copy => {
                                if i.modifiers.shift && (i.modifiers.ctrl || i.modifiers.command) {
                                    bytes_to_send.push(0x03);
                                    false
                                } else {
                                    true
                                }
                            }
                            _ => true,
                        }
                    });
                    i.events.retain(|e| match e {
                        egui::Event::Text(s) => {
                            bytes_to_send.extend_from_slice(s.as_bytes());
                            false
                        }
                        egui::Event::Paste(s) => {
                            bytes_to_send.extend_from_slice(s.as_bytes());
                            false
                        }
                        _ => true,
                    });

                    if !bytes_to_send.is_empty() {
                        if let Some(t) = state.terminals.get_mut(&terminal_id) {
                            t.follow_output = true;
                            if let Some(vt) = t.vt.as_mut() {
                                vt.screen_mut().set_scrollback(0);
                                t.rendered_output = crate::app::controllers::terminal_controller::vt_screen_to_string(vt);
                            }
                        }
                    }

                    if i.consume_key(egui::Modifiers::NONE, Key::Enter) {
                        bytes_to_send.push(b'\r');
                    }

                    if i.consume_key(egui::Modifiers::NONE, Key::Backspace) {
                        bytes_to_send.push(0x7f);
                    }

                    if i.consume_key(egui::Modifiers::NONE, Key::Tab) {
                        bytes_to_send.push(b'\t');
                    }

                    for k in [
                        Key::ArrowUp,
                        Key::ArrowDown,
                        Key::ArrowLeft,
                        Key::ArrowRight,
                        Key::Home,
                        Key::End,
                        Key::PageUp,
                        Key::PageDown,
                    ] {
                        if i.consume_key(egui::Modifiers::NONE, k) {
                            if let Some(seq) = key_to_ansi(k) {
                                bytes_to_send.extend_from_slice(seq);
                            }
                        }
                    }

                    let mut mods = egui::Modifiers::NONE;
                    mods.ctrl = true;
                    if i.consume_key(mods, Key::D) {
                        bytes_to_send.push(0x04);
                    }
                });

                if !bytes_to_send.is_empty() {
                    actions.push(Action::TerminalSendInput {
                        terminal_id,
                        data: bytes_to_send,
                    });
                }
            }
        }
    });

    actions
}
