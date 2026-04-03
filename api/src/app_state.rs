use sqlx::SqlitePool;
use tokio::sync::broadcast;

use crate::models::WorkflowEventStreamItem;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    workflow_events_tx: broadcast::Sender<WorkflowEventStreamItem>,
}

impl AppState {
    pub fn new(db: SqlitePool) -> Self {
        let (workflow_events_tx, _) = broadcast::channel(4096);
        Self { db, workflow_events_tx }
    }

    pub fn subscribe_workflow_events(&self) -> broadcast::Receiver<WorkflowEventStreamItem> {
        self.workflow_events_tx.subscribe()
    }

    pub fn publish_workflow_event(&self, event: WorkflowEventStreamItem) {
        let _ = self.workflow_events_tx.send(event);
    }
}
