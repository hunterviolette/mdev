import { useEffect, useMemo, useRef, useState } from 'react';
import { Badge, Button, Group, Modal, ScrollArea, Select, Stack, Table, Text, TextInput, Textarea } from '@mantine/core';
import { listTemplates, type WorkflowTemplate } from './api';
import {
  applyPlannerImport,
  ensureSupervisorPlannerRun,
  previewPlannerImport,
  refineSupervisorFeature,
  updateSupervisorPlan,
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

type Props = {
  opened: boolean;
  rootRepoPath: string;
  run?: SupervisorRun | null;
  templates?: WorkflowTemplate[];
  selectedFeatureId?: string | null;
  selectedPlannerId?: string | null;
  selectionMode?: boolean;
  onSelectFeature?: (selection: PlannerSelection) => void | Promise<void>;
  onClose: () => void;
  onSaved?: () => Promise<void> | void;
  onWorkflowRunCreated?: (workflowRunId: string) => Promise<void> | void;
  onError?: (message: string) => void;
};

const FEATURE_STATUSES: FeaturePlanItemStatus[] = ['rough', 'fine', 'scheduled', 'completed', 'applied'];
const IMPORT_ACTIONS: PlannerImportAction[] = ['create', 'create_copy', 'replace_existing', 'skip', 'reject'];

function normalizePlannerRoot(value: string): string {
  const normalized = value.trim().replace(/\\/g, '/');
  const supervisorShardMarker = '/.mdev/supervisors/';
  const supervisorIndex = normalized.indexOf(supervisorShardMarker);
  if (supervisorIndex > 0) return normalized.slice(0, supervisorIndex);
  return normalized.replace(/\/+$/g, '');
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

function templateOptions(templates: WorkflowTemplate[]) {
  return templates.map((template) => ({ value: template.id, label: template.name }));
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
    status: feature.status ?? 'rough',
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

export function PlannerModal(props: Props) {
  const rootRepoPath = normalizePlannerRoot(props.rootRepoPath || props.run?.root_repo_path || '');
  const [run, setRun] = useState<SupervisorRun | null>(props.run ?? null);
  const [templates, setTemplates] = useState<WorkflowTemplate[]>(props.templates ?? []);
  const [featureSearch, setFeatureSearch] = useState('');
  const [refinementTemplateId, setRefinementTemplateId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [busyFeatureId, setBusyFeatureId] = useState<string | null>(null);
  const [viewFeature, setViewFeature] = useState<FeaturePlanItem | null>(null);
  const [featureDraft, setFeatureDraft] = useState<FeaturePlanItem | null>(null);
  const [featureEditMode, setFeatureEditMode] = useState(false);
  const [importPayload, setImportPayload] = useState<unknown>(null);
  const [importPreview, setImportPreview] = useState<PlannerImportPreviewResponse | null>(null);
  const [importDecisions, setImportDecisions] = useState<Record<number, PlannerImportDecision>>({});
  const [importReviewOpen, setImportReviewOpen] = useState(false);
  const importInputRef = useRef<HTMLInputElement | null>(null);

  const effectiveTemplates = props.templates ?? templates;
  const refinementTemplateOptions = useMemo(() => templateOptions(effectiveTemplates), [effectiveTemplates]);
  const statusOptions = useMemo(() => FEATURE_STATUSES.map((status) => ({ value: status, label: titleCaseStatus(status) })), []);
  const importActions = useMemo(() => importActionOptions(), []);
  const features = run?.feature_plan_items ?? [];

  const filteredFeatures = useMemo(() => {
    const needle = featureSearch.trim().toLowerCase();
    if (!needle) return features;
    return features.filter((item) => {
      return featureTitle(item).toLowerCase().includes(needle)
        || (item.summary ?? '').toLowerCase().includes(needle)
        || String(item.status ?? '').toLowerCase().includes(needle);
    });
  }, [features, featureSearch]);

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
    let cancelled = false;

    async function load() {
      if (!rootRepoPath) return;
      try {
        setBusy(true);
        const [templateRows, plannerResponse] = await Promise.all([
          props.templates ? Promise.resolve(props.templates) : listTemplates(),
          ensureSupervisorPlannerRun({ root_repo_path: rootRepoPath, title: repoPlannerTitle(rootRepoPath) }),
        ]);
        if (cancelled) return;
        setTemplates(templateRows);
        setRun(plannerResponse.supervisor_run);
        setRefinementTemplateId((current) => current ?? (templateRows.find((template) => template.name.toLowerCase().includes('refinement')) ?? templateRows[0] ?? null)?.id ?? null);
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
    if (!rootRepoPath) return;
    const response = await ensureSupervisorPlannerRun({ root_repo_path: rootRepoPath, title: repoPlannerTitle(rootRepoPath) });
    setRun(response.supervisor_run);
    await props.onSaved?.();
  }

  async function createPlanner() {
    if (!rootRepoPath) {
      props.onError?.('Repo root is required before creating a planner.');
      return;
    }
    try {
      setBusy(true);
      await reload();
    } catch (err) {
      props.onError?.(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  function downloadPlanner() {
    if (!run) return;
    const payload = {
      version: 1,
      kind: 'planner_features',
      root_repo_path: run.root_repo_path,
      planner: {
        id: run.id,
        title: run.title,
        root_repo_path: run.root_repo_path,
      },
      features: run.feature_plan_items ?? [],
      feature_plan_items: run.feature_plan_items ?? [],
    };
    const blob = new Blob([JSON.stringify(payload, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = `${repoPlannerTitle(run.root_repo_path).replace(/[^a-z0-9_-]+/gi, '-').replace(/^-+|-+$/g, '').toLowerCase() || 'planner'}-features.json`;
    document.body.appendChild(link);
    link.click();
    link.remove();
    URL.revokeObjectURL(url);
  }

  async function importPlannerFile(file: File | null | undefined) {
    if (!file) return;
    if (!run) {
      props.onError?.('Planner must be loaded before importing features.');
      return;
    }

    try {
      setBusy(true);
      const payload = normalizePlannerImportPayload(JSON.parse(await file.text()) as unknown);
      const preview = await previewPlannerImport(run.id, payload);
      const decisions = Object.fromEntries(preview.items.map((item) => [
        item.import_index,
        {
          import_index: item.import_index,
          action: item.default_action,
          existing_feature_id: item.existing_feature_id ?? null,
        } satisfies PlannerImportDecision,
      ]));
      setImportPayload(payload);
      setImportPreview(preview);
      setImportDecisions(decisions);
      setImportReviewOpen(true);
    } catch (err) {
      props.onError?.(`Planner import preview failed: ${err instanceof Error ? err.message : String(err)}`);
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
    if (!props.onSelectFeature) return;
    await props.onSelectFeature({
      planner: run ? { id: run.id, root_repo_path: run.root_repo_path, title: run.title } : null,
      feature,
    });
    props.onClose();
  }

  async function persistFeatures(nextFeatures: FeaturePlanItem[]) {
    if (!run) return;
    await updateSupervisorPlan(run.id, nextFeatures, run.execution_plan_items, {
      sprint_strategy: run.strategy,
      planner_refinement_template_id: refinementTemplateId,
    });
    await reload();
  }

  async function saveFeatureDraft() {
    if (!run || !featureDraft) return;
    try {
      setBusyFeatureId(featureDraft.id);
      const nextFeatures = run.feature_plan_items.map((item) => item.id === featureDraft.id ? featureDraft : item);
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
    if (!run) return;
    try {
      setBusyFeatureId(feature.id);
      const nextFeature = { ...feature, status };
      const nextFeatures = run.feature_plan_items.map((item) => item.id === feature.id ? nextFeature : item);
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

  async function refineFeature(feature: FeaturePlanItem) {
    if (!run) return;
    try {
      setBusyFeatureId(feature.id);
      const response = await refineSupervisorFeature(run.id, feature.id, refinementTemplateId);
      await reload();
      if (response.workflow_run_id) {
        await props.onWorkflowRunCreated?.(response.workflow_run_id);
      }
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

  return (
    <Modal opened={props.opened} onClose={props.onClose} title="Planner" size="calc(100vw - 96px)" centered zIndex={300}>
      <Stack gap="md">
        <Group align="end" wrap="wrap">
          <Select
            label="Planner"
            placeholder="Planner"
            value={run?.id ?? null}
            data={run ? [{ value: run.id, label: run.title }] : []}
            disabled
            style={{ minWidth: 280 }}
          />
          <Button size="xs" onClick={() => void createPlanner()} loading={busy}>Create planner</Button>
          <Button size="xs" variant="light" onClick={() => void reload()} loading={busy} disabled={!rootRepoPath}>Refresh planner</Button>
          <Button size="xs" variant="light" onClick={downloadPlanner} disabled={!run}>Download</Button>
          <Button size="xs" variant="light" onClick={() => importInputRef.current?.click()} loading={busy} disabled={!run}>Upload</Button>
          <input
            ref={importInputRef}
            type="file"
            accept="application/json,.json"
            style={{ display: 'none' }}
            onChange={(event) => void importPlannerFile(event.currentTarget.files?.[0])}
          />
          <Select
            label="Refinement template"
            placeholder="Refinement template"
            value={refinementTemplateId}
            data={refinementTemplateOptions}
            onChange={setRefinementTemplateId}
            searchable
            clearable
            style={{ minWidth: 300 }}
          />
        </Group>

        {run ? (
          <Group gap="xs">
            <Badge variant="light">{run.title}</Badge>
            <Badge variant="light" color="green">Planner</Badge>
            <Text size="xs" c="dimmed">{run.root_repo_path}</Text>
          </Group>
        ) : (
          <Text c="dimmed" size="sm">No planner selected.</Text>
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
                  <Table.Th style={{ width: 320 }}>Actions</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {filteredFeatures.map((item) => {
                  const planStatus = String(item.status ?? 'rough');
                  const isScheduled = planStatus === 'scheduled';
                  const isSelected = props.selectedFeatureId === item.id;
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
                        <Badge variant="light" color={statusBadgeColor(planStatus)} tt="none" style={{ maxWidth: 'none', overflow: 'visible' }}>
                          {titleCaseStatus(planStatus)}
                        </Badge>
                      </Table.Td>
                      <Table.Td>
                        <Group gap="xs" wrap="nowrap">
                          <Button size="xs" variant="light" onClick={() => openFeature(item)}>Open</Button>
                          {props.selectionMode ? <Button size="xs" onClick={() => void selectFeature(item)} disabled={!item.id || isSelected}>Select</Button> : null}
                          <Button size="xs" variant="light" onClick={() => void refineFeature(item)} loading={busyForFeature} disabled={!run || !refinementTemplateId}>Refine</Button>
                          {isScheduled ? (
                            <Button size="xs" variant="light" color="orange" onClick={() => void setFeatureStatus(item, 'fine')} loading={busyForFeature} disabled={!run}>Unschedule</Button>
                          ) : (
                            <Button size="xs" variant="light" onClick={() => void setFeatureStatus(item, 'scheduled')} loading={busyForFeature} disabled={!run}>Schedule</Button>
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
          {props.selectionMode ? (
            <Button size="xs" variant="light" color="red" onClick={() => void selectFeature(null)} disabled={!props.selectedFeatureId}>Clear selection</Button>
          ) : <span />}
          <Button size="xs" variant="default" onClick={props.onClose}>Close</Button>
        </Group>
      </Stack>

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
                {props.selectionMode ? <Button size="xs" onClick={() => void selectFeature(featureDraft)} disabled={props.selectedFeatureId === featureDraft.id}>Select</Button> : null}
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
                <Button size="xs" variant="light" onClick={() => void refineFeature(featureDraft)} disabled={!run || !refinementTemplateId}>Refine</Button>
              </Group>
            </Group>

            {featureEditMode ? (
              <>
                <TextInput label="Title" value={featureDraft.title} onChange={(event) => setFeatureDraft({ ...featureDraft, title: event.currentTarget.value })} />
                <Select label="Status" value={featureDraft.status} data={statusOptions} onChange={(value) => setFeatureDraft({ ...featureDraft, status: (value as FeaturePlanItemStatus) ?? 'rough' })} allowDeselect={false} />
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
