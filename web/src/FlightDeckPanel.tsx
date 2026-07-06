import { useEffect, useMemo, useState, type CSSProperties } from 'react';
import {
  Alert,
  Anchor,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  Divider,
  Group,
  JsonInput,
  Loader,
  Modal,
  NumberInput,
  Paper,
  ScrollArea,
  Select,
  SimpleGrid,
  Stack,
  Text,
  TextInput,
  Title,
  Tooltip,
} from '@mantine/core';
import { listTemplates, type WorkflowTemplate } from './api';
import { PlannerModal } from './PlannerModal';
import { listPlannersForRepo, refinePlannerFeature, type PlannerWorkspace } from './planner_api';
import { getFlightDeck, getSupervisorQueue, getWorkflowEventHistory, regenerateSupervisorQueueFeature, runSupervisorAction, setSupervisorQueue, unscheduleSupervisorFeature, workflowEventHistoryStreamUrl, type FlightDeckResponse, type FlightDeckSupervisor, type FlightDeckWorkUnit, type SupervisorQueueProjection, type WorkflowEventHistoryItem, type WorkflowEventHistoryQuery } from './supervisor_api';

type FlightDeckPanelProps = {
  navigate?: (path: string) => void;
  onOpenPlanner?: (supervisor: FlightDeckSupervisor, options?: OpenPlannerOptions) => void;
};

type OpenPlannerOptions = {
  createFeature?: boolean;
  selectFeature?: boolean;
  refinementTemplateId?: string | null;
};

type TemplateOption = { value: string; label: string };

type FlightDeckPoolSetting = {
  template_id?: string | null;
  mode?: string | null;
  concurrency?: number | null;
};

type FlightDeckSettings = {
  pools?: Record<string, FlightDeckPoolSetting>;
};

type StageProjection = {
  key: string;
  label: string;
  state: 'complete' | 'active' | 'waiting' | 'failed' | 'up_next' | 'future' | 'draft';
  message?: string;
};

type CapabilityProjection = {
  id: string;
  label: string;
  state: string;
  message: string;
  step_id?: string | null;
  stage_execution_id?: string | null;
  capability_invocation_id?: string | null;
};

type EventHistoryAnchor = {
  title: string;
  runId: string;
  stage?: string | null;
  capability?: string | null;
  stageExecutionId?: string | null;
  capabilityInvocationId?: string | null;
};

type WorkflowCardHistoryHydration = {
  capabilities: CapabilityProjection[];
  stages: Record<string, unknown>[];
};

const STAGE_LABELS: Record<string, string> = {
  design: 'Design',
  code: 'Code',
  compile: 'Compile',
  review: 'Review',
  merge: 'Merge',
  merge_patches: 'Merge patches',
  integration: 'Integration',
};

const WORK_POOL_GROUPS = [
  {
    key: 'refine',
    title: 'Refine pool',
    description: 'Feature-definition and planning refinement work.',
    empty: 'No refine work is currently active.',
  },
  {
    key: 'feature_development',
    title: 'Feature pool',
    description: 'Queued and active feature implementation work.',
    empty: 'No feature work is currently active.',
  },
  {
    key: 'manual_shard',
    title: 'Manual pool',
    description: 'Operator-created shards with template-attached workflows.',
    empty: 'No manual work is currently active.',
  },
];

function workflowTemplateOptions(templates: WorkflowTemplate[]): TemplateOption[] {
  return templates.map((template) => ({ value: template.id, label: template.name }));
}

function supervisorFlightDeckSettings(supervisor: FlightDeckSupervisor): FlightDeckSettings {
  const raw = supervisor.context?.flight_deck_settings;
  return raw && typeof raw === 'object' && !Array.isArray(raw) ? raw as FlightDeckSettings : {};
}

function poolSetting(supervisor: FlightDeckSupervisor, groupKey: string): FlightDeckPoolSetting {
  return supervisorFlightDeckSettings(supervisor).pools?.[groupKey] ?? {};
}

function nextFlightDeckSettings(supervisor: FlightDeckSupervisor, groupKey: string, patch: FlightDeckPoolSetting): FlightDeckSettings {
  const current = supervisorFlightDeckSettings(supervisor);
  return {
    ...current,
    pools: {
      ...(current.pools ?? {}),
      [groupKey]: {
        ...(current.pools?.[groupKey] ?? {}),
        ...patch,
      },
    },
  };
}

const FEATURE_MODE_OPTIONS = [
  { value: 'series', label: 'Series' },
  { value: 'parallel', label: 'Parallel' },
];

const INTEGRATION_MODE_OPTIONS = [
  { value: 'manual', label: 'Manual start after development' },
  { value: 'auto', label: 'Auto-run after development' },
];

function normalize(value: string | null | undefined): string {
  return (value ?? '').trim().toLowerCase();
}

function tone(value: string | null | undefined): string {
  const normalized = normalize(value);
  if (['success', 'complete', 'completed', 'done', 'applied', 'integrated'].includes(normalized)) return 'green';
  if (['active', 'running', 'integrating', 'ready_for_integration', 'patch_ready'].includes(normalized)) return 'cyan';
  if (['waiting', 'waiting_user', 'paused', 'up_next', 'draft'].includes(normalized)) return 'yellow';
  if (['failed', 'blocked', 'error', 'cancelled', 'deleted'].includes(normalized)) return 'red';
  return 'gray';
}

function titleCase(value: string | null | undefined): string {
  return (value ?? 'unknown')
    .split(/[_\s-]+/g)
    .filter(Boolean)
    .map((part) => `${part.slice(0, 1).toUpperCase()}${part.slice(1).toLowerCase()}`)
    .join(' ');
}

function compactPath(value?: string | null): string {
  if (!value) return '—';
  const normalizedPath = value.replace(/\\/g, '/');
  const parts = normalizedPath.split('/').filter(Boolean);
  if (parts.length <= 4) return normalizedPath;
  return `…/${parts.slice(-4).join('/')}`;
}

function workflowHref(workflowRunId: string): string {
  return `/workflows/${encodeURIComponent(workflowRunId)}`;
}

function supervisorHref(supervisorId: string): string {
  return `/supervisors/${encodeURIComponent(supervisorId)}`;
}


function telemetryArray(unit: FlightDeckWorkUnit, key: string): Array<Record<string, unknown>> {
  const value = unit.telemetry?.[key];
  return Array.isArray(value) ? value.filter((item): item is Record<string, unknown> => Boolean(item) && typeof item === 'object') : [];
}

function textField(item: Record<string, unknown>, key: string): string {
  const value = item[key];
  return typeof value === 'string' && value.trim() ? value : '';
}

function telemetryString(unit: FlightDeckWorkUnit, key: string): string {
  const value = unit.telemetry?.[key];
  return typeof value === 'string' && value.trim() ? value : '';
}

function currentStepId(unit: FlightDeckWorkUnit): string | null {
  const value = unit.telemetry?.current_step_id;
  return typeof value === 'string' && value.trim() ? value : null;
}

function stageKeyFromText(value: string): string {
  const normalized = normalize(value)
    .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i, '')
    .replace(/[-_]+$/g, '')
    .replace(/[^a-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '');
  return normalized || 'stage';
}

function buildStageProjection(unit: FlightDeckWorkUnit): StageProjection[] {
  const templateStages = telemetryArray(unit, 'stage_template');
  const recentStages = telemetryArray(unit, 'recent_stage_executions');
  const activeStep = currentStepId(unit);
  const templateKeys = templateStages.map((stage, index) => textField(stage, 'key') || stageKeyFromText(textField(stage, 'step_id') || textField(stage, 'step_type') || textField(stage, 'label') || `stage_${index + 1}`));
  const labelByKey = new Map<string, string>();
  templateStages.forEach((stage, index) => {
    const key = templateKeys[index];
    if (key) labelByKey.set(key, textField(stage, 'label') || STAGE_LABELS[key] || titleCase(key));
  });
  const observedFromHistory = recentStages.map((stage) => stageKeyFromText(textField(stage, 'step_id'))).filter(Boolean);
  if (templateKeys.length === 0) {
    const error = textField(unit.telemetry, 'stage_template_error') || 'Workflow template stages are unavailable; Flight Deck cannot render stage rails.';
    return [{
      key: 'template_error',
      label: 'Template error',
      state: 'failed',
      message: error,
    }];
  }

  const baseStageOrder = templateKeys;
  const observedKeys = new Set(baseStageOrder);

  for (const stage of recentStages) {
    const stepId = textField(stage, 'step_id');
    if (stepId) observedKeys.add(stageKeyFromText(stepId));
  }
  if (activeStep) observedKeys.add(stageKeyFromText(activeStep));
  if (activeStep && !baseStageOrder.includes(stageKeyFromText(activeStep))) observedKeys.add(stageKeyFromText(activeStep));

  const ordered = [...baseStageOrder, ...[...observedKeys].filter((key) => !baseStageOrder.includes(key))];
  const activeKey = activeStep ? stageKeyFromText(activeStep) : null;
  const activeIndex = activeKey ? ordered.indexOf(activeKey) : -1;
  const failedKeys = new Set(
    recentStages
      .filter((stage) => normalize(textField(stage, 'status')) === 'failed')
      .map((stage) => stageKeyFromText(textField(stage, 'step_id')))
  );
  const waiting = unit.state === 'waiting_user';

  return ordered.map((key, index) => {
    const matching = recentStages.find((stage) => stageKeyFromText(textField(stage, 'step_id')) === key);
    const status = normalize(matching ? textField(matching, 'status') : '');
    let state: StageProjection['state'] = 'future';

    if (failedKeys.has(key)) state = 'failed';
    else if (activeKey === key) state = waiting ? 'waiting' : 'active';
    else if (status === 'success' || status === 'complete') state = 'complete';
    else if (activeIndex >= 0 && index < activeIndex) state = 'complete';
    else if (activeIndex >= 0 && index === activeIndex + 1) state = 'up_next';

    return {
      key,
      label: labelByKey.get(key) ?? STAGE_LABELS[key] ?? titleCase(key),
      state,
      message: matching ? textField(matching, 'message') : undefined,
    };
  });
}

function buildCapabilityProjection(unit: FlightDeckWorkUnit): CapabilityProjection[] {
  const terminalRank: Record<string, number> = {
    failed: 6,
    success: 5,
    complete: 5,
    completed: 5,
    waiting_user: 4,
    waiting: 4,
    running: 3,
    active: 3,
    event: 1,
  };

  const byInvocation = new Map<string, CapabilityProjection>();

  telemetryArray(unit, 'current_stage_recent_capabilities').forEach((capability, index) => {
    const id = textField(capability, 'capability_invocation_id') || `${unit.id}-capability-${index}`;
    const nextState = textField(capability, 'status') || 'event';
    const existing = byInvocation.get(id);
    const existingRank = terminalRank[normalize(existing?.state)] ?? 0;
    const nextRank = terminalRank[normalize(nextState)] ?? 0;
    if (existing && existingRank > nextRank) return;

    byInvocation.set(id, {
      id,
      label: titleCase(textField(capability, 'capability') || existing?.label || 'capability'),
      state: nextState,
      message: textField(capability, 'message') || existing?.message || 'No message',
      step_id: textField(capability, 'step_id') || existing?.step_id || currentStepId(unit),
      stage_execution_id: textField(capability, 'stage_execution_id') || existing?.stage_execution_id || null,
      capability_invocation_id: textField(capability, 'capability_invocation_id') || existing?.capability_invocation_id || null,
    });
  });

  return [...byInvocation.values()].slice(0, 4);
}

function StageRail(props: { stages: StageProjection[] }) {
  const { stages } = props;
  return (
    <Group gap={0} align="stretch" wrap="nowrap" style={{ overflowX: 'auto', paddingBottom: 4 }}>
      {stages.map((stage, index) => (
        <Group key={stage.key} gap={0} align="center" wrap="nowrap" style={{ flex: '0 0 auto' }}>
          <Tooltip label={stage.message || titleCase(stage.state)} disabled={!stage.message}>
            <Paper
              withBorder
              radius="lg"
              p="sm"
              style={{
                minWidth: 116,
                borderColor: `var(--mantine-color-${tone(stage.state)}-5)`,
                background:
                  stage.state === 'active'
                    ? 'linear-gradient(135deg, rgba(34, 184, 207, 0.24), rgba(28, 126, 214, 0.18))'
                    : stage.state === 'complete'
                      ? 'linear-gradient(135deg, rgba(47, 158, 68, 0.2), rgba(47, 158, 68, 0.08))'
                      : stage.state === 'failed'
                        ? 'linear-gradient(135deg, rgba(250, 82, 82, 0.22), rgba(250, 82, 82, 0.08))'
                        : stage.state === 'waiting'
                          ? 'linear-gradient(135deg, rgba(250, 176, 5, 0.24), rgba(250, 176, 5, 0.08))'
                          : undefined,
              }}
            >
              <Stack gap={4}>
                <Group justify="space-between" gap="xs">
                  <Badge size="sm" color={tone(stage.state)}>{index + 1}</Badge>
                  <Badge size="xs" color={tone(stage.state)} variant="light">{titleCase(stage.state)}</Badge>
                </Group>
                <Text fw={800} size="sm">{stage.label}</Text>
              </Stack>
            </Paper>
          </Tooltip>
          {index < stages.length - 1 ? (
            <Box
              style={{
                width: 28,
                height: 2,
                background: `linear-gradient(90deg, var(--mantine-color-${tone(stage.state)}-5), var(--mantine-color-gray-6))`,
              }}
            />
          ) : null}
        </Group>
      ))}
    </Group>
  );
}

function historyItemCapabilityLabel(item: WorkflowEventHistoryItem): string {
  const payload = item.payload ?? {};
  const raw = typeof payload.capability === 'string'
    ? payload.capability
    : typeof payload.capability_key === 'string'
      ? payload.capability_key
      : item.capability_invocation_id || '';
  return raw ? titleCase(raw) : '';
}

function eventPayloadText(item: WorkflowEventHistoryItem): string {
  try {
    return JSON.stringify(item.payload ?? {}, null, 2);
  } catch {
    return '{}';
  }
}

function EventHistoryModal(props: { anchor: EventHistoryAnchor | null; onClose: () => void }) {
  const [items, setItems] = useState<WorkflowEventHistoryItem[]>([]);
  const [cursor, setCursor] = useState<number | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<WorkflowEventHistoryItem | null>(null);
  const [stageFilter, setStageFilter] = useState('');
  const [capabilityFilter, setCapabilityFilter] = useState('');
  const [startFilter, setStartFilter] = useState('');
  const [endFilter, setEndFilter] = useState('');

  const baseQuery = useMemo<WorkflowEventHistoryQuery>(() => ({
    limit: 120,
    stage: stageFilter || props.anchor?.stage || null,
    capability: capabilityFilter || props.anchor?.capability || null,
    stage_execution_id: props.anchor?.stageExecutionId || null,
    capability_invocation_id: props.anchor?.capabilityInvocationId || null,
    start: startFilter || null,
    end: endFilter || null,
  }), [props.anchor, stageFilter, capabilityFilter, startFilter, endFilter]);

  async function loadInitial() {
    if (!props.anchor) return;
    setLoading(true);
    try {
      const response = await getWorkflowEventHistory(props.anchor.runId, baseQuery);
      setItems(response.items);
      setCursor(response.next_before_sequence ?? null);
      setHasMore(response.has_more);
      setSelected(response.items[response.items.length - 1] ?? null);
    } finally {
      setLoading(false);
    }
  }

  async function loadOlder() {
    if (!props.anchor || !hasMore || loading || cursor === null) return;
    setLoading(true);
    try {
      const response = await getWorkflowEventHistory(props.anchor.runId, { ...baseQuery, before_sequence: cursor });
      setItems((current) => [...response.items, ...current]);
      setCursor(response.next_before_sequence ?? null);
      setHasMore(response.has_more);
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    if (!props.anchor) return;
    void loadInitial();
  }, [props.anchor, baseQuery]);

  useEffect(() => {
    if (!props.anchor) return;
    const source = new EventSource(workflowEventHistoryStreamUrl(props.anchor.runId, baseQuery));
    source.addEventListener('workflow_event', (event) => {
      try {
        const item = JSON.parse((event as MessageEvent).data) as WorkflowEventHistoryItem;
        setItems((current) => current.some((existing) => existing.id === item.id) ? current : [...current, item]);
      } catch {}
    });
    return () => source.close();
  }, [props.anchor, baseQuery]);

  const grouped = useMemo(() => {
    const groups = new Map<string, WorkflowEventHistoryItem[]>();
    for (const item of items) {
      const key = `${item.step_id || 'workflow'}::${item.stage_execution_id || 'run'}`;
      groups.set(key, [...(groups.get(key) ?? []), item]);
    }
    return [...groups.entries()].map(([key, events]) => ({ key, events }));
  }, [items]);

  return (
    <Modal opened={Boolean(props.anchor)} onClose={props.onClose} title={props.anchor?.title ?? 'Event history'} size="90vw" centered>
      <Stack gap="sm">
        <SimpleGrid cols={{ base: 1, md: 4 }} spacing="xs">
          <TextInput label="Start" placeholder="2026-07-03T00:00:00Z" value={startFilter} onChange={(event) => setStartFilter(event.currentTarget.value)} />
          <TextInput label="End" placeholder="2026-07-03T23:59:59Z" value={endFilter} onChange={(event) => setEndFilter(event.currentTarget.value)} />
          <TextInput label="Stage" placeholder="code" value={stageFilter} onChange={(event) => setStageFilter(event.currentTarget.value)} />
          <TextInput label="Capability" placeholder="changeset" value={capabilityFilter} onChange={(event) => setCapabilityFilter(event.currentTarget.value)} />
        </SimpleGrid>
        <SimpleGrid cols={{ base: 1, xl: 2 }} spacing="sm">
          <ScrollArea h={520} onScrollPositionChange={({ y }) => { if (y < 32) void loadOlder(); }}>
            <Stack gap="xs" pr="xs">
              {loading ? <Loader size="sm" /> : null}
              {grouped.map((group) => {
                const first = group.events[0];
                return (
                  <Paper key={group.key} withBorder radius="md" p="sm" style={{ background: 'rgba(255,255,255,0.025)' }}>
                    <Stack gap="xs">
                      <Group gap="xs" wrap="nowrap">
                        <Badge color={tone(first.level)} size="sm">{titleCase(first.step_id || 'workflow')}</Badge>
                        <Text fw={800} size="sm" truncate>{first.stage_execution_id || 'workflow events'}</Text>
                      </Group>
                      {group.events.map((item) => {
                        const capabilityLabel = historyItemCapabilityLabel(item);
                        return (
                          <Paper
                            key={item.id}
                            withBorder
                            radius="sm"
                            p="xs"
                            onClick={() => setSelected(item)}
                            style={{
                              cursor: 'pointer',
                              marginLeft: item.capability_invocation_id ? 28 : 0,
                              borderColor: selected?.id === item.id ? `var(--mantine-color-${tone(item.level)}-5)` : undefined,
                              background: selected?.id === item.id ? 'rgba(34,184,207,0.08)' : 'rgba(255,255,255,0.018)',
                            }}
                          >
                            <Group justify="space-between" gap="xs" wrap="nowrap">
                              <Group gap="xs" wrap="nowrap" style={{ minWidth: 0 }}>
                                <Badge color={tone(item.level)} size="xs">{titleCase(item.level)}</Badge>
                                {capabilityLabel ? <Badge color="gray" variant="light" size="xs">{capabilityLabel}</Badge> : null}
                                <Text fw={700} size="sm" truncate>{titleCase(item.kind)}</Text>
                              </Group>
                              <Text size="xs" c="dimmed" style={{ flex: '0 0 auto' }}>{new Date(item.created_at).toLocaleTimeString()}</Text>
                            </Group>
                            <Text size="xs" c="dimmed" lineClamp={2} mt={3}>{item.message}</Text>
                          </Paper>
                        );
                      })}
                    </Stack>
                  </Paper>
                );
              })}
            </Stack>
          </ScrollArea>
          <JsonInput label="Event payload" value={selected ? eventPayloadText(selected) : '{}'} autosize minRows={22} maxRows={22} readOnly />
        </SimpleGrid>
      </Stack>
    </Modal>
  );
}

function executionStatusFromEvent(item: WorkflowEventHistoryItem): string {
  if (item.level === 'error') return 'failed';
  if (item.kind.endsWith('_failed')) return 'failed';
  if (item.kind.endsWith('_completed') || item.kind === 'stage_executed') return 'success';
  if (item.kind.includes('waiting')) return 'waiting';
  return item.level || 'event';
}

function deriveWorkflowCardHistoryHydration(unit: FlightDeckWorkUnit, items: WorkflowEventHistoryItem[]): WorkflowCardHistoryHydration {
  const capabilityByKey = new Map<string, CapabilityProjection>();
  const stageByKey = new Map<string, Record<string, unknown>>();

  for (const item of [...items].reverse()) {
    if (item.capability_invocation_id) {
      const label = historyItemCapabilityLabel(item) || 'Capability';
      capabilityByKey.set(item.capability_invocation_id, {
        id: item.capability_invocation_id,
        label,
        state: executionStatusFromEvent(item),
        message: item.message || titleCase(item.kind),
        step_id: item.step_id || currentStepId(unit),
        stage_execution_id: item.stage_execution_id || null,
        capability_invocation_id: item.capability_invocation_id,
      });
      continue;
    }

    if (item.stage_execution_id || item.step_id) {
      const key = item.stage_execution_id || `${item.step_id || 'workflow'}-${item.sequence_no}`;
      stageByKey.set(key, {
        id: item.id,
        status: executionStatusFromEvent(item),
        step_id: item.step_id || currentStepId(unit) || 'workflow',
        stage_execution_id: item.stage_execution_id || null,
        message: item.message || titleCase(item.kind),
        created_at: item.created_at,
      });
    }
  }

  return {
    capabilities: [...capabilityByKey.values()].slice(0, 4),
    stages: [...stageByKey.values()].slice(0, 4),
  };
}

function CapabilityStrip(props: { capabilities: CapabilityProjection[]; onOpenHistory?: (capability: CapabilityProjection) => void }) {
  const { capabilities } = props;
  if (capabilities.length === 0) {
    return (
      <Paper withBorder radius="md" p="md" style={{ background: 'rgba(255,255,255,0.025)' }}>
        <Text size="sm" c="dimmed">No capability invocations captured for the active stage.</Text>
      </Paper>
    );
  }

  return (
    <Group gap="sm" wrap="nowrap" style={{ overflowX: 'auto', paddingBottom: 4 }}>
      {capabilities.map((capability) => (
        <Paper
          key={capability.id}
          withBorder
          radius="lg"
          p="sm"
          onClick={() => props.onOpenHistory?.(capability)}
          style={{
            minWidth: 210,
            cursor: props.onOpenHistory ? 'pointer' : undefined,
            borderColor: `var(--mantine-color-${tone(capability.state)}-5)`,
            background: 'rgba(255,255,255,0.035)',
          }}
        >
          <Stack gap={4}>
            <Group justify="space-between" gap="xs">
              <Badge size="xs" color={tone(capability.state)}>{titleCase(capability.state)}</Badge>
              <Text size="xs" c="dimmed">{capability.id.slice(0, 8)}</Text>
            </Group>
            <Text fw={800} size="sm" truncate>{capability.label}</Text>
            <Text size="xs" c="dimmed" lineClamp={2}>{capability.message}</Text>
          </Stack>
        </Paper>
      ))}
    </Group>
  );
}

function RecentStageStrip(props: { unit: FlightDeckWorkUnit; fallbackStages?: Record<string, unknown>[]; onOpenHistory?: (stage: Record<string, unknown>) => void }) {
  const activeStepKey = currentStepId(props.unit);
  const nativeStages = telemetryArray(props.unit, 'recent_stage_executions');
  const sourceStages = nativeStages.length > 0 ? nativeStages : props.fallbackStages ?? [];
  const stages = sourceStages
    .map((stage, index) => ({ stage, index }))
    .sort((a, b) => {
      const aStatus = normalize(textField(a.stage, 'status'));
      const bStatus = normalize(textField(b.stage, 'status'));
      const aStepId = textField(a.stage, 'step_id');
      const bStepId = textField(b.stage, 'step_id');
      const aActive = activeStepKey ? stageKeyFromText(aStepId) === stageKeyFromText(activeStepKey) : false;
      const bActive = activeStepKey ? stageKeyFromText(bStepId) === stageKeyFromText(activeStepKey) : false;
      const aFailed = aStatus === 'failed' ? 1 : 0;
      const bFailed = bStatus === 'failed' ? 1 : 0;
      if (aActive !== bActive) return aActive ? -1 : 1;
      if (aFailed !== bFailed) return bFailed - aFailed;
      return a.index - b.index;
    })
    .slice(0, 4);

  if (stages.length === 0) {
    return <Text size="sm" c="dimmed">No previous stage executions yet.</Text>;
  }

  return (
    <Stack gap="xs">
      {stages.map(({ stage, index }) => {
        const status = textField(stage, 'status') || 'event';
        const stepId = textField(stage, 'step_id') || 'stage';
        const message = textField(stage, 'message') || 'No message';
        const createdAt = textField(stage, 'created_at');
        const isCurrentStep = activeStepKey ? stageKeyFromText(stepId) === stageKeyFromText(activeStepKey) : false;
        return (
          <Paper
            key={`${stepId}-${index}-${createdAt}`}
            withBorder
            radius="md"
            p="sm"
            onClick={() => props.onOpenHistory?.(stage)}
            style={{
              cursor: props.onOpenHistory ? 'pointer' : undefined,
              borderColor: isCurrentStep ? `var(--mantine-color-${tone(status)}-5)` : undefined,
              background: isCurrentStep ? 'rgba(34,184,207,0.08)' : 'rgba(255,255,255,0.025)',
            }}
          >
            <Group justify="space-between" gap="xs" wrap="nowrap">
              <Group gap="xs" wrap="nowrap" style={{ minWidth: 0 }}>
                <Badge color={tone(status)} size="sm">{titleCase(status)}</Badge>
                <Text fw={isCurrentStep ? 900 : 700} size="sm" truncate>{titleCase(stepId)}</Text>
                {isCurrentStep ? <Badge color="cyan" variant="light" size="xs">Current workflow</Badge> : null}
              </Group>
              <Text size="xs" c="dimmed" style={{ flex: '0 0 auto' }}>{createdAt ? new Date(createdAt).toLocaleTimeString() : ''}</Text>
            </Group>
            <Text size="xs" c="dimmed" lineClamp={2} mt={4}>{message}</Text>
          </Paper>
        );
      })}
    </Stack>
  );
}

function workflowType(unit: FlightDeckWorkUnit): string {
  return unit.workflow_type ?? unit.kind;
}

function manualShardIsIntegrationInput(unit: FlightDeckWorkUnit): boolean {
  return workflowType(unit) === 'manual_shard' && isIntegrationReadyState(unit.state);
}

function manualShardHasStagedChanges(unit: FlightDeckWorkUnit): boolean {
  return unit.telemetry?.manual_shard_has_staged_changes === true || unit.telemetry?.manual_shard_stageable === true;
}

function workflowIsProcessing(unit: FlightDeckWorkUnit): boolean {
  return unit.state === 'running' || unit.state === 'integrating';
}

function workflowCanRun(unit: FlightDeckWorkUnit): boolean {
  if (workflowType(unit) === 'integration') return !unit.workflow_deleted && !workflowIsProcessing(unit);
  return !manualShardIsIntegrationInput(unit) && !unit.workflow_deleted && !workflowIsProcessing(unit) && Boolean(unit.workflow_run_id && unit.feature_id);
}

function workflowCanPause(unit: FlightDeckWorkUnit): boolean {
  if (workflowType(unit) === 'integration') return !unit.workflow_deleted && workflowIsProcessing(unit);
  return !unit.workflow_deleted && workflowIsProcessing(unit) && Boolean(unit.workflow_run_id && unit.feature_id);
}

function workflowCanRegenerate(unit: FlightDeckWorkUnit): boolean {
  if (workflowType(unit) === 'integration') return !unit.workflow_deleted && Boolean(unit.workflow_run_id);
  return workflowType(unit) === 'feature_development' && !unit.workflow_deleted && !workflowIsProcessing(unit) && Boolean(unit.feature_id);
}

function workflowCanDelete(unit: FlightDeckWorkUnit): boolean {
  const type = workflowType(unit);
  if (type === 'manual_shard') return !manualShardIsIntegrationInput(unit) && !unit.workflow_deleted && Boolean(unit.feature_id);
  if (type === 'refine') return !unit.workflow_deleted && Boolean(unit.feature_id);
  if (type === 'feature_development') return !unit.workflow_deleted && Boolean(unit.feature_id);
  return false;
}

function workflowCanStageManual(unit: FlightDeckWorkUnit): boolean {
  return workflowType(unit) === 'manual_shard' && !unit.workflow_deleted && Boolean(unit.feature_id) && !manualShardIsIntegrationInput(unit) && manualShardHasStagedChanges(unit);
}

function workflowCanUnstageManual(unit: FlightDeckWorkUnit): boolean {
  return workflowType(unit) === 'manual_shard' && !unit.workflow_deleted && Boolean(unit.feature_id) && manualShardIsIntegrationInput(unit);
}

function WorkflowProjectionCard(props: {
  unit: FlightDeckWorkUnit;
  supervisor: FlightDeckSupervisor;
  compact?: boolean;
  navigate?: (path: string) => void;
  onActionComplete?: () => void;
}) {
  const { unit, navigate } = props;
  const stages = buildStageProjection(unit);
  const capabilities = buildCapabilityProjection(unit);
  const recentStageExecutions = telemetryArray(unit, 'recent_stage_executions');
  const [historyAnchor, setHistoryAnchor] = useState<EventHistoryAnchor | null>(null);
  const [historyHydration, setHistoryHydration] = useState<WorkflowCardHistoryHydration | null>(null);
  const [hydratedRunId, setHydratedRunId] = useState<string | null>(null);
  const openCapabilityHistory = (capability: CapabilityProjection) => {
    if (!unit.workflow_run_id) return;
    setHistoryAnchor({
      title: `Capability execution · ${capability.label}`,
      runId: unit.workflow_run_id,
      stage: capability.step_id ?? null,
      capability: capability.label,
      stageExecutionId: capability.stage_execution_id ?? null,
      capabilityInvocationId: capability.capability_invocation_id ?? null,
    });
  };
  const openStageHistory = (stage: Record<string, unknown>) => {
    if (!unit.workflow_run_id) return;
    const stepId = textField(stage, 'step_id') || null;
    setHistoryAnchor({
      title: `Stage execution · ${titleCase(stepId || 'stage')}`,
      runId: unit.workflow_run_id,
      stage: stepId,
      stageExecutionId: textField(stage, 'stage_execution_id') || null,
      capability: null,
      capabilityInvocationId: null,
    });
  };

  useEffect(() => {
    const runId = unit.workflow_run_id;
    if (!runId) return;
    if (capabilities.length > 0 || recentStageExecutions.length > 0) return;
    if (hydratedRunId === runId) return;

    let cancelled = false;
    setHydratedRunId(runId);
    void getWorkflowEventHistory(runId, { limit: 10 })
      .then((response) => {
        if (cancelled) return;
        setHistoryHydration(deriveWorkflowCardHistoryHydration(unit, response.items));
      })
      .catch(() => {
        if (cancelled) return;
        setHistoryHydration({ capabilities: [], stages: [] });
      });

    return () => {
      cancelled = true;
    };
  }, [unit.workflow_run_id, capabilities.length, recentStageExecutions.length, hydratedRunId]);

  const displayCapabilities = capabilities.length > 0 ? capabilities : historyHydration?.capabilities ?? [];
  const displayFallbackStages = recentStageExecutions.length > 0 ? [] : historyHydration?.stages ?? [];
  const canApplyFinalPatch = workflowType(unit) === 'integration'
    && !unit.workflow_deleted
    && Boolean(unit.workflow_run_id)
    && props.supervisor.integration_run_id === unit.workflow_run_id
    && normalize(props.supervisor.status) === 'ready_to_apply';


  const title = unit.workflow_run_id ? (
    <Anchor
      fw={900}
      size="md"
      c="gray.0"
      href={workflowHref(unit.workflow_run_id)}
      onClick={(event) => {
        if (!navigate || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) return;
        event.preventDefault();
        navigate(workflowHref(unit.workflow_run_id!));
      }}
      style={{ minWidth: 0 }}
    >
      <Text span inherit lineClamp={1}>{unit.title}</Text>
    </Anchor>
  ) : (
    <Title order={5} lineClamp={1}>{unit.title}</Title>
  );

  async function runWorkflowAction(action: 'start_child_workflow' | 'pause_child_workflow' | 'regenerate_queue_feature' | 'dequeue_feature' | 'delete_manual_shard' | 'delete_refine_workflow' | 'stage_manual_shard' | 'unstage_manual_shard' | 'start_integration' | 'restart_integration' | 'apply' | 'cancel') {
    if (workflowType(unit) !== 'integration' && !unit.feature_id) return;
    if (action === 'regenerate_queue_feature') {
      const confirmed = window.confirm(`Delete existing workflow/shard state for ${unit.title} and return it to queued draft state?`);
      if (!confirmed) return;
      await regenerateSupervisorQueueFeature(props.supervisor.id, unit.feature_id!);
      props.onActionComplete?.();
      return;
    }
    if (action === 'dequeue_feature') {
      const deleteWorkflow = window.confirm(`Remove ${unit.title} from the feature queue?\n\nOK: delete workflow/development state.\nCancel: keep workflow/development state and only dequeue.`);
      await unscheduleSupervisorFeature(props.supervisor.id, unit.feature_id!, deleteWorkflow ? 'delete_development' : 'preserve_development');
      props.onActionComplete?.();
      return;
    }
    if (action === 'restart_integration') {
      const confirmed = window.confirm(`Delete and regenerate integration workflow for ${unit.title}?`);
      if (!confirmed) return;
    }
    if (action === 'apply') {
      const confirmed = window.confirm('Apply the final integration patch to the root repository?');
      if (!confirmed) return;
    }
    if (action === 'cancel') {
      const confirmed = window.confirm(`Pause integration workflow ${unit.title}?`);
      if (!confirmed) return;
    }
    if (action === 'delete_manual_shard') {
      const confirmed = window.confirm(`Delete manual shard ${unit.title}? This will not regenerate it.`);
      if (!confirmed) return;
    }
    if (action === 'delete_refine_workflow') {
      const confirmed = window.confirm(`Delete refine workflow ${unit.title}? This removes it from the refine pool.`);
      if (!confirmed) return;
    }
    if (action === 'stage_manual_shard') {
      const confirmed = window.confirm(`Stage manual shard ${unit.title} for integration? Backend validation requires staged git changes in the shard.`);
      if (!confirmed) return;
    }
    if (action === 'unstage_manual_shard') {
      const confirmed = window.confirm(`Unstage manual shard ${unit.title} from integration?`);
      if (!confirmed) return;
    }
    await runSupervisorAction(props.supervisor.id, action, workflowType(unit) === 'integration' ? {} : action === 'delete_manual_shard' || action === 'stage_manual_shard' || action === 'unstage_manual_shard' ? { manual_shard_id: unit.feature_id } : { feature_id: unit.feature_id });
    props.onActionComplete?.();
  }

  return (
    <Card
      withBorder
      radius="lg"
      p="sm"
      style={{
        background: 'linear-gradient(135deg, rgba(255,255,255,0.045), rgba(255,255,255,0.018))',
        borderColor: `var(--mantine-color-${tone(unit.state)}-5)`,
        marginLeft: props.compact ? 16 : 0,
      }}
    >
      <Stack gap="sm">
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'max-content minmax(260px, 0.9fr) minmax(420px, 1.7fr)',
            gap: 'var(--mantine-spacing-sm)',
            alignItems: 'start',
            overflowX: 'auto',
          }}
        >
          <Stack gap={6} style={{ minWidth: 0 }}>
            <Group justify="space-between" align="center" gap="xs" wrap="nowrap">
              <Group gap="xs" wrap="nowrap" style={{ minWidth: 0 }}>
                <Badge color={tone(unit.state)}>{titleCase(unit.state)}</Badge>
                {title}
                {unit.workflow_deleted ? <Badge color="red" variant="outline">Deleted</Badge> : null}
                {unit.patch_id ? <Badge color="violet" variant="light">Patch {unit.patch_id.slice(0, 8)}</Badge> : null}
              </Group>
              <Group gap={6} wrap="nowrap">
                {workflowCanUnstageManual(unit) ? (
                  <Button size="compact-xs" color="yellow" variant="outline" onClick={() => void runWorkflowAction('unstage_manual_shard')}>Unstage</Button>
                ) : null}
                {workflowCanPause(unit) ? (
                  <Button size="compact-xs" variant="default" onClick={() => void runWorkflowAction(workflowType(unit) === 'integration' ? 'cancel' : 'pause_child_workflow')}>Pause</Button>
                ) : workflowCanRun(unit) ? (
                  <Button size="compact-xs" variant="default" onClick={() => void runWorkflowAction(workflowType(unit) === 'integration' ? 'start_integration' : 'start_child_workflow')}>Run</Button>
                ) : null}
                {workflowCanRegenerate(unit) ? (
                  <Button size="compact-xs" color="yellow" variant="outline" onClick={() => void runWorkflowAction(workflowType(unit) === 'integration' ? 'restart_integration' : 'regenerate_queue_feature')}>Regenerate</Button>
                ) : null}
                {workflowCanStageManual(unit) ? (
                  <Button size="compact-xs" color="green" variant="outline" onClick={() => void runWorkflowAction('stage_manual_shard')}>Stage to integration</Button>
                ) : null}
                {workflowCanDelete(unit) ? (
                  <Button size="compact-xs" color="red" variant="outline" onClick={() => void runWorkflowAction(workflowType(unit) === 'refine' ? 'delete_refine_workflow' : workflowType(unit) === 'feature_development' ? 'dequeue_feature' : 'delete_manual_shard')}>{workflowType(unit) === 'feature_development' ? 'Dequeue' : 'Delete'}</Button>
                ) : null}
              </Group>
            </Group>
            <Group gap="xs" wrap="nowrap">
              <Text fw={800} size="sm">Workflow stages</Text>
              <Badge variant="light" size="xs">Progression</Badge>
            </Group>
            <StageRail stages={stages} />
            {canApplyFinalPatch ? (
              <Group justify="center" mt="sm">
                <Button
                  size="md"
                  color="green"
                  variant="filled"
                  onClick={() => void runWorkflowAction('apply')}
                  style={{ minWidth: 260 }}
                >
                  Apply final patch to root
                </Button>
              </Group>
            ) : null}
          </Stack>
          <Stack gap={6} style={{ minWidth: 0 }}>
            <Group justify="space-between">
              <Text fw={800} size="sm">Capability execution</Text>
              <Badge variant="light" size="xs">Last 4</Badge>
            </Group>
            <CapabilityStrip capabilities={displayCapabilities} onOpenHistory={openCapabilityHistory} />
          </Stack>
          <Stack gap={6} style={{ minWidth: 0 }}>
            <Group justify="space-between">
              <Text fw={800} size="sm">Stage execution</Text>
              <Badge variant="light" size="xs">Last 4</Badge>
            </Group>
            <RecentStageStrip unit={unit} fallbackStages={displayFallbackStages} onOpenHistory={openStageHistory} />
          </Stack>
        </div>

        <EventHistoryModal anchor={historyAnchor} onClose={() => setHistoryAnchor(null)} />

        {unit.alerts.length > 0 ? (
          <Stack gap="xs">
            {unit.alerts.map((alert) => (
              <Alert key={alert.id} color={alert.level === 'error' ? 'red' : 'yellow'} variant="light">
                {alert.message}
              </Alert>
            ))}
          </Stack>
        ) : null}
      </Stack>
    </Card>
  );
}

function PoolSelectControl(props: { label: string; ariaLabel: string; data: TemplateOption[]; defaultValue?: string | null; value?: string | null; onChange?: (value: string | null) => void; width: number; searchable?: boolean }) {
  return (
    <Paper withBorder radius="md" p={0} style={{ display: 'flex', alignItems: 'stretch', overflow: 'hidden', background: 'rgba(255,255,255,0.035)' }}>
      <Box px="xs" style={{ display: 'flex', alignItems: 'center', background: 'rgba(34,184,207,0.14)', borderRight: '1px solid rgba(34,184,207,0.28)' }}>
        <Text size="xs" fw={900} tt="uppercase" c="cyan.1" style={{ whiteSpace: 'nowrap' }}>{props.label}</Text>
      </Box>
      <Select
        size="xs"
        variant="unstyled"
        aria-label={props.ariaLabel}
        data={props.data}
        value={props.value ?? undefined}
        defaultValue={props.value === undefined ? props.defaultValue ?? undefined : undefined}
        onChange={props.onChange}
        searchable={props.searchable}
        allowDeselect={false}
        disabled={props.data.length === 0}
        w={props.width}
        styles={{ input: { minHeight: 28, paddingLeft: 10, paddingRight: 24, background: 'rgba(20,20,24,0.32)' } }}
      />
    </Paper>
  );
}

function PoolNumberControl(props: { label: string; ariaLabel: string; value: number; min: number; max: number; width: number; onChange?: (value: number) => void }) {
  return (
    <Paper withBorder radius="md" p={0} style={{ display: 'flex', alignItems: 'stretch', overflow: 'hidden', background: 'rgba(255,255,255,0.035)' }}>
      <Box px="xs" style={{ display: 'flex', alignItems: 'center', background: 'rgba(34,184,207,0.14)', borderRight: '1px solid rgba(34,184,207,0.28)' }}>
        <Text size="xs" fw={900} tt="uppercase" c="cyan.1" style={{ whiteSpace: 'nowrap' }}>{props.label}</Text>
      </Box>
      <NumberInput
        size="xs"
        variant="unstyled"
        aria-label={props.ariaLabel}
        value={props.value}
        min={props.min}
        max={props.max}
        onChange={(value) => props.onChange?.(typeof value === 'number' ? value : props.min)}
        w={props.width}
        styles={{ input: { minHeight: 28, paddingLeft: 10, background: 'rgba(20,20,24,0.32)' } }}
      />
    </Paper>
  );
}

function PoolControls(props: { groupKey: string; templateOptions: TemplateOption[]; settings: FlightDeckPoolSetting; onSettingsChange: (patch: FlightDeckPoolSetting) => void }) {
  const defaultTemplate = props.templateOptions[0]?.value ?? null;
  const templateValue = props.settings.template_id ?? defaultTemplate;
  const modeValue = props.settings.mode ?? (props.groupKey === 'integration' ? 'manual' : 'series');
  const concurrencyValue = Math.max(1, Math.min(64, props.settings.concurrency ?? 1));

  if (props.groupKey === 'refine') {
    return (
      <Group gap="xs" align="center" wrap="nowrap">
        <PoolSelectControl label="Template" ariaLabel="Refine template" data={props.templateOptions} value={templateValue} onChange={(value) => props.onSettingsChange({ template_id: value })} width={160} searchable />
      </Group>
    );
  }

  if (props.groupKey === 'feature_development') {
    return (
      <Group gap="xs" align="center" wrap="nowrap">
        <PoolSelectControl label="Template" ariaLabel="Feature template" data={props.templateOptions} value={templateValue} onChange={(value) => props.onSettingsChange({ template_id: value })} width={160} searchable />
        <PoolSelectControl label="Mode" ariaLabel="Feature mode" data={FEATURE_MODE_OPTIONS} value={modeValue} onChange={(value) => props.onSettingsChange({ mode: value ?? 'series' })} width={110} />
        <PoolNumberControl label="Concurrency" ariaLabel="Feature concurrency" value={concurrencyValue} min={1} max={64} width={58} onChange={(value) => props.onSettingsChange({ concurrency: value })} />
      </Group>
    );
  }

  if (props.groupKey === 'manual_shard') {
    return (
      <Group gap="xs" align="center" wrap="nowrap">
        <PoolSelectControl label="Template" ariaLabel="Manual template" data={props.templateOptions} value={templateValue} onChange={(value) => props.onSettingsChange({ template_id: value })} width={160} searchable />
      </Group>
    );
  }

  if (props.groupKey === 'integration') {
    return (
      <Group gap="xs" align="center" wrap="nowrap">
        <PoolSelectControl label="Template" ariaLabel="Integration template" data={props.templateOptions} value={templateValue} onChange={(value) => props.onSettingsChange({ template_id: value })} width={190} searchable />
        <PoolSelectControl label="Mode" ariaLabel="Integration mode" data={INTEGRATION_MODE_OPTIONS} value={modeValue} onChange={(value) => props.onSettingsChange({ mode: value ?? 'manual' })} width={210} />
      </Group>
    );
  }

  return null;
}

function isIntegrationReadyState(state: string | null | undefined): boolean {
  return ['patch_ready', 'ready_for_integration', 'integrating', 'integrated'].includes(normalize(state));
}

function unitHasPatch(unit: FlightDeckWorkUnit): boolean {
  return Boolean(unit.patch_id || unit.telemetry?.patch_id || unit.telemetry?.current_patch_id);
}

function manualShardIsStaged(unit: FlightDeckWorkUnit): boolean {
  return unitHasPatch(unit) || isIntegrationReadyState(unit.state);
}

function featureIsIntegrationReady(unit: FlightDeckWorkUnit): boolean {
  return unitHasPatch(unit) || isIntegrationReadyState(unit.state);
}

function integrationInputIsSkipped(unit: FlightDeckWorkUnit): boolean {
  return unit.telemetry?.integration_skipped === true;
}

function integrationReadinessModel(supervisor: FlightDeckSupervisor) {
  const features = supervisor.work_units.filter((unit) => (unit.workflow_type ?? unit.kind) === 'feature_development' && !unit.workflow_deleted);
  const manual = supervisor.work_units.filter((unit) => (unit.workflow_type ?? unit.kind) === 'manual_shard' && !unit.workflow_deleted);
  const manualStaged = manual.filter(manualShardIsStaged);
  const manualUnstaged = manual.filter((unit) => !manualShardIsStaged(unit));
  const skippedFeatures = features.filter(integrationInputIsSkipped);
  const includedFeatures = features.filter((unit) => !integrationInputIsSkipped(unit));
  const skippedManualStaged = manualStaged.filter(integrationInputIsSkipped);
  const includedManualStaged = manualStaged.filter((unit) => !integrationInputIsSkipped(unit));
  const readyFeatures = includedFeatures.filter(featureIsIntegrationReady);
  const pendingFeatures = includedFeatures.filter((unit) => !featureIsIntegrationReady(unit));
  const blockedFeatures = includedFeatures.filter((unit) => ['blocked', 'failed'].includes(normalize(unit.state)));
  const skippedTotal = skippedFeatures.length + skippedManualStaged.length;
  const relevantTotal = includedFeatures.length + includedManualStaged.length;
  const readyTotal = readyFeatures.length + includedManualStaged.length;
  const readyPct = relevantTotal > 0 ? Math.round((readyTotal / relevantTotal) * 100) : 0;

  return {
    features,
    manual,
    manualStaged,
    manualUnstaged,
    skippedFeatures,
    includedFeatures,
    skippedManualStaged,
    includedManualStaged,
    readyFeatures,
    pendingFeatures,
    blockedFeatures,
    skippedTotal,
    relevantTotal,
    readyTotal,
    readyPct,
  };
}

function IntegrationReadinessBar(props: { supervisor: FlightDeckSupervisor; onActionComplete?: () => void; expanded?: boolean }) {
  const [opened, setOpened] = useState(false);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const model = integrationReadinessModel(props.supervisor);
  const readyWidth = model.relevantTotal > 0 ? `${Math.max(0, Math.min(100, (model.readyTotal / model.relevantTotal) * 100))}%` : '0%';
  const pendingWidth = model.relevantTotal > 0 ? `${Math.max(0, Math.min(100, (model.pendingFeatures.length / model.relevantTotal) * 100))}%` : '0%';
  const blockedWidth = model.relevantTotal > 0 ? `${Math.max(0, Math.min(100, (model.blockedFeatures.length / model.relevantTotal) * 100))}%` : '0%';

  async function skipFeature(unit: FlightDeckWorkUnit) {
    if (!unit.feature_id) return;
    const confirmed = window.confirm(`Skip ${unit.title} for this integration input set?`);
    if (!confirmed) return;
    setBusyKey(unit.id);
    try {
      await runSupervisorAction(props.supervisor.id, 'skip_integration_input', { feature_id: unit.feature_id });
      props.onActionComplete?.();
    } finally {
      setBusyKey(null);
    }
  }

  async function unskipFeature(unit: FlightDeckWorkUnit) {
    if (!unit.feature_id) return;
    setBusyKey(unit.id);
    try {
      await runSupervisorAction(props.supervisor.id, 'unskip_integration_input', { feature_id: unit.feature_id });
      props.onActionComplete?.();
    } finally {
      setBusyKey(null);
    }
  }

  async function skipManualShard(unit: FlightDeckWorkUnit) {
    if (!unit.feature_id) return;
    const confirmed = window.confirm(`Skip manual shard ${unit.title} for this integration input set?`);
    if (!confirmed) return;
    setBusyKey(unit.id);
    try {
      await runSupervisorAction(props.supervisor.id, 'skip_integration_input', { manual_shard_id: unit.feature_id });
      props.onActionComplete?.();
    } finally {
      setBusyKey(null);
    }
  }

  async function unskipManualShard(unit: FlightDeckWorkUnit) {
    if (!unit.feature_id) return;
    setBusyKey(unit.id);
    try {
      await runSupervisorAction(props.supervisor.id, 'unskip_integration_input', { manual_shard_id: unit.feature_id });
      props.onActionComplete?.();
    } finally {
      setBusyKey(null);
    }
  }

  function inputRow(unit: FlightDeckWorkUnit, bucket: 'ready_feature' | 'pending_feature' | 'blocked_feature' | 'skipped_feature' | 'staged_manual' | 'skipped_manual' | 'unstaged_manual') {
    const skipped = bucket === 'skipped_feature' || bucket === 'skipped_manual';
    const ready = bucket === 'ready_feature' || bucket === 'staged_manual' || bucket === 'skipped_manual';
    const blocked = bucket === 'blocked_feature';
    const manual = bucket === 'staged_manual' || bucket === 'skipped_manual' || bucket === 'unstaged_manual';
    const relevant = bucket !== 'unstaged_manual';
    const canSkip = Boolean(unit.feature_id) && relevant && !skipped;
    const canUnskip = Boolean(unit.feature_id) && skipped;
    return (
      <Paper key={`${bucket}-${unit.id}`} withBorder radius="md" p="sm" style={{ background: 'rgba(255,255,255,0.025)' }}>
        <Group justify="space-between" gap="sm" align="center" wrap="nowrap">
          <Stack gap={3} style={{ minWidth: 0, flex: '1 1 auto' }}>
            <Group gap="xs" wrap="nowrap" style={{ minWidth: 0 }}>
              <Badge size="xs" color={skipped ? 'gray' : ready ? 'green' : blocked ? 'red' : 'yellow'}>{skipped ? 'Skipped' : ready ? 'Ready' : blocked ? 'Blocked' : 'Pending'}</Badge>
              <Badge size="xs" color={manual ? 'violet' : 'cyan'} variant="light">{manual ? 'Manual shard' : 'Feature'}</Badge>
              <Badge size="xs" color={skipped ? 'gray' : relevant ? 'blue' : 'gray'} variant={skipped || !relevant ? 'outline' : 'light'}>{skipped ? 'Skipped' : relevant ? 'Integration input' : 'Not included'}</Badge>
              <Text fw={850} size="sm" truncate>{unit.title}</Text>
            </Group>
            <Group gap="xs" wrap="wrap">
              <Text size="xs" c="dimmed">State: {titleCase(unit.state)}</Text>
              {unit.patch_id ? <Text size="xs" c="dimmed">Patch: {unit.patch_id.slice(0, 8)}</Text> : null}
              {unit.workflow_run_id ? <Text size="xs" c="dimmed">Workflow: {unit.workflow_run_id.slice(0, 8)}</Text> : null}
              {unit.blocked_reason ? <Text size="xs" c="red.3">{unit.blocked_reason}</Text> : null}
            </Group>
          </Stack>
          {manual && canUnskip ? (
            <Button size="compact-xs" color="green" variant="outline" disabled={busyKey === unit.id} loading={busyKey === unit.id} onClick={() => void unskipManualShard(unit)}>
              Unskip shard
            </Button>
          ) : manual ? (
            <Button size="compact-xs" color="yellow" variant="outline" disabled={!canSkip || busyKey === unit.id} loading={busyKey === unit.id} onClick={() => void skipManualShard(unit)}>
              Skip shard
            </Button>
          ) : canUnskip ? (
            <Button size="compact-xs" color="green" variant="outline" disabled={busyKey === unit.id} loading={busyKey === unit.id} onClick={() => void unskipFeature(unit)}>
              Unskip feature
            </Button>
          ) : (
            <Button size="compact-xs" color="yellow" variant="outline" disabled={!canSkip || busyKey === unit.id} loading={busyKey === unit.id} onClick={() => void skipFeature(unit)}>
              Skip feature
            </Button>
          )}
        </Group>
      </Paper>
    );
  }

  return (
    <>
      <Stack gap={props.expanded ? 8 : 3} style={{ width: props.expanded ? '100%' : undefined, minWidth: props.expanded ? 0 : 280, maxWidth: props.expanded ? 'none' : 420, flex: props.expanded ? '1 1 auto' : '1 1 320px', cursor: 'pointer' }} onClick={() => setOpened(true)}>
        <Group justify="space-between" gap="xs" wrap="wrap">
          <Group gap="xs" wrap="wrap">
            <Text size="xs" fw={900} tt="uppercase" c="gray.2">Status</Text>
            <Badge size="xs" color="green" variant="light">Ready {model.readyTotal}</Badge>
            <Badge size="xs" color="yellow" variant="light">Pending {model.pendingFeatures.length}</Badge>
            <Badge size="xs" color="red" variant="light">Blocked {model.blockedFeatures.length}</Badge>
            <Badge size="xs" color="gray" variant="outline">Skipped {model.skippedTotal}</Badge>
            <Badge size="xs" color="gray" variant="outline">Unstaged manual {model.manualUnstaged.length}</Badge>
          </Group>
          <Text size="xs" c="gray.4">{model.readyPct}% ready</Text>
        </Group>
        <Paper withBorder radius="xl" p={2} style={{ height: props.expanded ? 20 : 12, width: '100%', overflow: 'hidden', background: 'rgba(255,255,255,0.05)' }}>
          <Group gap={0} wrap="nowrap" h="100%">
            <Box h="100%" style={{ width: readyWidth, background: 'var(--mantine-color-green-6)' }} />
            <Box h="100%" style={{ width: pendingWidth, background: 'var(--mantine-color-yellow-6)' }} />
            <Box h="100%" style={{ width: blockedWidth, background: 'var(--mantine-color-red-6)' }} />
          </Group>
        </Paper>
      </Stack>

      <Modal opened={opened} onClose={() => setOpened(false)} title="Integration input readiness" size="80vw" centered>
        <Stack gap="md">
          <Group gap="xs" wrap="wrap">
            <Badge color={model.readyTotal === model.relevantTotal && model.relevantTotal > 0 ? 'green' : 'yellow'}>{model.readyTotal}/{model.relevantTotal} ready</Badge>
            <Badge color="cyan" variant="light">Features {model.features.length}</Badge>
            <Badge color="violet" variant="light">Staged manual {model.manualStaged.length}</Badge>
            <Badge color="gray" variant="outline">Unstaged manual {model.manualUnstaged.length}</Badge>
          </Group>

          <SimpleGrid cols={{ base: 1, xl: 2 }} spacing="md">
            <Stack gap="xs">
              <Group gap="xs">
                <Text fw={900}>Feature pool</Text>
                <Badge color="green" variant="light">Ready {model.readyFeatures.length}</Badge>
                <Badge color="yellow" variant="light">Pending {model.pendingFeatures.length}</Badge>
                <Badge color="red" variant="light">Blocked {model.blockedFeatures.length}</Badge>
                <Badge color="gray" variant="outline">Skipped {model.skippedFeatures.length}</Badge>
              </Group>
              <ScrollArea h={420}>
                <Stack gap="xs" pr="xs">
                  {model.readyFeatures.map((unit) => inputRow(unit, 'ready_feature'))}
                  {model.pendingFeatures.map((unit) => inputRow(unit, 'pending_feature'))}
                  {model.blockedFeatures.map((unit) => inputRow(unit, 'blocked_feature'))}
                  {model.skippedFeatures.map((unit) => inputRow(unit, 'skipped_feature'))}
                  {model.features.length === 0 ? <Text size="sm" c="dimmed">No feature workflows are scheduled.</Text> : null}
                </Stack>
              </ScrollArea>
            </Stack>

            <Stack gap="xs">
              <Group gap="xs">
                <Text fw={900}>Manual pool</Text>
                <Badge color="violet" variant="light">Staged {model.includedManualStaged.length}</Badge>
                <Badge color="gray" variant="outline">Skipped {model.skippedManualStaged.length}</Badge>
              </Group>
              <ScrollArea h={420}>
                <Stack gap="xs" pr="xs">
                  {model.includedManualStaged.map((unit) => inputRow(unit, 'staged_manual'))}
                  {model.skippedManualStaged.map((unit) => inputRow(unit, 'skipped_manual'))}
                  {model.manualStaged.length === 0 ? <Text size="sm" c="dimmed">No manual shards are staged for integration.</Text> : null}
                </Stack>
              </ScrollArea>
            </Stack>
          </SimpleGrid>
        </Stack>
      </Modal>
    </>
  );
}

function ManualPoolWorkflowList(props: {
  units: FlightDeckWorkUnit[];
  supervisor: FlightDeckSupervisor;
  navigate?: (path: string) => void;
  onActionComplete?: () => void;
  pendingManualShardName?: string | null;
}) {
  const staged = props.units.filter(manualShardIsStaged);
  const unstaged = props.units.filter((unit) => !manualShardIsStaged(unit));

  return (
    <Stack gap="sm">
      {props.pendingManualShardName ? (
        <Paper withBorder radius="lg" p="sm" style={{ marginLeft: 16, borderColor: 'var(--mantine-color-yellow-5)', background: 'rgba(250,176,5,0.08)' }}>
          <Group gap="xs" wrap="nowrap">
            <Badge color="yellow">Creating</Badge>
            <Text fw={900} size="sm" truncate>{props.pendingManualShardName}</Text>
            <Text size="xs" c="dimmed">Creating manual shard workflow...</Text>
          </Group>
        </Paper>
      ) : null}
      {unstaged.length > 0 ? (
        <Stack gap="xs">
          <Group gap="xs">
            <Text fw={900} size="xs" tt="uppercase" c="gray.3">Unstaged manual workflows</Text>
            <Badge size="xs" color="gray" variant="outline">Not in integration input</Badge>
            <Badge size="xs" color="gray">{unstaged.length}</Badge>
          </Group>
          {unstaged.map((unit) => (
            <WorkflowProjectionCard key={unit.id} unit={unit} supervisor={props.supervisor} compact navigate={props.navigate} onActionComplete={props.onActionComplete} />
          ))}
        </Stack>
      ) : null}

      {staged.length > 0 ? (
        <Stack gap="xs">
          <Group gap="xs">
            <Text fw={900} size="xs" tt="uppercase" c="green.2">Staged manual workflows</Text>
            <Badge size="xs" color="green" variant="light">Relevant to integration</Badge>
            <Badge size="xs" color="green">{staged.length}</Badge>
          </Group>
          {staged.map((unit) => (
            <WorkflowProjectionCard key={unit.id} unit={unit} supervisor={props.supervisor} compact navigate={props.navigate} onActionComplete={props.onActionComplete} />
          ))}
        </Stack>
      ) : null}
    </Stack>
  );
}

function WorkPoolActionRail(props: { groupKey: string; units: FlightDeckWorkUnit[]; supervisor: FlightDeckSupervisor; templateOptions: TemplateOption[]; navigate?: (path: string) => void; onOpenPlanner?: (supervisor: FlightDeckSupervisor, options?: OpenPlannerOptions) => void; onActionComplete?: () => void; onManualShardCreating?: (name: string | null) => void }) {
  const childRunning = props.units.some((unit) => unit.state === 'running' || unit.state === 'integrating');
  const poolRunning = props.groupKey === 'feature_development' && props.supervisor.status === 'running_children';
  const poolPaused = props.groupKey === 'feature_development' && props.supervisor.status === 'paused';
  const running = props.groupKey === 'feature_development' ? poolRunning : childRunning;
  const queued = props.units.some((unit) => unit.state === 'queued');
  const waiting = props.units.some((unit) => unit.state === 'waiting_user');
  const settings = poolSetting(props.supervisor, props.groupKey);
  const selectedTemplate = settings.template_id ?? props.templateOptions[0]?.value ?? null;
  const [manualBusy, setManualBusy] = useState(false);
  const [manualNameOpen, setManualNameOpen] = useState(false);
  const [manualName, setManualName] = useState('');
  async function updateSettings(patch: FlightDeckPoolSetting) {
    const flightDeckSettings = nextFlightDeckSettings(props.supervisor, props.groupKey, patch);
    await runSupervisorAction(props.supervisor.id, 'update_flight_deck_settings', { flight_deck_settings: flightDeckSettings });
    props.onActionComplete?.();
  }
  const poolControls = <PoolControls groupKey={props.groupKey} templateOptions={props.templateOptions} settings={settings} onSettingsChange={(patch) => void updateSettings(patch)} />;

  async function createManualShard() {
    const name = manualName.trim() || `Manual shard ${props.units.length + 1}`;
    setManualBusy(true);
    props.onManualShardCreating?.(name);
    try {
      const payload: Record<string, unknown> = { title: name };
      if (selectedTemplate) payload.template_id = selectedTemplate;
      await runSupervisorAction(props.supervisor.id, 'create_manual_shard', payload);
      setManualName('');
      setManualNameOpen(false);
      props.onActionComplete?.();
    } finally {
      setManualBusy(false);
      props.onManualShardCreating?.(null);
    }
  }

  async function runFeaturePoolAction() {
    if (running) {
      await runSupervisorAction(props.supervisor.id, 'pause_feature_pool', {});
      props.onActionComplete?.();
      return;
    }

    await runSupervisorAction(props.supervisor.id, 'resume_feature_pool', {});
    const targets = props.units.filter((unit) => {
      if (!unit.feature_id || unit.workflow_deleted) return false;
      return unit.state === 'queued' || unit.state === 'waiting_user';
    });
    for (const unit of targets) {
      await runSupervisorAction(props.supervisor.id, 'start_child_workflow', { feature_id: unit.feature_id });
    }
    props.onActionComplete?.();
  }

  if (props.groupKey === 'integration') {
    const readiness = integrationReadinessModel(props.supervisor);
    const canRunIntegration = readiness.relevantTotal > 0 && readiness.readyTotal === readiness.relevantTotal && !running;
    return (
      <Group gap="xs" align="center" wrap="nowrap">
        {poolControls}
        <Button size="xs" variant="default" disabled={!canRunIntegration} onClick={() => void runSupervisorAction(props.supervisor.id, 'start_integration', {}).then(() => props.onActionComplete?.())}>Run integration</Button>
      </Group>
    );
  }

  if (props.groupKey === 'feature_development') {
    return (
      <Group gap="xs" align="center" wrap="nowrap">
        {poolControls}
        <Button
          size="xs"
          variant="light"
          onClick={() => props.onOpenPlanner?.(props.supervisor, { selectFeature: true })}
        >
          Manage queue
        </Button>
        <Button size="xs" variant="default" disabled={!running && !queued && !waiting && !poolPaused} onClick={() => void runFeaturePoolAction()}>
          {running ? 'Pause' : 'Run'}
        </Button>
      </Group>
    );
  }

  if (props.groupKey === 'refine') {
    return (
      <Group gap="xs" align="center" wrap="nowrap">
        {poolControls}
        <Button size="xs" variant="light" onClick={() => props.onOpenPlanner?.(props.supervisor, { refinementTemplateId: selectedTemplate })}>New fine</Button>
        <Button size="xs" variant="default" disabled={props.units.length === 0}>Promote refined feature</Button>
      </Group>
    );
  }

  if (props.groupKey === 'manual_shard') {
    return (
      <Group gap="xs" align="center" wrap="nowrap">
        {poolControls}
        {manualNameOpen ? (
          <TextInput
            size="xs"
            autoFocus
            placeholder="Shard name"
            value={manualName}
            onChange={(event) => setManualName(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') void createManualShard();
              if (event.key === 'Escape') {
                setManualName('');
                setManualNameOpen(false);
              }
            }}
            w={190}
          />
        ) : null}
        {manualNameOpen ? (
          <Button size="xs" variant="light" loading={manualBusy} onClick={() => void createManualShard()}>Create</Button>
        ) : (
          <Button size="xs" variant="light" onClick={() => setManualNameOpen(true)}>New shard</Button>
        )}
      </Group>
    );
  }

  return (
    <Group gap="xs">
      <Button size="xs" variant="default" disabled={props.units.length === 0}>Run</Button>
      <Button size="xs" variant="default" disabled={props.units.length === 0}>Pause</Button>
    </Group>
  );
}

function WorkPoolProjectionSection(props: {
  groupKey: string;
  title: string;
  description: string;
  empty: string;
  units: FlightDeckWorkUnit[];
  supervisor: FlightDeckSupervisor;
  navigate?: (path: string) => void;
  onOpenPlanner?: (supervisor: FlightDeckSupervisor, options?: OpenPlannerOptions) => void;
  onActionComplete?: () => void;
  templateOptions: TemplateOption[];
}) {
  const [pendingManualShardName, setPendingManualShardName] = useState<string | null>(null);

  return (
    <Box py="sm" style={{ borderTop: '1px solid rgba(255,255,255,0.10)' }}>
      <Stack gap="sm">
        <Stack
          gap={props.groupKey === 'integration' ? 'sm' : 0}
          px="xs"
          py={6}
          style={{
            background: 'linear-gradient(90deg, rgba(34,184,207,0.10), rgba(255,255,255,0.025), transparent)',
            borderLeft: '3px solid rgba(34,184,207,0.65)',
          }}
        >
          <Group justify="space-between" align="center" wrap="nowrap">
            <Group gap="md" align="center" wrap="nowrap" style={{ minWidth: 0, flex: '1 1 auto' }}>
              <Stack gap={1} style={{ minWidth: 240 }}>
                <Group gap="xs">
                  <Text fw={950} size="md" tt="uppercase" c="gray.0">{props.title}</Text>
                  <Badge color={props.units.length > 0 ? 'cyan' : 'gray'} variant="filled">{props.units.length}</Badge>
                  {props.groupKey === 'feature_development' ? <Badge color={tone(props.supervisor.status)} variant="light">{titleCase(props.supervisor.status)}</Badge> : null}
                </Group>
                {props.groupKey === 'integration' ? null : <Text size="xs" c="gray.4">{props.units.length > 0 ? props.description : props.empty}</Text>}
              </Stack>
            </Group>
            <WorkPoolActionRail groupKey={props.groupKey} units={props.units} supervisor={props.supervisor} templateOptions={props.templateOptions} navigate={props.navigate} onOpenPlanner={props.onOpenPlanner} onActionComplete={props.onActionComplete} onManualShardCreating={setPendingManualShardName} />
          </Group>

          {props.groupKey === 'integration' ? (
            <IntegrationReadinessBar supervisor={props.supervisor} onActionComplete={props.onActionComplete} expanded />
          ) : null}
        </Stack>

        {props.units.length > 0 ? (
          props.groupKey === 'manual_shard' ? (
            <ManualPoolWorkflowList units={props.units} supervisor={props.supervisor} navigate={props.navigate} onActionComplete={props.onActionComplete} pendingManualShardName={pendingManualShardName} />
          ) : (
            <Stack gap="sm">
              {props.units.map((unit) => (
                <WorkflowProjectionCard key={unit.id} unit={unit} supervisor={props.supervisor} compact navigate={props.navigate} onActionComplete={props.onActionComplete} />
              ))}
            </Stack>
          )
        ) : null}
      </Stack>
    </Box>
  );
}

function IntegrationProjectionSection(props: { supervisor: FlightDeckSupervisor; units: FlightDeckWorkUnit[]; templateOptions: TemplateOption[]; navigate?: (path: string) => void; onActionComplete?: () => void }) {
  return (
    <WorkPoolProjectionSection
      groupKey="integration"
      title="Integration pool"
      description="Integration work starts when feature patches are ready."
      empty="No integration work is currently active."
      units={props.units}
      supervisor={props.supervisor}
      templateOptions={props.templateOptions}
      navigate={props.navigate}
      onActionComplete={props.onActionComplete}
    />
  );
}

function TopologyMap(props: { supervisor: FlightDeckSupervisor; templateOptions: TemplateOption[]; navigate?: (path: string) => void; onOpenPlanner?: (supervisor: FlightDeckSupervisor, options?: OpenPlannerOptions) => void; onActionComplete?: () => void }) {
  const { supervisor, navigate } = props;
  const development = supervisor.work_units.filter((unit) => unit.kind !== 'integration');
  const integration = supervisor.work_units.filter((unit) => unit.kind === 'integration');

  return (
    <Box>
      <Stack gap="xs">
          {WORK_POOL_GROUPS.map((group) => (
            <WorkPoolProjectionSection
              key={group.key}
              groupKey={group.key}
              title={group.title}
              description={group.description}
              empty={group.empty}
              units={development.filter((unit) => (unit.workflow_type ?? unit.kind) === group.key)}
              supervisor={supervisor}
              templateOptions={props.templateOptions}
              navigate={navigate}
              onOpenPlanner={props.onOpenPlanner}
              onActionComplete={props.onActionComplete}
            />
          ))}

          <IntegrationProjectionSection supervisor={supervisor} units={integration} templateOptions={props.templateOptions} navigate={navigate} onActionComplete={props.onActionComplete} />
      </Stack>
    </Box>
  );
}

function SupervisorCockpit(props: { supervisor: FlightDeckSupervisor; templateOptions: TemplateOption[]; navigate?: (path: string) => void; onOpenPlanner?: (supervisor: FlightDeckSupervisor, options?: OpenPlannerOptions) => void; onActionComplete?: () => void }) {
  const { supervisor, navigate } = props;
  const waiting = supervisor.work_units.filter((unit) => unit.state === 'waiting_user').length;
  const failed = supervisor.work_units.filter((unit) => unit.state === 'failed' || unit.state === 'blocked').length;
  const ready = supervisor.work_units.filter((unit) => unit.state === 'patch_ready' || unit.state === 'ready_for_integration').length;
  const integrationState = String(supervisor.integration?.state ?? 'idle');

  return (
    <Card withBorder radius="xl" p="lg" style={{ background: 'linear-gradient(135deg, rgba(28,126,214,0.12), rgba(255,255,255,0.025))' }}>
      <Stack gap="sm">
        <Group justify="space-between" align="flex-start">
          <Stack gap={4}>
            <Group gap="xs" wrap="wrap">
              <Title order={2}>{supervisor.title}</Title>
              <Badge variant="outline">{supervisor.work_units.length} work units</Badge>
              <Badge color={waiting > 0 ? 'yellow' : 'gray'} variant="outline">{waiting} waiting</Badge>
              <Badge color={failed > 0 ? 'red' : 'gray'} variant="outline">{failed} blocked</Badge>
              <Badge color={ready > 0 ? 'cyan' : 'gray'} variant="outline">{ready} ready</Badge>
            </Group>
            <Text size="sm" c="dimmed">{compactPath(supervisor.root_repo_path)}</Text>
          </Stack>
          <Button
            variant="light"
            onClick={() => props.onOpenPlanner?.(supervisor)}
          >
            Planner options
          </Button>
        </Group>

        <TopologyMap supervisor={supervisor} templateOptions={props.templateOptions} navigate={navigate} onOpenPlanner={props.onOpenPlanner} onActionComplete={props.onActionComplete} />
      </Stack>
    </Card>
  );
}

function MissionBar(props: {
  deck: FlightDeckResponse | null;
  filtersOpen: boolean;
  supervisorFilter: string | null;
  stateFilter: string | null;
  kindFilter: string | null;
  includeDeleted: boolean;
  setFiltersOpen: (value: boolean) => void;
  setSupervisorFilter: (value: string | null) => void;
  setStateFilter: (value: string | null) => void;
  setKindFilter: (value: string | null) => void;
  setIncludeDeleted: (value: boolean) => void;
}) {
  const supervisorOptions = (props.deck?.supervisors ?? []).map((supervisor) => ({ value: supervisor.id, label: supervisor.title || supervisor.id.slice(0, 8) }));
  const stateOptions = ['queued', 'running', 'waiting_user', 'failed', 'patch_ready', 'ready_for_integration', 'integrating', 'integrated', 'deleted'].map((value) => ({ value, label: titleCase(value) }));
  const kindOptions = ['refine', 'feature_development', 'manual_shard', 'integration'].map((value) => ({ value, label: value === 'feature_development' ? 'Feature pool' : value === 'manual_shard' ? 'Manual pool' : `${titleCase(value)} pool` }));
  const alerts = props.deck?.alerts ?? [];

  return (
    <Card withBorder radius="xl" p="md" style={{ background: 'rgba(20,20,24,0.72)', backdropFilter: 'blur(14px)' }}>
      <Stack gap="md">
        <Group justify="space-between" align="center">
          <Group gap="xl">
            <Stack gap={0}>
              <Text size="xs" c="dimmed">Supervisors</Text>
              <Text fw={900}>{props.deck?.totals.supervisors ?? 0}</Text>
            </Stack>
            <Stack gap={0}>
              <Text size="xs" c="dimmed">Work units</Text>
              <Text fw={900}>{props.deck?.totals.work_units ?? 0}</Text>
            </Stack>
            <Stack gap={0}>
              <Text size="xs" c="dimmed">Waiting</Text>
              <Text fw={900}>{props.deck?.totals.waiting_user ?? 0}</Text>
            </Stack>
            <Stack gap={0}>
              <Text size="xs" c="dimmed">Failed</Text>
              <Text fw={900}>{props.deck?.totals.failed ?? 0}</Text>
            </Stack>
            <Stack gap={0}>
              <Text size="xs" c="dimmed">Ready integration</Text>
              <Text fw={900}>{props.deck?.totals.ready_for_integration ?? 0}</Text>
            </Stack>
            <Stack gap={0}>
              <Text size="xs" c="dimmed">Alerts</Text>
              <Text fw={900}>{alerts.length}</Text>
            </Stack>
          </Group>
          <Button variant="light" onClick={() => props.setFiltersOpen(!props.filtersOpen)}>{props.filtersOpen ? 'Hide filters' : 'Show filters'}</Button>
        </Group>

        {props.filtersOpen ? (
          <SimpleGrid cols={{ base: 1, md: 4 }} spacing="sm">
            <Select label="Supervisor" placeholder="All supervisors" data={supervisorOptions} value={props.supervisorFilter} onChange={props.setSupervisorFilter} clearable searchable />
            <Select label="State" placeholder="All states" data={stateOptions} value={props.stateFilter} onChange={props.setStateFilter} clearable />
            <Select label="Kind" placeholder="All work kinds" data={kindOptions} value={props.kindFilter} onChange={props.setKindFilter} clearable />
            <Box pt={28}>
              <Checkbox label="Show deleted workflow references" checked={props.includeDeleted} onChange={(event) => props.setIncludeDeleted(event.currentTarget.checked)} />
            </Box>
          </SimpleGrid>
        ) : null}

        {alerts.length > 0 ? (
          <Group gap="xs">
            {alerts.slice(0, 6).map((alert) => (
              <Badge key={alert.id} color={alert.level === 'error' ? 'red' : 'yellow'} variant="light">{alert.message}</Badge>
            ))}
          </Group>
        ) : null}
      </Stack>
    </Card>
  );
}

const pageShellStyle: CSSProperties = {
  minHeight: '100vh',
  background: 'radial-gradient(circle at top left, rgba(34,184,207,0.16), transparent 32%), radial-gradient(circle at top right, rgba(121,80,242,0.14), transparent 30%)',
};

function DequeueFeatureModal(props: {
  opened: boolean;
  feature: { feature_id: string; title: string; current_workflow_run_id?: string | null; development_state?: string | null } | null;
  onClose: () => void;
  onConfirm: (mode: 'preserve_development' | 'delete_development') => Promise<void>;
}) {
  const [submitting, setSubmitting] = useState(false);
  const hasWorkflow = Boolean(props.feature?.current_workflow_run_id);
  const developmentState = props.feature?.development_state ?? 'queued';

  async function confirm(mode: 'preserve_development' | 'delete_development') {
    setSubmitting(true);
    try {
      await props.onConfirm(mode);
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Modal opened={props.opened} onClose={props.onClose} title="Remove from queue" centered zIndex={340}>
      <Stack gap="sm">
        <Text size="sm">
          Remove <Text span fw={800}>{props.feature?.title ?? 'this feature'}</Text> from this supervisor queue.
        </Text>
        {hasWorkflow ? (
          <Alert color="yellow" title="Existing workflow found">
            This feature already has a workflow. Choose whether to keep the workflow record and development artifacts, or delete them and reset the feature for later scheduling.
          </Alert>
        ) : (
          <Text size="sm" c="dimmed">No workflow has been created yet. Delete will remove the queued development state and reset the feature for later scheduling.</Text>
        )}
        <Group gap="xs">
          <Badge size="xs" color="gray">{titleCase(developmentState)}</Badge>
          {props.feature?.current_workflow_run_id ? <Badge size="xs" color="blue">Workflow exists</Badge> : null}
        </Group>
        <Group justify="flex-end" gap="xs">
          <Button size="xs" variant="default" onClick={props.onClose} disabled={submitting}>Cancel</Button>
          <Button size="xs" variant="light" onClick={() => void confirm('preserve_development')} loading={submitting}>{hasWorkflow ? 'Keep workflow' : 'Dequeue only'}</Button>
          <Button size="xs" color="red" onClick={() => void confirm('delete_development')} loading={submitting}>{hasWorkflow ? 'Delete workflow' : 'Delete queue state'}</Button>
        </Group>
      </Stack>
    </Modal>
  );
}

function FeatureQueueModal(props: {
  opened: boolean;
  supervisor: FlightDeckSupervisor | null;
  onClose: () => void;
  onApplied: () => Promise<void>;
}) {
  const supervisor = props.supervisor;
  const supervisorId = supervisor?.id ?? null;
  const [queue, setQueue] = useState<SupervisorQueueProjection | null>(null);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [queueLoading, setQueueLoading] = useState(false);
  const [queueError, setQueueError] = useState<string | null>(null);
  const [plannerOptions, setPlannerOptions] = useState<PlannerWorkspace[]>([]);
  const [queuePlannerId, setQueuePlannerId] = useState<string | null>(null);
  const [dequeueFeature, setDequeueFeature] = useState<SupervisorQueueProjection['items'][number] | null>(null);

  useEffect(() => {
    let cancelled = false;
    if (!props.opened || !supervisor?.root_repo_path) {
      setPlannerOptions([]);
      setQueuePlannerId(null);
      return () => {
        cancelled = true;
      };
    }
    void listPlannersForRepo(supervisor.root_repo_path)
      .then((rows) => {
        if (cancelled) return;
        setPlannerOptions(rows);
        setQueuePlannerId((current) => current && rows.some((row) => row.id === current) ? current : rows.find((row) => row.is_default)?.id ?? rows[0]?.id ?? null);
      })
      .catch((err) => {
        if (!cancelled) setQueueError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [props.opened, supervisor?.root_repo_path]);

  useEffect(() => {
    let cancelled = false;
    if (!props.opened || !supervisorId) {
      setQueue(null);
      setSelectedIds([]);
      setQueueError(null);
      return () => {
        cancelled = true;
      };
    }
    setQueueLoading(true);
    setQueueError(null);
    void getSupervisorQueue(supervisorId, queuePlannerId)
      .then((next) => {
        if (cancelled) return;
        setQueue(next);
        setSelectedIds(next.feature_ids ?? []);
      })
      .catch((err) => {
        if (!cancelled) setQueueError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setQueueLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [props.opened, supervisorId, queuePlannerId]);

  const selectedSet = new Set(selectedIds);
  const rawItems = queue?.items ?? [];
  const items = useMemo(() => {
    const queueIndex = new Map(selectedIds.map((id, index) => [id, index]));
    return [...rawItems].sort((left, right) => {
      const leftQueued = queueIndex.has(left.feature_id);
      const rightQueued = queueIndex.has(right.feature_id);
      if (leftQueued && rightQueued) return (queueIndex.get(left.feature_id) ?? 0) - (queueIndex.get(right.feature_id) ?? 0);
      if (leftQueued !== rightQueued) return leftQueued ? -1 : 1;
      return 0;
    });
  }, [rawItems, selectedIds]);

  function latestQueueItem(featureId: string) {
    return items.find((item) => item.feature_id === featureId) ?? null;
  }

  async function autoDeleteDequeue(item: SupervisorQueueProjection['items'][number]) {
    if (!supervisor) return;
    await unscheduleSupervisorFeature(supervisor.id, item.feature_id, 'delete_development');
    const next = await getSupervisorQueue(supervisor.id, queuePlannerId);
    setQueue(next);
    setSelectedIds(next.feature_ids ?? []);
    await props.onApplied();
  }

  async function persistQueueSelection(nextIds: string[]) {
    if (!supervisor) return;
    const featureSettings = poolSetting(supervisor, 'feature_development');
    const integrationSettings = poolSetting(supervisor, 'integration');
    const uniqueIds = Array.from(new Set(nextIds.filter(Boolean)));
    setQueueLoading(true);
    setQueueError(null);
    try {
      await setSupervisorQueue(supervisor.id, uniqueIds, {
        workflow_template_id: featureSettings.template_id ?? null,
        integration_template_id: integrationSettings.template_id ?? null,
        feature_concurrency: featureSettings.concurrency ?? null,
        integration_policy: integrationSettings.mode === 'auto' ? 'auto' : 'manual',
        auto_start: false,
        planner_id: queuePlannerId
      });
      const next = await getSupervisorQueue(supervisor.id, queuePlannerId);
      setQueue(next);
      setSelectedIds(next.feature_ids ?? []);
      await props.onApplied();
    } catch (err) {
      setQueueError(err instanceof Error ? err.message : String(err));
    } finally {
      setQueueLoading(false);
    }
  }

  function setFeatureChecked(featureId: string, checked: boolean) {
    if (!checked) {
      const item = latestQueueItem(featureId);
      if (item && selectedSet.has(featureId)) {
        if (item.dequeue_without_prompt) {
          void autoDeleteDequeue(item);
          return;
        }
        setDequeueFeature(item);
        return;
      }
      void persistQueueSelection(selectedIds.filter((id) => id !== featureId));
      return;
    }

    void persistQueueSelection([...selectedIds, featureId]);
  }

  function moveQueuedFeature(featureId: string, direction: -1 | 1) {
    const index = selectedIds.indexOf(featureId);
    const nextIndex = index + direction;
    if (index < 0 || nextIndex < 0 || nextIndex >= selectedIds.length) return;
    const next = [...selectedIds];
    const current = next[index];
    next[index] = next[nextIndex];
    next[nextIndex] = current;
    setSelectedIds(next);
    void persistQueueSelection(next);
  }

  async function confirmDequeue(mode: 'preserve_development' | 'delete_development') {
    if (!supervisor || !dequeueFeature) return;
    await unscheduleSupervisorFeature(supervisor.id, dequeueFeature.feature_id, mode);
    const next = await getSupervisorQueue(supervisor.id, queuePlannerId);
    setQueue(next);
    setSelectedIds(next.feature_ids ?? []);
    setDequeueFeature(null);
    await props.onApplied();
  }

  async function applyQueue() {
    props.onClose();
  }

  return (
    <>
    <Modal opened={props.opened} onClose={props.onClose} title="Manage feature queue" size="calc(100vw - 160px)" centered zIndex={320}>
      <Stack gap="sm">
        <Text size="sm" c="dimmed">Queue and dequeue refined planner features for this supervisor. Planner remains the feature ledger; the supervisor owns queue execution.</Text>
        <Select
          label="Queueable planner"
          description="Only this planner's unqueued features can be queued. Features already queued from other planners stay visible for dequeue."
          data={plannerOptions.map((planner) => ({ value: planner.id, label: planner.title }))}
          value={queuePlannerId}
          onChange={setQueuePlannerId}
          searchable
          clearable={false}
        />
        <Group gap="xs">
          <Badge color="blue" variant="light">{selectedIds.length} queued</Badge>
          <Badge color="gray" variant="light">{items.length} supervisor-visible planner features</Badge>
        </Group>
        {queueError ? <Alert color="red" title="Queue load failed">{queueError}</Alert> : null}
        {queueLoading ? <Group gap="xs"><Loader size="xs" /><Text size="sm" c="dimmed">Loading supervisor queue…</Text></Group> : null}
        <ScrollArea h="60vh">
          <Stack gap="xs">
            {items.map((item) => {
              const checked = selectedSet.has(item.feature_id);
              const canToggle = checked ? !item.locked_by_other : item.can_queue;
              const queueIndex = selectedIds.indexOf(item.feature_id);
              const canMoveUp = checked && queueIndex > 0 && !queueLoading;
              const canMoveDown = checked && queueIndex >= 0 && queueIndex < selectedIds.length - 1 && !queueLoading;
              const active = ['running', 'development_running', 'ready_for_integration', 'integrating', 'integrated'].includes(normalize(item.development_state ?? item.queue_state));
              const queueDetail = checked
                ? titleCase(item.development_state ?? item.queue_state)
                : active
                  ? titleCase(item.development_state ?? item.queue_state)
                  : titleCase(item.queue_state ?? 'available');
              return (
                <Paper key={item.feature_id} withBorder p="sm" radius="md">
                  <Group justify="space-between" align="flex-start" wrap="nowrap">
                    <Group align="flex-start" wrap="nowrap" style={{ minWidth: 0 }}>
                      <Button
                        size="compact-xs"
                        variant={checked ? 'light' : 'filled'}
                        color={checked ? 'red' : 'blue'}
                        disabled={!canToggle}
                        onClick={() => setFeatureChecked(item.feature_id, !checked)}
                        style={{ width: 76, minWidth: 76, flexShrink: 0 }}
                      >
                        {checked ? 'Dequeue' : 'Queue'}
                      </Button>
                      {checked ? (
                        <Group gap={4} wrap="nowrap" style={{ width: 58, minWidth: 58, flexShrink: 0 }}>
                          <Button size="compact-xs" variant="default" disabled={!canMoveUp} onClick={() => moveQueuedFeature(item.feature_id, -1)} style={{ width: 26, minWidth: 26, padding: 0 }}>↑</Button>
                          <Button size="compact-xs" variant="default" disabled={!canMoveDown} onClick={() => moveQueuedFeature(item.feature_id, 1)} style={{ width: 26, minWidth: 26, padding: 0 }}>↓</Button>
                        </Group>
                      ) : null}
                      <Stack gap={2} style={{ minWidth: 0 }}>
                        <Group gap="xs">
                          <Text fw={800} size="sm">{item.title}</Text>
                          <Badge size="xs" color={checked ? 'green' : 'gray'}>{checked ? 'Queued' : titleCase(item.queue_state)}</Badge>
                          {item.planner_title ? <Badge size="xs" color={item.is_current_planner ? 'blue' : 'violet'} variant="light">{item.is_current_planner ? 'Current planner' : item.planner_title}</Badge> : null}
                          {active ? <Badge size="xs" color="cyan">Active</Badge> : null}
                          {item.locked_by_other ? <Badge size="xs" color="red">Locked elsewhere</Badge> : null}
                        </Group>
                        <Text size="xs" c="dimmed" lineClamp={2}>{item.summary || 'No summary.'}</Text>
                        {!canToggle && !checked && item.disabled_reason ? <Text size="xs" c="orange">{item.disabled_reason}</Text> : null}
                      </Stack>
                    </Group>
                    <Stack gap={2} align="flex-end" style={{ width: 120, minWidth: 120, flexShrink: 0 }}>
                      <Badge size="xs" color={tone(item.planner_status)}>{titleCase(item.planner_status ?? 'unknown')}</Badge>
                      <Text size="xs" c="dimmed" ta="right">{queueDetail}</Text>
                    </Stack>
                  </Group>
                </Paper>
              );
            })}
            {!queueLoading && items.length === 0 ? <Text c="dimmed" size="sm">No planner features are available for this supervisor.</Text> : null}
          </Stack>
        </ScrollArea>
        <Group justify="flex-end" gap="xs">
          <Button size="xs" variant="default" onClick={props.onClose}>Cancel</Button>
          <Button size="xs" onClick={() => void applyQueue()} disabled={!supervisor || queueLoading}>Done</Button>
        </Group>
      </Stack>
    </Modal>
    <DequeueFeatureModal
      opened={dequeueFeature !== null}
      feature={dequeueFeature}
      onClose={() => setDequeueFeature(null)}
      onConfirm={confirmDequeue}
    />
    </>
  );
}

export function FlightDeckPanel(props: FlightDeckPanelProps) {
  const [deck, setDeck] = useState<FlightDeckResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [supervisorFilter, setSupervisorFilter] = useState<string | null>(null);
  const [stateFilter, setStateFilter] = useState<string | null>(null);
  const [kindFilter, setKindFilter] = useState<string | null>(null);
  const [includeDeleted, setIncludeDeleted] = useState(false);
  const [filtersOpen, setFiltersOpen] = useState(true);
  const [templates, setTemplates] = useState<WorkflowTemplate[]>([]);
  const [plannerSupervisor, setPlannerSupervisor] = useState<FlightDeckSupervisor | null>(null);
  const [plannerCreateFeatureOnOpen, setPlannerCreateFeatureOnOpen] = useState(false);
  const [plannerSelectFeatureOnOpen, setPlannerSelectFeatureOnOpen] = useState(false);
    const [queueSupervisor, setQueueSupervisor] = useState<FlightDeckSupervisor | null>(null);
  const [newFineChoiceSupervisor, setNewFineChoiceSupervisor] = useState<FlightDeckSupervisor | null>(null);
  const [plannerRefinementTemplateId, setPlannerRefinementTemplateId] = useState<string | null>(null);
  const [creatingRefineFeatureId, setCreatingRefineFeatureId] = useState<string | null>(null);
  const templateOptions = useMemo(() => workflowTemplateOptions(templates), [templates]);

  useEffect(() => {
    let cancelled = false;
    void listTemplates()
      .then((rows) => {
        if (!cancelled) setTemplates(rows);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function refresh() {
    try {
      setError(null);
      const next = await getFlightDeck({ supervisor_id: supervisorFilter, state: stateFilter, kind: kindFilter, include_deleted: includeDeleted });
      setDeck(next);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    setLoading(true);
    void refresh();
    const timer = window.setInterval(() => void refresh(), 2000);
    return () => window.clearInterval(timer);
  }, [supervisorFilter, stateFilter, kindFilter, includeDeleted]);

  function openPlanner(supervisor: FlightDeckSupervisor, options?: OpenPlannerOptions) {
    if (options?.selectFeature) {
      setQueueSupervisor(supervisor);
      return;
    }
    if (options?.refinementTemplateId && !options.createFeature) {
      setNewFineChoiceSupervisor(supervisor);
      setPlannerRefinementTemplateId(options.refinementTemplateId);
      return;
    }
    setPlannerSupervisor(supervisor);
    setPlannerCreateFeatureOnOpen(Boolean(options?.createFeature));
    setPlannerSelectFeatureOnOpen(false);
    setPlannerRefinementTemplateId(options?.refinementTemplateId ?? null);
  }

  async function createRefineWorkflowForFeature(plannerId: string, featureId: string) {
    const supervisor = plannerSupervisor;
    const templateId = plannerRefinementTemplateId;
    if (!supervisor || !templateId) {
      setError('Refine template is required before creating a fine workflow.');
      return;
    }

    setCreatingRefineFeatureId(featureId);
    try {
      setPlannerSupervisor(null);
      setPlannerCreateFeatureOnOpen(false);
      setPlannerSelectFeatureOnOpen(false);
      await refinePlannerFeature(plannerId, featureId, {
        supervisor_id: supervisor.id,
        workflow_template_id: templateId,
      });
      setPlannerRefinementTemplateId(null);
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setCreatingRefineFeatureId(null);
    }
  }

  return (
    <Box p="md" style={pageShellStyle}>
      <Stack gap="md">
        <MissionBar
          deck={deck}
          filtersOpen={filtersOpen}
          supervisorFilter={supervisorFilter}
          stateFilter={stateFilter}
          kindFilter={kindFilter}
          includeDeleted={includeDeleted}
          setFiltersOpen={setFiltersOpen}
          setSupervisorFilter={setSupervisorFilter}
          setStateFilter={setStateFilter}
          setKindFilter={setKindFilter}
          setIncludeDeleted={setIncludeDeleted}
        />

        {error ? <Alert color="red">{error}</Alert> : null}
        {creatingRefineFeatureId ? <Alert color="blue">Creating refine workflow for selected feature…</Alert> : null}
        {loading && !deck ? <Loader /> : null}

        <Stack gap="lg" pr="sm">
          {deck?.supervisors.length ? deck.supervisors.map((supervisor) => (
            <SupervisorCockpit key={supervisor.id} supervisor={supervisor} templateOptions={templateOptions} navigate={props.navigate} onOpenPlanner={openPlanner} onActionComplete={() => void refresh()} />
          )) : !loading ? (
            <Card withBorder radius="xl" p="lg">
              <Text c="dimmed">No supervisors matched the current filters.</Text>
            </Card>
          ) : null}
        </Stack>
      <Modal
        opened={newFineChoiceSupervisor !== null}
        onClose={() => setNewFineChoiceSupervisor(null)}
        title="New fine"
        centered
        size="md"
      >
        <Stack gap="sm">
          <Text size="sm" c="dimmed">Create a fine/refinement workflow from an existing planner feature or start from a new planner feature.</Text>
          <Group justify="flex-end" gap="xs" wrap="nowrap">
            <Button size="xs" variant="default" onClick={() => setNewFineChoiceSupervisor(null)}>Cancel</Button>
            <Button size="xs" variant="light" onClick={() => {
              if (!newFineChoiceSupervisor) return;
              const supervisor = newFineChoiceSupervisor;
              const templateId = plannerRefinementTemplateId;
              setNewFineChoiceSupervisor(null);
              setPlannerCreateFeatureOnOpen(false);
              setPlannerSelectFeatureOnOpen(true);
              setPlannerSupervisor(supervisor);
              setPlannerRefinementTemplateId(templateId);

            }}>Refine existing feature</Button>
            <Button size="xs" onClick={() => {
              if (!newFineChoiceSupervisor) return;
              const supervisor = newFineChoiceSupervisor;
              const templateId = plannerRefinementTemplateId;
              setNewFineChoiceSupervisor(null);
              setPlannerCreateFeatureOnOpen(true);
              setPlannerSelectFeatureOnOpen(false);
              setPlannerSupervisor(supervisor);
              setPlannerRefinementTemplateId(templateId);

            }}>New feature</Button>
          </Group>
        </Stack>
      </Modal>
      <FeatureQueueModal
        opened={queueSupervisor !== null}
        supervisor={queueSupervisor ? deck?.supervisors.find((item) => item.id === queueSupervisor.id) ?? queueSupervisor : null}
        onClose={() => setQueueSupervisor(null)}
        onApplied={refresh}
      />
      <PlannerModal
        opened={plannerSupervisor !== null}
        rootRepoPath={plannerSupervisor?.root_repo_path ?? ''}
        run={null}
        templates={templates}
        selectedPlannerId={null}
        selectedFeatureId={null}
        createFeatureOnOpen={plannerCreateFeatureOnOpen}
        selectionMode={plannerSelectFeatureOnOpen}
        onClose={() => {
          setPlannerSupervisor(null);
          setPlannerCreateFeatureOnOpen(false);
          setPlannerSelectFeatureOnOpen(false);
          setPlannerRefinementTemplateId(null);
          setCreatingRefineFeatureId(null);
        }}
        onSelectFeature={async (selection) => {
          if (!selection.planner?.id || !selection.feature?.id) return;
          if (plannerRefinementTemplateId) {
            await createRefineWorkflowForFeature(selection.planner.id, selection.feature.id);
          }
        }}
        onFeatureCreated={async (selection) => {
          if (!selection.planner?.id || !selection.feature?.id) return;
          if (plannerRefinementTemplateId) {
            await createRefineWorkflowForFeature(selection.planner.id, selection.feature.id);
          }
        }}
        onSaved={refresh}
        onError={setError}
      />
      </Stack>
    </Box>
  );
}
