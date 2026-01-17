// src/app/controllers/changeset_controller.rs
//
// ChangeSet Applier controller.
// - Parses the JSON payload from the ChangeSet Applier UI component
// - Applies operations (write/move/delete/git_apply/edit)
// - Writes detailed diagnostics into the applier "status" output so git_apply issues
//   (corrupt patch / newline escaping / missing trailing newline) are debuggable.
//
// Key changes:
// - Add op: "edit" so the model provides anchored edits instead of unified diffs.
// - Make normalize_patch_text safer: only unescape "\\n" when the input has NO real newlines.

use crate::app::actions::{Action, ComponentId, ComponentKind, TerminalShell};
use crate::app::state::{AppState, WORKTREE_REF};
use crate::capabilities::{CapabilityRequest, CapabilityResponse, FileSource};

use anyhow::{anyhow, bail, Context, Result};
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

    /// Anchored edits applied against the current WORKTREE file text.
    /// The applier reads the file, applies the changes in-memory, then writes it back.
    #[serde(rename = "edit")]
    Edit { path: String, changes: Vec<EditChange> },
}

#[derive(Debug, Deserialize)]
struct EditChange {
    action: EditAction,
    #[serde(default, rename = "match")]
    match_: Option<EditMatch>,
    #[serde(default)]
    replacement: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EditAction {
    ReplaceBlock,
    InsertBefore,
    InsertAfter,
    DeleteBlock,
}

#[derive(Debug, Deserialize)]
struct EditMatch {
    #[serde(rename = "type")]
    match_type: String, // for now only "literal"
    text: String,
    #[serde(default)]
    occurrence: Option<usize>, // default 1
    #[serde(default)]
    mode: Option<String>, // "literal" (default) | "normalized_newlines"
    #[serde(default)]
    must_match: Option<String>, // "exactly_one" (default) | "at_least_one"
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

            Operation::Edit { path, changes } => {
                log.push_str(&format!("[{step}] edit {path}\n"));

                match apply_anchored_edits(state, &repo, path, changes) {
                    Ok(summary) => {
                        if !summary.is_empty() {
                            log.push_str(&summary);
                            if !log.ends_with('\n') {
                                log.push('\n');
                            }
                        }
                        log.push_str(&format!("[{step}] ok\n"));
                    }
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

/// Reads a WORKTREE file as UTF-8 text via the existing CapabilityRequest::ReadFile.
fn read_worktree_text(state: &mut AppState, repo: &std::path::PathBuf, path: &str) -> Result<String> {
    let resp = state
        .broker
        .exec(CapabilityRequest::ReadFile {
            repo: repo.clone(),
            path: path.to_string(),
            source: FileSource::Worktree,
        })
        .with_context(|| format!("ReadFile (worktree) failed for {path}"))?;

    let CapabilityResponse::Bytes(bytes) = resp else {
        bail!("Unexpected response reading {path}");
    };

    String::from_utf8(bytes).map_err(|e| anyhow!("File {path} is not valid UTF-8: {e}"))
}

fn write_worktree_text(state: &mut AppState, repo: &std::path::PathBuf, path: &str, text: &str) -> Result<()> {
    state
        .broker
        .exec(CapabilityRequest::WriteWorktreeFile {
            repo: repo.clone(),
            path: path.to_string(),
            contents: text.as_bytes().to_vec(),
        })
        .with_context(|| format!("WriteWorktreeFile failed for {path}"))?;
    Ok(())
}

/// Apply anchored edits to a worktree file and write back the result.
/// Returns a short human-readable summary for logs.
fn apply_anchored_edits(
    state: &mut AppState,
    repo: &std::path::PathBuf,
    path: &str,
    changes: &[EditChange],
) -> Result<String> {
    let original = read_worktree_text(state, repo, path)?;

    // We may normalize CRLF->LF for matching+writing depending on match.mode.
    // If any change requests normalized_newlines, we apply all edits on the LF-normalized text.
    let any_normalize = changes.iter().any(|ch| {
        ch.match_
            .as_ref()
            .map(|m| match_mode(m) == "normalized_newlines")
            .unwrap_or(false)
    });

    let mut text = if any_normalize {
        original.replace("\r\n", "\n")
    } else {
        original
    };

    let mut summary = String::new();

    for (i, ch) in changes.iter().enumerate() {
        let n = i + 1;
        let action = &ch.action;

        let m = ch
            .match_
            .as_ref()
            .ok_or_else(|| anyhow!("edit[{n}] missing 'match'"))?;
        ensure_literal_match(m, n)?;

        let mode = match_mode(m);
        let must = must_match_mode(m);

        validate_match_mode(n, mode)?;
        validate_must_match(n, must)?;

        // Match against either literal text or normalized-newlines text.
        let hay = if mode == "normalized_newlines" {
            text.replace("\r\n", "\n")
        } else {
            text.clone()
        };
        let needle = normalize_for_match(&m.text, mode);

        let found = count_occurrences(&hay, &needle);
        enforce_must_match(n, found, must)?;

        let occ = m.occurrence.unwrap_or(1);
        if occ == 0 {
            bail!("edit[{n}] match.occurrence must be >= 1");
        }
        if found > 0 && occ > found {
            bail!("edit[{n}] occurrence={occ} out of range (found {found})");
        }

        // Apply change using spans in the current text representation.
        // If mode==normalized_newlines and we are writing LF anyway, use LF spans.
        let (start, end) = find_match_span(&hay, &needle, occ)
            .ok_or_else(|| anyhow!("edit[{n}] match not found (occurrence={occ})"))?;

        match action {
            EditAction::ReplaceBlock => {
                let replacement = ch
                    .replacement
                    .as_ref()
                    .ok_or_else(|| anyhow!("edit[{n}] replace_block missing 'replacement'"))?;

                let mut base = hay;
                base.replace_range(start..end, replacement);
                text = base;

                summary.push_str(&format!("  - edit[{n}] replace_block (occurrence={occ}, must_match={must}, mode={mode})\n"));
            }
            EditAction::InsertBefore => {
                let insert_text = ch
                    .text
                    .as_ref()
                    .ok_or_else(|| anyhow!("edit[{n}] insert_before missing 'text'"))?;

                let mut base = hay;
                base.insert_str(start, insert_text);
                text = base;

                summary.push_str(&format!("  - edit[{n}] insert_before (occurrence={occ}, must_match={must}, mode={mode})\n"));
            }
            EditAction::InsertAfter => {
                let insert_text = ch
                    .text
                    .as_ref()
                    .ok_or_else(|| anyhow!("edit[{n}] insert_after missing 'text'"))?;

                let mut base = hay;
                base.insert_str(end, insert_text);
                text = base;

                summary.push_str(&format!("  - edit[{n}] insert_after (occurrence={occ}, must_match={must}, mode={mode})\n"));
            }
            EditAction::DeleteBlock => {
                let mut base = hay;
                base.replace_range(start..end, "");
                text = base;

                summary.push_str(&format!("  - edit[{n}] delete_block (occurrence={occ}, must_match={must}, mode={mode})\n"));
            }
        }
    }

    // If we normalized, we write LF back out (by design).
    write_worktree_text(state, repo, path, &text)?;
    Ok(summary)
}

fn match_mode(m: &EditMatch) -> &str {
    m.mode.as_deref().unwrap_or("literal")
}

fn validate_match_mode(idx: usize, mode: &str) -> Result<()> {
    match mode {
        "literal" | "normalized_newlines" => Ok(()),
        other => bail!(
            "edit[{idx}] unsupported match.mode '{other}' (supported: literal, normalized_newlines)"
        ),
    }
}

fn must_match_mode(m: &EditMatch) -> &str {
    m.must_match.as_deref().unwrap_or("exactly_one")
}

fn validate_must_match(idx: usize, must: &str) -> Result<()> {
    match must {
        "exactly_one" | "at_least_one" => Ok(()),
        other => bail!(
            "edit[{idx}] unsupported match.must_match '{other}' (supported: exactly_one, at_least_one)"
        ),
    }
}

fn normalize_for_match(s: &str, mode: &str) -> String {
    match mode {
        "normalized_newlines" => s.replace("\r\n", "\n"),
        _ => s.to_string(),
    }
}

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    haystack.match_indices(needle).count()
}

fn enforce_must_match(idx: usize, found: usize, must_match: &str) -> Result<()> {
    match must_match {
        "exactly_one" => {
            if found != 1 {
                bail!("edit[{idx}] must_match=exactly_one violated (found {found})");
            }
        }
        "at_least_one" => {
            if found < 1 {
                bail!("edit[{idx}] must_match=at_least_one violated (found {found})");
            }
        }
        other => {
            bail!("edit[{idx}] unsupported must_match '{other}' (supported: exactly_one, at_least_one)");
        }
    }
    Ok(())
}

fn ensure_literal_match(m: &EditMatch, idx: usize) -> Result<()> {
    if m.match_type.trim().to_ascii_lowercase() != "literal" {
        bail!("edit[{idx}] only match.type='literal' is supported right now");
    }
    if m.text.is_empty() {
        bail!("edit[{idx}] match.text must not be empty");
    }
    let occ = m.occurrence.unwrap_or(1);
    if occ == 0 {
        bail!("edit[{idx}] match.occurrence must be >= 1");
    }
    Ok(())
}

/// Returns the byte span of the Nth occurrence (1-based) of `needle` in `haystack`.
fn find_match_span(haystack: &str, needle: &str, occurrence: usize) -> Option<(usize, usize)> {
    if occurrence == 0 {
        return None;
    }
    let mut count = 0usize;
    for (start, _) in haystack.match_indices(needle) {
        count += 1;
        if count == occurrence {
            return Some((start, start + needle.len()));
        }
    }
    None
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
/// - Only unescape literal "\\n" / "\\r\\n" when the input appears to be fully escaped
///   (i.e., NO real newlines exist). This avoids corrupting patches (or code) that legitimately
///   contain the two-character sequence "\n" in context lines.
/// - Ensure a trailing newline (git apply can be finicky without it)
fn normalize_patch_text(patch: &str) -> String {
    // 1) normalize CRLF -> LF
    let mut out = patch.replace("\r\n", "\n");

    // 2) only convert literal escapes if patch has no real newlines
    let has_real_newlines = out.contains('\n');
    let has_literal_backslash_n = out.contains("\\n") || out.contains("\\r\\n");
    if !has_real_newlines && has_literal_backslash_n {
        out = out.replace("\\r\\n", "\n");
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
