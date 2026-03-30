export type AutomationMode = 'manual' | 'assisted' | 'automatic';

export type WorkflowCapabilityBinding = {
  capability: string;
  enabled: boolean;
  config: Record<string, unknown>;
  input_mapping: Record<string, unknown>;
  output_mapping: Record<string, unknown>;
};

export type WorkflowGlobalConfig = {
  inference: Record<string, unknown>;
  prompt_fragments: Record<string, unknown>;
  capabilities: WorkflowCapabilityBinding[];
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

export type WorkflowStepDefinition = {
  id: string;
  name: string;
  step_type: string;
  automation_mode: AutomationMode;
  execution: WorkflowStepExecutionConfig;
  prompt: WorkflowStepPromptConfig;
  config: Record<string, unknown>;
  capabilities: WorkflowCapabilityBinding[];
  transitions: Array<{ when: { type: string; value?: string }; target_step_id: string }>;
};

export type WorkflowTemplateDefinition = {
  version: number;
  globals: WorkflowGlobalConfig;
  steps: WorkflowStepDefinition[];
};

export type WorkflowTemplate = {
  id: string;
  name: string;
  description: string;
  definition: WorkflowTemplateDefinition;
  created_at: string;
  updated_at: string;
};

export type WorkflowRun = {
  id: string;
  template_id: string | null;
  status: 'draft' | 'running' | 'paused' | 'success' | 'error';
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

export type WorkflowRunActionResult = {
  ok: boolean;
  status?: string;
  step_id?: string;
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

export function createTemplate(body: { name: string; description: string; definition: WorkflowTemplateDefinition }) {
  return fetchJson<WorkflowTemplate>('/api/workflow-templates', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function listRuns() {
  return fetchJson<WorkflowRun[]>('/api/workflow-runs');
}

export function createRun(body: { template_id?: string | null; title: string; repo_ref: string; context: Record<string, unknown> }) {
  return fetchJson<WorkflowRun>('/api/workflow-runs', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function getRun(runId: string) {
  return fetchJson<WorkflowRun>(`/api/workflow-runs/${runId}`);
}

export function listRunEvents(runId: string) {
  return fetchJson<WorkflowEvent[]>(`/api/workflow-runs/${runId}/events`);
}

export function sendRunAction(runId: string, body: { action: string; step_id?: string | null; payload?: Record<string, unknown> }) {
  return fetchJson<WorkflowRunActionResult>(`/api/workflow-runs/${runId}/actions`, {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function selectWorkflowStep(runId: string, stepId: string) {
  return sendRunAction(runId, { action: 'select_step', step_id: stepId });
}

export function runCurrentWorkflowStep(runId: string, stepId?: string | null) {
  return sendRunAction(runId, { action: 'run_step', step_id: stepId ?? undefined });
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

export function deleteRun(runId: string) {
  return fetchJson<{ ok: boolean }>(`/api/workflow-runs/${runId}`, {
    method: 'DELETE'
  });
}

export function invokeModelInference(
  runId: string,
  body: {
    step_id?: string | null;
    action: 'configure' | 'launch_browser' | 'open_url' | 'probe_browser' | 'send_prompt';
    payload?: Record<string, unknown>;
  }
) {
  return fetchJson<Record<string, unknown>>(`/api/workflow-runs/${runId}/capabilities/model-inference`, {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function invokeContextExport(
  runId: string,
  body: {
    step_id?: string | null;
    payload: Record<string, unknown>;
  }
) {
  return fetchJson<Record<string, unknown>>(`/api/workflow-runs/${runId}/capabilities/context-export`, {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function getPayloadGatewaySchema() {
  return fetchJson<{
    ok: boolean;
    name: string;
    version: number;
    example: string;
  }>(`/api/capabilities/payload-gateway/schema`);
}

export function invokePayloadGateway(
  runId: string,
  body: {
    step_id?: string | null;
    payload: Record<string, unknown>;
  }
) {
  return fetchJson<Record<string, unknown>>(`/api/workflow-runs/${runId}/capabilities/payload-gateway`, {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export function invokeTerminal(
  runId: string,
  body: {
    step_id?: string | null;
    payload: Record<string, unknown>;
  }
) {
  return fetchJson<Record<string, unknown>>(`/api/workflow-runs/${runId}/capabilities/terminal`, {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

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

export type ModelInferenceResult = {
  transport: InferenceTransport;
  text: string;
  conversation_id?: string | null;
  browser_session_id?: string | null;
};
