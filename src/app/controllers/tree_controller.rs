use crate::app::actions::{Action, ExpandCmd};
use crate::app::state::{AppState, WORKTREE_REF};
use crate::capabilities::CapabilityRequest;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::ExpandAll => {
            state.tree.expand_cmd = Some(ExpandCmd::ExpandAll);
            true
        }
        Action::CollapseAll => {
            state.tree.expand_cmd = Some(ExpandCmd::CollapseAll);
            true
        }
        Action::OpenFile(path) => {
            state.deferred.open_file = Some(path.clone());
            state.deferred.open_file_target_viewer = state.active_file_viewer_id();
            true
        }
        Action::TreeDeletePath { path } => {
            if state.inputs.git_ref != WORKTREE_REF {
                return true;
            }
            let Some(repo) = state.inputs.repo.clone() else {
                return true;
            };
            let _ = state.broker.exec(CapabilityRequest::DeleteWorktreePath {
                repo,
                path: path.clone(),
            });
            state.tree.context_selected_files.remove(path);
            state.tree.rename_target = None;
            state.tree.rename_draft.clear();
            state.tree.create_parent = None;
            state.tree.create_draft.clear();
            state.tree.confirm_delete_target = None;
            state.start_analysis_refresh_async();
            true
        }
        Action::TreeRenamePath { from, to } => {
            if state.inputs.git_ref != WORKTREE_REF {
                return true;
            }
            let Some(repo) = state.inputs.repo.clone() else {
                return true;
            };
            let mut dst = to.trim().replace('\\', "/");
            if !dst.contains('/') {
                if let Some((parent, _)) = from.rsplit_once('/') {
                    dst = format!("{}/{}", parent, dst);
                }
            }
            if !dst.is_empty() && dst != *from {
                let _ = state.broker.exec(CapabilityRequest::MoveWorktreePath {
                    repo,
                    from: from.clone(),
                    to: dst,
                });
            }
            state.tree.rename_target = None;
            state.tree.rename_draft.clear();
            state.tree.create_parent = None;
            state.tree.create_draft.clear();
            state.tree.confirm_delete_target = None;
            state.start_analysis_refresh_async();
            true
        }
        Action::TreeCreateFile { path } => {
            if state.inputs.git_ref != WORKTREE_REF {
                return true;
            }
            let Some(repo) = state.inputs.repo.clone() else {
                return true;
            };
            let _ = state.broker.exec(CapabilityRequest::WriteWorktreeFile {
                repo,
                path: path.clone(),
                contents: Vec::new(),
            });
            state.tree.create_parent = None;
            state.start_analysis_refresh_async();
            true
        }
        Action::TreeCreateFolder { path } => {
            if state.inputs.git_ref != WORKTREE_REF {
                return true;
            }
            let Some(repo) = state.inputs.repo.clone() else {
                return true;
            };
            let _ = state.broker.exec(CapabilityRequest::CreateWorktreeDir {
                repo,
                path: path.clone(),
            });
            state.tree.create_parent = None;
            state.start_analysis_refresh_async();
            true
        }
        _ => false,
    }
}
