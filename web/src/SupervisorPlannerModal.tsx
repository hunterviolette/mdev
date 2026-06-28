import { useEffect, useMemo, useRef, useState } from 'react';
import { Alert, Badge, Button, Card, Checkbox, Group, Modal, Select, Stack, Table, Text, Textarea, TextInput } from '@mantine/core';
import type { WorkflowTemplate } from './api';
import { applyPlannerImport, previewPlannerImport, refineSupervisorFeature, updateSupervisorPlan, type FeaturePlanItem, type PlannerImportAction, type PlannerImportPreviewResponse, type SupervisorRun } from './supervisor_api';

type Props = {
  opened: boolean;
  run: SupervisorRun | null | undefined;
  templates: WorkflowTemplate[];
  onClose: () => void;
  onSaved: () => Promise<void> | void;
  onWorkflowRunCreated?: (workflowRunId: string) => Promise<void> | void;
  selectionMode?: boolean;
  selectedFeatureId?: string | null;
  onSelectFeature?: (feature: FeaturePlanItem) => Promise<void> | void;
};

function emptyPlannerItem(index: number): FeaturePlanItem {
  return {
    id: crypto.randomUUID(),
    title: `Feature ${index}`,
    status: 'rough',
    summary: '',
    rough_summary: null,
    refinement_workflow_run_id: null,
    requirements: [],
    acceptance_criteria: [],
    implementation_notes: [],
    review_expectations: [],
    target_files_or_areas: [],
    dependencies: []
  };
}

function lines(value: string): string[] {
  return value.split('\n').map((line) => line.trim()).filter(Boolean);
}

function text(values: string[]): string {
  return values.join('\n');
}

function importString(value: unknown): string {
  return typeof value === 'string' ? value : '';
}

function importStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.map((item) => typeof item === 'string' ? item.trim() : '').filter(Boolean);
}

function importFeature(value: unknown, index: number): FeaturePlanItem {
  const item = value && typeof value === 'object' ? value as Record<string, unknown> : {};
  const status = importString(item.status);
  const normalizedStatus: FeaturePlanItem['status'] = status === 'fine' || status === 'scheduled' || status === 'completed' ? status : 'rough';
  return {
    id: importString(item.id).trim() || crypto.randomUUID(),
    title: importString(item.title).trim() || `Feature ${index + 1}`,
    status: normalizedStatus,
    summary: importString(item.summary),
    rough_summary: typeof item.rough_summary === 'string' ? item.rough_summary : null,
    refinement_workflow_run_id: typeof item.refinement_workflow_run_id === 'string' ? item.refinement_workflow_run_id : null,
    requirements: importStringArray(item.requirements),
    acceptance_criteria: importStringArray(item.acceptance_criteria),
    implementation_notes: importStringArray(item.implementation_notes),
    review_expectations: importStringArray(item.review_expectations),
    target_files_or_areas: importStringArray(item.target_files_or_areas),
    dependencies: importStringArray(item.dependencies)
  };
}

function exportedPlannerFilename(run: SupervisorRun | null | undefined): string {
  const base = (run?.title ?? 'planner')
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, '-')
    .replace(/^-+|-+$/g, '') || 'planner';
  return `${base}-features.json`;
}

function definitionLabel(status: FeaturePlanItem['status']): string {
  if (status === 'completed') return 'Completed';
  if (status === 'scheduled') return 'Scheduled';
  if (status === 'fine') return 'Fine';
  return 'Rough';
}

function definitionBadgeColor(status: FeaturePlanItem['status']): string {
  if (status === 'completed') return 'green';
  if (status === 'scheduled') return 'blue';
  if (status === 'fine') return 'yellow';
  return 'gray';
}

function contextString(run: SupervisorRun, key: string): string | null {
  const value = run.context?.[key];
  return typeof value === 'string' && value.length > 0 ? value : null;
}

const DEFAULT_REFINEMENT_TEMPLATE_NAME = 'Default refinement workflow';

function normalizeFeature(item: FeaturePlanItem, index: number): FeaturePlanItem {
  return {
    ...item,
    id: item.id.trim() || crypto.randomUUID(),
    title: item.title.trim() || `Feature ${index + 1}`,
    summary: item.summary.trim(),
    rough_summary: item.rough_summary ?? (item.status === 'rough' ? item.summary.trim() : null),
    refinement_workflow_run_id: item.refinement_workflow_run_id ?? null,
    requirements: item.status === 'rough' ? [] : item.requirements.map((value) => value.trim()).filter(Boolean),
    acceptance_criteria: item.status === 'rough' ? [] : item.acceptance_criteria.map((value) => value.trim()).filter(Boolean),
    implementation_notes: item.status === 'rough' ? [] : item.implementation_notes.map((value) => value.trim()).filter(Boolean),
    review_expectations: item.status === 'rough' ? [] : item.review_expectations.map((value) => value.trim()).filter(Boolean),
    target_files_or_areas: item.status === 'rough' ? [] : item.target_files_or_areas.map((value) => value.trim()).filter(Boolean),
    dependencies: []
  };
}

export function SupervisorPlannerModal({ opened, run, templates, onClose, onSaved, onWorkflowRunCreated, selectionMode = false, selectedFeatureId = null, onSelectFeature }: Props) {
  const [features, setFeatures] = useState<FeaturePlanItem[]>([]);
  const [expandedFeatureId, setExpandedFeatureId] = useState<string | null>(null);
  const [refinementTemplateId, setRefinementTemplateId] = useState<string | null>(null);
  const [featureFilter, setFeatureFilter] = useState<string>('all');
  const [error, setError] = useState<string | null>(null);
  const [exportFeatureIds, setExportFeatureIds] = useState<Set<string>>(new Set());
  const [exportMode, setExportMode] = useState<'all' | 'selected'>('all');
  const [importPayload, setImportPayload] = useState<unknown | null>(null);
  const [importPreview, setImportPreview] = useState<PlannerImportPreviewResponse | null>(null);
  const [importActions, setImportActions] = useState<Record<number, PlannerImportAction>>({});
  const importInputRef = useRef<HTMLInputElement | null>(null);
  const templateOptions = useMemo(() => templates.map((template) => ({ value: template.id, label: template.name })), [templates]);
  const scheduledFeatureIds = useMemo(() => new Set((run?.execution_plan_items ?? []).map((item) => item.feature_plan_item_id)), [run?.execution_plan_items]);
  const visibleFeatures = useMemo(() => features.map((item, index) => ({ item, index })).filter(({ item }) => {
    const scheduled = scheduledFeatureIds.has(item.id);
    if (featureFilter === 'rough') return item.status === 'rough';
    if (featureFilter === 'fine') return item.status === 'fine';
    if (featureFilter === 'scheduled') return item.status === 'scheduled' || scheduled;
    if (featureFilter === 'completed') return item.status === 'completed';
    if (featureFilter === 'unscheduled') return !scheduled && item.status !== 'completed';
    return true;
  }), [features, featureFilter, scheduledFeatureIds]);
  const defaultRefinementTemplate = useMemo(() => templates.find((template) => template.name === DEFAULT_REFINEMENT_TEMPLATE_NAME) ?? null, [templates]);
  const effectiveRefinementTemplateId = refinementTemplateId ?? defaultRefinementTemplate?.id ?? null;

  useEffect(() => {
    if (!opened || !run) return;
    const nextFeatures = run.feature_plan_items ?? [];
    setFeatures(nextFeatures);
    setExpandedFeatureId(nextFeatures[0]?.id ?? null);
    setRefinementTemplateId(contextString(run, 'planner_refinement_template_id') ?? contextString(run, 'workflow_template_id') ?? defaultRefinementTemplate?.id ?? null);
    setError(null);
    setImportPayload(null);
    setImportPreview(null);
    setImportActions({});
  }, [opened, run?.id, defaultRefinementTemplate?.id]);

  useEffect(() => {
    setExportFeatureIds((prev) => {
      const currentIds = new Set(features.map((item) => item.id));
      return new Set(Array.from(prev).filter((id) => currentIds.has(id)));
    });
  }, [features]);

  function updateFeature(index: number, patch: Partial<FeaturePlanItem>) {
    setFeatures((prev) => prev.map((item, idx) => idx === index ? { ...item, ...patch } : item));
  }

  function addFeature() {
    setFeatures((prev) => {
      const next = emptyPlannerItem(prev.length + 1);
      setExpandedFeatureId(next.id);
      return [...prev, next];
    });
  }

  function removeFeature(index: number) {
    setFeatures((prev) => prev.filter((_, idx) => idx !== index));
  }

  function downloadFeatures() {
    const selectedFeatures = exportMode === 'all'
      ? features
      : features.filter((item) => exportFeatureIds.has(item.id));
    if (selectedFeatures.length === 0) {
      setError('Select at least one feature to download.');
      return;
    }
    const payload = {
      version: 1,
      exported_at: new Date().toISOString(),
      root_repo_path: run?.root_repo_path ?? null,
      export_mode: exportMode,
      features: selectedFeatures.map((item, index) => normalizeFeature(item, index))
    };
    const blob = new Blob([JSON.stringify(payload, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = exportedPlannerFilename(run);
    document.body.appendChild(link);
    link.click();
    link.remove();
    URL.revokeObjectURL(url);
  }

  function toggleExportFeature(featureId: string, checked: boolean) {
    setExportFeatureIds((prev) => {
      const next = new Set(prev);
      if (checked) next.add(featureId);
      else next.delete(featureId);
      return next;
    });
  }

  function selectAllExportFeatures() {
    setExportFeatureIds(new Set(features.map((item) => item.id)));
    setExportMode('selected');
  }

  function clearExportFeatures() {
    setExportFeatureIds(new Set());
    setExportMode('selected');
  }

  async function importFeaturesFile(file: File | null | undefined) {
    if (!file) return;
    if (!run) {
      setError('Planner must be loaded before importing features.');
      return;
    }
    setError(null);
    try {
      const payload = JSON.parse(await file.text()) as unknown;
      const preview = await previewPlannerImport(run.id, payload);
      setImportPayload(payload);
      setImportPreview(preview);
      setImportActions(Object.fromEntries(preview.items.map((item) => [item.import_index, item.default_action])) as Record<number, PlannerImportAction>);
    } catch (err) {
      setError(`Planner import failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      if (importInputRef.current) importInputRef.current.value = '';
    }
  }

  async function applyReviewedImport() {
    if (!run || !importPayload || !importPreview) return;
    setError(null);
    try {
      const decisions = importPreview.items.map((item) => ({
        import_index: item.import_index,
        action: importActions[item.import_index] ?? item.default_action,
        existing_feature_id: item.existing_feature_id ?? null
      }));
      const response = await applyPlannerImport(run.id, importPayload, decisions);
      setFeatures(response.supervisor_run.feature_plan_items ?? []);
      setImportPayload(null);
      setImportPreview(null);
      setImportActions({});
      await onSaved();
    } catch (err) {
      setError(`Planner import apply failed: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  async function saveFeature(index: number): Promise<FeaturePlanItem | null> {
    if (!run) return null;
    setError(null);
    try {
      const draft = features[index];
      if (!draft) return null;

      const originalId = run.feature_plan_items[index]?.id ?? draft.id;
      const normalizedFeature = normalizeFeature(draft, index);
      const mergedFeatures = [...(run.feature_plan_items ?? [])];
      const existingIndex = mergedFeatures.findIndex((item) => item.id === originalId);

      if (existingIndex >= 0) {
        mergedFeatures[existingIndex] = normalizedFeature;
      } else {
        mergedFeatures.push(normalizedFeature);
      }

      const existingSprintItems = (run.execution_plan_items ?? []).filter((sprintItem) => mergedFeatures.some((feature) => feature.id === sprintItem.feature_plan_item_id));
      await updateSupervisorPlan(run.id, mergedFeatures, existingSprintItems, {
        sprint_strategy: run.strategy,
        workflow_template_id: contextString(run, 'workflow_template_id'),
        integration_template_id: contextString(run, 'integration_template_id'),
        planner_refinement_template_id: effectiveRefinementTemplateId
      } as any);
      setFeatures((prev) => prev.map((item, idx) => idx === index ? normalizedFeature : item));
      setExpandedFeatureId(normalizedFeature.id);
      await onSaved();
      return normalizedFeature;
    } catch (err) {
      setError(String(err));
      return null;
    }
  }

  async function scheduleFeature(index: number) {
    if (!run) return;
    setError(null);
    try {
      const savedFeature = await saveFeature(index);
      if (!savedFeature) return;
      const existingSprintItems = run.execution_plan_items ?? [];
      if (existingSprintItems.some((item) => item.feature_plan_item_id === savedFeature.id)) return;
      const nextSprintItems = [
        ...existingSprintItems,
        {
          feature_plan_item_id: savedFeature.id,
          workflow_template_id: contextString(run, 'workflow_template_id'),
          order_index: existingSprintItems.length
        }
      ];
      const mergedFeatures = [...(run.feature_plan_items ?? [])];
      const existingIndex = mergedFeatures.findIndex((item) => item.id === savedFeature.id);
      if (existingIndex >= 0) {
        mergedFeatures[existingIndex] = savedFeature;
      } else {
        mergedFeatures.push(savedFeature);
      }
      await updateSupervisorPlan(run.id, mergedFeatures, nextSprintItems, {
        sprint_strategy: run.strategy,
        workflow_template_id: contextString(run, 'workflow_template_id'),
        integration_template_id: contextString(run, 'integration_template_id'),
        planner_refinement_template_id: effectiveRefinementTemplateId
      } as any);
      await onSaved();
    } catch (err) {
      setError(String(err));
    }
  }

  async function unscheduleFeature(featureId: string) {
    if (!run) return;
    setError(null);
    try {
      const nextSprintItems = (run.execution_plan_items ?? [])
        .filter((item) => item.feature_plan_item_id !== featureId)
        .map((item, index) => ({ ...item, order_index: index }));
      await updateSupervisorPlan(run.id, run.feature_plan_items ?? [], nextSprintItems, {
        sprint_strategy: run.strategy,
        workflow_template_id: contextString(run, 'workflow_template_id'),
        integration_template_id: contextString(run, 'integration_template_id'),
        planner_refinement_template_id: effectiveRefinementTemplateId
      } as any);
      await onSaved();
    } catch (err) {
      setError(String(err));
    }
  }

  async function refineFeature(index: number) {
    if (!run) return;
    setError(null);
    try {
      const savedFeature = await saveFeature(index);
      if (!savedFeature) return;
      const response = await refineSupervisorFeature(run.id, savedFeature.id, effectiveRefinementTemplateId);
      await onSaved();
      onClose();
      await onWorkflowRunCreated?.(response.workflow_run_id);
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <Modal
      opened={opened}
      onClose={onClose}
      title={run ? `${run.title} planner` : 'Planner'}
      size="calc(100vw - 32px)"
      centered
      padding="md"
      styles={{
        body: { paddingTop: 0, height: 'calc(100vh - 72px)', overflow: 'auto' },
        content: {
          background: 'var(--mantine-color-body)',
          maxHeight: 'calc(100vh - 32px)',
        },
      }}
    >
      <Stack gap="md">
        {error ? <Alert color="red">{error}</Alert> : null}
        <Group justify="space-between">
          <Text size="sm" c="dimmed">{selectionMode ? 'Open, add, edit, save, or select a planner feature for this workflow.' : 'Create rough features as a single prompt. Refinement is handled by a design workflow that emits structured output back into this planner.'}</Text>
          <Group>
            {!selectionMode ? <Select value={effectiveRefinementTemplateId} onChange={setRefinementTemplateId} data={templateOptions} placeholder="Refinement workflow" searchable w={300} /> : null}
            <Button size="xs" variant="light" onClick={addFeature}>New feature</Button>
            <Select
              size="xs"
              w={180}
              value={exportMode}
              onChange={(value) => setExportMode(value === 'selected' ? 'selected' : 'all')}
              data={[
                { value: 'all', label: 'Download all' },
                { value: 'selected', label: 'Download selected' }
              ]}
            />
            <Button size="xs" variant="default" onClick={downloadFeatures} disabled={features.length === 0 || (exportMode === 'selected' && exportFeatureIds.size === 0)}>Download JSON</Button>
            <Button size="xs" variant="default" onClick={() => importInputRef.current?.click()}>Upload JSON</Button>
            <input
              ref={importInputRef}
              type="file"
              accept="application/json,.json"
              style={{ display: 'none' }}
              onChange={(event) => void importFeaturesFile(event.currentTarget.files?.[0])}
            />
            {importPreview ? <Button size="xs" variant="filled" onClick={() => void applyReviewedImport()}>Apply import review</Button> : null}
          </Group>
        </Group>
        {importPreview ? (
          <Card withBorder>
            <Stack gap="sm">
              <Group justify="space-between">
                <Text fw={700}>Import review</Text>
                <Group gap="xs">
                  <Badge variant="light">{importPreview.summary.accepted} new</Badge>
                  <Badge color="yellow" variant="light">{importPreview.summary.duplicates} duplicates</Badge>
                  <Badge color="orange" variant="light">{importPreview.summary.conflicts} conflicts</Badge>
                  <Badge color="red" variant="light">{importPreview.summary.invalid} invalid</Badge>
                </Group>
              </Group>
              <Text size="sm" c="dimmed">Finish this import review before returning to the feature list. Use another planner page if you need to compare existing feature details.</Text>
              <Table striped highlightOnHover>
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>#</Table.Th>
                    <Table.Th>Status</Table.Th>
                    <Table.Th>Feature</Table.Th>
                    <Table.Th>Reason</Table.Th>
                    <Table.Th>Action</Table.Th>
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {importPreview.items.map((item) => (
                    <Table.Tr key={item.import_index}>
                      <Table.Td>{item.import_index + 1}</Table.Td>
                      <Table.Td><Badge color={item.status === 'accepted' ? 'green' : item.status === 'duplicate' ? 'yellow' : item.status === 'conflict' ? 'orange' : 'red'}>{item.status}</Badge></Table.Td>
                      <Table.Td>
                        <Stack gap={2}>
                          <Text fw={600}>{item.feature?.title ?? 'Invalid item'}</Text>
                          {item.existing_title ? <Text size="xs" c="dimmed">Existing: {item.existing_title}</Text> : null}
                        </Stack>
                      </Table.Td>
                      <Table.Td><Text size="sm">{item.reason}</Text></Table.Td>
                      <Table.Td>
                        <Select
                          size="xs"
                          value={importActions[item.import_index] ?? item.default_action}
                          onChange={(value) => setImportActions((prev) => ({ ...prev, [item.import_index]: (value ?? item.default_action) as PlannerImportAction }))}
                          data={[
                            { value: 'create', label: 'Create' },
                            { value: 'create_copy', label: 'Create copy' },
                            { value: 'replace_existing', label: 'Replace existing' },
                            { value: 'skip', label: 'Skip' },
                            { value: 'reject', label: 'Reject' }
                          ]}
                          allowDeselect={false}
                          w={170}
                        />
                      </Table.Td>
                    </Table.Tr>
                  ))}
                </Table.Tbody>
              </Table>
              <Group justify="flex-end">
                <Button variant="default" onClick={() => { setImportPayload(null); setImportPreview(null); setImportActions({}); }}>Cancel import</Button>
                <Button onClick={() => void applyReviewedImport()}>Apply selected actions</Button>
              </Group>
            </Stack>
          </Card>
        ) : (
          <>
        <Card withBorder>
          <Stack gap="sm">
            <Group justify="space-between">
              <Text fw={700}>Features</Text>
              <Badge variant="light">{features.length}</Badge>
            </Group>
            {features.length === 0 ? <Text size="sm" c="dimmed">No features yet.</Text> : null}
            <Group justify="space-between" align="flex-end">
              <Select
                label="Feature filter"
                value={featureFilter}
                onChange={(value) => setFeatureFilter(value ?? 'all')}
                data={[
                  { value: 'all', label: 'All features' },
                  { value: 'rough', label: 'Rough' },
                  { value: 'fine', label: 'Fine' },
                  { value: 'scheduled', label: 'Scheduled' },
                  { value: 'completed', label: 'Completed' },
                  { value: 'unscheduled', label: 'Unscheduled' }
                ]}
                allowDeselect={false}
                w={260}
              />
              <Text size="sm" c="dimmed">{visibleFeatures.length} shown / {features.length} total</Text>
            </Group>
            {exportMode === 'selected' ? (
              <Group>
                <Text size="xs" c="dimmed">{exportFeatureIds.size} selected for download</Text>
                <Button size="compact-xs" variant="subtle" onClick={selectAllExportFeatures} disabled={features.length === 0}>Select all</Button>
                <Button size="compact-xs" variant="subtle" color="gray" onClick={clearExportFeatures} disabled={exportFeatureIds.size === 0}>Clear</Button>
              </Group>
            ) : null}
            <Table striped highlightOnHover>
              <Table.Thead>
                <Table.Tr>
                  {exportMode === 'selected' ? <Table.Th>Download</Table.Th> : null}
                  <Table.Th>Feature</Table.Th>
                  <Table.Th>Definition</Table.Th>
                  <Table.Th>Requirements</Table.Th>
                  <Table.Th>Acceptance criteria</Table.Th>
                  <Table.Th />
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {visibleFeatures.map(({ item, index }) => (
                  <Table.Tr key={`${item.id}-${index}`}>
                    {exportMode === 'selected' ? (
                      <Table.Td>
                        <Checkbox
                          checked={exportFeatureIds.has(item.id)}
                          onChange={(event) => toggleExportFeature(item.id, event.currentTarget.checked)}
                        />
                      </Table.Td>
                    ) : null}
                    <Table.Td>
                      <Stack gap={2}>
                        <Text fw={600}>{item.title}</Text>
                        <Text size="xs" c="dimmed">{item.id}</Text>
                      </Stack>
                    </Table.Td>
                    <Table.Td><Badge color={definitionBadgeColor(item.status)}>{definitionLabel(item.status)}</Badge></Table.Td>
                    <Table.Td>{item.requirements.length}</Table.Td>
                    <Table.Td>{item.acceptance_criteria.length}</Table.Td>
                    <Table.Td>
                      <Group justify="flex-end" gap="xs">
                        <Button size="xs" variant="light" onClick={() => setExpandedFeatureId(expandedFeatureId === item.id ? null : item.id)}>{expandedFeatureId === item.id ? 'Close' : 'Open'}</Button>
                        {selectionMode ? (
                          <Button size="xs" variant={selectedFeatureId === item.id ? 'filled' : 'light'} onClick={() => onSelectFeature?.(item)}>{selectedFeatureId === item.id ? 'Selected' : 'Select'}</Button>
                        ) : (
                          <>
                            <Button size="xs" variant="light" onClick={() => refineFeature(index)}>Refine</Button>
                            {scheduledFeatureIds.has(item.id) ? (
                              <Button size="xs" variant="subtle" color="orange" onClick={() => unscheduleFeature(item.id)}>Unschedule</Button>
                            ) : (
                              <Button size="xs" variant="subtle" onClick={() => scheduleFeature(index)} disabled={item.status !== 'fine'}>Schedule</Button>
                            )}
                            <Button size="xs" color="red" variant="subtle" onClick={() => removeFeature(index)}>Remove</Button>
                          </>
                        )}
                      </Group>
                    </Table.Td>
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </Stack>
        </Card>
        {features.map((item, index) => expandedFeatureId === item.id ? (
          <Card key={`detail-${item.id}-${index}`} withBorder>
            <Stack gap="xs">
              <Group justify="space-between">
                <Text fw={700}>{item.status === 'rough' ? 'Rough feature' : 'Feature'}</Text>
                <Badge color={definitionBadgeColor(item.status)}>{definitionLabel(item.status)}</Badge>
              </Group>
              <Group grow>
                <TextInput label="Feature id" value={item.id} onChange={(event) => updateFeature(index, { id: event.currentTarget.value })} />
                <TextInput label="Title" value={item.title} onChange={(event) => updateFeature(index, { title: event.currentTarget.value })} />
                <Select label="Feature status" value={item.status} onChange={(value) => updateFeature(index, { status: (value as FeaturePlanItem['status']) ?? 'rough' })} data={[{ value: 'rough', label: 'Rough' }, { value: 'fine', label: 'Fine' }, { value: 'scheduled', label: 'Scheduled' }, { value: 'completed', label: 'Completed' }]} />
              </Group>
              {item.status === 'rough' ? (
                <Textarea label="Rough feature prompt" value={item.summary} onChange={(event) => updateFeature(index, { summary: event.currentTarget.value, rough_summary: event.currentTarget.value })} minRows={6} autosize />
              ) : (
                <>
                  <Textarea label="Original rough feature prompt" value={item.rough_summary ?? ''} minRows={3} autosize readOnly />
                  <Textarea label="Refined feature summary" value={item.summary} onChange={(event) => updateFeature(index, { summary: event.currentTarget.value })} minRows={6} autosize />
                </>
              )}
              {item.status !== 'rough' ? (
                <>
                  <Textarea label="Detailed requirements" value={text(item.requirements)} onChange={(event) => updateFeature(index, { requirements: lines(event.currentTarget.value) })} minRows={4} autosize />
                  <Textarea label="Acceptance criteria" value={text(item.acceptance_criteria)} onChange={(event) => updateFeature(index, { acceptance_criteria: lines(event.currentTarget.value) })} minRows={4} autosize />
                  <Textarea label="Implementation instructions" value={text(item.implementation_notes)} onChange={(event) => updateFeature(index, { implementation_notes: lines(event.currentTarget.value) })} minRows={3} autosize />
                  <Textarea label="Review instructions" value={text(item.review_expectations)} onChange={(event) => updateFeature(index, { review_expectations: lines(event.currentTarget.value) })} minRows={3} autosize />
                  <Textarea label="Target files or areas" value={text(item.target_files_or_areas)} onChange={(event) => updateFeature(index, { target_files_or_areas: lines(event.currentTarget.value) })} minRows={2} autosize />

                </>
              ) : null}
              <Group justify="flex-end">
                <Button variant="default" onClick={() => saveFeature(index)}>Save feature</Button>
                {selectionMode ? (
                  <Button variant={selectedFeatureId === item.id ? 'filled' : 'light'} onClick={() => onSelectFeature?.(item)}>{selectedFeatureId === item.id ? 'Selected' : 'Select'}</Button>
                ) : (
                  <Button variant="light" onClick={() => refineFeature(index)}>Refine with workflow</Button>
                )}
              </Group>
            </Stack>
          </Card>
        ) : null)}
          </>
        )}
      </Stack>
    </Modal>
  );
}
