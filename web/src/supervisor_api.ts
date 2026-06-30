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

export type EnsureSupervisorPlannerRequest = {
  root_repo_path: string;
  title?: string | null;
};

export type EnsureSupervisorPlannerResponse = {
  created: boolean;
  supervisor_run: SupervisorRun;
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
  const scheduledIds = new Set(executionPlanItems.map((item) => item.feature_plan_item_id));
  const completedIds = completedFeatureIds(run);
  return {
    ...run,
    feature_plan_items: featurePlanItems.map((item) => {
      const status = canonicalFeatureStatus(item.status);
      return {
        ...item,
        status: completedIds.has(item.id) ? 'completed' : scheduledIds.has(item.id) ? 'scheduled' : status,
        dependencies: []
      };
    }),
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

export async function ensureSupervisorPlannerRun(request: EnsureSupervisorPlannerRequest): Promise<EnsureSupervisorPlannerResponse> {
  const response = await fetch('/api/supervisor-runs/ensure-planner', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(request)
  });
  if (!response.ok) throw new Error(await response.text());
  const payload = await response.json();
  return {
    created: Boolean(payload.created),
    supervisor_run: normalizeSupervisorRun(payload.supervisor_run)
  };
}

export async function getSupervisorRun(id: string): Promise<SupervisorRun> {
  const response = await fetch(`/api/supervisor-runs/${id}`);
  if (!response.ok) throw new Error(await response.text());
  return normalizeSupervisorRun(await response.json());
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

export async function unscheduleSupervisorFeature(id: string, featureId: string, mode: UnscheduleSupervisorFeatureMode): Promise<UnscheduleSupervisorFeatureResponse> {
  const response = await runSupervisorAction(id, 'unschedule_feature', {
    feature_id: featureId,
    mode
  });
  return {
    ...response,
    supervisor_run: normalizeSupervisorRun(response.supervisor_run as SupervisorRun)
  } as UnscheduleSupervisorFeatureResponse;
}

export async function refineSupervisorFeature(id: string, featureId: string, workflowTemplateId?: string | null): Promise<RefineSupervisorFeatureResponse> {
  return runSupervisorAction(id, 'refine_feature', {
    feature_id: featureId,
    workflow_template_id: workflowTemplateId ?? null
  }) as Promise<RefineSupervisorFeatureResponse>;
}

export async function runSupervisorAction(id: string, action: 'start' | 'tick' | 'apply' | 'cancel' | 'start_integration' | 'restart_integration' | 'restart_sprint' | 'reopen_development' | 'new_sprint' | 'update_plan' | 'unschedule_feature' | 'preview_planner_import' | 'apply_planner_import' | 'refine_feature' | 'start_child_workflow' | 'pause_child_workflow' | 'remove_child_workflow', payload: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
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
