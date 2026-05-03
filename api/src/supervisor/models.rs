use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorExecutionStrategy {
    Series,
    Parallel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorStatus {
    Created,
    Snapshotting,
    RunningChildren,
    RunningIntegration,
    Validating,
    ReadyToApply,
    Applied,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeaturePlanItemStatus {
    Rough,
    #[serde(alias = "refined", alias = "approved")]
    Fine,
    Scheduled,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorChildRun {
    pub execution_item_id: String,
    pub title: String,
    pub shard_path: String,
    pub workflow_run_id: Option<Uuid>,
    pub status: String,
    pub patch_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorRun {
    pub id: Uuid,
    pub strategy: SupervisorExecutionStrategy,
    pub status: SupervisorStatus,
    pub title: String,
    pub root_repo_path: String,
    pub snapshot_path: Option<String>,
    pub integration_path: Option<String>,
    pub feature_plan_items: Vec<FeaturePlanItem>,
    pub execution_plan_items: Vec<ExecutionPlanItem>,
    pub child_runs: Vec<SupervisorChildRun>,
    pub integration_run_id: Option<Uuid>,
    pub final_patch_path: Option<String>,
    pub merge_report: Value,
    pub validation_report: Value,
    pub context: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSupervisorRunRequest {
    pub title: String,
    pub root_repo_path: String,
    pub strategy: SupervisorExecutionStrategy,
    #[serde(default)]
    pub workflow_template_id: Option<Uuid>,
    #[serde(default)]
    pub integration_template_id: Option<Uuid>,
    #[serde(default)]
    pub feature_plan_items: Vec<FeaturePlanItem>,
    #[serde(default)]
    pub execution_plan_items: Vec<ExecutionPlanItem>,
    #[serde(default)]
    pub context: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorActionRequest {
    pub action: String,
    #[serde(default)]
    pub payload: Value,
}
