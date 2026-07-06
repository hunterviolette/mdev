import { useEffect, useMemo, useRef, useState } from 'react';
import { Badge, Button, ComboboxItem, Group, Modal, ScrollArea, Select, Stack, Table, Text, TextInput, Textarea } from '@mantine/core';
import type { WorkflowTemplate } from './api';
import { createPlannerForRepo, deletePlannerForRepo, listPlannersForRepo, setDefaultPlanner, updatePlannerFeatures, type PlannerWorkspace } from './planner_api';
import {
  applyPlannerImport,
  previewPlannerImport,
  type FeaturePlanItem,
  type FeaturePlanItemStatus,
  type PlannerImportAction,
  type PlannerImportDecision,
  type PlannerImportPreviewResponse,
  type SupervisorRun,
} from './supervisor_api';

type PlannerSelection = {
  planner: { id: string; root_repo_path: string; title: string } | null;
  feature: FeaturePlanItem | null;
};

type PlannerMultiSelection = {
  planner: { id: string; root_repo_path: string; title: string } | null;
  featureIds: string[];
  features: FeaturePlanItem[];
};

type Props = {
  opened: boolean;
  rootRepoPath: string;
  run?: SupervisorRun | null;
  templates?: WorkflowTemplate[];
  plannerOptions?: PlannerWorkspace[];
  selectedFeatureId?: string | null;
  selectedFeatureIds?: string[];
  selectedPlannerId?: string | null;
  selectionMode?: boolean;
  createFeatureOnOpen?: boolean;
  onSelectFeature?: (selection: PlannerSelection) => void | Promise<void>;
  onSelectFeatures?: (selection: PlannerMultiSelection) => void | Promise<void>;
  onFeatureCreated?: (selection: PlannerSelection) => void | Promise<void>;
  onClose: () => void;
  onSaved?: () => Promise<void> | void;
  onWorkflowRunCreated?: (workflowRunId: string) => Promise<void> | void;
  onError?: (message: string) => void;
};

const FEATURE_STATUSES: FeaturePlanItemStatus[] = ['rough', 'fine', 'completed', 'applied'];
const IMPORT_ACTIONS: PlannerImportAction[] = ['create', 'create_copy', 'replace_existing', 'skip', 'reject'];

function normalizePlannerRoot(value: string): string {
  const normalized = value.trim().replace(/\\/g, '/');
  const supervisorShardMarker = '/.mdev/supervisors/';
  const supervisorIndex = normalized.indexOf(supervisorShardMarker);
  if (supervisorIndex > 0) return normalized.slice(0, supervisorIndex);
  return normalized.replace(/\/+$/g, '');
}

function plannerRootKey(value: string): string {
  return normalizePlannerRoot(value).toLowerCase();
}

function plannerRootMatches(candidate: string, root: string): boolean {
  const candidateKey = plannerRootKey(candidate);
  const rootKey = plannerRootKey(root);
  return !rootKey || candidateKey === rootKey || rootKey.startsWith(`${candidateKey}/`);
}

function repoPlannerTitle(rootRepoPath: string): string {
  const parts = normalizePlannerRoot(rootRepoPath).split('/').filter(Boolean);
  return `${parts[parts.length - 1] ?? 'Repo'} Planner`;
}

function statusBadgeColor(status: string): string {
  const normalized = status.toLowerCase();
  if (['fine', 'refined', 'approved', 'completed', 'applied', 'success'].includes(normalized)) return 'green';
  if (['scheduled', 'running', 'queued', 'waiting', 'paused', 'active'].includes(normalized)) return 'blue';
  if (['rough', 'created', 'not_started'].includes(normalized)) return 'yellow';
  if (['failed', 'error', 'cancelled', 'invalid', 'reject'].includes(normalized)) return 'red';
  if (['duplicate', 'conflict', 'skip'].includes(normalized)) return 'orange';
  return 'gray';
}

function titleCaseStatus(status: string): string {
  return status
    .split(/[_\s-]+/g)
    .filter(Boolean)
    .map((part) => `${part.slice(0, 1).toUpperCase()}${part.slice(1).toLowerCase()}`)
    .join(' ');
}

function featureTitle(item: FeaturePlanItem): string {
  return item.title?.trim() || item.summary?.trim() || item.id;
}

function importActionOptions() {
  return IMPORT_ACTIONS.map((action) => ({ value: action, label: titleCaseStatus(action) }));
}

function emptyStringList(value: string): string[] {
  return value.split(/\r?\n/).map((item) => item.trim()).filter(Boolean);
}

function stringListText(value: string[] | undefined): string {
  return (value ?? []).join('\n');
}

function ReadField(props: { label: string; value: string }) {
  return (
    <Stack gap={4}>
      <Text fw={600} size="sm">{props.label}</Text>
      <Text
        size="sm"
        style={{
          whiteSpace: 'pre-wrap',
          lineHeight: 1.55,
          border: '1px solid var(--mantine-color-dark-4)',
          borderRadius: 8,
          padding: 12,
          background: 'rgba(255,255,255,0.025)',
        }}
      >
        {props.value.trim() || '—'}
      </Text>
    </Stack>
  );
}

function defaultFeatureDraft(feature: FeaturePlanItem): FeaturePlanItem {
  return {
    ...feature,
    title: feature.title ?? '',
    status: feature.status === 'scheduled' ? 'fine' : feature.status ?? 'rough',
    summary: feature.summary ?? '',
    rough_summary: feature.rough_summary ?? '',
    requirements: feature.requirements ?? [],
    acceptance_criteria: feature.acceptance_criteria ?? [],
    implementation_notes: feature.implementation_notes ?? [],
    review_expectations: feature.review_expectations ?? [],
    target_files_or_areas: feature.target_files_or_areas ?? [],
    dependencies: feature.dependencies ?? [],
  };
}

function normalizePlannerImportPayload(payload: unknown): unknown {
  if (!payload || typeof payload !== 'object' || Array.isArray(payload)) return payload;
  const record = payload as Record<string, unknown>;
  if (Array.isArray(record.features)) return payload;
  if (Array.isArray(record.feature_plan_items)) {
    return {
      ...record,
      features: record.feature_plan_items,
    };
  }
  return payload;
}

function plannerImportFeatures(payload: unknown): FeaturePlanItem[] {
  const normalized = normalizePlannerImportPayload(payload);
  if (!normalized || typeof normalized !== 'object' || Array.isArray(normalized)) return [];
  const features = (normalized as Record<string, unknown>).features;
  if (!Array.isArray(features)) return [];
  return features.filter((item): item is FeaturePlanItem => {
    if (!item || typeof item !== 'object' || Array.isArray(item)) return false;
    const record = item as Record<string, unknown>;
    return typeof record.id === 'string' && typeof record.title === 'string';
  });
}

function mergePlannerFeatures(current: FeaturePlanItem[], incoming: FeaturePlanItem[]): FeaturePlanItem[] {
  const merged = [...current];
  for (const item of incoming) {
    const index = merged.findIndex((existing) => existing.id === item.id);
    if (index >= 0) {
      merged[index] = item;
    } else {
      merged.push(item);
    }
  }
  return merged;
}

function newFeatureId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) return `feature-${crypto.randomUUID()}`;
  return `feature-${Date.now().toString(36)}`;
}

function newFeatureDraft(index: number): FeaturePlanItem {
  return {
    id: newFeatureId(),
    title: `New feature ${index}`,
    status: 'rough',
    summary: '',
    rough_summary: '',
    requirements: [],
    acceptance_criteria: [],
    implementation_notes: [],
    review_expectations: [],
    target_files_or_areas: [],
    dependencies: [],
  };
}

function plannerFeatureCountLabel(count: number): string {
  return `${count} ${count === 1 ? 'feature' : 'features'}`;
}

function plannerOptionLabel(option: PlannerWorkspace, appliedPlannerId: string | null): string {
  const parts = [option.title, plannerFeatureCountLabel(option.feature_plan_items?.length ?? 0)];
  if (option.id === appliedPlannerId) parts.push('current');
  if (option.is_default) parts.push('default');
  return parts.join(' · ');
}

function mergePlannerWorkspaceRows(apiRows: PlannerWorkspace[], propRows: PlannerWorkspace[] | undefined): PlannerWorkspace[] {
  const byId = new Map<string, PlannerWorkspace>();
  for (const item of propRows ?? []) byId.set(item.id, item);
  for (const item of apiRows) byId.set(item.id, item);
  return Array.from(byId.values());
}

export function PlannerModal(props: Props) {
  const rootRepoPath = normalizePlannerRoot(props.rootRepoPath || props.run?.root_repo_path || '');
  const [run, setRun] = useState<SupervisorRun | null>(props.run ?? null);
  const [featureSearch, setFeatureSearch] = useState('');
  const [busy, setBusy] = useState(false);
  const [plannerOptionsOpen, setPlannerOptionsOpen] = useState(false);
  const [createPlannerOpen, setCreatePlannerOpen] = useState(false);
  const [createFeatureOpen, setCreateFeatureOpen] = useState(false);
  const [createFeatureTitle, setCreateFeatureTitle] = useState('');
  const [createFeatureSummary, setCreateFeatureSummary] = useState('');
  const [plannerSelectionId, setPlannerSelectionId] = useState<string | null>(props.selectedPlannerId ?? null);
  const [appliedPlannerId, setAppliedPlannerId] = useState<string | null>(props.selectedPlannerId ?? null);
  const [newPlannerTitle, setNewPlannerTitle] = useState(repoPlannerTitle(rootRepoPath));
  const [plannerWorkspaces, setPlannerWorkspaces] = useState<PlannerWorkspace[]>(props.plannerOptions ?? []);
  const [busyFeatureId, setBusyFeatureId] = useState<string | null>(null);
  const [stagedFeatureIds, setStagedFeatureIds] = useState<string[]>(props.selectedFeatureIds ?? []);
  const [viewFeature, setViewFeature] = useState<FeaturePlanItem | null>(null);
  const [featureDraft, setFeatureDraft] = useState<FeaturePlanItem | null>(null);
  const [featureEditMode, setFeatureEditMode] = useState(false);
  const [importPayload, setImportPayload] = useState<unknown>(null);
  const [importPreview, setImportPreview] = useState<PlannerImportPreviewResponse | null>(null);
  const [importDecisions, setImportDecisions] = useState<Record<number, PlannerImportDecision>>({});
  const [importReviewOpen, setImportReviewOpen] = useState(false);
  const importInputRef = useRef<HTMLInputElement | null>(null);

  const statusOptions = useMemo(() => FEATURE_STATUSES.map((status) => ({ value: status, label: titleCaseStatus(status) })), []);
  const importActions = useMemo(() => importActionOptions(), []);
  const plannerOptions = useMemo(() => {
    return plannerWorkspaces.filter((option) => plannerRootMatches(option.root_repo_path, rootRepoPath));
  }, [plannerWorkspaces, rootRepoPath]);
  const selectedPlannerWorkspace = useMemo(() => {
    return plannerOptions.find((option) => option.id === appliedPlannerId) ?? plannerOptions.find((option) => option.is_default) ?? plannerOptions[0] ?? null;
  }, [plannerOptions, appliedPlannerId]);
  const features = selectedPlannerWorkspace?.feature_plan_items ?? [];
  const plannerSelectOptions = useMemo(() => plannerOptions.map((option) => ({
    value: option.id,
    label: plannerOptionLabel(option, appliedPlannerId),
  })), [plannerOptions, appliedPlannerId]);

  const filteredFeatures = useMemo(() => {
    const source = props.selectionMode
      ? features.filter((item) => ['fine', 'scheduled'].includes(String(item.status ?? '')))
      : features;
    const needle = featureSearch.trim().toLowerCase();
    if (!needle) return source;
    return source.filter((item) => {
      const plannerStatus = String(item.status ?? '').toLowerCase();
      const displayStatus = plannerStatus === 'scheduled' ? 'fine' : plannerStatus;
      return featureTitle(item).toLowerCase().includes(needle)
        || (item.summary ?? '').toLowerCase().includes(needle)
        || displayStatus.includes(needle);
    });
  }, [features, featureSearch, props.selectionMode]);

  const importSummaryText = useMemo(() => {
    if (!importPreview) return '';
    const summary = importPreview.summary;
    return `${summary.total} features · ${summary.accepted} accepted · ${summary.duplicates} duplicates · ${summary.conflicts} conflicts · ${summary.invalid} invalid`;
  }, [importPreview]);

  useEffect(() => {
    setRun(props.run ?? null);
  }, [props.run?.id]);

  useEffect(() => {
    if (!props.opened) return;
    setStagedFeatureIds(props.selectedFeatureIds ?? []);
  }, [props.opened, props.selectedFeatureIds?.join('|')]);

  useEffect(() => {
    if (!props.plannerOptions || props.plannerOptions.length === 0) return;
    setPlannerWorkspaces((current) => {
      const byId = new Map(current.map((item) => [item.id, item]));
      for (const item of props.plannerOptions ?? []) byId.set(item.id, item);
      return Array.from(byId.values());
    });
  }, [props.plannerOptions]);

  useEffect(() => {
    if (!props.selectedPlannerId) return;
    setPlannerSelectionId(props.selectedPlannerId);
    setAppliedPlannerId(props.selectedPlannerId);
  }, [props.selectedPlannerId]);

  useEffect(() => {
    setNewPlannerTitle((current) => current.trim() || repoPlannerTitle(rootRepoPath));
  }, [rootRepoPath]);

  useEffect(() => {
    const fallbackId = plannerOptions.find((option) => option.is_default)?.id ?? plannerOptions[0]?.id ?? null;
    setAppliedPlannerId((current) => current && plannerOptions.some((option) => option.id === current) ? current : fallbackId);
    setPlannerSelectionId((current) => current && plannerOptions.some((option) => option.id === current) ? current : fallbackId);
  }, [plannerOptions]);

  useEffect(() => {
    if (!props.opened) return;
    let cancelled = false;

    if (props.selectionMode) {
      setCreateFeatureOpen(false);
      setCreateFeatureTitle('');
      setCreateFeatureSummary('');
    }

    if (props.createFeatureOnOpen) {
      setCreateFeatureTitle(`New feature ${features.length + 1}`);
      setCreateFeatureSummary('');
      setCreateFeatureOpen(true);
    }

    async function load() {
      if (!rootRepoPath) return;
      try {
        setBusy(true);
        const plannerRows = await listPlannersForRepo(rootRepoPath);
        if (cancelled) return;
        setPlannerWorkspaces(mergePlannerWorkspaceRows(plannerRows, props.plannerOptions));
        setRun(props.run ?? null);
      } catch (err) {
        if (!cancelled) props.onError?.(err instanceof Error ? err.message : String(err));
      } finally {
        if (!cancelled) setBusy(false);
      }
    }

    void load();
    return () => {
      cancelled = true;
    };
  }, [props.opened, rootRepoPath]);

  async function reload() {
    if (rootRepoPath) {
      const plannerRows = await listPlannersForRepo(rootRepoPath);
      setPlannerWorkspaces(mergePlannerWorkspaceRows(plannerRows, props.plannerOptions));
    }
    setRun(props.run ?? run);
    await props.onSaved?.();
  }

  async function applyPlannerSelection() {
    if (!plannerSelectionId) return;
    const selected = plannerOptions.find((option) => option.id === plannerSelectionId);
    if (!selected) return;

    try {
      setBusy(true);
      await setDefaultPlanner(selected.id);
      const plannerRows = rootRepoPath ? await listPlannersForRepo(rootRepoPath) : [];
      setPlannerWorkspaces(mergePlannerWorkspaceRows(plannerRows, props.plannerOptions));
      setAppliedPlannerId(selected.id);
      setPlannerSelectionId(selected.id);
      setPlannerOptionsOpen(false);
      await props.onSaved?.();
    } catch (err) {
      props.onError?.(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function deleteSelectedPlanner() {
    if (!plannerSelectionId) return;
    const selected = plannerOptions.find((option) => option.id === plannerSelectionId);
    if (!selected) return;
    const confirmed = window.confirm(`Delete planner "${selected.title}"? This removes the planner feature log and cannot be undone.`);
    if (!confirmed) return;

    try {
      setBusy(true);
      await deletePlannerForRepo(selected.id);
      const plannerRows = rootRepoPath ? await listPlannersForRepo(rootRepoPath) : [];
      setPlannerWorkspaces(() => {
        const byId = new Map(plannerRows.map((item) => [item.id, item]));
        for (const item of props.plannerOptions ?? []) {
          if (item.id !== selected.id) byId.set(item.id, item);
        }
        return Array.from(byId.values());
      });
      const fallback = plannerRows.find((item) => item.is_default)?.id ?? plannerRows[0]?.id ?? null;
      setAppliedPlannerId((current) => current === selected.id ? fallback : current);
      setPlannerSelectionId(fallback);
      await props.onSaved?.();
    } catch (err) {
      props.onError?.(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function requestCreatePlanner() {
    if (!rootRepoPath) {
      props.onError?.('Repo root is required before creating a planner.');
      return;
    }

    try {
      setBusy(true);
      const planner = await createPlannerForRepo({
        root_repo_path: rootRepoPath,
        title: newPlannerTitle.trim() || repoPlannerTitle(rootRepoPath),
        make_default: plannerOptions.length === 0,
        feature_plan_items: [],
      });
      await setDefaultPlanner(planner.id);
      const plannerRows = await listPlannersForRepo(rootRepoPath);
      setPlannerWorkspaces(mergePlannerWorkspaceRows(plannerRows.some((item) => item.id === planner.id) ? plannerRows : [planner, ...plannerRows], props.plannerOptions));
      setAppliedPlannerId(planner.id);
      setPlannerSelectionId(planner.id);
      setPlannerOptionsOpen(false);
      setCreatePlannerOpen(false);
      await props.onSaved?.();
    } catch (err) {
      props.onError?.(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  function downloadPlanner() {
    if (!selectedPlannerWorkspace) return;
    const payload = {
      version: 1,
      kind: 'planner_features',
      root_repo_path: selectedPlannerWorkspace.root_repo_path,
      planner: {
        id: selectedPlannerWorkspace.id,
        title: selectedPlannerWorkspace.title,
        root_repo_path: selectedPlannerWorkspace.root_repo_path,
      },
      features: selectedPlannerWorkspace.feature_plan_items ?? [],
      feature_plan_items: selectedPlannerWorkspace.feature_plan_items ?? [],
    };
    const blob = new Blob([JSON.stringify(payload, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = `${repoPlannerTitle(selectedPlannerWorkspace.root_repo_path).replace(/[^a-z0-9_-]+/gi, '-').replace(/^-+|-+$/g, '').toLowerCase() || 'planner'}-features.json`;
    document.body.appendChild(link);
    link.click();
    link.remove();
    URL.revokeObjectURL(url);
  }

  async function importPlannerFile(file: File | null | undefined) {
    if (!file) return;
    if (!selectedPlannerWorkspace) {
      props.onError?.('Planner feature log must be selected before importing features.');
      return;
    }

    try {
      setBusy(true);
      const payload = JSON.parse(await file.text()) as unknown;
      const importedFeatures = plannerImportFeatures(payload);
      if (importedFeatures.length === 0) {
        props.onError?.('Planner import did not contain any valid features.');
        return;
      }
      const planner = await updatePlannerFeatures(selectedPlannerWorkspace.id, mergePlannerFeatures(features, importedFeatures));
      setPlannerWorkspaces((current) => current.map((item) => item.id === planner.id ? planner : item));
      setPlannerSelectionId(planner.id);
      await props.onSaved?.();
    } catch (err) {
      props.onError?.(`Planner import failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(false);
      if (importInputRef.current) importInputRef.current.value = '';
    }
  }

  async function applyImportedPlanner() {
    if (!run || !importPayload || !importPreview) return;
    try {
      setBusy(true);
      const decisions = importPreview.items.map((item) => importDecisions[item.import_index]).filter(Boolean);
      const response = await applyPlannerImport(run.id, importPayload, decisions);
      setRun(response.supervisor_run);
      setImportPayload(null);
      setImportPreview(null);
      setImportDecisions({});
      setImportReviewOpen(false);
      await props.onSaved?.();
    } catch (err) {
      props.onError?.(`Planner import failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(false);
    }
  }

  function updateImportDecision(importIndex: number, action: PlannerImportAction) {
    setImportDecisions((current) => ({
      ...current,
      [importIndex]: {
        ...current[importIndex],
        import_index: importIndex,
        action,
      },
    }));
  }

  async function selectFeature(feature: FeaturePlanItem | null) {
    if (props.onSelectFeatures) {
      if (!feature?.id) {
        setStagedFeatureIds([]);
        return;
      }
      setStagedFeatureIds((current) => current.includes(feature.id) ? current.filter((id) => id !== feature.id) : Array.from(new Set([...current, feature.id])));
      return;
    }

    if (!props.onSelectFeature) return;
    await props.onSelectFeature({
      planner: selectedPlannerWorkspace ? { id: selectedPlannerWorkspace.id, root_repo_path: selectedPlannerWorkspace.root_repo_path, title: selectedPlannerWorkspace.title } : null,
      feature,
    });
    props.onClose();
  }

  function normalizedFeatureIds(ids: string[]): string[] {
    return Array.from(new Set(ids.filter(Boolean))).sort();
  }

  function stagedFeatureSelectionChanged(): boolean {
    const initialIds = normalizedFeatureIds(props.selectedFeatureIds ?? []);
    const selectedIds = normalizedFeatureIds(stagedFeatureIds);
    if (initialIds.length !== selectedIds.length) return true;
    return initialIds.some((id, index) => id !== selectedIds[index]);
  }

  async function commitStagedFeatureSelection() {
    if (!props.onSelectFeatures) return;
    const selectedIds = normalizedFeatureIds(stagedFeatureIds);
    const selectedSet = new Set(selectedIds);
    await props.onSelectFeatures({
      planner: selectedPlannerWorkspace ? { id: selectedPlannerWorkspace.id, root_repo_path: selectedPlannerWorkspace.root_repo_path, title: selectedPlannerWorkspace.title } : null,
      featureIds: selectedIds,
      features: features.filter((item) => selectedSet.has(item.id)),
    });
    setStagedFeatureIds([]);
  }

  async function selectFeatures() {
    await commitStagedFeatureSelection();
    props.onClose();
  }

  async function closePlannerModal() {
    if (props.selectionMode && props.onSelectFeatures && stagedFeatureSelectionChanged()) {
      await commitStagedFeatureSelection();
    }
    props.onClose();
  }

  async function persistFeatures(nextFeatures: FeaturePlanItem[]) {
    if (!selectedPlannerWorkspace) return;
    const planner = await updatePlannerFeatures(selectedPlannerWorkspace.id, nextFeatures);
    setPlannerWorkspaces((current) => current.map((item) => item.id === planner.id ? planner : item));
    await props.onSaved?.();
  }

  function openCreateFeature() {
    setCreateFeatureTitle(`New feature ${features.length + 1}`);
    setCreateFeatureSummary('');
    setCreateFeatureOpen(true);
  }

  async function saveCreatedFeature() {
    if (!selectedPlannerWorkspace) return;
    const title = createFeatureTitle.trim();
    if (!title) {
      props.onError?.('Feature title is required.');
      return;
    }

    try {
      const draft = {
        ...newFeatureDraft(features.length + 1),
        title,
        summary: createFeatureSummary.trim(),
        rough_summary: createFeatureSummary.trim(),
      };
      setBusyFeatureId(draft.id);
      const planner = await updatePlannerFeatures(selectedPlannerWorkspace.id, [draft, ...features]);
      setPlannerWorkspaces((current) => current.map((item) => item.id === planner.id ? planner : item));
      setCreateFeatureOpen(false);
      await props.onFeatureCreated?.({
        planner: { id: selectedPlannerWorkspace.id, root_repo_path: selectedPlannerWorkspace.root_repo_path, title: selectedPlannerWorkspace.title },
        feature: draft,
      });
      await props.onSaved?.();
    } catch (err) {
      props.onError?.(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyFeatureId(null);
    }
  }

  async function saveFeatureDraft() {
    if (!selectedPlannerWorkspace || !featureDraft) return;
    try {
      setBusyFeatureId(featureDraft.id);
      const nextFeatures = features.map((item) => item.id === featureDraft.id ? featureDraft : item);
      await persistFeatures(nextFeatures);
      setViewFeature(featureDraft);
      setFeatureDraft(defaultFeatureDraft(featureDraft));
      setFeatureEditMode(false);
    } catch (err) {
      props.onError?.(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyFeatureId(null);
    }
  }

  async function setFeatureStatus(feature: FeaturePlanItem, status: FeaturePlanItemStatus) {
    if (!selectedPlannerWorkspace) return;
    try {
      setBusyFeatureId(feature.id);
      const nextFeature = { ...feature, status };
      const nextFeatures = features.map((item) => item.id === feature.id ? nextFeature : item);
      await persistFeatures(nextFeatures);
      if (viewFeature?.id === feature.id) {
        setViewFeature(nextFeature);
        setFeatureDraft(defaultFeatureDraft(nextFeature));
      }
    } catch (err) {
      props.onError?.(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyFeatureId(null);
    }
  }

  async function deleteFeature(feature: FeaturePlanItem) {
    if (!selectedPlannerWorkspace) return;
    const confirmed = window.confirm(`Delete planner feature "${featureTitle(feature)}"? This removes it from the planner feature log.`);
    if (!confirmed) return;
    try {
      setBusyFeatureId(feature.id);
      const nextFeatures = features.filter((item) => item.id !== feature.id);
      await persistFeatures(nextFeatures);
      if (viewFeature?.id === feature.id) {
        setViewFeature(null);
        setFeatureDraft(null);
        setFeatureEditMode(false);
      }
      await reload();
    } catch (err) {
      props.onError?.(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyFeatureId(null);
    }
  }

  function openFeature(feature: FeaturePlanItem) {
    setViewFeature(feature);
    setFeatureDraft(defaultFeatureDraft(feature));
    setFeatureEditMode(false);
  }

  function beginEditFeature() {
    if (!viewFeature) return;
    setFeatureDraft(defaultFeatureDraft(viewFeature));
    setFeatureEditMode(true);
  }

  function renderPlannerOption(item: ComboboxItem) {
    const option = plannerOptions.find((planner) => planner.id === item.value);
    if (!option) return item.label;
    const isApplied = option.id === appliedPlannerId;
    const isPending = option.id === plannerSelectionId && option.id !== appliedPlannerId;

    return (
      <Group
        justify="space-between"
        wrap="nowrap"
        style={{
          borderRadius: 6,
          padding: '4px 6px',
          background: isApplied ? 'var(--mantine-color-blue-light)' : undefined,
        }}
      >
        <Stack gap={1} style={{ minWidth: 0 }}>
          <Group gap="xs" wrap="nowrap">
            <Text size="sm" fw={isApplied ? 700 : 500} truncate>{option.title}</Text>
            {isApplied ? <Badge size="xs" color="blue" variant="filled">Current</Badge> : null}
            {isPending ? <Badge size="xs" color="yellow" variant="light">Pending</Badge> : null}
            {option.is_default ? <Badge size="xs" color="gray" variant="light">Default</Badge> : null}
          </Group>
          <Text size="xs" c="dimmed">{option.id.slice(0, 8)} · {plannerFeatureCountLabel(option.feature_plan_items?.length ?? 0)}</Text>
        </Stack>
      </Group>
    );
  }

  return (
    <Modal opened={props.opened} onClose={() => void closePlannerModal()} title="Planner" size="calc(100vw - 96px)" centered zIndex={300}>
      <Stack gap="md">
        <Group align="end" wrap="wrap">
          <Select
            label="Planner"
            placeholder="Planner"
            value={selectedPlannerWorkspace?.id ?? null}
            data={selectedPlannerWorkspace ? [{ value: selectedPlannerWorkspace.id, label: selectedPlannerWorkspace.title }] : []}
            disabled
            style={{ minWidth: 280 }}
          />
          <Button
            size="xs"
            variant="light"
            onClick={() => {
              setPlannerSelectionId(selectedPlannerWorkspace?.id ?? null);
              setPlannerOptionsOpen(true);
            }}
          >
            Planner options
          </Button>
          <Button size="xs" variant="light" onClick={() => void reload()} loading={busy} disabled={!rootRepoPath}>Refresh planner</Button>
          <Button size="xs" variant="light" onClick={downloadPlanner} disabled={!selectedPlannerWorkspace}>Download</Button>
          <Button size="xs" variant="light" onClick={() => importInputRef.current?.click()} loading={busy} disabled={!selectedPlannerWorkspace}>Upload</Button>
          <Button size="xs" variant="light" onClick={openCreateFeature} loading={busyFeatureId?.startsWith('feature-')} disabled={!selectedPlannerWorkspace}>Create feature</Button>
          <input
            ref={importInputRef}
            type="file"
            accept="application/json,.json"
            style={{ display: 'none' }}
            onChange={(event) => void importPlannerFile(event.currentTarget.files?.[0])}
          />

        </Group>

        {selectedPlannerWorkspace ? (
          <Group gap="xs">
            <Badge variant="light">{selectedPlannerWorkspace.title}</Badge>
            <Badge variant="light" color="green">Planner</Badge>
            {selectedPlannerWorkspace.is_default ? <Badge variant="light" color="blue">Default</Badge> : null}
            <Text size="xs" c="dimmed">{selectedPlannerWorkspace.root_repo_path}</Text>
          </Group>
        ) : (
          <Text c="dimmed" size="sm">No planner feature log selected.</Text>
        )}

        <TextInput
          label="Search planner features"
          placeholder="Search by feature name, summary, or planner status"
          value={featureSearch}
          onChange={(event) => setFeatureSearch(event.currentTarget.value)}
        />

        {features.length === 0 ? (
          <Text c="dimmed" size="sm">No planner features available.</Text>
        ) : filteredFeatures.length === 0 ? (
          <Text c="dimmed" size="sm">No planner features match the current search.</Text>
        ) : (
          <ScrollArea h="calc(100vh - 390px)" type="auto">
            <Table striped highlightOnHover withTableBorder>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Feature</Table.Th>
                  <Table.Th style={{ width: 140 }}>Planner status</Table.Th>
                  <Table.Th style={{ width: 560 }}>Actions</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {filteredFeatures.map((item) => {
                  const planStatus = String(item.status ?? 'rough');
                  const displayStatus = props.selectionMode && planStatus === 'scheduled' ? 'fine' : planStatus;
                  const isReady = planStatus === 'fine' || planStatus === 'scheduled';
                  const isSelected = props.selectedFeatureId === item.id || stagedFeatureIds.includes(item.id);
                  const busyForFeature = busyFeatureId === item.id;

                  return (
                    <Table.Tr key={item.id}>
                      <Table.Td>
                        <Stack gap={2}>
                          <Group gap="xs" wrap="nowrap">
                            <Text fw={600} size="sm">{featureTitle(item)}</Text>
                            {isSelected ? <Badge size="xs" color="green" variant="light">Selected</Badge> : null}
                          </Group>
                          {item.summary ? <Text size="xs" c="dimmed" lineClamp={2}>{item.summary}</Text> : null}
                        </Stack>
                      </Table.Td>
                      <Table.Td style={{ minWidth: 140 }}>
                        <Badge variant="light" color={statusBadgeColor(displayStatus)} tt="none" style={{ maxWidth: 'none', overflow: 'visible' }}>
                          {titleCaseStatus(displayStatus)}
                        </Badge>
                      </Table.Td>
                      <Table.Td>
                        <Group gap="xs" wrap="nowrap">
                          {props.selectionMode ? (
                            <>
                              <Button size="xs" variant="light" onClick={() => openFeature(item)}>Open</Button>
                              {isReady ? (
                                <Button size="xs" color={isSelected ? 'orange' : 'green'} variant={isSelected ? 'light' : 'filled'} onClick={() => void selectFeature(item)} disabled={!item.id}>{isSelected ? 'Remove selection' : 'Select for pool'}</Button>
                              ) : (
                                <Button size="xs" variant="light" onClick={() => void setFeatureStatus(item, 'fine')} loading={busyForFeature} disabled={!selectedPlannerWorkspace}>Mark ready</Button>
                              )}
                            </>
                          ) : (
                            <>
                              <Button size="xs" variant="light" onClick={() => openFeature(item)}>Open</Button>
                              {isReady ? (
                                <Badge size="sm" variant="light" color="green">Available to supervisor</Badge>
                              ) : (
                                <Button size="xs" variant="light" onClick={() => void setFeatureStatus(item, 'fine')} loading={busyForFeature} disabled={!selectedPlannerWorkspace}>Mark ready</Button>
                              )}
                              <Button size="xs" variant="light" color="red" onClick={() => void deleteFeature(item)} loading={busyForFeature} disabled={!selectedPlannerWorkspace}>Delete</Button>
                            </>
                          )}
                        </Group>
                      </Table.Td>
                    </Table.Tr>
                  );
                })}
              </Table.Tbody>
            </Table>
          </ScrollArea>
        )}

        <Group justify="space-between">
          {props.selectionMode && props.onSelectFeatures ? (
            <Group gap="xs">
              <Badge variant="light">{stagedFeatureIds.length} selected</Badge>
              <Button size="xs" color="green" onClick={() => void selectFeatures()} disabled={stagedFeatureIds.length === 0}>Queue selected features</Button>
            </Group>
          ) : <span />}
          <Button size="xs" variant="default" onClick={() => void closePlannerModal()}>Close</Button>
        </Group>
      </Stack>

      <Modal
        opened={createFeatureOpen}
        onClose={() => setCreateFeatureOpen(false)}
        title="New feature"
        centered
        size="lg"
        zIndex={315}
      >
        <Stack gap="sm">
          <TextInput
            label="Title"
            placeholder="Feature title"
            value={createFeatureTitle}
            onChange={(event) => setCreateFeatureTitle(event.currentTarget.value)}
            data-autofocus
          />
          <Textarea
            label="Summary"
            placeholder="Brief summary"
            value={createFeatureSummary}
            onChange={(event) => setCreateFeatureSummary(event.currentTarget.value)}
            autosize
            minRows={4}
          />
          <Group justify="flex-end" gap="xs" wrap="nowrap">
            <Button size="xs" variant="default" onClick={() => setCreateFeatureOpen(false)}>Cancel</Button>
            <Button size="xs" onClick={() => void saveCreatedFeature()} loading={Boolean(busyFeatureId?.startsWith('feature-'))} disabled={!selectedPlannerWorkspace || !createFeatureTitle.trim()}>Save</Button>
          </Group>
        </Stack>
      </Modal>

      <Modal opened={viewFeature !== null} onClose={() => { setViewFeature(null); setFeatureDraft(null); setFeatureEditMode(false); }} title="Planner feature" size="calc(100vw - 96px)" centered zIndex={310}>
        {viewFeature && featureDraft ? (
          <Stack gap="md">
            <Group justify="space-between" align="flex-start">
              <Stack gap={4} style={{ minWidth: 0, flex: 1 }}>
                <Text fw={700} size="lg">{featureTitle(featureDraft)}</Text>
                <Badge variant="light" color={statusBadgeColor(String(featureDraft.status ?? 'rough'))}>
                  {titleCaseStatus(String(featureDraft.status ?? 'rough'))}
                </Badge>
              </Stack>
              <Group gap="xs">
                {props.selectionMode ? <Button size="xs" onClick={() => void selectFeature(featureDraft)} disabled={props.selectedFeatureId === featureDraft.id}>{stagedFeatureIds.includes(featureDraft.id) ? 'Remove selection' : 'Select for pool'}</Button> : null}
                {featureEditMode ? <Button size="xs" onClick={() => void saveFeatureDraft()} loading={busyFeatureId === featureDraft.id}>Save</Button> : null}
                <Button
                  size="xs"
                  variant={featureEditMode ? 'default' : 'filled'}
                  onClick={() => {
                    if (featureEditMode) {
                      setFeatureDraft(defaultFeatureDraft(viewFeature));
                      setFeatureEditMode(false);
                    } else {
                      beginEditFeature();
                    }
                  }}
                >
                  {featureEditMode ? 'Read' : 'Edit'}
                </Button>
                <Button size="xs" variant="light" color="red" onClick={() => void deleteFeature(featureDraft)} disabled={!selectedPlannerWorkspace} loading={busyFeatureId === featureDraft.id}>Delete</Button>
              </Group>
            </Group>

            {featureEditMode ? (
              <>
                <TextInput label="Title" value={featureDraft.title} onChange={(event) => setFeatureDraft({ ...featureDraft, title: event.currentTarget.value })} />
                <Select label="Status" value={String(featureDraft.status ?? 'rough')} data={statusOptions} onChange={(value) => setFeatureDraft({ ...featureDraft, status: (value as FeaturePlanItemStatus) ?? 'rough' })} allowDeselect={false} comboboxProps={{ withinPortal: true, zIndex: 600 }} />
                <Textarea label="Rough summary" value={featureDraft.rough_summary ?? ''} onChange={(event) => setFeatureDraft({ ...featureDraft, rough_summary: event.currentTarget.value })} autosize minRows={3} />
                <Textarea label="Summary" value={featureDraft.summary} onChange={(event) => setFeatureDraft({ ...featureDraft, summary: event.currentTarget.value })} autosize minRows={3} />
                <Textarea label="Requirements" value={stringListText(featureDraft.requirements)} onChange={(event) => setFeatureDraft({ ...featureDraft, requirements: emptyStringList(event.currentTarget.value) })} autosize minRows={3} />
                <Textarea label="Acceptance criteria" value={stringListText(featureDraft.acceptance_criteria)} onChange={(event) => setFeatureDraft({ ...featureDraft, acceptance_criteria: emptyStringList(event.currentTarget.value) })} autosize minRows={3} />
                <Textarea label="Implementation notes" value={stringListText(featureDraft.implementation_notes)} onChange={(event) => setFeatureDraft({ ...featureDraft, implementation_notes: emptyStringList(event.currentTarget.value) })} autosize minRows={3} />
                <Textarea label="Review expectations" value={stringListText(featureDraft.review_expectations)} onChange={(event) => setFeatureDraft({ ...featureDraft, review_expectations: emptyStringList(event.currentTarget.value) })} autosize minRows={3} />
                <Textarea label="Target files or areas" value={stringListText(featureDraft.target_files_or_areas)} onChange={(event) => setFeatureDraft({ ...featureDraft, target_files_or_areas: emptyStringList(event.currentTarget.value) })} autosize minRows={3} />
              </>
            ) : (
              <>
                <ReadField label="Title" value={featureDraft.title} />
                <ReadField label="Status" value={titleCaseStatus(String(featureDraft.status ?? 'rough'))} />
                <ReadField label="Rough summary" value={featureDraft.rough_summary ?? ''} />
                <ReadField label="Summary" value={featureDraft.summary} />
                <ReadField label="Requirements" value={stringListText(featureDraft.requirements)} />
                <ReadField label="Acceptance criteria" value={stringListText(featureDraft.acceptance_criteria)} />
                <ReadField label="Implementation notes" value={stringListText(featureDraft.implementation_notes)} />
                <ReadField label="Review expectations" value={stringListText(featureDraft.review_expectations)} />
                <ReadField label="Target files or areas" value={stringListText(featureDraft.target_files_or_areas)} />
              </>
            )}
          </Stack>
        ) : null}
      </Modal>

      <Modal
        opened={plannerOptionsOpen}
        onClose={() => setPlannerOptionsOpen(false)}
        title="Planner options"
        centered
        size="lg"
        zIndex={330}
      >
        <Stack gap="sm">
          <Text size="sm" c="dimmed">
            Planner mapping selects the planner feature log for this repo. Supervisors orchestrate workflows separately.
          </Text>
          <Select
            label="Mapped planner feature log"
            placeholder="Select planner feature log"
            value={plannerSelectionId}
            data={plannerSelectOptions}
            onChange={setPlannerSelectionId}
            searchable
            comboboxProps={{ withinPortal: true, zIndex: 500 }}
            renderOption={({ option }) => renderPlannerOption(option)}
          />
          <Group justify="space-between" align="center" wrap="nowrap">
            <Button size="xs" variant="light" color="red" onClick={() => void deleteSelectedPlanner()} loading={busy} disabled={!plannerSelectionId}>
              Delete planner
            </Button>
            <Group gap="xs" wrap="nowrap">
              <Button size="xs" variant="light" onClick={() => {
                setNewPlannerTitle(repoPlannerTitle(rootRepoPath));
                setCreatePlannerOpen(true);
              }} disabled={!rootRepoPath}>
                Create planner
              </Button>
              <Button size="xs" onClick={() => void applyPlannerSelection()} loading={busy} disabled={!plannerSelectionId || plannerSelectionId === appliedPlannerId}>
                Apply
              </Button>
            </Group>
          </Group>
        </Stack>
      </Modal>

      <Modal
        opened={createPlannerOpen}
        onClose={() => setCreatePlannerOpen(false)}
        title="Create planner"
        centered
        size="md"
        zIndex={340}
      >
        <Stack gap="sm">
          <TextInput
            label="Planner name"
            placeholder={repoPlannerTitle(rootRepoPath)}
            value={newPlannerTitle}
            onChange={(event) => setNewPlannerTitle(event.currentTarget.value)}
            data-autofocus
          />
          <Group justify="flex-end" gap="xs" wrap="nowrap">
            <Button size="xs" variant="default" onClick={() => setCreatePlannerOpen(false)}>Cancel</Button>
            <Button size="xs" onClick={() => void requestCreatePlanner()} loading={busy} disabled={!rootRepoPath || !newPlannerTitle.trim()}>Create planner</Button>
          </Group>
        </Stack>
      </Modal>

      <Modal
        opened={importReviewOpen}
        onClose={() => setImportReviewOpen(false)}
        title="Review planner import"
        size="calc(100vw - 96px)"
        centered
        zIndex={320}
      >
        <Stack gap="md">
          {importPreview ? (
            <>
              <Group justify="space-between" align="center">
                <Stack gap={2}>
                  <Text fw={700}>Feature import verification</Text>
                  <Text size="sm" c="dimmed">{importSummaryText}</Text>
                </Stack>
                <Group gap="xs">
                  <Button size="xs" variant="default" onClick={() => setImportReviewOpen(false)}>Cancel</Button>
                  <Button size="xs" onClick={() => void applyImportedPlanner()} loading={busy}>Apply import</Button>
                </Group>
              </Group>

              <ScrollArea h="calc(100vh - 340px)" type="auto">
                <Table striped highlightOnHover withTableBorder>
                  <Table.Thead>
                    <Table.Tr>
                      <Table.Th>Feature</Table.Th>
                      <Table.Th style={{ width: 130 }}>Verification</Table.Th>
                      <Table.Th>Reason</Table.Th>
                      <Table.Th style={{ width: 180 }}>Existing</Table.Th>
                      <Table.Th style={{ width: 190 }}>Action</Table.Th>
                    </Table.Tr>
                  </Table.Thead>
                  <Table.Tbody>
                    {importPreview.items.map((item) => {
                      const decision = importDecisions[item.import_index];
                      const feature = item.feature;
                      return (
                        <Table.Tr key={item.import_index}>
                          <Table.Td>
                            <Stack gap={2}>
                              <Text fw={600} size="sm">{feature ? featureTitle(feature) : `Import item ${item.import_index + 1}`}</Text>
                              {feature?.summary ? <Text size="xs" c="dimmed" lineClamp={2}>{feature.summary}</Text> : null}
                            </Stack>
                          </Table.Td>
                          <Table.Td>
                            <Badge variant="light" color={statusBadgeColor(item.status)} tt="none">
                              {titleCaseStatus(item.status)}
                            </Badge>
                          </Table.Td>
                          <Table.Td><Text size="sm">{item.reason || '—'}</Text></Table.Td>
                          <Table.Td>
                            <Text size="sm" c={item.existing_title ? undefined : 'dimmed'}>
                              {item.existing_title ?? item.existing_feature_id ?? '—'}
                            </Text>
                          </Table.Td>
                          <Table.Td>
                            <Select
                              value={decision?.action ?? item.default_action}
                              data={importActions}
                              onChange={(value) => updateImportDecision(item.import_index, (value as PlannerImportAction) ?? item.default_action)}
                              allowDeselect={false}
                              size="xs"
                            />
                          </Table.Td>
                        </Table.Tr>
                      );
                    })}
                  </Table.Tbody>
                </Table>
              </ScrollArea>
            </>
          ) : (
            <Text c="dimmed" size="sm">No import preview loaded.</Text>
          )}
        </Stack>
      </Modal>
    </Modal>
  );
}
