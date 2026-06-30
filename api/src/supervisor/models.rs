use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::engine::capabilities::planner::{ExecutionPlanItem, FeaturePlanItem};

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
    DevelopmentComplete,
    RunningIntegration,
    Validating,
    ReadyToApply,
    Applied,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorFeatureWorkflow {
    pub feature_id: String,
    pub title: String,
    pub shard_path: Option<String>,
    pub workflow_run_id: Option<Uuid>,
    pub status: String,
    pub development_state: String,
    pub current_step_id: Option<String>,
    pub current_patch_id: Option<String>,
    pub last_error: Option<String>,
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
    #[serde(default)]
    pub feature_workflows: Vec<SupervisorFeatureWorkflow>,
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
pub struct EnsureSupervisorPlannerRequest {
    pub root_repo_path: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsureSupervisorPlannerResponse {
    pub created: bool,
    pub supervisor_run: SupervisorRun,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorActionRequest {
    pub action: String,
    #[serde(default)]
    pub payload: Value,
}
