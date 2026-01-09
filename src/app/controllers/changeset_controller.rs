// src/app/controllers/changeset_controller.rs
use crate::app::actions::{Action, ComponentId, TerminalShell};
use crate::app::state::{AppState, ChangeSetApplierState};
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
    shell: Option<TerminalShell>,
    cmd: String,
    #[serde(default)]
    cwd: Option<String>,
}

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::ClearChangeSet { applier_id } => {
            if let Some(st) = state.changeset_appliers.get_mut(applier_id) {
                st.payload.clear();
                st.status = None;
            }
            true
        }

        Action::ApplyChangeSet { applier_id } => {
            apply_changeset(state, *applier_id);
            true
        }

        _ => false,
    }
}

impl AppState {
    /// Called by layout/workspace controllers after layout changes.
    pub fn rebuild_changeset_appliers_from_layout(&mut self) {
        self.changeset_appliers.clear();

        let ids: Vec<ComponentId> = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == crate::app::actions::ComponentKind::ChangeSetApplier)
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

fn normalize_patch_text(patch: &str) -> String {
    // 1) Normalize CRLF -> LF (safe even if already LF)
    let mut p = patch.replace("\r\n", "\n");

    // 2) If the patch has *no real newlines* but does contain literal "\\n",
    //    the payload likely double-escaped. Convert those into real newlines.
    //    This is the most common cause of `git apply` reporting "corrupt patch".
    if !p.contains('\n') && p.contains("\\n") {
        p = p.replace("\\r\\n", "\n");
        p = p.replace("\\n", "\n");
    }

    p
}

fn apply_changeset(state: &mut AppState, applier_id: ComponentId) {
    let Some(repo) = state.inputs.repo.clone() else {
        if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
            st.status = Some("No repo selected. Pick a repo first.".into());
        }
        return;
    };

    let payload_text = match state.changeset_appliers.get(&applier_id) {
        Some(st) => st.payload.clone(),
        None => return,
    };

    let parsed: ChangeSetPayload = match serde_json::from_str(&payload_text) {
        Ok(p) => p,
        Err(e) => {
            if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
                st.status = Some(format!("Invalid JSON: {e}"));
            }
            return;
        }
    };

    if parsed.version != 1 {
        if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
            st.status = Some(format!(
                "Unsupported payload version {} (expected 1).",
                parsed.version
            ));
        }
        return;
    }

    let mut log = String::new();
    if let Some(d) = &parsed.description {
        log.push_str(&format!("description: {d}\n\n"));
    }

    // Apply operations
    for (i, op) in parsed.operations.iter().enumerate() {
        let step = i + 1;
        let r = (|| -> anyhow::Result<()> {
            match op {
                Operation::Write { path, contents } => {
                    state.broker.exec(CapabilityRequest::WriteWorktreeFile {
                        repo: repo.clone(),
                        path: path.clone(),
                        contents: contents.as_bytes().to_vec(),
                    })?;
                    Ok(())
                }
                Operation::Delete { path } => {
                    state.broker.exec(CapabilityRequest::DeleteWorktreePath {
                        repo: repo.clone(),
                        path: path.clone(),
                    })?;
                    Ok(())
                }
                Operation::Move { from, to } => {
                    state.broker.exec(CapabilityRequest::MoveWorktreePath {
                        repo: repo.clone(),
                        from: from.clone(),
                        to: to.clone(),
                    })?;
                    Ok(())
                }
                Operation::GitApply { patch } => {
                    let patch_norm = normalize_patch_text(patch);
                    state.broker.exec(CapabilityRequest::ApplyGitPatch {
                        repo: repo.clone(),
                        patch: patch_norm,
                    })?;
                    Ok(())
                }
            }
        })();

        match r {
            Ok(_) => {
                log.push_str(&format!("[{step}] ok\n"));
            }
            Err(e) => {
                log.push_str(&format!("[{step}] FAILED: {:#}\n", e));
                if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
                    st.status = Some(log);
                }
                return;
            }
        }
    }

    // Post commands (optional)
    for cmd in &parsed.post_commands {
        let shell = cmd.shell.clone().unwrap_or(TerminalShell::Auto);
        let cwd = cmd
            .cwd
            .as_ref()
            .and_then(|s| if s.trim().is_empty() { None } else { Some(repo.join(s)) });

        log.push_str(&format!("\n$ {}\n", cmd.cmd));
        match state.broker.exec(CapabilityRequest::RunShellCommand {
            shell,
            cmd: cmd.cmd.clone(),
            cwd,
        }) {
            Ok(CapabilityResponse::ShellOutput { code, stdout, stderr }) => {
                log.push_str(&stdout);
                if !stdout.ends_with('\n') && !stdout.is_empty() {
                    log.push('\n');
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

    if let Some(st) = state.changeset_appliers.get_mut(&applier_id) {
        st.status = Some(log);
    }
}
