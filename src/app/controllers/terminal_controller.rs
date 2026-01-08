use crate::app::actions::{Action, ComponentId, ComponentKind, TerminalShell};
use crate::app::layout::{ComponentInstance, WindowLayout};
use crate::app::state::{AppState, TerminalState};

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
        self.terminals.clear();

        let term_ids: Vec<ComponentId> = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::Terminal)
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
                shell: TerminalShell::Auto,
                cwd: self.inputs.repo.clone(),
                input: String::new(),
                output: String::new(),
                last_status: None,
            },
        );

        self.layout_epoch = self.layout_epoch.wrapping_add(1);
    }

    fn run_terminal_command(&mut self, terminal_id: ComponentId, cmd: &str) {
        let Some(t) = self.terminals.get_mut(&terminal_id) else {
            return;
        };

        let cwd = t.cwd.clone().or_else(|| self.inputs.repo.clone());
        let shell = t.shell.clone();

        t.output.push_str(&format!("\n$ {}\n", cmd));

        match self.platform.run_shell_command(shell, cmd, cwd) {
            Ok(out) => {
                t.last_status = Some(out.code);

                if !out.stdout.is_empty() {
                    t.output.push_str(&out.stdout);
                    if !t.output.ends_with('\n') {
                        t.output.push('\n');
                    }
                }
                if !out.stderr.is_empty() {
                    t.output.push_str(&out.stderr);
                    if !t.output.ends_with('\n') {
                        t.output.push('\n');
                    }
                }

                t.output.push_str(&format!("[exit: {}]\n", out.code));
            }
            Err(e) => {
                t.last_status = Some(-1);
                t.output
                    .push_str(&format!("Failed to run command: {:#}\n", e));
            }
        }
    }
}
