use crate::app::actions::{Action, ComponentId};
use crate::app::state::AppState;

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
    pub fn rebuild_terminals_from_layout(&mut self) {
        use crate::app::actions::TerminalShell;
        use crate::app::state::TerminalState;

        // Terminals are ephemeral. Workspace load restores the *layout* and terminal component IDs,
        // but the runtime terminal state must be recreated fresh each time.
        self.terminals.clear();

        let term_ids: Vec<ComponentId> = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == crate::app::actions::ComponentKind::Terminal)
            .map(|c| c.id)
            .collect();

        for id in term_ids {
            self.terminals.insert(
                id,
                TerminalState {
                    shell: TerminalShell::Auto,
                    cwd: self.inputs.repo.clone(),
                    input: String::new(),
                    output: String::new(),
                    last_status: None,
                },
            );
        }
    }

    pub fn new_terminal(&mut self) {
        use crate::app::layout::{ComponentInstance, WindowLayout};
        use crate::app::state::TerminalState;
        use crate::app::actions::ComponentKind;

        self.layout.merge_with_defaults();

        let id = self.layout.next_free_id();

        let term_count = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::Terminal)
            .count();

        let title = format!("Terminal {}", term_count + 1);

        self.layout.components.push(ComponentInstance {
            id,
            kind: ComponentKind::Terminal,
            title,
        });

        self.layout.windows.insert(
            id,
            WindowLayout {
                open: true,
                locked: false,
                pos: [90.0, 90.0],
                size: [760.0, 420.0],
            },
        );

        self.terminals.insert(
            id,
            TerminalState {
                shell: crate::app::actions::TerminalShell::Auto,
                cwd: self.inputs.repo.clone(),
                input: String::new(),
                output: String::new(),
                last_status: None,
            },
        );

        self.layout_epoch = self.layout_epoch.wrapping_add(1);
    }

    fn run_terminal_command(&mut self, terminal_id: ComponentId, cmd: &str) {
        use std::process::Command;

        let Some(t) = self.terminals.get_mut(&terminal_id) else {
            return;
        };

        let cwd = t.cwd.clone().or_else(|| self.inputs.repo.clone());

        let (program, args): (&str, Vec<String>) = match t.shell {
            crate::app::actions::TerminalShell::Auto => {
                if cfg!(windows) {
                    ("powershell", vec!["-NoProfile".into(), "-Command".into(), cmd.into()])
                } else {
                    ("bash", vec!["-lc".into(), cmd.into()])
                }
            }
            crate::app::actions::TerminalShell::PowerShell => (
                "powershell",
                vec!["-NoProfile".into(), "-Command".into(), cmd.into()],
            ),
            crate::app::actions::TerminalShell::Cmd => ("cmd", vec!["/C".into(), cmd.into()]),
            crate::app::actions::TerminalShell::Bash => ("bash", vec!["-lc".into(), cmd.into()]),
            crate::app::actions::TerminalShell::Zsh => ("zsh", vec!["-lc".into(), cmd.into()]),
            crate::app::actions::TerminalShell::Sh => ("sh", vec!["-lc".into(), cmd.into()]),
        };

        t.output.push_str(&format!("\n$ {}\n", cmd));

        let mut c = Command::new(program);
        c.args(args);

        if let Some(dir) = cwd {
            c.current_dir(dir);
        }

        match c.output() {
            Ok(out) => {
                let code = out.status.code().unwrap_or(-1);
                t.last_status = Some(code);

                if !out.stdout.is_empty() {
                    t.output.push_str(&String::from_utf8_lossy(&out.stdout));
                    if !t.output.ends_with('\n') {
                        t.output.push('\n');
                    }
                }
                if !out.stderr.is_empty() {
                    t.output.push_str(&String::from_utf8_lossy(&out.stderr));
                    if !t.output.ends_with('\n') {
                        t.output.push('\n');
                    }
                }

                t.output.push_str(&format!("[exit: {}]\n", code));
            }
            Err(e) => {
                t.last_status = Some(-1);
                t.output.push_str(&format!("Failed to run command: {}\n", e));
            }
        }
    }
}
