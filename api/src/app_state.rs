use sqlx::SqlitePool;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::models::{SprintEventStreamItem, WorkflowEventStreamItem};

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    workflow_events_tx: broadcast::Sender<WorkflowEventStreamItem>,
    sprint_events_tx: broadcast::Sender<SprintEventStreamItem>,
    process_session_id: String,
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

    pub fn process_session_id(&self) -> &str {
        &self.process_session_id
    }
}
