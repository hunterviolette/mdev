use anyhow::{Context, Result};
use regex::Regex;
use std::sync::Arc;

use crate::analyze;
use crate::app::state::WORKTREE_REF;
use crate::git;
use crate::platform::Platform;

use super::types::*;

pub struct CapabilityBroker {
    platform: Arc<dyn Platform>,
}

impl CapabilityBroker {
    pub fn new(platform: Arc<dyn Platform>) -> Self {
        Self { platform }
    }

    pub fn exec(&self, req: CapabilityRequest) -> Result<CapabilityResponse> {
        match req {
            CapabilityRequest::EnsureGitRepo { repo } => {
                git::ensure_git_repo(&repo)?;
                Ok(CapabilityResponse::Unit)
            }

            CapabilityRequest::ListGitRefs { repo } => {
                let refs = git::list_git_refs_for_dropdown(&repo)?;
                Ok(CapabilityResponse::GitRefs(refs))
            }

            CapabilityRequest::AnalyzeRepo {
                repo,
                git_ref,
                exclude_regex,
                max_exts,
            } => {
                let compiled = compile_excludes(&exclude_regex)?;
                let res = analyze::analyze_repo(&repo, &git_ref, &compiled, max_exts)?;
                Ok(CapabilityResponse::Analysis(res))
            }

            CapabilityRequest::ReadFile { repo, path, source } => {
                let bytes = match source {
                    FileSource::Worktree => git::read_worktree_file(&repo, &path)
                        .with_context(|| format!("read_worktree_file failed for {path}"))?,
                    FileSource::GitRef(r) => {
                        let spec = format!("{r}:{path}");
                        git::show_file_at(&repo, &spec)
                            .with_context(|| format!("git show failed for {spec}"))?
                    }
                };
                Ok(CapabilityResponse::Bytes(bytes))
            }

            CapabilityRequest::WriteWorktreeFile {
                repo,
                path,
                contents,
            } => {
                git::write_worktree_file(&repo, &path, &contents)
                    .with_context(|| format!("write_worktree_file failed for {path}"))?;
                Ok(CapabilityResponse::Unit)
            }

            CapabilityRequest::FileHistory { repo, path, max } => {
                let bytes = git::file_history(&repo, &path, max)
                    .with_context(|| format!("file_history failed for {path}"))?;
                Ok(CapabilityResponse::Bytes(bytes))
            }

            CapabilityRequest::DiffFileBetween {
                repo,
                from_ref,
                to_ref,
                path,
            } => {
                let bytes = git::diff_file_between(&repo, &from_ref, &to_ref, &path)
                    .with_context(|| format!("diff_file_between failed for {path}"))?;
                Ok(CapabilityResponse::Bytes(bytes))
            }

            CapabilityRequest::ExportContext(r) => {
                let compiled = compile_excludes(&r.exclude_regex)?;
                let opts = git::ContextExportOptions {
                    git_ref: &r.git_ref,
                    exclude: &compiled,
                    max_bytes_per_file: r.max_bytes_per_file,
                    skip_binary: r.skip_binary,
                    include_files: r.include_files.as_deref(),
                };
                git::export_repo_context(&r.repo, &r.out_path, opts)
                    .with_context(|| "export_repo_context failed")?;
                Ok(CapabilityResponse::Unit)
            }

            CapabilityRequest::RunShellCommand { shell, cmd, cwd } => {
                let out = self
                    .platform
                    .run_shell_command(shell, &cmd, cwd)
                    .with_context(|| "run_shell_command failed")?;
                Ok(CapabilityResponse::ShellOutput {
                    code: out.code,
                    stdout: out.stdout,
                    stderr: out.stderr,
                })
            }
        }
    }

    /// Convenience: map top-bar git_ref to a FileSource for file reads.
    pub fn file_source_from_ref(git_ref: &str) -> FileSource {
        if git_ref == WORKTREE_REF {
            FileSource::Worktree
        } else {
            FileSource::GitRef(git_ref.to_string())
        }
    }
}

fn compile_excludes(patterns: &[String]) -> Result<Vec<Regex>> {
    let mut out = Vec::new();
    for p in patterns {
        out.push(Regex::new(p).with_context(|| format!("Bad exclude regex '{p}'"))?);
    }
    Ok(out)
}
