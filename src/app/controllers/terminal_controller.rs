use crate::app::actions::{Action, ComponentId, ComponentKind, TerminalShell};
use crate::app::layout::{ComponentInstance, WindowLayout};
use crate::app::state::{AppState, TerminalEvent, TerminalState};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};

use std::sync::{mpsc, Arc, Mutex};
use std::thread;

pub(crate) fn vt_screen_to_string(p: &vt100::Parser) -> String {
    p.screen().contents()
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }

        match chars.peek().copied() {
            Some('[') => {
                let _ = chars.next();
                while let Some(c) = chars.next() {
                    if c.is_ascii_alphabetic() || c == '~' {
                        break;
                    }
                }
            }
            Some(']') => {
                let _ = chars.next();
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                }
            }
            Some(_) => {
                let _ = chars.next();
            }
            None => {}
        }
    }

    out
}

pub(crate) fn append_scrollback_lines(t: &mut TerminalState, chunk: &str) {
    let cleaned = strip_ansi(chunk);

    for ch in cleaned.chars() {
        match ch {
            '\r' => {
                t.scrollback_partial.clear();
            }
            '\n' => {
                let line = std::mem::take(&mut t.scrollback_partial);
                t.scrollback.push_back(line);
                while t.scrollback.len() > t.scrollback_max_lines {
                    t.scrollback.pop_front();
                }
            }
            _ => {
                t.scrollback_partial.push(ch);
            }
        }
    }
}

pub(crate) fn render_scrollback(t: &TerminalState) -> String {
    let mut out = String::new();
    for (i, line) in t.scrollback.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
    if !t.scrollback_partial.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&t.scrollback_partial);
    }
    out
}


pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::SetTerminalShell { terminal_id, shell } => {
            if let Some(t) = state.terminals.get_mut(terminal_id) {
                t.shell = shell.clone();
            }
            true
        }

        Action::ClearTerminal { terminal_id } => {
            if let Some(t) = state.terminals.get_mut(terminal_id) {
                if let Some(vt) = t.vt.as_mut() {
                    *vt = vt100::Parser::new(30, 120, t.scrollback_max_lines);
                }
                t.rendered_output.clear();
                t.scrollback.clear();
                t.scrollback_partial.clear();
                t.pending_rx = None;
                t.pty_in = None;
                t.pty_child = None;
                t.pty_master = None;
            }
            true
        }

        Action::InterruptTerminal { terminal_id } => {
            if let Some(t) = state.terminals.get_mut(terminal_id) {
                if let Some(pty_in) = t.pty_in.as_ref() {
                    if let Ok(mut w) = pty_in.lock() {
                        let _ = w.write_all(&[0x03]);
                        let _ = w.flush();
                        t.rendered_output.push_str("\n[interrupt] sent Ctrl+C\n");
                        return true;
                    }
                }

                t.rendered_output
                    .push_str("\n[interrupt] terminal session not available\n");
            }
            true
        }

        Action::RunTerminalCommand { terminal_id, cmd } => {
            state.run_terminal_command(*terminal_id, cmd);
            true
        }

        Action::StartTerminalSession { terminal_id, rows, cols } => {
            state.ensure_terminal_session(*terminal_id, *rows, *cols);
            true
        }

        Action::ResizeTerminal { terminal_id, rows, cols } => {
            if let Some(t) = state.terminals.get_mut(terminal_id) {
                if let Some(master) = t.pty_master.as_ref() {
                    if let Ok(m) = master.lock() {
                        let _ = m.resize(PtySize {
                            rows: *rows,
                            cols: *cols,
                            pixel_width: 0,
                            pixel_height: 0,
                        });
                        t.pty_size = Some((*rows, *cols));
                    }
                }

                if let Some(vt) = t.vt.as_mut() {
                    *vt = vt100::Parser::new(*rows, *cols, t.scrollback_max_lines);
                    t.rendered_output = vt_screen_to_string(vt);
                }
            }
            true
        }

        Action::TerminalSendInput { terminal_id, data } => {
            if let Some(t) = state.terminals.get_mut(terminal_id) {
                if let Some(pty_in) = t.pty_in.as_ref() {
                    if let Ok(mut w) = pty_in.lock() {
                        let _ = w.write_all(data);
                        let _ = w.flush();
                    }
                }
            }
            true
        }

        _ => false,
    }
}



impl AppState {

    pub fn rebuild_terminals_from_layout(&mut self) {
        self.terminals.clear();

        let mut term_ids: Vec<ComponentId> = self
            .all_layouts()
            .flat_map(|l| l.components.iter())
            .filter(|c| c.kind == ComponentKind::Terminal)
            .map(|c| c.id)
            .collect();

        term_ids.sort_unstable();
        term_ids.dedup();

        for id in term_ids {
            self.terminals.insert(
                id,
                TerminalState {
                    vt: Some(vt100::Parser::new(30, 120, 2000)),
                    rendered_output: String::new(),
                    scrollback: std::collections::VecDeque::new(),
                    scrollback_partial: String::new(),
                    scrollback_max_lines: 2000,
                    follow_output: true,
                    pty_master: None,
                    pty_child: None,
                    pty_size: None,
                    shell: TerminalShell::Auto,
                    cwd: self.inputs.repo.clone(),
                    pending_rx: None,
                    pty_in: None,
                },
            );
        }
    }

    pub fn new_terminal(&mut self) {
        self.active_layout_mut().merge_with_defaults();

        let id = self.alloc_component_id();

        let term_count = self
            .active_layout()
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::Terminal)
            .count();

        let title = format!("Terminal {}", term_count + 1);

        self.active_layout_mut().components.push(ComponentInstance {
            id,
            kind: ComponentKind::Terminal,
            title,
        });

        self.active_layout_mut().windows.insert(
            id,
            WindowLayout {
                open: true,
                locked: false,
                pos_norm: None,
                size_norm: None,
                pos: [90.0, 90.0],
                size: [760.0, 420.0],
            },
        );

        self.terminals.insert(
            id,
            TerminalState {
                vt: Some(vt100::Parser::new(30, 120, 2000)),
                rendered_output: String::new(),
                scrollback: std::collections::VecDeque::new(),
                scrollback_partial: String::new(),
                scrollback_max_lines: 2000,
                follow_output: true,
                pty_master: None,
                pty_child: None,
                pty_size: None,
                shell: TerminalShell::Auto,
                cwd: self.inputs.repo.clone(),
                pending_rx: None,
                pty_in: None,
            },
        );

        self.layout_epoch = self.layout_epoch.wrapping_add(1);
    }

    fn spawn_shell_pty(
        shell: &TerminalShell,
        cwd: Option<&std::path::PathBuf>,
        rows: u16,
        cols: u16,
    ) -> anyhow::Result<(Box<dyn MasterPty + Send>, Box<dyn portable_pty::Child + Send>)> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cb = match shell {
            TerminalShell::PowerShell => {
                #[cfg(windows)]
                let exe = "powershell.exe";
                #[cfg(not(windows))]
                let exe = "pwsh";

                let mut b = CommandBuilder::new(exe);
                b.arg("-NoLogo");
                b.arg("-NoProfile");
                b
            }
            TerminalShell::Cmd => {
                #[cfg(windows)]
                {
                    CommandBuilder::new("cmd.exe")
                }
                #[cfg(not(windows))]
                {
                    CommandBuilder::new("sh")
                }
            }
            TerminalShell::Bash => CommandBuilder::new("bash"),
            TerminalShell::Zsh => CommandBuilder::new("zsh"),
            TerminalShell::Sh => CommandBuilder::new("sh"),
            TerminalShell::Auto => {
                #[cfg(windows)]
                let mut b = CommandBuilder::new("powershell.exe");
                #[cfg(not(windows))]
                let mut b = CommandBuilder::new("bash");
                #[cfg(windows)]
                {
                    b.arg("-NoLogo");
                    b.arg("-NoProfile");
                }
                b
            }
        };

        if let Some(dir) = cwd {
            cb.cwd(dir);
        }

        let child = pair.slave.spawn_command(cb)?;
        Ok((pair.master, child))
    }

    fn ensure_terminal_session(&mut self, terminal_id: ComponentId, rows: u16, cols: u16) {
        let Some(t) = self.terminals.get_mut(&terminal_id) else {
            return;
        };

        if t.pty_in.is_some() && t.pty_master.is_some() {
            if t.pty_size != Some((rows, cols)) {
                if let Some(master) = t.pty_master.as_ref() {
                    if let Ok(m) = master.lock() {
                        let _ = m.resize(PtySize {
                            rows,
                            cols,
                            pixel_width: 0,
                            pixel_height: 0,
                        });
                    }
                }
                t.pty_size = Some((rows, cols));

                if let Some(vt) = t.vt.as_mut() {
                    *vt = vt100::Parser::new(rows, cols, t.scrollback_max_lines);
                    t.rendered_output = vt_screen_to_string(vt);
                }
            }
            return;
        }

        let cwd = t.cwd.clone().or_else(|| self.inputs.repo.clone());
        let (tx, rx) = mpsc::channel();
        t.pending_rx = Some(rx);

        let (master, child) = match Self::spawn_shell_pty(&t.shell, cwd.as_ref(), rows, cols) {
            Ok(v) => v,
            Err(e) => {
                t.rendered_output
                    .push_str(&format!("Failed to spawn PTY shell: {e}\n"));
                return;
            }
        };

        let master = Arc::new(Mutex::new(master));
        t.pty_master = Some(master.clone());
        t.pty_child = Some(Arc::new(Mutex::new(child)));
        t.pty_size = Some((rows, cols));

        t.vt = Some(vt100::Parser::new(rows, cols, t.scrollback_max_lines));
        t.rendered_output.clear();

        let (mut reader, writer) = {
            let m = match master.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let r = match m.try_clone_reader() {
                Ok(v) => v,
                Err(e) => {
                    t.rendered_output
                        .push_str(&format!("Failed to open PTY reader: {e}\n"));
                    t.pty_master = None;
                    t.pty_child = None;
                    return;
                }
            };
            let w = match m.take_writer() {
                Ok(v) => v,
                Err(e) => {
                    t.rendered_output
                        .push_str(&format!("Failed to open PTY writer: {e}\n"));
                    t.pty_master = None;
                    t.pty_child = None;
                    return;
                }
            };
            (r, w)
        };

        t.pty_in = Some(Arc::new(Mutex::new(writer)));

        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let s = String::from_utf8_lossy(&buf[..n]).to_string();
                        let _ = tx.send(TerminalEvent::Stdout(s));
                    }
                    Err(e) => {
                        let _ = tx.send(TerminalEvent::Error(format!("pty read error: {e}")));
                        break;
                    }
                }
            }
        });
    }

    fn run_terminal_command(&mut self, terminal_id: ComponentId, cmd: &str) {
        let Some(t) = self.terminals.get_mut(&terminal_id) else {
            return;
        };


        let _cwd = t.cwd.clone().or_else(|| self.inputs.repo.clone());
        let _shell = t.shell.clone();

        if let Some(pty_in) = t.pty_in.as_ref() {
            if let Ok(mut w) = pty_in.lock() {
                let _ = w.write_all(cmd.as_bytes());
                let _ = w.write_all(b"\r\n");
                let _ = w.flush();
                return;
            }
        }

        let need_start = t.pty_in.is_none() || t.pty_master.is_none();
        if need_start {
        }

        if need_start {
            drop(t);
            self.ensure_terminal_session(terminal_id, 30, 120);
        }

        let Some(t) = self.terminals.get_mut(&terminal_id) else {
            return;
        };

        if let Some(pty_in) = t.pty_in.as_ref() {
            if let Ok(mut w) = pty_in.lock() {
                let _ = w.write_all(cmd.as_bytes());
                let _ = w.write_all(b"\r\n");
                let _ = w.flush();
                return;
            }
        }

        if let Some(t) = self.terminals.get_mut(&terminal_id) {
            t.rendered_output
                .push_str("\n[error] PTY session unavailable; command not sent.\n");
        }
        return;
    }
}
