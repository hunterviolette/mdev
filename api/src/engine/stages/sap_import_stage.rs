use anyhow::Result;
use serde_json::Value;

use crate::models::WorkflowStepDefinition;

use super::{
    configured_execution_plan,
    Stage,
    StageCapabilities,
    StagePlanContext,
    StagePrepareContext,
};

pub struct SapImportStage;

pub static STAGE: SapImportStage = SapImportStage;

inventory::submit! {
    super::StageRegistration::new(&STAGE)
}

impl Stage for SapImportStage {
    fn stage_type(&self) -> &'static str {
        "sap_import"
    }

    fn capabilities(&self) -> StageCapabilities {
        StageCapabilities::new(["sap/import"])
    }

    fn prepare_state(
        &self,
        context: StagePrepareContext<'_>,
        local_state: Value,
    ) -> Result<Value> {
        prepare_sap_import_state(context.step, local_state)
    }

    fn build_execution_plan(
        &self,
        context: StagePlanContext<'_>,
    ) -> Result<Vec<crate::models::StageExecutionNode>> {
        Ok(build_sap_import_execution_plan(context.step))
    }
}

fn build_sap_import_execution_plan(
    step: &WorkflowStepDefinition,
) -> Vec<crate::models::StageExecutionNode> {
    configured_execution_plan(step)
}

fn prepare_sap_import_state(
    _step: &WorkflowStepDefinition,
    local_state: Value,
) -> Result<Value> {
    Ok(local_state)
}
