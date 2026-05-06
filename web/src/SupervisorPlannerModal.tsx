import { useEffect, useMemo, useState } from 'react';
import { Alert, Badge, Button, Card, Group, Modal, Select, Stack, Table, Text, Textarea, TextInput } from '@mantine/core';
import type { WorkflowTemplate } from './api';
import { refineSupervisorFeature, updateSupervisorPlan, type FeaturePlanItem, type SupervisorRun } from './supervisor_api';

type Props = {
  opened: boolean;
  run: SupervisorRun | null | undefined;
  templates: WorkflowTemplate[];
  onClose: () => void;
  onSaved: () => Promise<void> | void;
  onWorkflowRunCreated?: (workflowRunId: string) => Promise<void> | void;
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

export function SupervisorPlannerModal({ opened, run, templates, onClose, onSaved, onWorkflowRunCreated }: Props) {
  const [features, setFeatures] = useState<FeaturePlanItem[]>([]);
  const [expandedFeatureId, setExpandedFeatureId] = useState<string | null>(null);
  const [refinementTemplateId, setRefinementTemplateId] = useState<string | null>(null);
  const [featureFilter, setFeatureFilter] = useState<string>('all');
  const [error, setError] = useState<string | null>(null);
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
  }, [opened, run?.id, defaultRefinementTemplate?.id]);

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
    <Modal opened={opened} onClose={onClose} title={run ? `${run.title} planner` : 'Planner'} size="95%" centered>
      <Stack gap="md">
        {error ? <Alert color="red">{error}</Alert> : null}
        <Group justify="space-between">
          <Text size="sm" c="dimmed">Create rough features as a single prompt. Refinement is handled by a design workflow that emits structured output back into this planner.</Text>
          <Group>
            <Select value={effectiveRefinementTemplateId} onChange={setRefinementTemplateId} data={templateOptions} placeholder="Refinement workflow" searchable w={300} />
            <Button size="xs" variant="light" onClick={addFeature}>Add feature</Button>
          </Group>
        </Group>
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
            <Table striped highlightOnHover>
              <Table.Thead>
                <Table.Tr>

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
                        <Button size="xs" variant="light" onClick={() => refineFeature(index)}>Refine</Button>
                        {scheduledFeatureIds.has(item.id) ? (
                          <Button size="xs" variant="subtle" color="orange" onClick={() => unscheduleFeature(item.id)}>Unschedule</Button>
                        ) : (
                          <Button size="xs" variant="subtle" onClick={() => scheduleFeature(index)} disabled={item.status !== 'fine'}>Schedule</Button>
                        )}
                        <Button size="xs" color="red" variant="subtle" onClick={() => removeFeature(index)}>Remove</Button>
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
                <Button variant="light" onClick={() => refineFeature(index)}>Refine with workflow</Button>
              </Group>
            </Stack>
          </Card>
        ) : null)}
      </Stack>
    </Modal>
  );
}
