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
};

export type WorkflowTemplateDefinition = {
  version: number;
  globals: WorkflowGlobalConfig;
  steps: WorkflowStepDefinition[];
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
  status: WorkflowRunStatus;
  current_step_id: string | null;
  title: string;
  repo_ref: string;
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
  duration_ms: number | null;
  latest_created_at: string;
  is_active: boolean;
  event_count: number;
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

export type WorkflowRunActionResult = {
  ok: boolean;
  status?: string;
  step_id?: string;
  next_step_id?: string;
  step_type?: string;
  message?: string;
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

export function createTemplate(body: { name: string; description: string; definition: WorkflowTemplateDefinition }) {
  return fetchJson<WorkflowTemplate>('/api/workflow-templates', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function listRuns() {
  return fetchJson<WorkflowRun[]>('/api/workflow-runs');
}

export function getRun(runId: string) {
  return fetchJson<WorkflowRun>(`/api/workflow-runs/${runId}`);
}

export function createRun(body: { template_id?: string | null; title: string; repo_ref: string; context: Record<string, unknown> }) {
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

export function sendRunAction(runId: string, body: { action: string; step_id?: string | null; payload?: Record<string, unknown> }) {
  return fetchJson<WorkflowRunActionResult>(`/api/workflow-runs/${runId}/actions`, {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function startWorkflowRun(runId: string) {
  return sendRunAction(runId, { action: 'start_run' });
}

export function resumeWorkflowRun(runId: string) {
  return sendRunAction(runId, { action: 'resume_run' });
}

export function pauseWorkflowRun(runId: string) {
  return sendRunAction(runId, { action: 'pause_run' });
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


