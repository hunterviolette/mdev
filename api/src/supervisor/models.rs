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
    Paused,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorWorkPoolKind {
    Refine,
    FeatureDevelopment,
    ManualShard,
    Integration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSupervisorWorkUnitRequest {
    pub pool_kind: SupervisorWorkPoolKind,
    pub name: String,
    #[serde(default)]
    pub feature_id: Option<String>,
    #[serde(default)]
    pub template_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SupervisorActionRequest {
    CreateWorkUnit(CreateSupervisorWorkUnitRequest),
    DeleteWorkUnit { work_unit_id: String },
    RegenerateWorkUnit { work_unit_id: String },
    StartWorkUnit { work_unit_id: String },
    PauseWorkUnit { work_unit_id: String },
    StageWorkUnit { work_unit_id: String, #[serde(default = "default_stage_work_unit")] staged: bool },
    UpdateSupervisorConfig { config: Value },
    SelectPlanner { planner_id: String },
    PauseFeaturePool,
    ResumeFeaturePool,
    SkipIntegrationInput { work_unit_id: String },
    UnskipIntegrationInput { work_unit_id: String },
    ApplyIntegration,
    Cancel,
}

fn default_stage_work_unit() -> bool {
    true
}

impl SupervisorActionRequest {
    pub fn action_name(&self) -> &'static str {
        match self {
            Self::CreateWorkUnit(_) => "create_work_unit",
            Self::DeleteWorkUnit { .. } => "delete_work_unit",
            Self::RegenerateWorkUnit { .. } => "regenerate_work_unit",
            Self::StartWorkUnit { .. } => "start_work_unit",
            Self::PauseWorkUnit { .. } => "pause_work_unit",
            Self::StageWorkUnit { .. } => "stage_work_unit",
            Self::UpdateSupervisorConfig { .. } => "update_supervisor_config",
            Self::SelectPlanner { .. } => "select_planner",
            Self::PauseFeaturePool => "pause_feature_pool",
            Self::ResumeFeaturePool => "resume_feature_pool",
            Self::SkipIntegrationInput { .. } => "skip_integration_input",
            Self::UnskipIntegrationInput { .. } => "unskip_integration_input",
            Self::ApplyIntegration => "apply_integration",
            Self::Cancel => "cancel",
        }
    }
}
