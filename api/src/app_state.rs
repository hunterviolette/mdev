use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::{
    engine::{
        capabilities::terminal_runtime::ProcessRegistry,
        workflow_lifecycle::WorkflowCoordinator,
    },
    models::{SprintEventStreamItem, WorkflowEventStreamItem},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowTransientPromptFragment {
    ChangesetApplyError { text: String },
}

impl WorkflowTransientPromptFragment {
    pub fn text(&self) -> &str {
        match self {
            Self::ChangesetApplyError { text } => text,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::ChangesetApplyError { .. } => "changeset_apply_error",
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    workflow_events_tx: broadcast::Sender<WorkflowEventStreamItem>,
    sprint_events_tx: broadcast::Sender<SprintEventStreamItem>,
    process_session_id: String,
    pub workflow_coordinator: WorkflowCoordinator,
    pub process_registry: ProcessRegistry,
    transient_prompt_fragments: DashMap<Uuid, Vec<WorkflowTransientPromptFragment>>,
}

impl AppState {
    pub fn new(db: SqlitePool) -> Self {
        let (workflow_events_tx, _) = broadcast::channel(4096);
        let (sprint_events_tx, _) = broadcast::channel(4096);
        Self {
            db,
            workflow_events_tx,
            sprint_events_tx,
            process_session_id: Uuid::new_v4().to_string(),
            workflow_coordinator: WorkflowCoordinator::default(),
            process_registry: ProcessRegistry::default(),
            transient_prompt_fragments: DashMap::new(),
        }
    }

    pub fn subscribe_workflow_events(&self) -> broadcast::Receiver<WorkflowEventStreamItem> {
        self.workflow_events_tx.subscribe()
    }

    pub fn publish_workflow_event(&self, event: WorkflowEventStreamItem) {
        let _ = self.workflow_events_tx.send(event);
    }

    pub fn subscribe_sprint_events(&self) -> broadcast::Receiver<SprintEventStreamItem> {
        self.sprint_events_tx.subscribe()
    }

    pub fn publish_sprint_event(&self, event: SprintEventStreamItem) {
        let _ = self.sprint_events_tx.send(event);
    }

    pub fn replace_transient_prompt_fragments(
        &self,
        run_id: Uuid,
        fragments: Vec<WorkflowTransientPromptFragment>,
    ) {
        if fragments.is_empty() {
            self.transient_prompt_fragments.remove(&run_id);
        } else {
            self.transient_prompt_fragments.insert(run_id, fragments);
        }
    }

    pub fn take_transient_prompt_fragments(
        &self,
        run_id: Uuid,
    ) -> Vec<WorkflowTransientPromptFragment> {
        self.transient_prompt_fragments
            .remove(&run_id)
            .map(|(_, fragments)| fragments)
            .unwrap_or_default()
    }

    pub fn clear_transient_prompt_fragments(&self, run_id: Uuid) {
        self.transient_prompt_fragments.remove(&run_id);
    }

    pub fn process_session_id(&self) -> &str {
        &self.process_session_id
    }
}
