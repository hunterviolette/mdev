use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Draft,
    Queued,
    Running,
    Waiting,
    Paused,
    Success,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationMode {
    Manual,
    Assisted,
    Automatic,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowCapabilityBinding {
    pub capability: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub config: Value,
    #[serde(default)]
    pub input_mapping: Value,
    #[serde(default)]
    pub output_mapping: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StageExecutionNodeKind {
    Capability,
    StageLogic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageExecutionNode {
    pub kind: StageExecutionNodeKind,
    pub key: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub config: Value,
    #[serde(default)]
    pub input_mapping: Value,
    #[serde(default)]
    pub output_mapping: Value,
    #[serde(default)]
    pub run_after: Vec<String>,
    #[serde(default)]
    pub condition: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowGlobalConfig {
    #[serde(default)]
    pub resources: Value,
    #[serde(default)]
    pub capabilities: Value,
    #[serde(default)]
    pub automation: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowStepExecutionConfig {
    #[serde(default)]
    pub changeset_apply: Value,
    #[serde(default)]
    pub compile_checks: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowStepPromptConfig {
    #[serde(default)]
    pub include_repo_context: bool,
    #[serde(default)]
    pub include_changeset_schema: bool,
    #[serde(default)]
    pub include_user_context: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowStepAdvancementConfig {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub auto_run_on_enter: bool,
    #[serde(default)]
    pub auto_advance_on_success: bool,
    #[serde(default)]
    pub auto_advance_on_error: bool,
    #[serde(default)]
    pub auto_advance_on_paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTransition {
    pub when: TransitionWhen,
    pub target_step_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum TransitionWhen {
    Success,
    Error,
    Paused,
    RetryStage,
    ErrorCode(String),
    Outcome(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStepDefinition {
    pub id: String,
    pub name: String,
    pub step_type: String,
    #[serde(default)]
    pub governance: WorkflowStepGovernanceConfig,
    pub automation_mode: AutomationMode,
    #[serde(default)]
    pub execution: WorkflowStepExecutionConfig,
    #[serde(default)]
    pub prompt: WorkflowStepPromptConfig,
    #[serde(default)]
    pub config: Value,
    #[serde(default)]
    pub capabilities: Vec<WorkflowCapabilityBinding>,
    #[serde(default)]
    pub execution_logic: Value,
    #[serde(default)]
    pub execution_plan: Vec<StageExecutionNode>,
    #[serde(default)]
    pub transitions: Vec<WorkflowTransition>,
    #[serde(default)]
    pub advancement: WorkflowStepAdvancementConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTemplateDefinition {
    pub version: u32,
    #[serde(default)]
    pub globals: WorkflowGlobalConfig,
    pub steps: Vec<WorkflowStepDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowBuilderCatalog {
    pub version: u32,
    #[serde(default)]
    pub stage_descriptors: Vec<WorkflowStageDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStageDescriptor {
    pub step_type: String,
    pub label: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub description: String,
    pub definition_template: WorkflowStepDefinition,
    #[serde(default)]
    pub editable_fields: Vec<WorkflowStageFieldGroup>,
    #[serde(default)]
    pub available_governance_policies: Vec<WorkflowGovernancePolicyDescriptor>,
    #[serde(default)]
    pub routes: Vec<WorkflowStageRoute>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowStageFieldGroup {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub fields: Vec<WorkflowStageField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowGovernancePolicyDescriptor {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub capability: String,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[serde(default)]
    pub fields: Vec<WorkflowStageField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowStageGovernancePolicy {
    pub key: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowStepGovernanceConfig {
    #[serde(default)]
    pub policies: Vec<WorkflowStageGovernancePolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStageField {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub field_type: String,
    pub bind_to: String,
    #[serde(default)]
    pub default: Value,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub options: Vec<WorkflowStageFieldOption>,
    #[serde(default)]
    pub ui: WorkflowStageFieldUi,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowStageFieldOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowStageFieldUi {
    #[serde(default)]
    pub control: String,
    #[serde(default)]
    pub placeholder: String,
    #[serde(default)]
    pub min_rows: u32,
    #[serde(default)]
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowStageRoute {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub target_required: bool,
    #[serde(default)]
    pub allow_terminate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowBuilderDocument {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub globals: WorkflowGlobalConfig,
    #[serde(default)]
    pub stages: Vec<WorkflowBuilderStageDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowBuilderStageDocument {
    pub id: String,
    pub name: String,
    pub step_type: String,
    #[serde(default)]
    pub field_values: Value,
    #[serde(default)]
    pub governance_policies: Vec<WorkflowStageGovernancePolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileWorkflowBuilderRequest {
    pub document: WorkflowBuilderDocument,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowCapabilitySummaryItem {
    pub key: String,
    #[serde(default)]
    pub stage_ids: Vec<String>,
    #[serde(default)]
    pub stage_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileWorkflowBuilderResponse {
    pub ok: bool,
    pub definition: WorkflowTemplateDefinition,
    #[serde(default)]
    pub capability_summary: Vec<WorkflowCapabilitySummaryItem>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTemplate {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub repo_ref: String,
    pub definition: WorkflowTemplateDefinition,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: Uuid,
    pub template_id: Option<Uuid>,
    pub definition: WorkflowTemplateDefinition,
    pub status: RunStatus,
    pub current_step_id: Option<String>,
    pub title: String,
    pub repo_ref: String,
    #[serde(default)]
    pub context: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEvent {
    pub id: Uuid,
    pub run_id: Uuid,
    pub step_id: Option<String>,
    pub level: String,
    pub kind: String,
    pub message: String,
    #[serde(default)]
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEventStreamItem {
    pub id: String,
    pub run_id: String,
    pub step_id: Option<String>,
    pub stage_execution_id: Option<String>,
    pub capability_invocation_id: Option<String>,
    pub parent_invocation_id: Option<String>,
    pub sequence_no: i64,
    pub level: String,
    pub kind: String,
    pub message: String,
    #[serde(default)]
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTemplateRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub repo_ref: String,
    pub definition: WorkflowTemplateDefinition,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreateRunRequest {
    pub template_id: Option<Uuid>,
    pub title: String,
    pub repo_ref: String,
    #[serde(default)]
    pub definition: Option<WorkflowTemplateDefinition>,
    #[serde(default)]
    pub context: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunActionRequest {
    pub action: String,
    pub step_id: Option<String>,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppSettings {
    #[serde(default)]
    pub browser: Value,
    #[serde(default)]
    pub bridges: Value,
    #[serde(default)]
    pub git: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PatchAppSettingsRequest {
    #[serde(default)]
    pub patch: Value,
}
