export type SupervisorExecutionStrategy = 'series' | 'parallel';

export type FeaturePlanItemStatus = 'rough' | 'fine' | 'scheduled' | 'completed' | 'refined' | 'approved';

export type FeaturePlanItem = {
  id: string;
  title: string;
  status: FeaturePlanItemStatus;
  summary: string;
  rough_summary?: string | null;
  refinement_workflow_run_id?: string | null;
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
  const scheduledIds = new Set((run.execution_plan_items ?? []).map((item) => item.feature_plan_item_id));
  const completedIds = completedFeatureIds(run);
  return {
    ...run,
    feature_plan_items: (run.feature_plan_items ?? []).map((item) => {
      const status = canonicalFeatureStatus(item.status);
      return {
        ...item,
        status: completedIds.has(item.id) ? 'completed' : scheduledIds.has(item.id) ? 'scheduled' : status,
        dependencies: []
      };
    })
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
    workflow_start_step_id?: string | null;
    integration_template_id?: string | null;
    planner_refinement_template_id?: string | null;
  }
): Promise<Record<string, unknown>> {
  return runSupervisorAction(id, 'update_plan', {
    planner_log_items: plannerLogItems.map(serializeFeatureForApi),
    sprint_items: sprintItems,
    ...sprintConfig
  });
}

export type RefineSupervisorFeatureResponse = {
  ok: boolean;
  workflow_run_id: string;
};

export async function refineSupervisorFeature(id: string, featureId: string, workflowTemplateId?: string | null): Promise<RefineSupervisorFeatureResponse> {
  return runSupervisorAction(id, 'refine_feature', {
    feature_id: featureId,
    workflow_template_id: workflowTemplateId ?? null
  }) as Promise<RefineSupervisorFeatureResponse>;
}

export async function runSupervisorAction(id: string, action: 'start' | 'tick' | 'apply' | 'cancel' | 'update_plan' | 'refine_feature', payload: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const response = await fetch(`/api/supervisor-runs/${id}/actions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ action, payload })
  });
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}
