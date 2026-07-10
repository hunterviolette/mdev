export type FeaturePlanItemStatus = 'rough' | 'fine' | 'scheduled' | 'applied' | 'completed' | 'refined' | 'approved';

export type FeaturePlanItem = {
  id: string;
  title: string;
  status: FeaturePlanItemStatus;
  summary: string;
  rough_summary?: string | null;
  refinement_workflow_run_id?: string | null;
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
  feature_count: number;
  features: FeaturePlanItem[];
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

export type RefinePlannerFeatureResponse = {
  ok: boolean;
  workflow_run_id: string;
  reused?: boolean;
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

export function createPlannerForRepo(body: { root_repo_path: string; title?: string | null; make_default?: boolean; features?: FeaturePlanItem[] }): Promise<PlannerWorkspace> {
  return fetchJson<PlannerWorkspace>('/api/planners/create', {
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
  return fetchJson<PlannerWorkspace>(`/api/planners/${encodeURIComponent(id)}`);
}

export function getPlannerFeature(featureId: string): Promise<FeaturePlanItem> {
  return fetchJson<FeaturePlanItem>(`/api/planner-features/${encodeURIComponent(featureId)}`);
}

export function updatePlannerFeatures(id: string, features: FeaturePlanItem[]): Promise<PlannerWorkspace> {
  return fetchJson<PlannerWorkspace>(`/api/planners/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: JSON.stringify({ features }),
  });
}

export function deletePlannerForRepo(id: string): Promise<{ ok: boolean }> {
  return fetchJson<{ ok: boolean }>(`/api/planners/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
}

export function setDefaultPlanner(id: string): Promise<SetDefaultPlannerResponse> {
  return fetchJson<SetDefaultPlannerResponse>(`/api/planners/${encodeURIComponent(id)}/default`, {
    method: 'POST',
    body: JSON.stringify({}),
  });
}

export function refinePlannerFeature(plannerId: string, featureId: string, body: { supervisor_id?: string | null; workflow_template_id?: string | null }): Promise<RefinePlannerFeatureResponse> {
  return fetchJson<RefinePlannerFeatureResponse>(`/api/planners/${encodeURIComponent(plannerId)}/features/${encodeURIComponent(featureId)}/refine`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}
