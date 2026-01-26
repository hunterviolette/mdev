use crate::app::actions::Action;
use crate::app::state::AppState;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::TaskSetPaused { task_id, paused } => {
            if let Some(t) = state.tasks.get_mut(task_id) {
                t.paused = *paused;

                // Mirror pause onto the bound Execute Loop.
                if let Some(loop_id) = t.bound_execute_loop {
                    if let Some(ls) = state.execute_loops.get_mut(&loop_id) {
                        ls.paused = *paused;
                    }
                }

                // Write-through persistence
                state.task_store_dirty = true;
                state.save_repo_task_store();
            }
            true
        }

        Action::TaskBindExecuteLoop { task_id, loop_id } => {
            if let Some(t) = state.tasks.get_mut(task_id) {
                t.bound_execute_loop = Some(*loop_id);

                // Mirror current task pause onto the loop immediately.
                if let Some(ls) = state.execute_loops.get_mut(loop_id) {
                    ls.paused = t.paused;
                }

                // Write-through persistence
                state.task_store_dirty = true;
                state.save_repo_task_store();
            }
            true
        }

        Action::TaskCreateAndBindExecuteLoop { task_id } => {
            // Create a new chat thread (Execute Loop), bind it to the task, and open it.
            let new_id = state.new_execute_loop_component();

            // Baseline snapshot exists even if the window closes later.
            state.persist_execute_loop_snapshot(new_id);

            if let Some(t) = state.tasks.get_mut(task_id) {
                t.bound_execute_loop = Some(new_id);

                // Mirror task pause to the new loop.
                if let Some(ls) = state.execute_loops.get_mut(&new_id) {
                    ls.paused = t.paused;
                }

                // Write-through persistence
                state.task_store_dirty = true;
                state.save_repo_task_store();
            }

            if let Some(w) = state.layout.get_window_mut(new_id) {
                w.open = true;
            }

            true
        }

        Action::TaskOpenExecuteLoop { task_id } => {
            // If bound, open and hydrate the ExecuteLoop view on-demand.
            // If not bound, create a new ExecuteLoop, bind, persist snapshot, and open.
            let bound = state
                .tasks
                .get(task_id)
                .and_then(|t| t.bound_execute_loop);

            let loop_id = match bound {
                Some(id) => id,
                None => {
                    let new_id = state.new_execute_loop_component();

                    // Baseline snapshot exists even if the window closes later.
                    state.persist_execute_loop_snapshot(new_id);

                    if let Some(t) = state.tasks.get_mut(task_id) {
                        t.bound_execute_loop = Some(new_id);
                        if let Some(ls) = state.execute_loops.get_mut(&new_id) {
                            ls.paused = t.paused;
                        }

                        // Write-through persistence
                        state.task_store_dirty = true;
                        state.save_repo_task_store();
                    }
                    new_id
                }
            };

            // Ensure layout + ephemeral state exists, hydrate from snapshot store.
            state.ensure_execute_loop_component_open(loop_id);
            state.ensure_execute_loop_state_loaded(loop_id);

            if let Some(w) = state.layout.get_window_mut(loop_id) {
                w.open = true;
            }

            true
        }

        _ => false,
    }
}
