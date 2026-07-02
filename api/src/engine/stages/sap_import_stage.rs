use anyhow::Result;
use serde_json::Value;

use crate::models::WorkflowStepDefinition;

pub fn capabilities() -> crate::engine::stages::capability_contract::StageCapabilities {
    crate::engine::stages::capability_contract::StageCapabilities::new(["sap/import"])
}

pub fn prepare_stage_state(
    _step: &WorkflowStepDefinition,
    local_state: Value,
) -> Result<Value> {
    Ok(local_state)
}
