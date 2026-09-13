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
  planner?: unknown;
  supervisor_run?: SupervisorRun;
};

function importFeatureId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') return crypto.randomUUID();
  return `${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function importString(value: unknown): string {
  return typeof value === 'string' ? value.trim() : '';
}

function importStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.map((item) => importString(item)).filter(Boolean);
}

function importStatus(value: unknown): FeaturePlanItemStatus {
  const status = importString(value);
  if (status === 'fine' || status === 'refined' || status === 'approved') return 'fine';
  if (status === 'scheduled') return 'scheduled';
  if (status === 'applied') return 'applied';
  if (status === 'completed') return 'completed';
  return 'rough';
}

function importedFeatureValues(payload: unknown): unknown[] {
  if (Array.isArray(payload)) return payload;
  if (payload && typeof payload === 'object' && Array.isArray((payload as { features?: unknown[] }).features)) return (payload as { features: unknown[] }).features;
  throw new Error('planner import must be a JSON feature array or an object with a features array');
}

function normalizeImportedFeature(value: unknown, index: number): FeaturePlanItem {
  if (!value || typeof value !== 'object') throw new Error(`feature import item ${index + 1} must be an object`);
  const item = value as Record<string, unknown>;
  const title = importString(item.title);
  if (!title) throw new Error('missing required title');
  const status = importStatus(item.status);
  const summary = importString(item.summary);
  const roughSummary = importString(item.rough_summary) || (status === 'rough' ? summary : '');
  return {
    id: importString(item.id) || importFeatureId(),
    title,
    status,
    summary,
    rough_summary: roughSummary || null,
    refinement_workflow_run_id: importString(item.refinement_workflow_run_id) || null,
    applied_sprint_id: importString(item.applied_sprint_id) || null,
    applied_sprint_title: importString(item.applied_sprint_title) || null,
    applied_at: importString(item.applied_at) || null,
    requirements: importStringArray(item.requirements),
    acceptance_criteria: importStringArray(item.acceptance_criteria),
    implementation_notes: importStringArray(item.implementation_notes),
    review_expectations: importStringArray(item.review_expectations),
    target_files_or_areas: importStringArray(item.target_files_or_areas),
    dependencies: []
  };
}

function normalizedImportText(value: string): string {
  return value.split(/\s+/).filter(Boolean).join(' ').toLowerCase();
}

function importTitleKey(feature: FeaturePlanItem): string {
  return normalizedImportText(feature.title);
}

function importContentFingerprint(feature: FeaturePlanItem): string {
  return JSON.stringify({
    title: normalizedImportText(feature.title),
    summary: normalizedImportText(feature.summary),
    requirements: feature.requirements.map(normalizedImportText),
    acceptance_criteria: feature.acceptance_criteria.map(normalizedImportText),
    implementation_notes: feature.implementation_notes.map(normalizedImportText),
    review_expectations: feature.review_expectations.map(normalizedImportText),
    target_files_or_areas: feature.target_files_or_areas.map(normalizedImportText)
  });
}

function plannerImportSummary(items: PlannerImportPreviewItem[]): PlannerImportPreviewResponse['summary'] {
  return {
    total: items.length,
    accepted: items.filter((item) => item.status === 'accepted').length,
    duplicates: items.filter((item) => item.status === 'duplicate').length,
    conflicts: items.filter((item) => item.status === 'conflict').length,
    invalid: items.filter((item) => item.status === 'invalid').length
  };
}

async function fetchPlannerImportWorkspace(plannerId: string): Promise<{ id: string; features: FeaturePlanItem[] }> {
  const response = await fetch(`/api/planners/${encodeURIComponent(plannerId)}`);
  if (!response.ok) throw new Error(await response.text());
  const planner = await response.json() as { id: string; features?: FeaturePlanItem[] };
  return {
    id: planner.id,
    features: Array.isArray(planner.features) ? planner.features : []
  };
}

function buildPlannerImportPreview(existingFeatures: FeaturePlanItem[], payload: unknown): PlannerImportPreviewResponse {
  const existingById = new Map(existingFeatures.map((item) => [item.id, item]));
  const existingByTitle = new Map<string, FeaturePlanItem>();
  for (const feature of existingFeatures) {
    const key = importTitleKey(feature);
    if (key && !existingByTitle.has(key)) existingByTitle.set(key, feature);
  }

  const seenImportIds = new Set<string>();
  const seenImportTitles = new Set<string>();
  const items: PlannerImportPreviewItem[] = [];

  for (const [index, raw] of importedFeatureValues(payload).entries()) {
    let feature: FeaturePlanItem;
    try {
      feature = normalizeImportedFeature(raw, index);
    } catch (err) {
      items.push({
        import_index: index,
        status: 'invalid',
        default_action: 'reject',
        reason: err instanceof Error ? err.message : String(err),
        raw
      });
      continue;
    }

    const titleKey = importTitleKey(feature);
    const contentFingerprint = importContentFingerprint(feature);

    if (!seenImportIds.has(feature.id)) {
      seenImportIds.add(feature.id);
    } else {
      items.push({
        import_index: index,
        status: 'invalid',
        default_action: 'reject',
        reason: 'duplicate feature id inside uploaded file',
        feature,
        content_fingerprint: contentFingerprint
      });
      continue;
    }

    if (titleKey) {
      if (seenImportTitles.has(titleKey)) {
        items.push({
          import_index: index,
          status: 'invalid',
          default_action: 'reject',
          reason: 'duplicate feature title inside uploaded file',
          feature,
          content_fingerprint: contentFingerprint
        });
        continue;
      }
      seenImportTitles.add(titleKey);
    }

    const existingByFeatureId = existingById.get(feature.id);
    const existingByFeatureTitle = titleKey ? existingByTitle.get(titleKey) : undefined;
    const existing = existingByFeatureId ?? existingByFeatureTitle;

    if (existing) {
      const duplicate = importContentFingerprint(existing) === contentFingerprint;
      items.push({
        import_index: index,
        status: duplicate ? 'duplicate' : 'conflict',
        default_action: 'skip',
        reason: duplicate ? 'feature already exists' : 'feature matches an existing edited feature with different content',
        existing_feature_id: existing.id,
        existing_title: existing.title,
        feature,
        content_fingerprint: contentFingerprint
      });
      continue;
    }

    items.push({
      import_index: index,
      status: 'accepted',
      default_action: 'create',
      reason: 'new feature',
      feature,
      content_fingerprint: contentFingerprint
    });
  }

  return {
    ok: true,
    summary: plannerImportSummary(items),
    items
  };
}

export async function previewPlannerImport(plannerId: string, payload: unknown): Promise<PlannerImportPreviewResponse> {
  const planner = await fetchPlannerImportWorkspace(plannerId);
  return buildPlannerImportPreview(planner.features, payload);
}

export async function applyPlannerImport(plannerId: string, payload: unknown, decisions: PlannerImportDecision[]): Promise<PlannerImportApplyResponse> {
  const planner = await fetchPlannerImportWorkspace(plannerId);
  const preview = buildPlannerImportPreview(planner.features, payload);
  const decisionByIndex = new Map(decisions.map((item) => [item.import_index, item]));
  const nextFeatures = [...planner.features];
  const created: PlannerImportPreviewItem[] = [];
  const replaced: PlannerImportPreviewItem[] = [];
  const skipped: PlannerImportPreviewItem[] = [];
  const rejected: PlannerImportPreviewItem[] = [];

  for (const item of preview.items) {
    const decision = decisionByIndex.get(item.import_index);
    const action = decision?.action ?? item.default_action;
    const feature = item.feature;

    if (!feature) {
      rejected.push(item);
      continue;
    }

    if (action === 'create' && item.status === 'accepted') {
      nextFeatures.push(feature);
      created.push(item);
      continue;
    }

    if (action === 'create_copy' && (item.status === 'accepted' || item.status === 'duplicate' || item.status === 'conflict')) {
      nextFeatures.push({ ...feature, id: importFeatureId() });
      created.push(item);
      continue;
    }

    if (action === 'replace_existing' && item.status === 'conflict') {
      const existingFeatureId = decision?.existing_feature_id ?? item.existing_feature_id ?? null;
      const existingIndex = existingFeatureId ? nextFeatures.findIndex((existing) => existing.id === existingFeatureId) : -1;
      if (existingIndex >= 0 && existingFeatureId) {
        nextFeatures[existingIndex] = { ...feature, id: existingFeatureId };
        replaced.push(item);
      } else {
        rejected.push({ ...item, reason: `existing feature ${existingFeatureId ?? ''} is missing` });
      }
      continue;
    }

    if (action === 'skip') {
      skipped.push(item);
      continue;
    }

    rejected.push(item);
  }

  const response = await fetch(`/api/planners/${encodeURIComponent(plannerId)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ features: nextFeatures })
  });
  if (!response.ok) throw new Error(await response.text());
  const updatedPlanner = await response.json();

  return {
    ok: true,
    summary: {
      created: created.length,
      replaced: replaced.length,
      skipped: skipped.length,
      rejected: rejected.length
    },
    planner: updatedPlanner
  };
}


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

export async function createSupervisorRun(request: CreateSupervisorRunRequest): Promise<SupervisorRun> {
  const response = await fetch('/api/supervisor-runs', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(serializeCreateSupervisorRunRequest(request))
  });
  if (!response.ok) throw new Error(await response.text());
  return normalizeSupervisorRun(await response.json());
}

export type SupervisorQueuePlanner = {
  id: string;
  root_repo_path: string;
  title: string;
  is_default: boolean;
  feature_count?: number;
  created_at?: string;
  updated_at?: string;
};

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
  current_workflow_run_id?: string | null;
  current_patch_id?: string | null;
  development_state?: string | null;
  locked_at?: string | null;
  completed_at?: string | null;
  applied_at?: string | null;
};

export type SupervisorQueueProjection = {
  ok: boolean;
  supervisor_run_id: string;
  root_repo_path: string;
  current_planner_id?: string | null;
  planners?: SupervisorQueuePlanner[];
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
  selected_planner_id?: string | null;
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

export async function getSupervisorQueue(id: string): Promise<SupervisorQueueProjection> {
  const response = await fetch(`/api/supervisor-runs/${id}/queue`);
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}

export async function setSupervisorQueue(
  id: string,
  queuedFeatures: SupervisorQueuedFeature[]
): Promise<{ ok: boolean; supervisor_run: SupervisorRun }> {
  const response = await fetch(`/api/supervisor-runs/${id}/queue`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      queued_features: queuedFeatures
    })
  });
  if (!response.ok) throw new Error(await response.text());
  const payload = await response.json();
  return {
    ...payload,
    supervisor_run: normalizeSupervisorRun(payload.supervisor_run as SupervisorRun)
  } as { ok: boolean; supervisor_run: SupervisorRun };
}
export type SupervisorWorkPoolKind = 'refine' | 'feature_development' | 'manual_shard' | 'integration';

export type SupervisorActionRequest =
  | { action: 'create_work_unit'; pool_kind: SupervisorWorkPoolKind; name: string; feature_id?: string | null; template_id?: string | null }
  | { action: 'delete_work_unit'; work_unit_id: string }
  | { action: 'regenerate_work_unit'; work_unit_id: string }
  | { action: 'start_work_unit'; work_unit_id: string }
  | { action: 'pause_work_unit'; work_unit_id: string }
  | { action: 'stage_work_unit'; work_unit_id: string; staged?: boolean }
  | { action: 'update_supervisor_config'; config: Record<string, unknown> }
  | { action: 'select_planner'; planner_id: string }
  | { action: 'pause_feature_pool' }
  | { action: 'resume_feature_pool' }
  | { action: 'skip_integration_input'; work_unit_id: string }
  | { action: 'unskip_integration_input'; work_unit_id: string }
  | { action: 'apply_integration' }
  | { action: 'cancel' };

export async function runSupervisorAction(id: string, request: SupervisorActionRequest): Promise<Record<string, unknown>> {
  const response = await fetch(`/api/supervisor-runs/${id}/actions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(request)
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
