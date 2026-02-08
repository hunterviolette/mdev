use crate::app::actions::{Action, ComponentId, ComponentKind, TerminalShell};
use crate::app::layout::{ComponentInstance, WindowLayout};
use crate::app::state::{AppState, TerminalEvent, TerminalState};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

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
                t.output.clear();
                t.last_status = None;
                t.running = false;
                t.pending_rx = None;
                t.child = None;
            }
            true
        }

        Action::InterruptTerminal { terminal_id } => {
            if let Some(t) = state.terminals.get_mut(terminal_id) {
                if let Some(ch) = t.child.as_ref() {
                    // Best-effort: kill. True Ctrl+C/SIGINT requires a PTY.
                    let _ = ch.lock().ok().and_then(|mut c| c.kill().ok());
                    t.output.push_str("\n[interrupt] sent stop/kill to running command\n");
                } else {
                    t.output.push_str("\n[interrupt] no running process\n");
                }
            }
            true
        }

        Action::RunTerminalCommand { terminal_id, cmd } => {
            state.run_terminal_command(*terminal_id, cmd);
            true
        }

        _ => false,
    }
}

impl AppState {
    fn spawn_shell_streaming(
        shell: &TerminalShell,
        cmd: &str,
        cwd: Option<&std::path::PathBuf>,
    ) -> std::io::Result<std::process::Child> {
        let mut c = match shell {
            TerminalShell::PowerShell => {
                let mut cc = Command::new("powershell");
                cc.args(["-NoProfile", "-Command", cmd]);
                cc
            }
            TerminalShell::Cmd => {
                let mut cc = Command::new("cmd");
                cc.args(["/C", cmd]);
                cc
            }
            TerminalShell::Bash => {
                let mut cc = Command::new("bash");
                cc.args(["-lc", cmd]);
                cc
            }
            TerminalShell::Zsh => {
                let mut cc = Command::new("zsh");
                cc.args(["-lc", cmd]);
                cc
            }
            TerminalShell::Sh | TerminalShell::Auto => {
                let mut cc = Command::new("sh");
                cc.args(["-lc", cmd]);
                cc
            }
        };

        if let Some(dir) = cwd {
            c.current_dir(dir);
        }

        c.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
    }

    fn terminal_prompt(shell: &TerminalShell, cwd: &Option<std::path::PathBuf>) -> String {
        match (shell, cwd) {
            (TerminalShell::PowerShell, Some(dir)) => format!("PS {}> ", dir.display()),
            (TerminalShell::Cmd, Some(dir)) => format!("{}> ", dir.display()),
            (_, Some(dir)) => format!("{}$ ", dir.display()),
            (TerminalShell::PowerShell, None) => "PS > ".to_string(),
            (TerminalShell::Cmd, None) => "> ".to_string(),
            (_, None) => "$ ".to_string(),
        }
    }

    /// Called by layout/workspace controllers after layout changes.
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
                    shell: TerminalShell::Auto,
                    cwd: self.inputs.repo.clone(),
                    input: String::new(),
                    output: String::new(),
                    last_status: None,
                    running: false,
                    pending_rx: None,
                    child: None,
                },
            );
        }
    }

    /// Used by layout controller when adding a Terminal component.
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
                shell: TerminalShell::Auto,
                cwd: self.inputs.repo.clone(),
                input: String::new(),
                output: String::new(),
                last_status: None,
                running: false,
                pending_rx: None,
                child: None,
            },
        );

        self.layout_epoch = self.layout_epoch.wrapping_add(1);
    }

    fn run_terminal_command(&mut self, terminal_id: ComponentId, cmd: &str) {
        let Some(t) = self.terminals.get_mut(&terminal_id) else {
            return;
        };

        // Do not allow overlapping runs in the same terminal.
        if t.running {
            t.output
                .push_str("\n[busy] command already running; wait for it to finish.\n");
            return;
        }

        let cwd = t.cwd.clone().or_else(|| self.inputs.repo.clone());
        let shell = t.shell.clone();

        // Shell-like prompt prefix (best-effort; not a PTY).
        let prompt = match (&shell, &cwd) {
            (TerminalShell::PowerShell, Some(dir)) => format!("PS {}> ", dir.display()),
            (TerminalShell::Cmd, Some(dir)) => format!("{}> ", dir.display()),
            (_, Some(dir)) => format!("{}$ ", dir.display()),
            (TerminalShell::PowerShell, None) => "PS > ".to_string(),
            (TerminalShell::Cmd, None) => "> ".to_string(),
            (_, None) => "$ ".to_string(),
        };

        t.output.push_str("\n");
        t.output.push_str(&prompt);
        t.output.push_str(cmd);
        t.output.push_str("\n");

        t.running = true;
        t.last_status = None;

        let (tx, rx) = mpsc::channel();
        t.pending_rx = Some(rx);

        // Build a real streaming child process (do NOT use Command::output()).
        let c = match Self::spawn_shell_streaming(&shell, cmd, cwd.as_ref()) {
            Ok(c) => c,
            Err(e) => {
                t.running = false;
                t.pending_rx = None;
                t.child = None;
                t.output.push_str(&format!("Failed to spawn command: {}\n", e));
                return;
            }
        };

        let child_arc = Arc::new(Mutex::new(c));
        t.child = Some(child_arc.clone());

        thread::spawn(move || {
            // Take stdout/stderr handles by temporarily locking.
            let (stdout, stderr) = {
                let mut ch = child_arc.lock().expect("child lock");
                (ch.stdout.take(), ch.stderr.take())
            };

            if let Some(out) = stdout {
                let txo = tx.clone();
                thread::spawn(move || {
                    let mut r = BufReader::new(out);
                    let mut line = String::new();
                    loop {
                        line.clear();
                        match r.read_line(&mut line) {
                            Ok(0) => break,
                            Ok(_) => {
                                let _ = txo.send(TerminalEvent::Stdout(line.clone()));
                            }
                            Err(e) => {
                                let _ = txo.send(TerminalEvent::Error(format!(
                                    "stdout read error: {}",
                                    e
                                )));
                                break;
                            }
                        }
                    }
                });
            }

            if let Some(err) = stderr {
                let txe = tx.clone();
                thread::spawn(move || {
                    let mut r = BufReader::new(err);
                    let mut line = String::new();
                    loop {
                        line.clear();
                        match r.read_line(&mut line) {
                            Ok(0) => break,
                            Ok(_) => {
                                let _ = txe.send(TerminalEvent::Stderr(line.clone()));
                            }
                            Err(e) => {
                                let _ = txe.send(TerminalEvent::Error(format!(
                                    "stderr read error: {}",
                                    e
                                )));
                                break;
                            }
                        }
                    }
                });
            }

            // Wait for exit.
            let code = {
                let mut ch = child_arc.lock().expect("child lock");
                match ch.wait() {
                    Ok(status) => status.code().unwrap_or(-1),
                    Err(_) => -1,
                }
            };
            let _ = tx.send(TerminalEvent::Exit(code));
        });
    }
}
