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

export type PlannerWorkspace = {
  id: string;
  root_repo_path: string;
  title: string;
  is_default: boolean;
  feature_plan_items: FeaturePlanItem[];
  created_at: string;
  updated_at: string;
};

export type EnsurePlannerResponse = {
  created: boolean;
  planner: PlannerWorkspace;
};

export type SetDefaultPlannerResponse = {
  ok: boolean;
  planner: PlannerWorkspace;
};

async function fetchJson<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    headers: { 'Content-Type': 'application/json' },
    ...init,
  });
  if (!response.ok) throw new Error(await response.text());
  return response.json() as Promise<T>;
}

export function listPlannersForRepo(rootRepoPath: string): Promise<PlannerWorkspace[]> {
  const params = new URLSearchParams({ root_repo_path: rootRepoPath });
  return fetchJson<PlannerWorkspace[]>(`/api/planners?${params.toString()}`);
}

export function createPlannerForRepo(body: { root_repo_path: string; title?: string | null; make_default?: boolean; feature_plan_items?: FeaturePlanItem[] }): Promise<PlannerWorkspace> {
  return fetchJson<PlannerWorkspace>('/api/planners', {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

export function ensurePlannerForRepo(body: { root_repo_path: string; title?: string | null }): Promise<EnsurePlannerResponse> {
  return fetchJson<EnsurePlannerResponse>('/api/planners/ensure', {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

export function getPlanner(id: string): Promise<PlannerWorkspace> {
  return fetchJson<PlannerWorkspace>(`/api/planners/${id}`);
}

export function updatePlannerFeatures(id: string, featurePlanItems: FeaturePlanItem[]): Promise<PlannerWorkspace> {
  return fetchJson<PlannerWorkspace>(`/api/planners/${id}`, {
    method: 'PUT',
    body: JSON.stringify({ feature_plan_items: featurePlanItems }),
  });
}

export function setDefaultPlanner(id: string): Promise<SetDefaultPlannerResponse> {
  return fetchJson<SetDefaultPlannerResponse>(`/api/planners/${id}/default`, {
    method: 'POST',
    body: JSON.stringify({}),
  });
}
