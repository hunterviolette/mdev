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

            CapabilityRequest::DeleteWorktreePath { repo, path } => {
                git::delete_worktree_path(&repo, &path)
                    .with_context(|| format!("delete_worktree_path failed for {path}"))?;
                Ok(CapabilityResponse::Unit)
            }

            CapabilityRequest::MoveWorktreePath { repo, from, to } => {
                git::move_worktree_path(&repo, &from, &to)
                    .with_context(|| format!("move_worktree_path failed for {from} -> {to}"))?;
                Ok(CapabilityResponse::Unit)
            }

            CapabilityRequest::ApplyGitPatch { repo, patch } => {
                git::apply_git_patch(&repo, &patch)
                    .with_context(|| "apply_git_patch failed")?;
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

            // -----------------------------------------------------------------
            // Source control (git)
            // -----------------------------------------------------------------
            CapabilityRequest::GitStatus { repo } => {
                let st = git::git_status(&repo)?;
                Ok(CapabilityResponse::GitStatus(st))
            }

            CapabilityRequest::GitStagePaths { repo, paths } => {
                git::git_stage_paths(&repo, &paths)?;
                Ok(CapabilityResponse::Unit)
            }
            CapabilityRequest::GitUnstagePaths { repo, paths } => {
                git::git_unstage_paths(&repo, &paths)?;
                Ok(CapabilityResponse::Unit)
            }
            CapabilityRequest::GitStageAll { repo } => {
                git::git_stage_all(&repo)?;
                Ok(CapabilityResponse::Unit)
            }
            CapabilityRequest::GitUnstageAll { repo } => {
                git::git_unstage_all(&repo)?;
                Ok(CapabilityResponse::Unit)
            }

            CapabilityRequest::GitCurrentBranch { repo } => {
                let b = git::git_current_branch(&repo)?;
                Ok(CapabilityResponse::GitBranch(b))
            }
            CapabilityRequest::GitListLocalBranches { repo } => {
                let bs = git::git_list_local_branches(&repo)?;
                Ok(CapabilityResponse::GitBranches(bs))
            }
            CapabilityRequest::GitListRemotes { repo } => {
                let rs = git::git_list_remotes(&repo)?;
                Ok(CapabilityResponse::GitRemotes(rs))
            }

            CapabilityRequest::GitCheckoutBranch {
                repo,
                branch,
                create_if_missing,
            } => {
                let out = git::git_checkout_branch(&repo, &branch, create_if_missing)?;
                Ok(CapabilityResponse::Text(out))
            }

            CapabilityRequest::GitFetch { repo, remote } => {
                let out = git::git_fetch(&repo, remote.as_deref())?;
                Ok(CapabilityResponse::Text(out))
            }
            CapabilityRequest::GitPull {
                repo,
                remote,
                branch,
            } => {
                let out = git::git_pull(&repo, remote.as_deref(), branch.as_deref())?;
                Ok(CapabilityResponse::Text(out))
            }

            CapabilityRequest::GitCommit {
                repo,
                message,
                branch,
            } => {
                let out = git::git_commit(&repo, &message, branch.as_deref())?;
                Ok(CapabilityResponse::Text(out))
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
