use crate::{format};

use super::actions::Action;
use super::state::AppState;

use super::controllers::{
    changeset_loop_controller,
    analysis_controller, changeset_controller, context_exporter_controller, diff_viewer_controller,
    file_viewer_controller, layout_controller, palette_controller, source_control_controller,
    terminal_controller, tree_controller, canvas_tint, workspace_controller,
    task_controller,
};

use crate::app::layout::{ExecuteLoopSnapshot, TaskSnapshot};
use crate::app::task_store::{RepoTaskStoreFile, repo_key_for_path, task_store_path};

impl AppState {
    fn task_store_dir(&self) -> anyhow::Result<std::path::PathBuf> {
        let mut dir = self.platform.app_data_dir("devApp")?;
        dir.push("task_store");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn repo_task_store_path(&self, repo: &std::path::PathBuf) -> anyhow::Result<std::path::PathBuf> {
        let dir = self.task_store_dir()?;
        Ok(task_store_path(&dir, repo))
    }

    fn atomic_write_json(path: &std::path::Path, text: &str) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Save ExecuteLoop + Task state globally per-repo.
    pub fn save_repo_task_store(&mut self) {
        let Some(repo) = self.inputs.repo.clone() else {
            self.task_store_dirty = false;
            return;
        };

        let path = match self.repo_task_store_path(&repo) {
            Ok(p) => p,
            Err(_) => return,
        };

        // Persist from repo-global snapshot store. ExecuteLoopState is ephemeral.
        let execute_loops = self.execute_loop_store.clone();

        let mut tasks = std::collections::HashMap::new();
        for (id, t) in self.tasks.iter() {
            tasks.insert(
                *id,
                TaskSnapshot {
                    bound_execute_loop: t.bound_execute_loop,
                    paused: t.paused,
                },
            );
        }

        let repo_key = repo_key_for_path(&repo);
        let file = RepoTaskStoreFile {
            version: 1,
            repo_key,
            execute_loops,
            tasks,
        };

        let Ok(text) = serde_json::to_string_pretty(&file) else {
            return;
        };

        let _ = Self::atomic_write_json(&path, &text);
        self.task_store_dirty = false;
    }

    /// Load ExecuteLoop + Task state globally per-repo.
    pub fn load_repo_task_store(&mut self) -> bool {
        let Some(repo) = self.inputs.repo.clone() else {
            return false;
        };

        let path = match self.repo_task_store_path(&repo) {
            Ok(p) => p,
            Err(_) => return false,
        };

        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => return false,
        };

        let parsed: RepoTaskStoreFile = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => return false,
        };

        // Load persisted ExecuteLoop snapshots into repo-global store.
        // Do NOT hydrate ExecuteLoopState here (on-demand view/controller).
        self.execute_loop_store = parsed.execute_loops.clone();

        // Hydrate tasks (existing component ids only; layout is workspace-owned)
        for (task_id, ts) in parsed.tasks.iter() {
            if !self
                .layout
                .components
                .iter()
                .any(|c| c.kind == crate::app::actions::ComponentKind::Task && c.id == *task_id)
            {
                continue;
            }

            self.tasks.insert(
                *task_id,
                crate::app::state::TaskState {
                    bound_execute_loop: ts.bound_execute_loop,
                    paused: ts.paused,
                },
            );

            if let Some(loop_id) = ts.bound_execute_loop {
                if let Some(ls) = self.execute_loops.get_mut(&loop_id) {
                    ls.paused = ts.paused;
                }
            }
        }

        self.task_store_dirty = false;
        true
    }


    /// Ensure ExecuteLoopState exists and is hydrated from execute_loop_store.
    pub fn ensure_execute_loop_state_loaded(&mut self, loop_id: crate::app::actions::ComponentId) {
        use crate::app::state::ExecuteLoopState;

        let st = self
            .execute_loops
            .entry(loop_id)
            .or_insert_with(ExecuteLoopState::new);

        if let Some(snap) = self.execute_loop_store.get(&loop_id) {
            st.model = snap.model.clone();
            st.instruction = snap.instruction.clone();
            st.mode = snap.mode;
            st.include_context_next = snap.include_context_next;
            st.auto_fill_first_changeset_applier = snap.auto_fill_first_changeset_applier;
            st.messages = snap.messages.clone();
            st.conversation_id = snap.conversation_id.clone();
            st.changesets_total = snap.changesets_total;
            st.changeset_auto = snap.changeset_auto;
            st.postprocess_cmd = snap.postprocess_cmd.clone();
            st.changesets_ok = snap.changesets_ok;
            st.changesets_err = snap.changesets_err;
            st.postprocess_ok = snap.postprocess_ok;
            st.postprocess_err = snap.postprocess_err;
            st.paused = snap.paused;
        }
    }

    /// Write-through current ExecuteLoopState into execute_loop_store.
    pub fn persist_execute_loop_snapshot(&mut self, loop_id: crate::app::actions::ComponentId) {
        let Some(st) = self.execute_loops.get(&loop_id) else {
            return;
        };

        self.execute_loop_store.insert(
            loop_id,
            ExecuteLoopSnapshot {
                model: st.model.clone(),
                instruction: st.instruction.clone(),
                mode: st.mode,
                include_context_next: st.include_context_next,
                auto_fill_first_changeset_applier: st.auto_fill_first_changeset_applier,
                messages: st.messages.clone(),
                conversation_id: st.conversation_id.clone(),
                paused: st.paused,
                changeset_auto: st.changeset_auto,
                postprocess_cmd: st.postprocess_cmd.clone(),
                changesets_total: st.changesets_total,
                changesets_ok: st.changesets_ok,
                changesets_err: st.changesets_err,
                postprocess_ok: st.postprocess_ok,
                postprocess_err: st.postprocess_err,
            },
        );
    }

    /// Ensure the ExecuteLoop component/window exists in the layout and is open.
    pub fn ensure_execute_loop_component_open(&mut self, loop_id: crate::app::actions::ComponentId) {
        use crate::app::actions::ComponentKind;
        use crate::app::layout::{ComponentInstance, WindowLayout};

        let exists = self
            .layout
            .components
            .iter()
            .any(|c| c.kind == ComponentKind::ExecuteLoop && c.id == loop_id);

        if !exists {
            self.layout.merge_with_defaults();
            self.layout.components.push(ComponentInstance {
                id: loop_id,
                kind: ComponentKind::ExecuteLoop,
                title: format!("Execute Loop {}", loop_id),
            });
            self.layout.windows.insert(
                loop_id,
                WindowLayout {
                    open: true,
                    locked: false,
                    pos: [150.0, 150.0],
                    size: [860.0, 680.0],
                },
            );
            self.layout_epoch = self.layout_epoch.wrapping_add(1);
        }

        if let Some(w) = self.layout.get_window_mut(loop_id) {
            w.open = true;
        }
    }
    pub fn apply_action(&mut self, action: Action) {
        // Keep ordering stable (global -> domain -> layout/workspace)
        if palette_controller::handle(self, &action) {
            return;
        }
        if canvas_tint::handle(self, &action) {
            return;
        }
        if analysis_controller::handle(self, &action) {
            return;
        }
        if changeset_controller::handle(self, &action) {
            return;
        }
        if changeset_loop_controller::handle(self, &action) {
            return;
        }

        if tree_controller::handle(self, &action) {
            return;
        }
        if file_viewer_controller::handle(self, &action) {
            return;
        }
        if terminal_controller::handle(self, &action) {
            return;
        }
        if context_exporter_controller::handle(self, &action) {
            return;
        }
        if source_control_controller::handle(self, &action) {
            return;
        }
        if diff_viewer_controller::handle(self, &action) {
            return;
        }
        if task_controller::handle(self, &action) {
            return;
        }
        if layout_controller::handle(self, &action) {
            return;
        }
        if workspace_controller::handle(self, &action) {
            return;
        }
    }

    pub fn finalize_frame(&mut self) {
        // Deferred effects (open file, select commit, refresh viewer)
        file_viewer_controller::finalize_frame(self);
    }

    // helpers used by UI (left here to avoid churn)
    pub fn excludes_joined(&self) -> String {
        format::join_excludes(&self.inputs.exclude_regex)
    }

    pub fn set_excludes_from_joined(&mut self, joined: &str) {
        self.inputs.exclude_regex = format::parse_excludes(joined);
    }
}
