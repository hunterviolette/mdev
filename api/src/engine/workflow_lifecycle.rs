use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine,
    models::{RunStatus, WorkflowRun},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowExecutionMode {
    SingleStage,
    MultiStage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WorkflowCommand {
    Start {
        mode: WorkflowExecutionMode,
        step_id: Option<String>,
    },
    Pause,
    Resume,
    ResolveCheckpoint {
        disposition: String,
        selected_step_id: Option<String>,
    },
    MoveTo {
        step_id: String,
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
    Accepted {
        command: String,
        mode: Option<WorkflowExecutionMode>,
        workflow: WorkflowSnapshot,
    },
    AlreadyActive {
        workflow: WorkflowSnapshot,
    },
    AlreadyPaused {
        workflow: WorkflowSnapshot,
    },
    AlreadyCancelled {
        workflow: WorkflowSnapshot,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowCommandResult {
    pub workflow_run_id: Uuid,
    pub command_id: Uuid,
    pub outcome: WorkflowCommandOutcome,
}

#[derive(Debug)]
pub(crate) enum WorkflowRuntimeCommand {
    Start {
        mode: WorkflowExecutionMode,
        step_id: Option<String>,
    },
    Pause,
    Resume,
    ResolveCheckpoint {
        disposition: String,
        selected_step_id: Option<String>,
    },
    MoveTo {
        step_id: String,
    },
    Cancel {
        reason: String,
    },
    Archive,
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
    execution_task: Option<std::thread::JoinHandle<()>>,
    command_tx: Option<mpsc::Sender<WorkflowRuntimeCommand>>,
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

    async fn ensure_runtime(
        &self,
        state: &AppState,
        workflow_run_id: Uuid,
    ) -> Result<mpsc::Sender<WorkflowRuntimeCommand>> {
        let guard = self.guard_for(workflow_run_id);
        let mut runtime = guard.runtime.lock().await;

        if runtime
            .execution_task
            .as_ref()
            .is_some_and(|task| !task.is_finished())
        {
            return runtime
                .command_tx
                .clone()
                .ok_or_else(|| anyhow!("workflow runtime command channel is unavailable"));
        }

        let (command_tx, command_rx) = mpsc::channel(64);
        let cancellation = runtime
            .cancellation
            .take()
            .filter(|token| !token.is_cancelled())
            .unwrap_or_else(CancellationToken::new);
        let task_state = state.clone();
        let task_cancellation = cancellation.clone();

        let execution_task = std::thread::Builder::new()
            .name(format!("workflow-runtime-{}", workflow_run_id))
            .spawn(move || {
                let workflow_runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        tracing::error!(
                            workflow_run_id = %workflow_run_id,
                            error = %error,
                            "failed to create workflow runtime"
                        );
                        return;
                    }
                };

                workflow_runtime.block_on(async move {
                    if let Err(error) = engine::run_workflow_runtime(
                        &task_state,
                        workflow_run_id,
                        command_rx,
                        task_cancellation,
                    )
                    .await
                    {
                        let error_message = format!("{:#}", error);

                        tracing::error!(
                            workflow_run_id = %workflow_run_id,
                            error = %error_message,
                            "workflow runtime failed"
                        );

                        let _ = engine::fail_runtime_workflow(
                            &task_state,
                            workflow_run_id,
                            "workflow_runtime_failed",
                            error_message.as_str(),
                        )
                        .await;
                    }
                });
            })
            .map_err(|error| anyhow!("failed to spawn workflow runtime thread: {}", error))?;

        runtime.command_tx = Some(command_tx.clone());
        runtime.cancellation = Some(cancellation);
        runtime.execution_task = Some(execution_task);

        Ok(command_tx)
    }

    pub async fn execution_token(&self, workflow_run_id: Uuid) -> CancellationToken {
        let guard = self.guard_for(workflow_run_id);
        let mut runtime = guard.runtime.lock().await;
        if let Some(token) = runtime
            .cancellation
            .as_ref()
            .filter(|token| !token.is_cancelled())
        {
            return token.clone();
        }
        let token = CancellationToken::new();
        runtime.cancellation = Some(token.clone());
        token
    }

    pub async fn cancel_execution(&self, workflow_run_id: Uuid) {
        let guard = self.guard_for(workflow_run_id);
        let runtime = guard.runtime.lock().await;
        if let Some(cancellation) = runtime.cancellation.as_ref() {
            cancellation.cancel();
        }
    }

    pub async fn stop_active_executions(
        &self,
        state: &AppState,
        reason: &str,
    ) -> Vec<Uuid> {
        let guards = self
            .workflows
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect::<Vec<_>>();

        let mut active_run_ids = Vec::new();

        for (workflow_run_id, guard) in guards {
            let runtime = guard.runtime.lock().await;
            let active = runtime
                .execution_task
                .as_ref()
                .is_some_and(|task| !task.is_finished());

            if !active {
                continue;
            }

            active_run_ids.push(workflow_run_id);

            if let Some(cancellation) = runtime.cancellation.as_ref() {
                cancellation.cancel();
            }

            if let Some(command_tx) = runtime.command_tx.clone() {
                let _ = command_tx
                    .send(WorkflowRuntimeCommand::Cancel {
                        reason: reason.to_string(),
                    })
                    .await;
            }
        }

        for workflow_run_id in &active_run_ids {
            let _ = engine::fail_runtime_workflow(
                state,
                *workflow_run_id,
                "api_shutdown",
                reason,
            )
            .await;
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

        let run = engine::load_run(state, envelope.workflow_run_id).await?;
        let outcome = match &envelope.command {
            WorkflowCommand::Start { mode, step_id } => {
                if matches!(run.status, RunStatus::Queued | RunStatus::Running) {
                    WorkflowCommandOutcome::AlreadyActive {
                        workflow: WorkflowSnapshot::from(&run),
                    }
                } else {
                    let command_tx = self
                        .ensure_runtime(state, envelope.workflow_run_id)
                        .await?;
                    command_tx
                        .send(WorkflowRuntimeCommand::Start {
                            mode: *mode,
                            step_id: step_id.clone(),
                        })
                        .await
                        .map_err(|_| anyhow!("workflow runtime command channel closed"))?;
                    WorkflowCommandOutcome::Accepted {
                        command: "start".to_string(),
                        mode: Some(*mode),
                        workflow: reload_snapshot(state, envelope.workflow_run_id).await?,
                    }
                }
            }
            WorkflowCommand::Pause => {
                if matches!(run.status, RunStatus::Paused) {
                    WorkflowCommandOutcome::AlreadyPaused {
                        workflow: WorkflowSnapshot::from(&run),
                    }
                } else {
                    self.ensure_runtime(state, envelope.workflow_run_id)
                        .await?
                        .send(WorkflowRuntimeCommand::Pause)
                        .await
                        .map_err(|_| anyhow!("workflow runtime command channel closed"))?;
                    WorkflowCommandOutcome::Accepted {
                        command: "pause".to_string(),
                        mode: None,
                        workflow: reload_snapshot(state, envelope.workflow_run_id).await?,
                    }
                }
            }
            WorkflowCommand::Resume => {
                self.ensure_runtime(state, envelope.workflow_run_id)
                    .await?
                    .send(WorkflowRuntimeCommand::Resume)
                    .await
                    .map_err(|_| anyhow!("workflow runtime command channel closed"))?;
                WorkflowCommandOutcome::Accepted {
                    command: "resume".to_string(),
                    mode: None,
                    workflow: reload_snapshot(state, envelope.workflow_run_id).await?,
                }
            }
            WorkflowCommand::ResolveCheckpoint {
                disposition,
                selected_step_id,
            } => {
                self.ensure_runtime(state, envelope.workflow_run_id)
                    .await?
                    .send(WorkflowRuntimeCommand::ResolveCheckpoint {
                        disposition: disposition.clone(),
                        selected_step_id: selected_step_id.clone(),
                    })
                    .await
                    .map_err(|_| anyhow!("workflow runtime command channel closed"))?;
                WorkflowCommandOutcome::Accepted {
                    command: "resolve_checkpoint".to_string(),
                    mode: None,
                    workflow: reload_snapshot(state, envelope.workflow_run_id).await?,
                }
            }
            WorkflowCommand::MoveTo { step_id } => {
                self.ensure_runtime(state, envelope.workflow_run_id)
                    .await?
                    .send(WorkflowRuntimeCommand::MoveTo {
                        step_id: step_id.clone(),
                    })
                    .await
                    .map_err(|_| anyhow!("workflow runtime command channel closed"))?;
                WorkflowCommandOutcome::Accepted {
                    command: "move_to".to_string(),
                    mode: None,
                    workflow: reload_snapshot(state, envelope.workflow_run_id).await?,
                }
            }
            WorkflowCommand::Cancel => {
                if matches!(run.status, RunStatus::Cancelled) {
                    WorkflowCommandOutcome::AlreadyCancelled {
                        workflow: WorkflowSnapshot::from(&run),
                    }
                } else {
                    let command_tx = self
                        .ensure_runtime(state, envelope.workflow_run_id)
                        .await?;
                    self.cancel_execution(envelope.workflow_run_id).await;
                    let _ = command_tx
                        .send(WorkflowRuntimeCommand::Cancel {
                            reason: "user_cancelled".to_string(),
                        })
                        .await;
                    WorkflowCommandOutcome::Accepted {
                        command: "cancel".to_string(),
                        mode: None,
                        workflow: reload_snapshot(state, envelope.workflow_run_id).await?,
                    }
                }
            }
            WorkflowCommand::Archive => {
                self.ensure_runtime(state, envelope.workflow_run_id)
                    .await?
                    .send(WorkflowRuntimeCommand::Archive)
                    .await
                    .map_err(|_| anyhow!("workflow runtime command channel closed"))?;
                WorkflowCommandOutcome::Accepted {
                    command: "archive".to_string(),
                    mode: None,
                    workflow: reload_snapshot(state, envelope.workflow_run_id).await?,
                }
            }
        };

        let result = WorkflowCommandResult {
            workflow_run_id: envelope.workflow_run_id,
            command_id: envelope.command_id,
            outcome,
        };

        guard
            .completed_commands
            .lock()
            .await
            .insert(envelope.command_id, result.clone());

        Ok(result)
    }
}

async fn reload_snapshot(
    state: &AppState,
    workflow_run_id: Uuid,
) -> Result<WorkflowSnapshot> {
    let run = engine::load_run(state, workflow_run_id).await?;
    Ok(WorkflowSnapshot::from(&run))
}

pub async fn execute_workflow_command_value(
    state: &AppState,
    workflow_run_id: Uuid,
    command: WorkflowCommand,
) -> Result<Value> {
    serde_json::to_value(
        execute_workflow_command(
            state,
            WorkflowCommandEnvelope {
                command_id: Uuid::new_v4(),
                workflow_run_id,
                command,
            },
        )
        .await?,
    )
    .map_err(Into::into)
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

pub(crate) fn command_payload(command: &WorkflowRuntimeCommand) -> Value {
    match command {
        WorkflowRuntimeCommand::Start { mode, step_id } => serde_json::json!({
            "kind": "start",
            "mode": mode,
            "step_id": step_id
        }),
        WorkflowRuntimeCommand::Pause => serde_json::json!({ "kind": "pause" }),
        WorkflowRuntimeCommand::Resume => serde_json::json!({ "kind": "resume" }),
        WorkflowRuntimeCommand::ResolveCheckpoint {
            disposition,
            selected_step_id,
        } => serde_json::json!({
            "kind": "resolve_checkpoint",
            "disposition": disposition,
            "selected_step_id": selected_step_id
        }),
        WorkflowRuntimeCommand::MoveTo { step_id } => serde_json::json!({
            "kind": "move_to",
            "step_id": step_id
        }),
        WorkflowRuntimeCommand::Cancel { reason } => serde_json::json!({
            "kind": "cancel",
            "reason": reason
        }),
        WorkflowRuntimeCommand::Archive => serde_json::json!({ "kind": "archive" }),
    }
}
