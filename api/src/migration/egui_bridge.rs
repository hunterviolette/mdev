use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct EguiBridge;

#[allow(dead_code)]
impl EguiBridge {
    #[allow(dead_code)]
    pub async fn start_run(&self, _run_id: Uuid, _payload: Value) -> anyhow::Result<()> {
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn send_action(&self, _run_id: Uuid, _action: &str, _payload: Value) -> anyhow::Result<()> {
        Ok(())
    }
}
