export type SupervisorExecutionStrategy = 'series' | 'parallel';

export type FeaturePlanItemStatus = 'rough' | 'fine' | 'scheduled' | 'applied' | 'completed' | 'refined' | 'approved';

export type FeaturePlanItem = {
  id: string;
  title: string;
  status: FeaturePlanItemStatus;
  summary: string;
  rough_summary?: string | null;
  refinement_workflow_run_id?: string | null;
  applied_sprint_id?: string | null;
  applied_sprint_title?: string | null;
  applied_at?: string | null;
  requirements: string[];
  acceptance_criteria: string[];
  implementation_notes: string[];
  review_expectations: string[];
  target_files_or_areas: string[];
  dependencies: string[];
};

export type ExecutionPlanItem = {
  feature_plan_item_id: string;
  workflow_template_id?: string | null;
  order_index?: number | null;
};

export type SupervisorChildRun = {
  execution_item_id: string;
  title: string;
  shard_path: string;
  workflow_run_id?: string | null;
  status: string;
  patch_path?: string | null;
};

export type SupervisorFeatureWorkflow = {
  feature_id: string;
  title: string;
  shard_path?: string | null;
  workflow_run_id?: string | null;
  status: string;
  development_state?: string | null;
  current_step_id?: string | null;
  current_patch_id?: string | null;
  last_error?: string | null;
};

export type SupervisorRun = {
  id: string;
  strategy: SupervisorExecutionStrategy;
  status: string;
  title: string;
  root_repo_path: string;
  snapshot_path?: string | null;
  integration_path?: string | null;
  feature_plan_items: FeaturePlanItem[];
  execution_plan_items: ExecutionPlanItem[];
  child_runs: SupervisorChildRun[];
  feature_workflows: SupervisorFeatureWorkflow[];
  integration_run_id?: string | null;
  final_patch_path?: string | null;
  merge_report: Record<string, unknown>;
  validation_report: Record<string, unknown>;
  context: Record<string, unknown>;
  created_at: string;
  updated_at: string;
};

export type CreateSupervisorRunRequest = {
  title: string;
  root_repo_path: string;
  strategy: SupervisorExecutionStrategy;
  workflow_template_id?: string | null;
  integration_template_id?: string | null;
  feature_plan_items: FeaturePlanItem[];
  execution_plan_items?: ExecutionPlanItem[];
  context?: Record<string, unknown>;
};

export type PlannerImportStatus = 'accepted' | 'duplicate' | 'conflict' | 'invalid';
export type PlannerImportAction = 'create' | 'create_copy' | 'replace_existing' | 'skip' | 'reject';

export type PlannerImportPreviewItem = {
  import_index: number;
  status: PlannerImportStatus;
  default_action: PlannerImportAction;
  reason: string;
  feature?: FeaturePlanItem;
  existing_feature_id?: string | null;
  existing_title?: string | null;
  content_fingerprint?: string | null;
  raw?: unknown;
};

export type PlannerImportPreviewResponse = {
  ok: boolean;
  summary: {
    total: number;
    accepted: number;
    duplicates: number;
    conflicts: number;
    invalid: number;
  };
  items: PlannerImportPreviewItem[];
};

export type PlannerImportDecision = {
  import_index: number;
  action: PlannerImportAction;
  existing_feature_id?: string | null;
};

export type PlannerImportApplyResponse = {
  ok: boolean;
  summary: {
    created: number;
    replaced: number;
    skipped: number;
    rejected: number;
  };
  supervisor_run: SupervisorRun;
};

function canonicalFeatureStatus(status: FeaturePlanItemStatus): FeaturePlanItemStatus {
  if (status === 'refined' || status === 'approved') return 'fine';
  return status;
}

function completedFeatureIds(run: SupervisorRun): Set<string> {
  const completedFeatures = run.context?.completed_features;
  if (!Array.isArray(completedFeatures)) return new Set();
  return new Set(
    completedFeatures
      .map((item) => {
        if (item && typeof item === 'object' && 'id' in item && typeof item.id === 'string') return item.id;
        return '';
      })
      .filter(Boolean)
  );
}

function normalizeSupervisorRun(run: SupervisorRun): SupervisorRun {
  const executionPlanItems = Array.isArray(run.execution_plan_items) ? run.execution_plan_items : [];
  const featurePlanItems = Array.isArray(run.feature_plan_items) ? run.feature_plan_items : [];
  const childRuns = Array.isArray(run.child_runs) ? run.child_runs : [];
  const featureWorkflows = Array.isArray(run.feature_workflows) ? run.feature_workflows : [];
  return {
    ...run,
    feature_plan_items: featurePlanItems.map((item) => ({
      ...item,
      status: canonicalFeatureStatus(item.status === 'scheduled' ? 'fine' : item.status),
      dependencies: []
    })),
    execution_plan_items: executionPlanItems,
    child_runs: childRuns,
    feature_workflows: featureWorkflows,
    merge_report: run.merge_report ?? {},
    validation_report: run.validation_report ?? {},
    context: run.context ?? {}
  };
}

function serializeFeatureForApi(item: FeaturePlanItem): FeaturePlanItem {
  return {
    ...item,
    status: item.status === 'rough' ? 'rough' : 'refined',
    dependencies: []
  };
}

function serializeCreateSupervisorRunRequest(request: CreateSupervisorRunRequest): CreateSupervisorRunRequest {
  return {
    ...request,
    feature_plan_items: (request.feature_plan_items ?? []).map(serializeFeatureForApi)
  };
}

export async function listSupervisorRuns(): Promise<SupervisorRun[]> {
  const response = await fetch('/api/supervisor-runs');
  if (!response.ok) throw new Error(await response.text());
  const runs = await response.json();
  return runs.map(normalizeSupervisorRun);
}

export async function createSupervisorRun(request: CreateSupervisorRunRequest): Promise<SupervisorRun> {
  const response = await fetch('/api/supervisor-runs', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(serializeCreateSupervisorRunRequest(request))
  });
  if (!response.ok) throw new Error(await response.text());
  return normalizeSupervisorRun(await response.json());
}

function supervisorRouteIsActive(): boolean {
  if (typeof window === 'undefined') return true;
  const path = window.location.pathname;
  return path === '/supervisors' || path.startsWith('/supervisors/');
}

export async function getSupervisorRun(id: string): Promise<SupervisorRun> {
  if (!supervisorRouteIsActive()) {
    throw new Error(`Refusing stale supervisor run fetch outside supervisor route: ${id}`);
  }
  const response = await fetch(`/api/supervisor-runs/${id}`);
  if (!response.ok) throw new Error(await response.text());
  return normalizeSupervisorRun(await response.json());
}

export type SupervisorQueuedFeature = {
  feature_id: string;
  planner_id: string;
  planner_title: string;
};

export type SupervisorQueueItem = {
  feature_id: string;
  planner_id?: string | null;
  planner_title?: string | null;
  is_current_planner?: boolean;
  title: string;
  summary?: string | null;
  planner_status?: string | null;
  queue_state: string;
  queued: boolean;
  can_queue: boolean;
  can_dequeue: boolean;
  dequeue_without_prompt?: boolean;
  has_development_diff?: boolean;
  locked_by_other: boolean;
  lock_owner_supervisor_run_id?: string | null;
  disabled_reason?: string | null;
  current_sprint_id?: string | null;
  current_workflow_run_id?: string | null;
  current_patch_id?: string | null;
  development_state?: string | null;
  scheduled_at?: string | null;
  development_started_at?: string | null;
  development_completed_at?: string | null;
  integration_completed_at?: string | null;
  applied_at?: string | null;
};

export type SupervisorQueueProjection = {
  ok: boolean;
  supervisor_run_id: string;
  root_repo_path: string;
  queued_features: SupervisorQueuedFeature[];
  feature_ids: string[];
  items: SupervisorQueueItem[];
};

export type FlightDeckAlert = {
  id: string;
  supervisor_id: string;
  work_unit_id?: string | null;
  workflow_run_id?: string | null;
  level: string;
  kind: string;
  message: string;
  created_at?: string | null;
};

export type FlightDeckWorkUnit = {
  id: string;
  supervisor_id: string;
  repo_id?: string | null;
  feature_id?: string | null;
  workflow_run_id?: string | null;
  patch_id?: string | null;
  kind: string;
  workflow_type?: string | null;
  title: string;
  state: string;
  root_repo_path: string;
  shard_path?: string | null;
  integration_path?: string | null;
  queue_position?: number | null;
  blocked_reason?: string | null;
  telemetry: Record<string, unknown>;
  alerts: FlightDeckAlert[];
  created_at?: string | null;
  updated_at?: string | null;
  workflow_deleted: boolean;
};

export type FlightDeckTopologyNode = {
  id: string;
  parent_id?: string | null;
  kind: string;
  label: string;
  state: string;
  work_unit_id?: string | null;
  workflow_run_id?: string | null;
};

export type FlightDeckSupervisor = {
  id: string;
  mode: string;
  status: string;
  title: string;
  root_repo_path: string;
  snapshot_path?: string | null;
  integration_path?: string | null;
  integration_run_id?: string | null;
  topology: FlightDeckTopologyNode[];
  work_units: FlightDeckWorkUnit[];
  alerts: FlightDeckAlert[];
  integration: Record<string, unknown>;
  context: Record<string, unknown>;
  created_at: string;
  updated_at: string;
};

export type FlightDeckResponse = {
  supervisors: FlightDeckSupervisor[];
  alerts: FlightDeckAlert[];
  totals: {
    supervisors: number;
    work_units: number;
    running: number;
    waiting_user: number;
    failed: number;
    ready_for_integration: number;
    integrating: number;
  };
};

export type FlightDeckFilters = {
  supervisor_id?: string | null;
  root_repo_path?: string | null;
  state?: string | null;
  kind?: string | null;
  include_deleted?: boolean;
};

export type WorkflowEventHistoryItem = {
  id: string;
  run_id: string;
  step_id?: string | null;
  stage_execution_id?: string | null;
  capability_invocation_id?: string | null;
  parent_invocation_id?: string | null;
  sequence_no: number;
  level: string;
  kind: string;
  message: string;
  payload: Record<string, unknown>;
  created_at: string;
};

export type WorkflowEventHistoryQuery = {
  before_sequence?: number | null;
  after_sequence?: number | null;
  limit?: number | null;
  start?: string | null;
  end?: string | null;
  stage?: string | null;
  capability?: string | null;
  stage_execution_id?: string | null;
  capability_invocation_id?: string | null;
};

export type WorkflowEventHistoryResponse = {
  run_id: string;
  items: WorkflowEventHistoryItem[];
  next_before_sequence?: number | null;
  has_more: boolean;
};

function workflowEventHistoryParams(query: WorkflowEventHistoryQuery = {}): URLSearchParams {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== null && value !== undefined && `${value}`.trim() !== '') params.set(key, `${value}`);
  }
  return params;
}

export function workflowEventHistoryStreamUrl(runId: string, query: WorkflowEventHistoryQuery = {}): string {
  const params = workflowEventHistoryParams(query);
  const queryText = params.toString();
  return `/api/workflow-runs/${encodeURIComponent(runId)}/events/stream${queryText ? `?${queryText}` : ''}`;
}

export async function getWorkflowEventHistory(runId: string, query: WorkflowEventHistoryQuery = {}): Promise<WorkflowEventHistoryResponse> {
  const params = workflowEventHistoryParams(query);
  const queryText = params.toString();
  const response = await fetch(`/api/workflow-runs/${encodeURIComponent(runId)}/event-history${queryText ? `?${queryText}` : ''}`);
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}

export async function getFlightDeck(filters: FlightDeckFilters = {}): Promise<FlightDeckResponse> {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(filters)) {
    if (typeof value === 'boolean') {
      if (value) params.set(key, 'true');
    } else if (value) {
      params.set(key, value);
    }
  }
  const query = params.toString();
  const response = await fetch(`/api/flight-deck${query ? `?${query}` : ''}`);
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}

export async function deleteSupervisorRun(id: string): Promise<{ ok: boolean }> {
  const response = await fetch(`/api/supervisor-runs/${id}`, { method: 'DELETE' });
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}

export async function updateSupervisorPlan(
  id: string,
  plannerLogItems: FeaturePlanItem[],
  sprintItems: ExecutionPlanItem[],
  sprintConfig: {
    sprint_strategy: SupervisorExecutionStrategy;
    workflow_template_id?: string | null;
    integration_template_id?: string | null;
    planner_refinement_template_id?: string | null;
    feature_concurrency?: number | null;
    integration_policy?: 'auto' | 'manual' | null;
  }
): Promise<Record<string, unknown>> {
  return runSupervisorAction(id, 'update_plan', {
    planner_log_items: plannerLogItems.map(serializeFeatureForApi),
    sprint_items: sprintItems,
    ...sprintConfig
  });
}

export async function getSupervisorQueue(id: string, plannerId?: string | null): Promise<SupervisorQueueProjection> {
  const params = new URLSearchParams();
  if (plannerId) params.set('planner_id', plannerId);
  const query = params.toString();
  const response = await fetch(`/api/supervisor-runs/${id}/queue${query ? `?${query}` : ''}`);
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}

export async function setSupervisorQueue(
  id: string,
  queuedFeatures: SupervisorQueuedFeature[],
  config: {
    workflow_template_id?: string | null;
    integration_template_id?: string | null;
    feature_concurrency?: number | null;
    integration_policy?: 'auto' | 'manual' | null;
    auto_start?: boolean;
    planner_id?: string | null;
  } = {}
): Promise<{ ok: boolean; supervisor_run: SupervisorRun }> {
  const response = await fetch(`/api/supervisor-runs/${id}/queue`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      queued_features: queuedFeatures,
      ...config
    })
  });
  if (!response.ok) throw new Error(await response.text());
  const payload = await response.json();
  return {
    ...payload,
    supervisor_run: normalizeSupervisorRun(payload.supervisor_run as SupervisorRun)
  } as { ok: boolean; supervisor_run: SupervisorRun };
}

export async function selectSupervisorFeaturePool(
  id: string,
  queuedFeatures: SupervisorQueuedFeature[],
  config: {
    workflow_template_id?: string | null;
    integration_template_id?: string | null;
    feature_concurrency?: number | null;
    integration_policy?: 'auto' | 'manual' | null;
    auto_start?: boolean;
    planner_id?: string | null;
  } = {}
): Promise<{ ok: boolean; supervisor_run: SupervisorRun }> {
  return setSupervisorQueue(id, queuedFeatures, config);
}

export async function previewPlannerImport(id: string, payload: unknown): Promise<PlannerImportPreviewResponse> {
  return runSupervisorAction(id, 'preview_planner_import', payload as Record<string, unknown>) as Promise<PlannerImportPreviewResponse>;
}

export async function applyPlannerImport(id: string, payload: unknown, decisions: PlannerImportDecision[]): Promise<PlannerImportApplyResponse> {
  const response = await runSupervisorAction(id, 'apply_planner_import', {
    import: payload,
    decisions
  });
  return {
    ...response,
    supervisor_run: normalizeSupervisorRun(response.supervisor_run as SupervisorRun)
  } as PlannerImportApplyResponse;
}

export type RefineSupervisorFeatureResponse = {
  ok: boolean;
  workflow_run_id: string;
};

export type UnscheduleSupervisorFeatureMode = 'preserve_development' | 'delete_development';

export type UnscheduleSupervisorFeatureResponse = {
  ok: boolean;
  supervisor_run: SupervisorRun;
};

export async function regenerateSupervisorQueueFeature(id: string, featureId: string): Promise<{ ok: boolean; supervisor_run: SupervisorRun }> {
  const response = await fetch(`/api/supervisor-runs/${id}/queue/${encodeURIComponent(featureId)}/regenerate`, {
    method: 'POST'
  });
  if (!response.ok) throw new Error(await response.text());
  const body = await response.json();
  return {
    ...body,
    supervisor_run: normalizeSupervisorRun(body.supervisor_run as SupervisorRun)
  } as { ok: boolean; supervisor_run: SupervisorRun };
}

export async function unscheduleSupervisorFeature(id: string, featureId: string, mode: UnscheduleSupervisorFeatureMode): Promise<UnscheduleSupervisorFeatureResponse> {
  const params = new URLSearchParams();
  params.set('mode', mode);
  const response = await fetch(`/api/supervisor-runs/${id}/queue/${encodeURIComponent(featureId)}?${params.toString()}`, {
    method: 'DELETE'
  });
  if (!response.ok) throw new Error(await response.text());
  const body = await response.json();
  return {
    ...body,
    supervisor_run: normalizeSupervisorRun(body.supervisor_run as SupervisorRun)
  } as UnscheduleSupervisorFeatureResponse;
}

export async function refineSupervisorFeature(id: string, featureId: string, workflowTemplateId?: string | null): Promise<RefineSupervisorFeatureResponse> {
  return runSupervisorAction(id, 'refine_feature', {
    feature_id: featureId,
    workflow_template_id: workflowTemplateId ?? null
  }) as Promise<RefineSupervisorFeatureResponse>;
}

export async function runSupervisorAction(id: string, action: 'start' | 'tick' | 'apply' | 'cancel' | 'start_integration' | 'restart_integration' | 'restart_sprint' | 'reopen_development' | 'new_sprint' | 'update_plan' | 'update_flight_deck_settings' | 'preview_planner_import' | 'apply_planner_import' | 'refine_feature' | 'start_child_workflow' | 'pause_child_workflow' | 'pause_feature_pool' | 'resume_feature_pool' | 'remove_child_workflow' | 'create_manual_shard' | 'delete_manual_shard' | 'delete_refine_workflow' | 'stage_manual_shard' | 'unstage_manual_shard' | 'skip_integration_input' | 'unskip_integration_input', payload: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const response = await fetch(`/api/supervisor-runs/${id}/actions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ action, payload })
  });
  if (!response.ok) throw new Error(await response.text());
  const result = await response.json();
  if (result && typeof result === 'object' && result.supervisor_run) {
    return {
      ...result,
      supervisor_run: normalizeSupervisorRun(result.supervisor_run as SupervisorRun)
    };
  }
  return result;
}
