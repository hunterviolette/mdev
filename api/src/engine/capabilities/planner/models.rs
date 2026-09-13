use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeaturePlanItemStatus {
    Rough,
    #[serde(alias = "refined", alias = "approved")]
    Fine,
    Scheduled,
    Applied,
    #[serde(alias = "applied")]
    Completed,
}

impl Default for FeaturePlanItemStatus {
    fn default() -> Self {
        Self::Rough
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeaturePlanItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub status: FeaturePlanItemStatus,
    #[serde(default)]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rough_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refinement_workflow_run_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_sprint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_sprint_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_at: Option<String>,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub implementation_notes: Vec<String>,
    #[serde(default)]
    pub review_expectations: Vec<String>,
    #[serde(default)]
    pub target_files_or_areas: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannerCapabilityBinding {
    pub planner_id: String,
    pub feature_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlannerCapabilityState {
    #[serde(default)]
    pub planner_id: String,
    #[serde(default)]
    pub feature_id: String,
    #[serde(default)]
    pub fragment_armed: bool,
    #[serde(default)]
    pub schema_armed: bool,
    #[serde(default)]
    pub auto_apply_armed: bool,
}

#[derive(Debug, Deserialize)]
struct WorkflowGlobalState {
    capabilities: WorkflowCapabilities,
}

#[derive(Debug, Deserialize)]
struct WorkflowCapabilities {
    planner: PlannerCapabilityState,
}

impl PlannerCapabilityState {
    pub fn binding_if_present(&self) -> Result<Option<PlannerCapabilityBinding>> {
        let planner_id = self.planner_id.trim();
        let feature_id = self.feature_id.trim();

        match (planner_id.is_empty(), feature_id.is_empty()) {
            (true, true) => Ok(None),
            (true, false) => Err(anyhow!("planner_id is required when feature_id is set")),
            (false, true) => Err(anyhow!("feature_id is required when planner_id is set")),
            (false, false) => Ok(Some(PlannerCapabilityBinding {
                planner_id: planner_id.to_string(),
                feature_id: feature_id.to_string(),
            })),
        }
    }

    pub fn binding(&self) -> Result<PlannerCapabilityBinding> {
        self.binding_if_present()?
            .ok_or_else(|| anyhow!("planner capability binding is not configured"))
    }

    pub fn from_global_state(global_state: &Value) -> Result<Self> {
        let planner = global_state
            .get("capabilities")
            .and_then(|capabilities| capabilities.get("planner"))
            .cloned()
            .unwrap_or_else(|| json!({}));

        serde_json::from_value(planner)
            .context("invalid planner capability state")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlanItem {
    pub feature_plan_item_id: String,
    #[serde(default)]
    pub workflow_template_id: Option<Uuid>,
    #[serde(default)]
    pub order_index: Option<i64>,
}
