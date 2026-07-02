export type AutomationMode = 'manual' | 'assisted' | 'automatic';

export type WorkflowCapabilityBinding = {
  capability: string;
  enabled: boolean;
  config: Record<string, unknown>;
  input_mapping: Record<string, unknown>;
  output_mapping: Record<string, unknown>;
};

export type WorkflowStepExecutionConfig = {
  changeset_apply: Record<string, unknown>;
  compile_checks: Record<string, unknown>;
};

export type WorkflowStepPromptConfig = {
  include_repo_context: boolean;
  include_changeset_schema: boolean;
  include_user_context: boolean;
};

export type WorkflowStepAdvancementConfig = {
  mode?: string | null;
  auto_run_on_enter?: boolean;
  auto_advance_on_success?: boolean;
  auto_advance_on_error?: boolean;
  auto_advance_on_paused?: boolean;
};

export type WorkflowTransitionWhen = {
  type: 'success' | 'error' | 'paused' | 'retry_stage' | 'error_code' | 'outcome';
  value?: string;
};

export type WorkflowTransition = {
  when: WorkflowTransitionWhen;
  target_step_id: string;
};

export type WorkflowExecutionNode = {
  kind: 'capability' | 'stage_logic';
  key: string;
  enabled: boolean;
  config: Record<string, unknown>;
  input_mapping: Record<string, unknown>;
  output_mapping: Record<string, unknown>;
  run_after: string[];
  condition: unknown;
};

export type WorkflowStageGovernancePolicy = {
  key: string;
  config: Record<string, unknown>;
};

export type WorkflowGovernanceConfig = Record<string, Record<string, unknown>>;

export type WorkflowStepDefinition = {
  id: string;
  name: string;
  step_type: string;
  automation_mode: AutomationMode;
  execution: WorkflowStepExecutionConfig;
  prompt: WorkflowStepPromptConfig;
  config: Record<string, unknown>;
  capabilities: WorkflowCapabilityBinding[];
  execution_logic?: Record<string, unknown>;
  execution_plan?: WorkflowExecutionNode[];
  transitions: WorkflowTransition[];
  advancement?: WorkflowStepAdvancementConfig;
};

export type WorkflowGlobalConfig = {
  resources: Record<string, unknown>;
  capabilities: Record<string, unknown>;
  automation: Record<string, unknown>;
};

export type WorkflowTemplateDefinition = {
  version: number;
  globals: WorkflowGlobalConfig;
  governance?: WorkflowGovernanceConfig;
  steps: WorkflowStepDefinition[];
};

export type InferenceConfigPanelSession = {
  name: string;
  transport: 'api' | 'browser' | string;
  provider?: string | null;
  model?: string | null;
  endpoint?: string | null;
  browser_url?: string | null;
  is_default: boolean;
};

export type InferenceConfigPanelStageMapping = {
  stage_type: string;
  session: string;
};

export type InferenceConfigPanel = {
  sessions: InferenceConfigPanelSession[];
  stage_mappings: InferenceConfigPanelStageMapping[];
};

export type InferenceConfigPanelResponse = {
  ok: boolean;
  panel: InferenceConfigPanel;
  inference: Record<string, unknown>;
};

export type WorkflowStageFieldOption = {
  value: string;
  label: string;
};

export type WorkflowStageFieldUi = {
  control: 'text' | 'textarea' | 'switch' | 'number' | 'select';
  placeholder?: string;
  min_rows?: number;
  format?: string;
};

export type WorkflowStageFieldVisibility = {
  path: string;
  equals: unknown;
};

export type WorkflowStageField = {
  key: string;
  label: string;
  type: 'boolean' | 'integer' | 'text' | 'multiline_text';
  bind_to: string;
  default: unknown;
  description?: string;
  required?: boolean;
  options?: WorkflowStageFieldOption[];
  visible_when?: WorkflowStageFieldVisibility[];
  ui?: WorkflowStageFieldUi;
};

export type WorkflowStageFieldGroup = {
  key: string;
  label: string;
  fields: WorkflowStageField[];
};

export type WorkflowGovernancePolicyDescriptor = {
  key: string;
  label: string;
  description: string;
  capability: string;
  required_capabilities: string[];
  fields: WorkflowStageField[];
};

export type WorkflowStageRoute = {
  key: string;
  label: string;
  description?: string;
  target: string;
  target_required?: boolean;
  allow_terminate?: boolean;
};

export type WorkflowStageDescriptor = {
  step_type: string;
  label: string;
  category: string;
  description: string;
  definition_template: WorkflowStepDefinition;
  editable_fields: WorkflowStageFieldGroup[];
  available_governance_policies?: WorkflowGovernancePolicyDescriptor[];
  routes: WorkflowStageRoute[];
};

export type WorkflowBuilderCatalog = {
  version: number;
  stage_descriptors: WorkflowStageDescriptor[];
};

export type WorkflowBuilderStageDocument = {
  id: string;
  name: string;
  step_type: string;
  field_values: Record<string, unknown>;
};

export type WorkflowBuilderDocument = {
  version: number;
  globals: WorkflowGlobalConfig;
  governance?: WorkflowGovernanceConfig;
  stages: WorkflowBuilderStageDocument[];
};

export type WorkflowCapabilitySummaryItem = {
  key: string;
  stage_ids: string[];
  stage_types: string[];
};

export type CompileWorkflowBuilderResponse = {
  ok: boolean;
  definition: WorkflowTemplateDefinition;
  capability_summary: WorkflowCapabilitySummaryItem[];
  warnings: string[];
  errors: string[];
};

export type WorkflowBuilderFieldContract = {
  key: string;
  label: string;
  type: 'boolean' | 'integer' | 'text' | 'multiline_text';
  default: boolean | number | string;
};

export type WorkflowBuilderStageContract = {
  step_type: string;
  label: string;
  automation_mode_default: AutomationMode;
  fields: WorkflowBuilderFieldContract[];
  transition_defaults: {
    on_success: string;
    on_error: string;
    on_paused: string;
  };
};

export type WorkflowBuilderContract = {
  version: number;
  stages: WorkflowBuilderStageContract[];
};

export type WorkflowTemplate = {
  id: string;
  name: string;
  description: string;
  repo_ref: string;
  definition: WorkflowTemplateDefinition;
  created_at: string;
  updated_at: string;
};

export type WorkflowRunStatus = 'draft' | 'queued' | 'running' | 'waiting' | 'paused' | 'success' | 'error' | 'cancelled';

export type InferenceTransport = 'api' | 'browser';

export type BrowserProbeResult = {
  session_id: string;
  browser_connected: boolean;
  page_open: boolean;
  url: string;
  profile: string;
  chat_input_found: boolean;
  chat_input_visible: boolean;
  chat_submit_found: boolean;
  ready: boolean;
};

export type WorkflowRun = {
  id: string;
  template_id: string | null;
  definition: WorkflowTemplateDefinition;
  status: WorkflowRunStatus;
  current_step_id: string | null;
  title: string;
  repo_ref: string;
  workflow_key: string;
  context: Record<string, unknown>;
  created_at: string;
  updated_at: string;
};

export type WorkflowEvent = {
  id: string;
  run_id: string;
  step_id: string | null;
  level: string;
  kind: string;
  message: string;
  payload: Record<string, unknown>;
  created_at: string;
};

export type EventChainCapabilitySummaryItem = {
  key: string;
  capability_id: string;
  name: string;
  status_color: string;
  status_label: string;
  message: string;
  started_at: string | null;
  completed_at?: string | null;
  duration_ms: number | null;
  latest_created_at: string;
  latest_kind?: string;
  latest_level?: string;
  is_active: boolean;
  event_count: number;
  start_event_id?: string | null;
  end_event_id?: string | null;
  start_payload?: Record<string, unknown> | null;
  end_payload?: Record<string, unknown> | null;
  latest_payload?: Record<string, unknown> | null;
};

export type EventChainSummaryItem = {
  key: string;
  step_id: string;
  label: string;
  stage_execution_id: string;
  latest_kind: string;
  latest_message: string;
  latest_level: string;
  latest_created_at: string;
  is_current: boolean;
  is_active: boolean;
  event_count: number;
  duration_ms: number | null;
  capabilities: EventChainCapabilitySummaryItem[];
};

export type EventChainSummaryResponse = {
  run_id: string;
  stages: EventChainSummaryItem[];
};

export type StageExecutionEvent = {
  id: string;
  run_id: string;
  step_id: string | null;
  stage_execution_id: string | null;
  capability_invocation_id: string | null;
  parent_invocation_id: string | null;
  sequence_no: number;
  level: string;
  kind: string;
  message: string;
  payload: Record<string, unknown>;
  created_at: string;
};

export type StageExecutionChain = {
  run_id: string;
  step_id: string;
  stage_execution_id: string;
  items: StageExecutionEvent[];
};

export type RuntimeNode = {
  key: string;
  node_type: string;
  id: string;
  status: string;
  title: string;
  repo_ref: string;
  workflow_key?: string | null;
  current_step_id?: string | null;
  updated_at: string;
  payload: Record<string, unknown>;
};

export type RuntimeEdge = {
  key: string;
  parent_key: string;
  child_key: string;
  edge_type: string;
  label: string;
  sort_order: number;
  payload: Record<string, unknown>;
};

export type RuntimeSnapshotResponse = {
  nodes: RuntimeNode[];
  edges: RuntimeEdge[];
  latest_sequence_no: number;
  server_time: string;
};

export type RuntimeEventEnvelope = {
  scope: string;
  node_key: string;
  run_id?: string | null;
  supervisor_run_id?: string | null;
  workflow_key?: string | null;
  repo_ref?: string | null;
  event: StageExecutionEvent;
};

export type RuntimeProjectionResponse = {
  runs: EventChainSummaryResponse[];
};

export type RuntimeEventQuery = {
  run_id?: string | null;
  supervisor_run_id?: string | null;
  workflow_key?: string | null;
  repo_ref?: string | null;
  scope?: string | null;
  after_sequence?: number | null;
};

export type WorkflowRunActionResult = {
  ok: boolean;
  status?: string;
  step_id?: string;
  current_step_id?: string;
  next_step_id?: string;
  step_type?: string;
  message?: string | null;
  run?: WorkflowRun;
  local_state?: Record<string, unknown>;
  execution_plan?: Array<Record<string, unknown>>;
  capability_results?: Array<Record<string, unknown>>;
};

export type RepoTreeEntry = {
  name: string;
  path: string;
  kind: 'file' | 'dir';
  has_children: boolean;
};

export type RepoTreeResponse = {
  repo_ref: string;
  git_ref: string;
  base_path: string;
  entries: RepoTreeEntry[];
  refreshed_at: string;
};

export type RepoFilesResponse = {
  repo_ref: string;
  git_ref: string;
  files: string[];
  refreshed_at: string;
};

export type RepoValidateResponse = {
  ok: boolean;
  repo_ref: string;
  exists: boolean;
  is_dir: boolean;
  git_repo: boolean;
  message: string;
};

async function fetchJson<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    headers: { 'Content-Type': 'application/json' },
    ...init
  });
  if (!res.ok) {
    throw new Error(await res.text());
  }
  return res.json() as Promise<T>;
}

export function listTemplates() {
  return fetchJson<WorkflowTemplate[]>('/api/workflow-templates');
}

export function getWorkflowBuilderContract() {
  return fetchJson<WorkflowBuilderContract>('/api/workflow-builder-contract');
}

export function getWorkflowBuilderCatalog() {
  return fetchJson<WorkflowBuilderCatalog>('/api/workflow-builder-catalog');
}

export function compileWorkflowBuilderDocument(document: WorkflowBuilderDocument) {
  return fetchJson<CompileWorkflowBuilderResponse>('/api/workflow-builder/compile', {
    method: 'POST',
    body: JSON.stringify({ document })
  });
}

export function buildWorkflowBuilderInferencePanel(body: {
  definition: WorkflowTemplateDefinition;
  globals: WorkflowGlobalConfig;
  panel?: InferenceConfigPanel;
}) {
  return fetchJson<InferenceConfigPanelResponse>('/api/workflow-builder/inference-panel', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function listRepoTree(
  repoRef: string,
  gitRef = 'WORKTREE',
  options?: { basePath?: string; skipBinary?: boolean; skipGitignore?: boolean }
) {
  const params = new URLSearchParams({
    repo_ref: repoRef,
    git_ref: gitRef,
    base_path: options?.basePath ?? '',
    skip_binary: String(Boolean(options?.skipBinary)),
    skip_gitignore: String(Boolean(options?.skipGitignore))
  });
  return fetchJson<RepoTreeResponse>(`/api/repo-tree?${params.toString()}`);
}

export function listRepoFiles(
  repoRef: string,
  gitRef = 'WORKTREE',
  options?: { skipBinary?: boolean; skipGitignore?: boolean }
) {
  const params = new URLSearchParams({
    repo_ref: repoRef,
    git_ref: gitRef,
    skip_binary: String(Boolean(options?.skipBinary)),
    skip_gitignore: String(Boolean(options?.skipGitignore))
  });
  return fetchJson<RepoFilesResponse>(`/api/repo-files?${params.toString()}`);
}

export function validateRepoRef(repoRef: string) {
  const params = new URLSearchParams({
    repo_ref: repoRef,
  });
  return fetchJson<RepoValidateResponse>(`/api/repo/validate?${params.toString()}`);
}

export type FileContentsResponse = {
  ok: boolean;
  repo_ref: string;
  path: string;
  contents: string;
};

export type MutatePathResponse = {
  ok: boolean;
  repo_ref: string;
  path: string;
  kind: string;
  bytes: number;
};

export function readWorkspaceFile(repoRef: string, path: string) {
  const params = new URLSearchParams({
    repo_ref: repoRef,
    path,
  });
  return fetchJson<FileContentsResponse>(`/api/file?${params.toString()}`);
}

export function writeWorkspaceFile(body: { repo_ref: string; path: string; contents: string }) {
  return fetchJson<MutatePathResponse>('/api/file', {
    method: 'PUT',
    body: JSON.stringify(body),
  });
}

export function createWorkspaceFile(body: { repo_ref: string; path: string; contents?: string }) {
  return fetchJson<MutatePathResponse>('/api/file', {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

export function createWorkspaceFolder(body: { repo_ref: string; path: string }) {
  return fetchJson<MutatePathResponse>('/api/folder', {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

export function deleteWorkspacePath(repoRef: string, path: string) {
  const params = new URLSearchParams({
    repo_ref: repoRef,
    path,
  });
  return fetchJson<{ ok: boolean; repo_ref: string; path: string }>(`/api/file?${params.toString()}`, {
    method: 'DELETE',
  });
}

export function createTemplate(body: { name: string; description: string; repo_ref: string; definition: WorkflowTemplateDefinition }) {
  return fetchJson<WorkflowTemplate>('/api/workflow-templates', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function deleteTemplate(templateId: string) {
  return fetchJson<{ ok: boolean }>(`/api/workflow-templates/${templateId}`, {
    method: 'DELETE'
  });
}

export function listRuns() {
  return fetchJson<WorkflowRun[]>('/api/workflow-runs');
}

export function getRun(runId: string) {
  return fetchJson<WorkflowRun>(`/api/workflow-runs/${runId}`);
}

export function openWorkflowRun(runId: string) {
  return fetchJson<WorkflowRun>(`/api/workflow-runs/${runId}/open`, {
    method: 'POST'
  });
}

export function createRun(body: { template_id?: string | null; title: string; repo_ref: string; definition?: WorkflowTemplateDefinition; context: Record<string, unknown> }) {
  return fetchJson<WorkflowRun>('/api/workflow-runs', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function deleteRun(runId: string) {
  return fetchJson<{ ok: boolean }>(`/api/workflow-runs/${runId}`, {
    method: 'DELETE'
  });
}

export function listRunEvents(runId: string) {
  return fetchJson<WorkflowEvent[]>(`/api/workflow-runs/${runId}/events`);
}

export function getEventChainSummary(runId: string) {
  return fetchJson<EventChainSummaryResponse>(`/api/workflow-runs/${runId}/event-chain`);
}

export function getStageExecutionChain(runId: string, stepId: string, stageExecutionId: string) {
  return fetchJson<StageExecutionChain>(
    `/api/workflow-runs/${runId}/stages/${encodeURIComponent(stepId)}/executions/${encodeURIComponent(stageExecutionId)}`
  );
}

export function openEventStream(runId: string, afterSequence = 0): EventSource {
  return new EventSource(`/api/workflow-runs/${runId}/events/stream?after_sequence=${afterSequence}`);
}

function runtimeEventQueryString(query: RuntimeEventQuery = {}) {
  const params = new URLSearchParams();
  if (query.run_id) params.set('run_id', query.run_id);
  if (query.supervisor_run_id) params.set('supervisor_run_id', query.supervisor_run_id);
  if (query.workflow_key) params.set('workflow_key', query.workflow_key);
  if (query.repo_ref) params.set('repo_ref', query.repo_ref);
  if (query.scope) params.set('scope', query.scope);
  if (typeof query.after_sequence === 'number') params.set('after_sequence', String(query.after_sequence));
  const value = params.toString();
  return value ? `?${value}` : '';
}

export function getRuntimeSnapshot(query: RuntimeEventQuery = {}) {
  return fetchJson<RuntimeSnapshotResponse>(`/api/events/snapshot${runtimeEventQueryString(query)}`);
}

export function getRuntimeProjection(query: RuntimeEventQuery = {}) {
  return fetchJson<RuntimeProjectionResponse>(`/api/events/projection${runtimeEventQueryString(query)}`);
}

export function openRuntimeEventStream(query: RuntimeEventQuery = {}): EventSource {
  return new EventSource(`/api/events/stream${runtimeEventQueryString(query)}`);
}

export function sendRunAction(runId: string, body: { action: string; step_id?: string | null; payload?: Record<string, unknown> }) {
  return fetchJson<WorkflowRunActionResult>(`/api/workflow-runs/${runId}/actions`, {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function startWorkflowRun(runId: string) {
  return sendRunAction(runId, { action: 'start_run' });
}

export function prepareWorkflowStage(runId: string, stepId?: string | null) {
  return sendRunAction(runId, { action: 'prepare_stage', step_id: stepId ?? undefined });
}

export function resumeWorkflowRun(runId: string) {
  return sendRunAction(runId, { action: 'resume_run' });
}

export function pauseWorkflowRun(runId: string) {
  return sendRunAction(runId, { action: 'pause_run' });
}

export function forceWaitWorkflowRun(runId: string) {
  return sendRunAction(runId, { action: 'cancel_run' });
}

export function selectWorkflowStep(runId: string, stepId: string) {
  return sendRunAction(runId, { action: 'select_step', step_id: stepId });
}

export function runCurrentWorkflowStep(
  runId: string,
  stepId?: string | null,
  payload?: Record<string, unknown>
) {
  return sendRunAction(runId, {
    action: 'run_step',
    step_id: stepId ?? undefined,
    payload
  });
}

export function resolveWorkflowOperatorCheckpoint(runId: string, disposition: string, selectedStepId?: string | null) {
  return sendRunAction(runId, {
    action: 'resolve_operator_checkpoint',
    payload: {
      disposition,
      ...(selectedStepId ? { selected_step_id: selectedStepId } : {})
    }
  });
}

export function resolveWorkflowDispositionReview(runId: string, disposition: string, selectedStepId?: string | null) {
  return resolveWorkflowOperatorCheckpoint(runId, disposition, selectedStepId);
}

export function nextWorkflowStep(runId: string) {
  return sendRunAction(runId, { action: 'next_step' });
}

export function previousWorkflowStep(runId: string) {
  return sendRunAction(runId, { action: 'previous_step' });
}

export function patchWorkflowStageState(runId: string, stepId: string, payload: Record<string, unknown>) {
  return sendRunAction(runId, { action: 'patch_stage_state', step_id: stepId, payload });
}

export function patchWorkflowGlobalState(runId: string, payload: Record<string, unknown>) {
  return sendRunAction(runId, { action: 'patch_global_state', payload });
}

export function getPayloadGatewaySchema() {
  return fetchJson<{
    ok: boolean;
    name: string;
    version: number;
    example: string;
  }>(`/api/capabilities/payload-gateway/schema`);
}

export function getChangesetSchema() {
  return fetchJson<{
    ok: boolean;
    schema: string;
  }>(`/api/capabilities/changeset-schema`);
}

export function listWorkflowCapabilities(runId: string) {
  return fetchJson<Record<string, unknown>>(`/api/workflow-runs/${runId}/capabilities`);
}

export function executeWorkflowCapability(runId: string, capabilityId: string, input: unknown) {
  return fetchJson<Record<string, unknown>>(`/api/workflow-runs/${runId}/capabilities/${encodeURIComponent(capabilityId)}/execute`, {
    method: 'POST',
    body: JSON.stringify({ input })
  });
}

export function listWorkflowRepoTree(
  runId: string,
  gitRef = 'WORKTREE',
  options?: { basePath?: string; skipBinary?: boolean; skipGitignore?: boolean }
) {
  const params = new URLSearchParams({
    git_ref: gitRef || 'WORKTREE',
    base_path: options?.basePath ?? '',
    skip_binary: String(Boolean(options?.skipBinary)),
    skip_gitignore: String(Boolean(options?.skipGitignore))
  });
  return fetchJson<RepoTreeResponse>(`/api/workflow-runs/${runId}/repository/tree?${params.toString()}`);
}

export function readWorkflowFile(runId: string, path: string) {
  const params = new URLSearchParams({ path });
  return fetchJson<FileContentsResponse>(`/api/workflow-runs/${runId}/filesystem/read?${params.toString()}`);
}

export function writeWorkflowFile(runId: string, body: { path: string; contents: string }) {
  return fetchJson<MutatePathResponse>(`/api/workflow-runs/${runId}/filesystem/write`, {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export type ChangesetAttemptSummary = {
  id: string;
  run_id: string | null;
  step_id: string | null;
  repo_ref: string;
  git_ref: string;
  direction: string;
  reverses_attempt_id: string | null;
  source: string;
  status: 'applied' | 'failed' | 'partial' | string;
  total_ops: number;
  applied_ops: number;
  failed_ops: number;
  skipped_ops: number;
  total_actions: number;
  applied_actions: number;
  failed_actions: number;
  touched_file_count: number;
  success_rate: number;
  created_count: number;
  modified_count: number;
  deleted_count: number;
  moved_count: number;
  duration_ms: number | null;
  error_summary: string | null;
  display_summary: string;
  created_at: string;
  successful_files: string[];
  failed_files?: string[];
  file_action_summaries?: Array<{ path: string; applied: number; failed: number; total: number }>;
};

export type ChangesetAttemptDetail = ChangesetAttemptSummary & {
  payload_text: string;
  normalized_payload_json: string;
  result_json: unknown;
};

export type ApplyChangesetResponse = Record<string, unknown> & {
  ok?: boolean;
  summary?: string;
  status?: string;
  lines?: string[];
  payload_text?: string;
  normalized_payload?: string;
  changeset_attempt_id?: string;
  stats?: Record<string, unknown>;
};

export function listWorkflowChangesets(workflowKey: string, limit = 50) {
  const params = new URLSearchParams({ limit: String(limit) });
  return fetchJson<ChangesetAttemptSummary[]>(`/api/workflows/${encodeURIComponent(workflowKey)}/changesets?${params.toString()}`);
}

export function getWorkflowChangeset(workflowKey: string, attemptId: string) {
  return fetchJson<ChangesetAttemptDetail>(`/api/workflows/${encodeURIComponent(workflowKey)}/changesets/${encodeURIComponent(attemptId)}`);
}

export function applyWorkflowChangeset(workflowKey: string, body: { git_ref?: string; payload_text: string }) {
  return fetchJson<ApplyChangesetResponse>(`/api/workflows/${encodeURIComponent(workflowKey)}/changesets/apply`, {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export type ReviewDiffScope = 'staged' | 'unstaged';

export type GitPatchScope = 'staged' | 'unstaged' | 'both';

export type ReviewStatusFileEntry = {
  path: string;
  additions: number;
  deletions: number;
  index_status: string;
  worktree_status: string;
  untracked: boolean;
};

export type ReviewStatusResponse = {
  ok: boolean;
  branch: string | null;
  upstream: string | null;
  ahead: number;
  behind: number;
  staged: ReviewStatusFileEntry[];
  unstaged: ReviewStatusFileEntry[];
};

export type ReviewDiffResponse = {
  ok: boolean;
  scope: ReviewDiffScope;
  path?: string | null;
  from_ref: string;
  to_ref: string;
  patch: string;
};

export type ReviewDiffSessionResponse = {
  ok: boolean;
  session_id: string;
  scope: ReviewDiffScope;
  from_ref: string;
  to_ref: string;
  files: ReviewDiffManifestFileEntry[];
  file_count: number;
  byte_count: number;
};

export type ReviewDiffSessionWindowResponse = {
  ok: boolean;
  session_id: string;
  path: string;
  start_line: number;
  line_count: number;
  total_lines: number;
  has_more: boolean;
  lines: string[];
};

export type ReviewDiffSessionCloseResponse = {
  ok: boolean;
  session_id: string;
  removed: boolean;
};

export type GitPatchResponse = {
  ok: boolean;
  scope: GitPatchScope;
  from_ref: string;
  to_ref: string;
  base_head: string;
  patch: string;
};

export function getReviewStatus(repoRef: string) {
  return fetchJson<ReviewStatusResponse>('/api/review/status', {
    method: 'POST',
    body: JSON.stringify({ repo_ref: repoRef })
  });
}

export function getReviewDiff(body: {
  repo_ref: string;
  scope: ReviewDiffScope;
  path?: string | null;
  context_lines?: number;
  whole_file?: boolean;
}) {
  return fetchJson<ReviewDiffResponse>('/api/review/diff', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function createReviewDiffSession(body: {
  repo_ref: string;
  scope: ReviewDiffScope;
  context_lines?: number;
  whole_file?: boolean;
}) {
  return fetchJson<ReviewDiffSessionResponse>('/api/review/diff/session', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function getReviewDiffSessionWindow(body: {
  session_id: string;
  path: string;
  start_line?: number;
  line_count?: number;
}) {
  return fetchJson<ReviewDiffSessionWindowResponse>('/api/review/diff/session/window', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function createReviewCommitDiffSession(body: {
  repo_ref: string;
  commit: string;
  context_lines?: number;
  whole_file?: boolean;
}) {
  return fetchJson<ReviewDiffSessionResponse>('/api/review/diff/session/commit', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function closeReviewDiffSession(body: {
  session_id: string;
}) {
  return fetchJson<ReviewDiffSessionCloseResponse>('/api/review/diff/session/close', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function generateGitApplyPatch(body: {
  repo_ref: string;
  scope: GitPatchScope;
  paths?: string[] | null;
  context_lines?: number;
}) {
  return fetchJson<GitPatchResponse>('/api/review/git-patch', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export type ReviewDiffManifestFileEntry = {
  path: string;
  additions: number;
  deletions: number;
  index_status: string;
  worktree_status: string;
  untracked: boolean;
};

export type ReviewDiffManifestResponse = {
  ok: boolean;
  scope: ReviewDiffScope;
  from_ref: string;
  to_ref: string;
  files: ReviewDiffManifestFileEntry[];
};

export type ReviewFilePatchResponse = {
  ok: boolean;
  scope: ReviewDiffScope;
  path: string;
  from_ref: string;
  to_ref: string;
  patch: string;
};

export function getReviewDiffManifest(body: {
  repo_ref: string;
  scope: ReviewDiffScope;
}) {
  return fetchJson<ReviewDiffManifestResponse>('/api/review/diff/manifest', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function getReviewFilePatch(body: {
  repo_ref: string;
  scope: ReviewDiffScope;
  path: string;
  context_lines?: number;
  whole_file?: boolean;
}) {
  return fetchJson<ReviewFilePatchResponse>('/api/review/diff/file', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export type ReviewCommitFileStat = {
  path: string;
  additions: number;
  deletions: number;
};

export type ReviewCommitSummary = {
  sha: string;
  short_sha: string;
  subject: string;
  author_name: string;
  author_email: string;
  authored_at: string;
  files_changed?: number | null;
  additions?: number | null;
  deletions?: number | null;
  files?: ReviewCommitFileStat[];
};

export type ReviewCommitListResponse = {
  ok: boolean;
  commits: ReviewCommitSummary[];
  next_offset?: number | null;
  next_cursor?: string | null;
  has_more: boolean;
};

export type ReviewCommitReportExtensionBucket = {
  extension: string;
  additions: number;
  deletions: number;
  net: number;
};

export type ReviewCommitReportMonthBucket = {
  month: string;
  additions: number;
  deletions: number;
  net: number;
  files_changed: number;
  commits: number;
  extensions: ReviewCommitReportExtensionBucket[];
  groups?: ReviewCommitReportGroupBucket[];
};

export type ReviewCommitReportGroupBucket = {
  key: string;
  label: string;
  additions: number;
  deletions: number;
  net: number;
};

export type ReviewCommitReportBucket = {
  period: string;
  additions: number;
  deletions: number;
  net: number;
  files_changed: number;
  commits: number;
  groups: ReviewCommitReportGroupBucket[];
};

export type ReviewCommitReportResponse = {
  ok: boolean;
  commits: ReviewCommitSummary[];
  months: ReviewCommitReportMonthBucket[];
  buckets?: ReviewCommitReportBucket[];
  aggregation_window?: string;
  aggregation_days?: number;
  color_by?: string;
  exclude_regex: string[];
  next_offset?: number | null;
  has_more: boolean;
};

export type ReviewCommitAnalyticsResponse = {
  ok: boolean;
  status: 'complete' | 'partial' | string;
  months: ReviewCommitReportMonthBucket[];
  buckets?: ReviewCommitReportBucket[];
  totals: {
    commits: number;
    additions: number;
    deletions: number;
    files_changed: number;
    net: number;
  };
  aggregation_window?: string;
  aggregation_days?: number;
  color_by?: string;
  exclude_regex: string[];
};

export type ReviewCommitRefOption = {
  value: string;
  label: string;
};

export type ReviewCommitOptionsResponse = {
  ok: boolean;
  refs: ReviewCommitRefOption[];
  default_ref: string;
  default_since?: string | null;
};

export type ReviewCommitDiffManifestResponse = {
  ok: boolean;
  commit: string;
  from_ref: string;
  to_ref: string;
  files: ReviewDiffManifestFileEntry[];
};

export type ReviewCommitDiffResponse = {
  ok: boolean;
  commit: string;
  path?: string | null;
  from_ref: string;
  to_ref: string;
  patch: string;
};

export function getReviewCommits(body: {
  repo_ref: string;
  limit?: number;
  offset?: number;
  cursor?: string | null;
  ref_name?: string | null;
  since?: string | null;
  until?: string | null;
  include_paths?: string[] | null;
  exclude_paths?: string[] | null;
  include_extensions?: string[] | null;
  exclude_extensions?: string[] | null;
  include_regex?: string[] | null;
  exclude_regex?: string[] | null;
}) {
  return fetchJson<ReviewCommitListResponse>('/api/review/commits/query', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function getReviewCommitReport(body: {
  repo_ref: string;
  limit?: number;
  offset?: number;
  ref_name?: string | null;
  aggregation_window?: string | null;
  aggregation_days?: number | null;
  color_by?: string | null;
  since?: string | null;
  until?: string | null;
  include_paths?: string[] | null;
  exclude_paths?: string[] | null;
  include_extensions?: string[] | null;
  exclude_extensions?: string[] | null;
  include_regex?: string[] | null;
  exclude_regex?: string[] | null;
}) {
  return fetchJson<ReviewCommitReportResponse>('/api/review/commit-dataset', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function getReviewCommitAnalytics(body: {
  repo_ref: string;
  ref_name?: string | null;
  aggregation_window?: string | null;
  aggregation_days?: number | null;
  color_by?: string | null;
  since?: string | null;
  until?: string | null;
  include_paths?: string[] | null;
  exclude_paths?: string[] | null;
  include_extensions?: string[] | null;
  exclude_extensions?: string[] | null;
  include_regex?: string[] | null;
  exclude_regex?: string[] | null;
}) {
  return fetchJson<ReviewCommitAnalyticsResponse>('/api/review/commits/analytics', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function getReviewCommitOptions(body: { repo_ref: string }) {
  return fetchJson<ReviewCommitOptionsResponse>('/api/review/commit-options', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function getReviewCommitDiffManifest(body: {
  repo_ref: string;
  commit: string;
}) {
  return fetchJson<ReviewCommitDiffManifestResponse>('/api/review/commit/diff/manifest', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function getReviewCommitDiff(body: {
  repo_ref: string;
  commit: string;
  path?: string | null;
  context_lines?: number;
  whole_file?: boolean;
}) {
  return fetchJson<ReviewCommitDiffResponse>('/api/review/commit/diff', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function stageReviewDiff(body: {
  repo_ref: string;
  scope: ReviewDiffScope;
  path?: string | null;
}) {
  return fetchJson<{ ok: boolean }>('/api/review/stage', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function unstageReviewDiff(body: {
  repo_ref: string;
  scope: ReviewDiffScope;
  path?: string | null;
}) {
  return fetchJson<{ ok: boolean }>('/api/review/unstage', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function discardWorkflowReviewDiff(runId: string, body: {
  scope: ReviewDiffScope;
  path?: string | null;
}) {
  return fetchJson<{ ok: boolean }>(`/api/workflow-runs/${encodeURIComponent(runId)}/review/discard`, {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function getWorkflowReviewStatus(runId: string) {
  return fetchJson<ReviewStatusResponse>(`/api/workflow-runs/${runId}/review/status`);
}

export function getWorkflowReviewDiff(runId: string, body: {
  scope: ReviewDiffScope;
  path?: string | null;
  context_lines?: number;
  whole_file?: boolean;
}) {
  const params = new URLSearchParams({
    scope: body.scope,
    path: body.path ?? '',
    context_lines: String(body.context_lines ?? ''),
    whole_file: String(Boolean(body.whole_file))
  });
  return fetchJson<ReviewDiffResponse>(`/api/workflow-runs/${runId}/review/diff?${params.toString()}`);
}

export function getWorkflowReviewDiffManifest(runId: string, scope: ReviewDiffScope) {
  const params = new URLSearchParams({ scope });
  return fetchJson<ReviewDiffManifestResponse>(`/api/workflow-runs/${runId}/review/diff/manifest?${params.toString()}`);
}

export function stageWorkflowReviewDiff(runId: string, body: { scope: ReviewDiffScope; path?: string | null }) {
  return fetchJson<{ ok: boolean }>(`/api/workflow-runs/${runId}/review/stage`, {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function unstageWorkflowReviewDiff(runId: string, body: { scope: ReviewDiffScope; path?: string | null }) {
  return fetchJson<{ ok: boolean }>(`/api/workflow-runs/${runId}/review/unstage`, {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function getWorkflowCommits(runId: string, body?: {
  limit?: number;
  offset?: number;
  since?: string | null;
  until?: string | null;
  exclude_regex?: string[] | null;
}) {
  return fetchJson<ReviewCommitListResponse>(`/api/workflow-runs/${runId}/commits`, {
    method: 'POST',
    body: JSON.stringify(body ?? {})
  });
}

export type SapSearchObject = {
  uri: string;
  source_uri?: string | null;
  name: string;
  object_type: string;
  package_name?: string | null;
};

export type SapSearchResponse = {
  ok: boolean;
  package_name: string;
  objects: SapSearchObject[];
  count: number;
};

export type SapManifestResource = {
  id: string;
  path?: string | null;
  uri?: string | null;
  source_uri?: string | null;
  content_type?: string | null;
};

export type SapManifestDocument = {
  path: string;
  contents: string;
  content_type?: string | null;
  resource_id?: string | null;
};

export type SapObjectManifest = {
  schema_version?: number;
  metadata_uri?: string | null;
  object_uri?: string | null;
  object_name?: string | null;
  object_type?: string | null;
  package_name?: string | null;
  resources?: SapManifestResource[];
  documents?: SapManifestDocument[];
};

export type SapExportScanItem = {
  manifest_path: string;
  object_name: string;
  object_type: string;
  package_name?: string | null;
  candidate_count: number;
  resource_paths: string[];
};

export type SapExportScanResponse = {
  ok: boolean;
  manifests: SapExportScanItem[];
  count: number;
};

export function sapSearchObjects(packageName: string, includeSubpackages: boolean) {
  return fetchJson<SapSearchResponse>('/api/sap/search', {
    method: 'POST',
    body: JSON.stringify({
      package_name: packageName,
      include_subpackages: includeSubpackages
    })
  });
}

export function sapScanExportCandidates(repoRef: string) {
  return fetchJson<SapExportScanResponse>('/api/sap/export-scan', {
    method: 'POST',
    body: JSON.stringify({ repo_ref: repoRef })
  });
}


