// src/app/controllers/changeset_controller.rs
//
// ChangeSet Applier controller.
// - Parses the JSON payload from the ChangeSet Applier UI component
// - Applies operations (write/move/delete/git_apply)
// - Writes detailed diagnostics into the applier "status" output so git_apply issues
//   (corrupt patch / newline escaping / missing trailing newline) are debuggable.

use crate::app::actions::{Action, ComponentId, ComponentKind, TerminalShell};
use crate::app::state::{AppState, WORKTREE_REF};
use crate::capabilities::{CapabilityRequest, CapabilityResponse};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ChangeSetPayload {
    version: u32,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    operations: Vec<Operation>,
    #[serde(default)]
    post_commands: Vec<PostCommand>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op")]
enum Operation {
    #[serde(rename = "write")]
    Write { path: String, contents: String },

    #[serde(rename = "delete")]
    Delete { path: String },

    #[serde(rename = "move")]
    Move { from: String, to: String },

    #[serde(rename = "git_apply")]
    GitApply { patch: String },
}

#[derive(Debug, Deserialize)]
struct PostCommand {
    #[serde(default)]
    shell: Option<String>, // "Auto" | "Bash" | "PowerShell" ...
    cmd: String,
    #[serde(default)]
    cwd: Option<String>,
}

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::ApplyChangeSet { applier_id } => {
            apply_changeset(state, *applier_id);
            true
        }
        Action::ClearChangeSet { applier_id } => {
            if let Some(st) = state.changeset_appliers.get_mut(applier_id) {
                st.payload.clear();
                st.status = None;
            }
            true
        }
        _ => false,
    }
}

impl AppState {
    /// Keep `changeset_appliers` in sync with the current layout.
    /// Called by layout/workspace controllers after layout changes.
    pub fn rebuild_changeset_appliers_from_layout(&mut self) {
        use crate::app::state::ChangeSetApplierState;

        self.changeset_appliers.clear();

        let ids: Vec<ComponentId> = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::ChangeSetApplier)
            .map(|c| c.id)
            .collect();

        for id in ids {
            self.changeset_appliers.insert(
                id,
                ChangeSetApplierState {
                    payload: String::new(),
                    status: None,
                },
            );
        }
    }
}

fn apply_changeset(state: &mut AppState, applier_id: ComponentId) {
    let Some(repo) = state.inputs.repo.clone() else {
        set_applier_status(
            state,
            applier_id,
            "No repo selected. Pick a folder first.".to_string(),
        );
        return;
    };

    let payload_text = match state.changeset_appliers.get(&applier_id) {
        Some(st) => st.payload.clone(),
        None => return,
    };

    let parsed: ChangeSetPayload = match serde_json::from_str(&payload_text) {
        Ok(p) => p,
        Err(e) => {
            set_applier_status(state, applier_id, format!("Invalid JSON: {e}"));
            return;
        }
    };

    if parsed.version != 1 {
        set_applier_status(
            state,
            applier_id,
            format!("Unsupported payload version {} (expected 1).", parsed.version),
        );
        return;
    }

    let mut log = String::new();
    if let Some(d) = &parsed.description {
        log.push_str(&format!("description: {d}\n\n"));
    }

    // Helpful: include current ref in output (debugging when ref/view changes)
    log.push_str(&format!(
        "repo: {:?}\nref: {}\n\n",
        repo,
        state.inputs.git_ref
    ));
    if state.inputs.git_ref == WORKTREE_REF {
        log.push_str("(note) Applying against WORKTREE.\n\n");
    }

    for (idx, op) in parsed.operations.iter().enumerate() {
        let step = idx + 1;

        match op {
            Operation::Write { path, contents } => {
                log.push_str(&format!("[{step}] write {path}\n"));
                let r = state.broker.exec(CapabilityRequest::WriteWorktreeFile {
                    repo: repo.clone(),
                    path: path.clone(),
                    contents: contents.as_bytes().to_vec(),
                });
                match r {
                    Ok(_) => log.push_str(&format!("[{step}] ok\n")),
                    Err(e) => {
                        log.push_str(&format!("[{step}] FAILED: {:#}\n", e));
                        set_applier_status(state, applier_id, log);
                        return;
                    }
                }
            }

            Operation::Delete { path } => {
                log.push_str(&format!("[{step}] delete {path}\n"));
                let r = state.broker.exec(CapabilityRequest::DeleteWorktreePath {
                    repo: repo.clone(),
                    path: path.clone(),
                });
                match r {
                    Ok(_) => log.push_str(&format!("[{step}] ok\n")),
                    Err(e) => {
                        log.push_str(&format!("[{step}] FAILED: {:#}\n", e));
                        set_applier_status(state, applier_id, log);
                        return;
                    }
                }
            }

            Operation::Move { from, to } => {
                log.push_str(&format!("[{step}] move {from} -> {to}\n"));
                let r = state.broker.exec(CapabilityRequest::MoveWorktreePath {
                    repo: repo.clone(),
                    from: from.clone(),
                    to: to.clone(),
                });
                match r {
                    Ok(_) => log.push_str(&format!("[{step}] ok\n")),
                    Err(e) => {
                        log.push_str(&format!("[{step}] FAILED: {:#}\n", e));
                        set_applier_status(state, applier_id, log);
                        return;
                    }
                }
            }

            Operation::GitApply { patch } => {
                log.push_str(&format!("[{step}] git_apply\n"));
                log.push_str(&patch_diagnostics("raw", patch));

                let normalized = normalize_patch_text(patch);
                log.push_str(&patch_diagnostics("normalized", &normalized));

                let r = state.broker.exec(CapabilityRequest::ApplyGitPatch {
                    repo: repo.clone(),
                    patch: normalized,
                });

                match r {
                    Ok(_) => log.push_str(&format!("[{step}] ok\n")),
                    Err(e) => {
                        log.push_str(&format!("[{step}] FAILED: {:#}\n", e));
                        set_applier_status(state, applier_id, log);
                        return;
                    }
                }
            }
        }
    }

    // post_commands (optional)
    if !parsed.post_commands.is_empty() {
        log.push_str("\npost_commands:\n");
    }

    for cmd in &parsed.post_commands {
        let shell = parse_shell(cmd.shell.as_deref());
        let cwd = cmd.cwd.as_ref().map(|s| repo.join(s));

        log.push_str(&format!("\n$ {}\n", cmd.cmd));

        match state.broker.exec(CapabilityRequest::RunShellCommand {
            shell,
            cmd: cmd.cmd.clone(),
            cwd,
        }) {
            Ok(CapabilityResponse::ShellOutput { code, stdout, stderr }) => {
                if !stdout.is_empty() {
                    log.push_str(&stdout);
                    if !log.ends_with('\n') {
                        log.push('\n');
                    }
                }
                if !stderr.is_empty() {
                    log.push_str(&stderr);
                    if !log.ends_with('\n') {
                        log.push('\n');
                    }
                }
                log.push_str(&format!("[exit: {code}]\n"));
                if code != 0 {
                    break;
                }
            }
            Ok(_) => {
                log.push_str("Unexpected response from RunShellCommand.\n");
                break;
            }
            Err(e) => {
                log.push_str(&format!("Command failed: {:#}\n", e));
                break;
            }
        }
    }

    set_applier_status(state, applier_id, log);
}

fn set_applier_status(state: &mut AppState, applier_id: ComponentId, status: String) {
    if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
        st.status = Some(status);
    }
}

/// Diagnostics meant to show whether the patch string is actually suitable for `git apply`.
fn patch_diagnostics(label: &str, s: &str) -> String {
    let len = s.len();
    let nl_count = s.as_bytes().iter().filter(|&&b| b == b'\n').count();
    let cr_count = s.as_bytes().iter().filter(|&&b| b == b'\r').count();

    let has_diff_git = s.contains("diff --git ");
    let has_unified_hunk = s.contains("\n@@ ") || s.starts_with("@@ ");
    let has_literal_backslash_n = s.contains("\\n");
    let has_real_newlines = s.contains('\n');
    let ends_with_newline = s.ends_with('\n');

    // Render a one-line preview: show real newlines as "\n" for readability.
    let mut preview = s.chars().take(240).collect::<String>();
    preview = preview.replace('\r', "\\r");
    preview = preview.replace('\n', "\\n");

    format!(
        "{label}: patch_len={len} nl_count={nl_count} cr_count={cr_count} has_diff_git={has_diff_git} has_unified_hunk={has_unified_hunk} has_literal_backslash_n={has_literal_backslash_n} has_real_newlines={has_real_newlines} ends_with_newline={ends_with_newline}\npreview='{preview}'\n"
    )
}

/// Normalization strategy:
/// - Always normalize CRLF -> LF
/// - If the input contains any literal "\\n" sequences, convert them to real '\n'
///   (even if some real newlines already exist; mixed input happens a lot)
/// - Ensure a trailing newline (git apply can fail / or concatenation can break without it)
fn normalize_patch_text(patch: &str) -> String {
    // 1) normalize CRLF -> LF
    let mut out = patch.replace("\r\n", "\n");

    // 2) convert literal escapes into real newlines if present
    if out.contains("\\r\\n") {
        out = out.replace("\\r\\n", "\n");
    }
    if out.contains("\\n") {
        out = out.replace("\\n", "\n");
    }

    // 3) ensure trailing newline
    if !out.ends_with('\n') {
        out.push('\n');
    }

    out
}

/// Map user-provided strings to TerminalShell.
fn parse_shell(s: Option<&str>) -> TerminalShell {
    let Some(s) = s else {
        return TerminalShell::Auto;
    };
    match s.trim().to_ascii_lowercase().as_str() {
        "auto" => TerminalShell::Auto,
        "powershell" | "pwsh" => TerminalShell::PowerShell,
        "bash" => TerminalShell::Bash,
        "zsh" => TerminalShell::Zsh,
        "sh" => TerminalShell::Sh,
        "cmd" => TerminalShell::Cmd,
        _ => TerminalShell::Auto,
    }
}
