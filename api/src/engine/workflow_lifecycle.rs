use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine,
    models::{RunStatus, WorkflowRun},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunMode {
    Automatic,
    Manual,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowContinuation {
    StayPaused,
    RunAutomatic,
    RunManual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WorkflowCommand {
    Run {
        mode: WorkflowRunMode,
    },
    Pause,
    Resume {
        mode: WorkflowRunMode,
    },
    MoveTo {
        step_id: String,
        continuation: WorkflowContinuation,
    },
    Cancel,
    Archive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowCommandEnvelope {
    pub command_id: Uuid,
    pub workflow_run_id: Uuid,
    pub command: WorkflowCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowSnapshot {
    pub workflow_run_id: Uuid,
    pub status: RunStatus,
    pub current_step_id: Option<String>,
}

impl From<&WorkflowRun> for WorkflowSnapshot {
    fn from(run: &WorkflowRun) -> Self {
        Self {
            workflow_run_id: run.id,
            status: run.status.clone(),
            current_step_id: run.current_step_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WorkflowCommandOutcome {
    Started {
        mode: WorkflowRunMode,
        workflow: WorkflowSnapshot,
    },
    AlreadyRunning {
        mode: WorkflowRunMode,
        workflow: WorkflowSnapshot,
    },
    Paused {
        workflow: WorkflowSnapshot,
    },
    AlreadyPaused {
        workflow: WorkflowSnapshot,
    },
    Resumed {
        mode: WorkflowRunMode,
        workflow: WorkflowSnapshot,
    },
    Moved {
        step_id: String,
        continuation: WorkflowContinuation,
        workflow: WorkflowSnapshot,
    },
    AlreadyAtTarget {
        step_id: String,
        workflow: WorkflowSnapshot,
    },
    Cancelled {
        workflow: WorkflowSnapshot,
    },
    AlreadyCancelled {
        workflow: WorkflowSnapshot,
    },
    Archived {
        workflow: WorkflowSnapshot,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowCommandResult {
    pub workflow_run_id: Uuid,
    pub command_id: Uuid,
    pub outcome: WorkflowCommandOutcome,
}

#[derive(Clone, Default)]
pub struct WorkflowCoordinator {
    workflows: Arc<DashMap<Uuid, Arc<WorkflowGuard>>>,
}

struct WorkflowGuard {
    command_lock: Mutex<()>,
    runtime: Mutex<WorkflowRuntimeState>,
    completed_commands: Mutex<HashMap<Uuid, WorkflowCommandResult>>,
}

#[derive(Default)]
struct WorkflowRuntimeState {
    execution_task: Option<tokio::task::JoinHandle<()>>,
    cancellation: Option<CancellationToken>,
}

impl Default for WorkflowGuard {
    fn default() -> Self {
        Self {
            command_lock: Mutex::new(()),
            runtime: Mutex::new(WorkflowRuntimeState::default()),
            completed_commands: Mutex::new(HashMap::new()),
        }
    }
}

impl WorkflowCoordinator {
    fn guard_for(&self, workflow_run_id: Uuid) -> Arc<WorkflowGuard> {
        self.workflows
            .entry(workflow_run_id)
            .or_insert_with(|| Arc::new(WorkflowGuard::default()))
            .clone()
    }

    pub async fn stop_active_executions(&self) -> Vec<Uuid> {
        let guards = self
            .workflows
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect::<Vec<_>>();

        let mut active_run_ids = Vec::new();

        for (workflow_run_id, guard) in guards {
            let mut runtime = guard.runtime.lock().await;
            let is_active = runtime
                .execution_task
                .as_ref()
                .is_some_and(|task| !task.is_finished());

            if !is_active {
                continue;
            }

            active_run_ids.push(workflow_run_id);

            if let Some(cancellation) = runtime.cancellation.take() {
                cancellation.cancel();
            }

            if let Some(task) = runtime.execution_task.take() {
                task.abort();
            }
        }

        active_run_ids
    }

    pub async fn execute(
        &self,
        state: &AppState,
        envelope: WorkflowCommandEnvelope,
    ) -> Result<WorkflowCommandResult> {
        let guard = self.guard_for(envelope.workflow_run_id);
        let _command_lock = guard.command_lock.lock().await;

        if let Some(result) = guard
            .completed_commands
            .lock()
            .await
            .get(&envelope.command_id)
            .cloned()
        {
            return Ok(result);
        }

        let result = execute_locked_command(state, &guard, &envelope).await?;
        guard
            .completed_commands
            .lock()
            .await
            .insert(envelope.command_id, result.clone());

        Ok(result)
    }
}

async fn stop_execution_task(guard: &WorkflowGuard) {
    let mut runtime = guard.runtime.lock().await;

    if let Some(cancellation) = runtime.cancellation.take() {
        cancellation.cancel();
    }

    if let Some(task) = runtime.execution_task.take() {
        task.abort();
    }
}

async fn spawn_automatic_execution(
    state: &AppState,
    guard: &WorkflowGuard,
    workflow_run_id: Uuid,
) -> Result<bool> {
    let mut runtime = guard.runtime.lock().await;

    if runtime
        .execution_task
        .as_ref()
        .is_some_and(|task| !task.is_finished())
    {
        return Ok(false);
    }

    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task_state = state.clone();

    runtime.cancellation = Some(cancellation);
    runtime.execution_task = Some(tokio::spawn(async move {
        if task_cancellation.is_cancelled() {
            return;
        }

        if let Err(error) = engine::start_run(&task_state, workflow_run_id, None).await {
            tracing::error!(
                workflow_run_id = %workflow_run_id,
                error = %format!("{:#}", error),
                "automatic workflow execution failed"
            );
        }
    }));

    Ok(true)
}

async fn reload_snapshot(state: &AppState, workflow_run_id: Uuid) -> Result<WorkflowSnapshot> {
    let run = engine::load_run(state, workflow_run_id).await?;
    Ok(WorkflowSnapshot::from(&run))
}

async fn run_workflow(
    state: &AppState,
    guard: &WorkflowGuard,
    run: &WorkflowRun,
    mode: WorkflowRunMode,
) -> Result<WorkflowCommandOutcome> {
    if matches!(run.status, RunStatus::Running) {
        return Ok(WorkflowCommandOutcome::AlreadyRunning {
            mode,
            workflow: WorkflowSnapshot::from(run),
        });
    }

    match mode {
        WorkflowRunMode::Automatic => {
            if !spawn_automatic_execution(state, guard, run.id).await? {
                return Ok(WorkflowCommandOutcome::AlreadyRunning {
                    mode,
                    workflow: reload_snapshot(state, run.id).await?,
                });
            }
        }
        WorkflowRunMode::Manual => {
            engine::run_step(state, run.id, run.current_step_id.as_deref()).await?;
        }
    }

    Ok(WorkflowCommandOutcome::Started {
        mode,
        workflow: reload_snapshot(state, run.id).await?,
    })
}

async fn resume_workflow(
    state: &AppState,
    guard: &WorkflowGuard,
    run: &WorkflowRun,
    mode: WorkflowRunMode,
) -> Result<WorkflowCommandOutcome> {
    if matches!(run.status, RunStatus::Running) {
        return Ok(WorkflowCommandOutcome::AlreadyRunning {
            mode,
            workflow: WorkflowSnapshot::from(run),
        });
    }

    engine::resume_run(state, run.id).await?;

    match mode {
        WorkflowRunMode::Automatic => {
            let _started = spawn_automatic_execution(state, guard, run.id).await?;
        }
        WorkflowRunMode::Manual => {}
    }

    Ok(WorkflowCommandOutcome::Resumed {
        mode,
        workflow: reload_snapshot(state, run.id).await?,
    })
}

async fn move_workflow(
    state: &AppState,
    guard: &WorkflowGuard,
    run: &WorkflowRun,
    step_id: &str,
    continuation: WorkflowContinuation,
) -> Result<WorkflowCommandOutcome> {
    if run.current_step_id.as_deref() == Some(step_id) {
        return Ok(WorkflowCommandOutcome::AlreadyAtTarget {
            step_id: step_id.to_string(),
            workflow: WorkflowSnapshot::from(run),
        });
    }

    stop_execution_task(guard).await;
    engine::select_step(state, run.id, step_id).await?;

    match continuation {
        WorkflowContinuation::StayPaused => {
            engine::pause_run(state, run.id).await?;
        }
        WorkflowContinuation::RunAutomatic => {
            engine::resume_run(state, run.id).await?;
            let _started = spawn_automatic_execution(state, guard, run.id).await?;
        }
        WorkflowContinuation::RunManual => {
            engine::resume_run(state, run.id).await?;
            engine::run_step(state, run.id, Some(step_id)).await?;
        }
    }

    Ok(WorkflowCommandOutcome::Moved {
        step_id: step_id.to_string(),
        continuation,
        workflow: reload_snapshot(state, run.id).await?,
    })
}

async fn execute_locked_command(
    state: &AppState,
    guard: &WorkflowGuard,
    envelope: &WorkflowCommandEnvelope,
) -> Result<WorkflowCommandResult> {
    let run = engine::load_run(state, envelope.workflow_run_id).await?;

    let outcome = match &envelope.command {
        WorkflowCommand::Run { mode } => {
            run_workflow(state, guard, &run, *mode).await?
        }
        WorkflowCommand::Pause => {
            if matches!(run.status, RunStatus::Paused) {
                WorkflowCommandOutcome::AlreadyPaused {
                    workflow: WorkflowSnapshot::from(&run),
                }
            } else {
                stop_execution_task(guard).await;
                engine::pause_run(state, run.id).await?;
                WorkflowCommandOutcome::Paused {
                    workflow: reload_snapshot(state, run.id).await?,
                }
            }
        }
        WorkflowCommand::Resume { mode } => {
            resume_workflow(state, guard, &run, *mode).await?
        }
        WorkflowCommand::MoveTo {
            step_id,
            continuation,
        } => {
            move_workflow(state, guard, &run, step_id, *continuation).await?
        }
        WorkflowCommand::Cancel => {
            if matches!(run.status, RunStatus::Cancelled) {
                WorkflowCommandOutcome::AlreadyCancelled {
                    workflow: WorkflowSnapshot::from(&run),
                }
            } else {
                stop_execution_task(guard).await;
                engine::force_wait_run(state, run.id).await?;
                WorkflowCommandOutcome::Cancelled {
                    workflow: reload_snapshot(state, run.id).await?,
                }
            }
        }
        WorkflowCommand::Archive => {
            stop_execution_task(guard).await;
            engine::force_wait_run(state, run.id).await?;
            WorkflowCommandOutcome::Archived {
                workflow: reload_snapshot(state, run.id).await?,
            }
        }
    };

    Ok(WorkflowCommandResult {
        workflow_run_id: envelope.workflow_run_id,
        command_id: envelope.command_id,
        outcome,
    })
}

pub async fn execute_workflow_command(
    state: &AppState,
    envelope: WorkflowCommandEnvelope,
) -> Result<WorkflowCommandResult> {
    if envelope.workflow_run_id.is_nil() {
        return Err(anyhow!("workflow_run_id is required"));
    }

    state.workflow_coordinator.execute(state, envelope).await
}
