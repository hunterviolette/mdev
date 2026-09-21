use std::path::PathBuf;

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
    #[serde(default)]
    pub selected_planner_id: Option<String>,
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





#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorWorkPoolKind {
    Refine,
    Feature,
    Manual,
    Integration,
}

impl SupervisorWorkPoolKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Refine => "refine",
            Self::Feature => "feature",
            Self::Manual => "manual",
            Self::Integration => "integration",
        }
    }
}

impl TryFrom<&str> for SupervisorWorkPoolKind {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "refine" => Ok(Self::Refine),
            "feature" => Ok(Self::Feature),
            "manual" => Ok(Self::Manual),
            "integration" => Ok(Self::Integration),
            other => Err(format!("unknown supervisor work pool kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationInputState {
    #[default]
    Available,
    Included,
    Skipped,
}

impl IntegrationInputState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Included => "included",
            Self::Skipped => "skipped",
        }
    }

    pub const fn is_integration_input(self) -> bool {
        !matches!(self, Self::Available)
    }
}

impl TryFrom<&str> for IntegrationInputState {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "available" => Ok(Self::Available),
            "included" => Ok(Self::Included),
            "skipped" => Ok(Self::Skipped),
            other => Err(format!("unknown integration input state: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorWorkUnitState {
    Draft,
    Queued,
    Running,
    Waiting,
    Paused,
    Completed,
    Failed,
    Cancelled,
    Archived,
}

impl SupervisorWorkUnitState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Archived => "archived",
        }
    }
}

impl TryFrom<&str> for SupervisorWorkUnitState {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "draft" => Ok(Self::Draft),
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "waiting" => Ok(Self::Waiting),
            "paused" => Ok(Self::Paused),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "archived" => Ok(Self::Archived),
            other => Err(format!("unknown supervisor work unit state: {other}")),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SupervisorWorkUnitStoredContext {
    #[serde(default)]
    pub template_id: Option<Uuid>,
    #[serde(default)]
    pub planned_workflow_template_id: Option<Uuid>,
    #[serde(default)]
    pub planner_id: Option<String>,
    #[serde(default)]
    pub feature_id: Option<String>,
}

impl SupervisorWorkUnitStoredContext {
    pub fn workflow_template_id(&self) -> Option<Uuid> {
        self.planned_workflow_template_id.or(self.template_id)
    }
}

#[derive(Debug, Clone)]
pub struct SupervisorWorkUnitRecord {
    pub id: String,
    pub kind: SupervisorWorkPoolKind,
    pub feature_id: Option<String>,
    pub title: String,
    pub workflow_run_id: Option<Uuid>,
    pub workspace_path: Option<PathBuf>,
    pub integration_state: IntegrationInputState,
    pub state: SupervisorWorkUnitState,
    pub context: SupervisorWorkUnitStoredContext,
    pub queue_position: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct SupervisorIntegrationCandidate {
    pub id: String,
    pub kind: SupervisorWorkPoolKind,
    pub workspace_path: Option<PathBuf>,
    pub integration_state: IntegrationInputState,
}

impl SupervisorIntegrationCandidate {
    pub const fn supports_integration_input(&self) -> bool {
        matches!(self.kind, SupervisorWorkPoolKind::Feature | SupervisorWorkPoolKind::Manual)
    }

    pub fn can_stage_to_integration(&self, changes: &super::patches::ChangeStatus) -> bool {
        self.supports_integration_input()
            && self.integration_state == IntegrationInputState::Available
            && changes.has_staged_changes()
    }

    pub const fn can_unstage_from_integration(&self) -> bool {
        self.supports_integration_input() && self.integration_state.is_integration_input()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorIntegrationInput {
    pub work_unit_id: String,
    #[serde(default)]
    pub feature_id: Option<String>,
    pub workspace_path: PathBuf,
    #[serde(default)]
    pub workflow_run_id: Option<Uuid>,
    #[serde(alias = "workflow_type")]
    pub kind: SupervisorWorkPoolKind,
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
    EnqueueFeature { planner_id: String, feature_id: String },
    DequeueFeature { planner_id: String, feature_id: String },
    ReorderFeaturePool { feature_ids: Vec<String> },
    RefineFeature {
        feature_id: String,
        #[serde(default)]
        workflow_template_id: Option<Uuid>,
    },
    PauseFeaturePool,
    ResumeFeaturePool,
    SkipIntegrationInput { work_unit_id: String },
    UnskipIntegrationInput { work_unit_id: String },
    ApplyIntegration {
        work_unit_id: String,
        #[serde(default)]
        archive_integrated_workflows: bool,
    },
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
            Self::EnqueueFeature { .. } => "enqueue_feature",
            Self::DequeueFeature { .. } => "dequeue_feature",
            Self::ReorderFeaturePool { .. } => "reorder_feature_pool",
            Self::RefineFeature { .. } => "refine_feature",
            Self::PauseFeaturePool => "pause_feature_pool",
            Self::ResumeFeaturePool => "resume_feature_pool",
            Self::SkipIntegrationInput { .. } => "skip_integration_input",
            Self::UnskipIntegrationInput { .. } => "unskip_integration_input",
            Self::ApplyIntegration { .. } => "apply_integration",
            Self::Cancel => "cancel",
        }
    }
}
