use serde::{Deserialize, Serialize};
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
pub struct ExecutionPlanItem {
    pub feature_plan_item_id: String,
    #[serde(default)]
    pub workflow_template_id: Option<Uuid>,
    #[serde(default)]
    pub order_index: Option<i64>,
}
