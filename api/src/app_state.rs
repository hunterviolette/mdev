use std::sync::Arc;

use dashmap::DashMap;
use sqlx::SqlitePool;
use tokio::sync::{broadcast, Mutex, OwnedMutexGuard};
use uuid::Uuid;

use crate::engine::capabilities::{
    operator_checkpoint::OperatorInputRegistry,
    planner::PlannerService,
    repo_sync::RepoSyncRuntime,
};
use crate::engine::runtime_endpoints::RuntimeEndpointManager;

use crate::{
    engine::{
        capabilities::terminal_runtime::ProcessRegistry,
        orchestration_inputs::OrchestrationInputStore,
        workflow_lifecycle::WorkflowCoordinator,
    },
    models::{SupervisorEventStreamItem, WorkflowEventStreamItem},
};

#[derive(Clone, Default)]
pub struct SupervisorCoordinator {
    supervisors: Arc<DashMap<Uuid, Arc<Mutex<()>>>>,
}

impl SupervisorCoordinator {
    pub async fn lock(&self, supervisor_id: Uuid) -> OwnedMutexGuard<()> {
        let guard = self
            .supervisors
            .entry(supervisor_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        guard.lock_owned().await
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    workflow_events_tx: broadcast::Sender<WorkflowEventStreamItem>,
    supervisor_events_tx: broadcast::Sender<SupervisorEventStreamItem>,
    process_session_id: String,
    pub workflow_coordinator: WorkflowCoordinator,
    pub supervisor_coordinator: SupervisorCoordinator,
    pub process_registry: ProcessRegistry,
    pub orchestration_inputs: OrchestrationInputStore,
    pub operator_inputs: OperatorInputRegistry,
    pub repo_sync: RepoSyncRuntime,
    pub runtime_endpoints: RuntimeEndpointManager,
}

impl AppState {
    pub fn new(db: SqlitePool) -> Self {
        let (workflow_events_tx, _) = broadcast::channel(4096);
        let (supervisor_events_tx, _) = broadcast::channel(4096);
        Self {
            db,
            workflow_events_tx,
            supervisor_events_tx,
            process_session_id: Uuid::new_v4().to_string(),
            workflow_coordinator: WorkflowCoordinator::default(),
            supervisor_coordinator: SupervisorCoordinator::default(),
            process_registry: ProcessRegistry::default(),
            orchestration_inputs: OrchestrationInputStore::default(),
            operator_inputs: OperatorInputRegistry::default(),
            repo_sync: RepoSyncRuntime::default(),
            runtime_endpoints: RuntimeEndpointManager::default(),
        }
    }

    pub fn subscribe_workflow_events(&self) -> broadcast::Receiver<WorkflowEventStreamItem> {
        self.workflow_events_tx.subscribe()
    }

    pub fn publish_workflow_event(&self, event: WorkflowEventStreamItem) {
        let _ = self.workflow_events_tx.send(event);
    }

    pub fn subscribe_supervisor_events(&self) -> broadcast::Receiver<SupervisorEventStreamItem> {
        self.supervisor_events_tx.subscribe()
    }

    pub fn publish_supervisor_event(&self, event: SupervisorEventStreamItem) {
        let _ = self.supervisor_events_tx.send(event);
    }

    pub fn planner(&self) -> PlannerService<'_> {
        PlannerService::new(&self.db)
    }

    pub fn process_session_id(&self) -> &str {
        &self.process_session_id
    }
}
