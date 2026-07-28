import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
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
import { createPlannerForRepo, deletePlannerForRepo, listPlannersForRepo, refinePlannerFeature, type PlannerWorkspace } from './planner_api';
import { createSupervisorRun, deleteSupervisorRun, getFlightDeck, getSupervisorQueue, getWorkflowEventHistory, runSupervisorAction, setSupervisorQueue, workflowEventHistoryStreamUrl, type FlightDeckResponse, type FlightDeckSupervisor, type FlightDeckWorkUnit, type SupervisorQueuedFeature, type SupervisorQueueProjection, type WorkflowEventHistoryItem, type WorkflowEventHistoryQuery } from './supervisor_api';

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
  execution_event_limit?: number;
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
  created_at?: string;
  duration_ms?: number;
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

function supervisorExecutionEventLimit(supervisor: FlightDeckSupervisor): number {
  const value = supervisorFlightDeckSettings(supervisor).execution_event_limit;
  return typeof value === 'number' && Number.isFinite(value)
    ? Math.max(10, Math.min(1000, Math.floor(value)))
    : 100;
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

function numberField(item: Record<string, unknown>, key: string): number | undefined {
  const value = item[key];
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function relativeExecutionTime(value?: string): string {
  if (!value) return '';

  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return '';

  const elapsedSeconds = Math.max(0, Math.floor((Date.now() - timestamp) / 1000));

  if (elapsedSeconds < 60) {
    return `${elapsedSeconds} second${elapsedSeconds === 1 ? '' : 's'} ago`;
  }

  const elapsedMinutes = Math.floor(elapsedSeconds / 60);
  if (elapsedMinutes < 60) {
    return `${elapsedMinutes} minute${elapsedMinutes === 1 ? '' : 's'} ago`;
  }

  const elapsedHours = Math.floor(elapsedMinutes / 60);
  if (elapsedHours < 72) {
    return `${elapsedHours} hour${elapsedHours === 1 ? '' : 's'} ago`;
  }

  const elapsedDays = Math.floor(elapsedHours / 24);
  return `${elapsedDays} day${elapsedDays === 1 ? '' : 's'} ago`;
}

function executionDuration(durationMs?: number): string {
  if (durationMs === undefined || durationMs < 0) return '';

  const totalSeconds = Math.max(0, Math.round(durationMs / 1000));
  if (totalSeconds < 60) return `${totalSeconds}s`;

  const totalMinutes = Math.floor(totalSeconds / 60);
  const remainingSeconds = totalSeconds % 60;
  if (totalMinutes < 60) {
    return remainingSeconds > 0
      ? `${totalMinutes}m ${remainingSeconds}s`
      : `${totalMinutes}m`;
  }

  const hours = Math.floor(totalMinutes / 60);
  const remainingMinutes = totalMinutes % 60;
  return remainingMinutes > 0
    ? `${hours}h ${remainingMinutes}m`
    : `${hours}h`;
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

function stageExecutionDisplayName(stepId: string): string {
  const key = stageKeyFromText(stepId);
  return STAGE_LABELS[key] ?? titleCase(key);
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
  const waiting = unit.state === 'waiting_user';

  const latestStageByKey = new Map<string, Record<string, unknown>>();
  for (const stage of recentStages) {
    const stepId = textField(stage, 'step_id');
    if (!stepId) continue;
    const key = stageKeyFromText(stepId);
    if (!latestStageByKey.has(key)) {
      latestStageByKey.set(key, stage);
    }
  }

  return ordered.map((key, index) => {
    const matching = latestStageByKey.get(key);
    const status = normalize(matching ? textField(matching, 'status') : '');
    let state: StageProjection['state'] = 'future';

    if (activeKey === key) state = waiting ? 'waiting' : 'active';
    else if (status === 'success' || status === 'complete' || status === 'completed') state = 'complete';
    else if (status === 'failed' || status === 'error' || status === 'cancelled') state = 'failed';
    else if (status === 'running' || status === 'active') state = 'active';
    else if (status === 'waiting' || status === 'waiting_user' || status === 'paused') state = 'waiting';
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

  const recentCapabilities = telemetryArray(unit, 'recent_capability_executions');
  const capabilityEvents = recentCapabilities.length > 0
    ? recentCapabilities
    : telemetryArray(unit, 'current_stage_recent_capabilities');

  capabilityEvents.forEach((capability, index) => {
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
      created_at: textField(capability, 'created_at') || existing?.created_at,
      duration_ms: numberField(capability, 'duration_ms') ?? existing?.duration_ms,
      step_id: textField(capability, 'step_id') || existing?.step_id || currentStepId(unit),
      stage_execution_id: textField(capability, 'stage_execution_id') || existing?.stage_execution_id || null,
      capability_invocation_id: textField(capability, 'capability_invocation_id') || existing?.capability_invocation_id || null,
    });
  });

  return [...byInvocation.values()];
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
        created_at: item.created_at,
        duration_ms:
          numberField(item.payload ?? {}, 'duration_ms')
          ?? numberField(item.payload ?? {}, 'elapsed_ms'),
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
    capabilities: [...capabilityByKey.values()],
    stages: [...stageByKey.values()],
  };
}

type ExecutionEventRowItem = {
  id: string;
  label: string;
  status: string;
  message?: string;
  createdAt?: string;
  durationMs?: number;
};

function ExecutionEventRow(props: {
  item: ExecutionEventRowItem;
  onClick?: () => void;
}) {
  const { item } = props;
  const statusTone = tone(item.status);
  const relativeTime = relativeExecutionTime(item.createdAt);
  const duration = executionDuration(item.durationMs);
  const timing = [duration, relativeTime].filter(Boolean).join(' · ');
  const exactTime = item.createdAt && Number.isFinite(Date.parse(item.createdAt))
    ? new Date(item.createdAt).toLocaleString()
    : '';

  const messageText = (
    <Text
      size="10px"
      c="dimmed"
      truncate
      style={{ flex: '1 1 auto', minWidth: 0 }}
    >
      {item.message}
    </Text>
  );

  return (
    <Group
      gap={8}
      wrap="nowrap"
      px={8}
      py={3}
      onClick={props.onClick}
      style={{
        minHeight: 24,
        cursor: props.onClick ? 'pointer' : undefined,
        border: `1px solid var(--mantine-color-${statusTone}-7)`,
        borderRadius: 'var(--mantine-radius-sm)',
        background: `var(--mantine-color-${statusTone}-light)`,
      }}
    >
      <Text
        fw={600}
        size="xs"
        truncate
        style={{ flex: '0 1 120px', minWidth: 64, maxWidth: 140 }}
      >
        {item.label}
      </Text>

      {item.message ? (
        <Tooltip
          label={(
            <Stack gap={4}>
              <Text size="xs" style={{ whiteSpace: 'normal' }}>
                {item.message}
              </Text>
              {duration ? (
                <Text size="10px" c="dimmed">
                  Duration: {duration}
                </Text>
              ) : null}
              {exactTime ? (
                <Text size="10px" c="dimmed">
                  Recorded: {exactTime}
                </Text>
              ) : null}
            </Stack>
          )}
          position="bottom-start"
          multiline
          maw={360}
          withArrow
          openDelay={350}
        >
          {messageText}
        </Tooltip>
      ) : (
        <Box style={{ flex: '1 1 auto', minWidth: 0 }} />
      )}

      {timing ? (
        <Text
          size="10px"
          c="dimmed"
          truncate
          style={{ flex: '0 0 auto', whiteSpace: 'nowrap', maxWidth: '42%' }}
        >
          {timing}
        </Text>
      ) : null}
    </Group>
  );
}

function CapabilityStrip(props: { capabilities: CapabilityProjection[]; onOpenHistory?: (capability: CapabilityProjection) => void }) {
  const { capabilities } = props;
  if (capabilities.length === 0) {
    return <Text size="sm" c="dimmed">No capability executions yet.</Text>;
  }

  let previousStageExecutionId: string | null | undefined;

  return (
    <Stack gap={4}>
      {capabilities.map((capability) => {
        const stageExecutionId = capability.stage_execution_id || capability.step_id || null;
        const stageChanged = previousStageExecutionId !== undefined
          && previousStageExecutionId !== stageExecutionId;
        previousStageExecutionId = stageExecutionId;

        return (
          <Box key={capability.id}>
            {stageChanged ? (
              <Group gap={6} my={2} wrap="nowrap">
                <Text
                  size="9px"
                  fw={700}
                  c="dimmed"
                  tt="uppercase"
                  style={{ flex: '0 0 auto', lineHeight: 1 }}
                >
                  {capability.step_id
                    ? stageExecutionDisplayName(capability.step_id)
                    : 'Stage'}
                </Text>
                <Box
                  style={{
                    height: 1,
                    flex: '1 1 auto',
                    background: 'var(--mantine-color-dark-4)',
                  }}
                />
              </Group>
            ) : null}
            <ExecutionEventRow
              item={{
                id: capability.id,
                label: capability.label,
                status: capability.state,
                message: capability.message,
                createdAt: capability.created_at,
                durationMs: capability.duration_ms,
              }}
              onClick={props.onOpenHistory
                ? () => props.onOpenHistory?.(capability)
                : undefined}
            />
          </Box>
        );
      })}
    </Stack>
  );
}

function RecentStageStrip(props: { unit: FlightDeckWorkUnit; fallbackStages?: Record<string, unknown>[]; onOpenHistory?: (stage: Record<string, unknown>) => void }) {
  const nativeStages = telemetryArray(props.unit, 'recent_stage_executions');
  const sourceStages = nativeStages.length > 0 ? nativeStages : props.fallbackStages ?? [];
  const stages = sourceStages
    .map((stage, index) => ({ stage, index }))
    .sort((a, b) => {
      const aStartedAt = textField(a.stage, 'started_at') || textField(a.stage, 'created_at');
      const bStartedAt = textField(b.stage, 'started_at') || textField(b.stage, 'created_at');
      const aTime = Date.parse(aStartedAt);
      const bTime = Date.parse(bStartedAt);

      if (Number.isFinite(aTime) && Number.isFinite(bTime) && aTime !== bTime) {
        return bTime - aTime;
      }

      return b.index - a.index;
    });

  if (stages.length === 0) {
    return <Text size="sm" c="dimmed">No previous stage executions yet.</Text>;
  }

  return (
    <Stack gap={4}>
      {stages.map(({ stage, index }) => {
        const status = textField(stage, 'status') || 'event';
        const stepId = textField(stage, 'step_id') || 'stage';
        const stageExecutionId = textField(stage, 'stage_execution_id');
        const stageName = stageExecutionDisplayName(stepId);
        const message = textField(stage, 'message') || '';
        const createdAt = textField(stage, 'created_at');
        const durationMs = numberField(stage, 'duration_ms');

        return (
          <ExecutionEventRow
            key={stageExecutionId || `${stepId}-${index}-${createdAt}`}
            item={{
              id: stageExecutionId || `${stepId}-${index}-${createdAt}`,
              label: stageName,
              status,
              message,
              createdAt,
              durationMs,
            }}
            onClick={props.onOpenHistory
              ? () => props.onOpenHistory?.(stage)
              : undefined}
          />
        );
      })}
    </Stack>
  );
}

function workflowType(unit: FlightDeckWorkUnit): string {
  return unit.workflow_type ?? unit.kind;
}

function manualShardIsIntegrationInput(unit: FlightDeckWorkUnit): boolean {
  return workflowType(unit) === 'manual_shard'
    && (
      unit.telemetry?.staged_to_integration === true
      || unit.telemetry?.integration_input === true
      || normalize(unit.state) === 'ready_for_integration'
      || normalize(unit.state) === 'integrating'
      || normalize(unit.state) === 'integrated'
    );
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
  const type = workflowType(unit);
  if (type === 'integration') return !unit.workflow_deleted && Boolean(unit.workflow_run_id);
  if (type === 'manual_shard') return !unit.workflow_deleted && !workflowIsProcessing(unit) && Boolean(unit.feature_id);
  return (type === 'feature_development' || type === 'refine') && !unit.workflow_deleted && !workflowIsProcessing(unit) && Boolean(unit.feature_id);
}

function workflowCanDelete(unit: FlightDeckWorkUnit): boolean {
  const type = workflowType(unit);
  if (type === 'manual_shard') return !manualShardIsIntegrationInput(unit) && !unit.workflow_deleted && Boolean(unit.feature_id);
  if (type === 'refine') return !unit.workflow_deleted && Boolean(unit.feature_id);
  if (type === 'feature_development') return !unit.workflow_deleted && Boolean(unit.feature_id);
  return false;
}

function workflowDeleteLabel(unit: FlightDeckWorkUnit): string {
  return workflowType(unit) === 'feature_development' ? 'Unqueue' : 'Delete';
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
  const integrationState = normalize(unit.state);
  const canApplyFinalPatch = workflowType(unit) === 'integration'
    && !unit.workflow_deleted
    && Boolean(unit.workflow_run_id)
    && ['patch_ready', 'integrated', 'ready_to_apply'].includes(integrationState);


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

  async function runWorkflowAction(action: 'start_work_unit' | 'pause_work_unit' | 'regenerate_work_unit' | 'delete_work_unit' | 'stage_work_unit' | 'unstage_work_unit' | 'apply_integration' | 'cancel') {
    if (action === 'regenerate_work_unit') {
      const confirmed = workflowType(unit) === 'refine'
        ? window.confirm(`Delete and recreate the refine workflow for ${unit.title}?`)
        : window.confirm(`Delete existing workflow/shard state for ${unit.title} and return it to queued draft state?`);
      if (!confirmed) return;
    }
    if (action === 'delete_work_unit') {
      const confirmed = workflowType(unit) === 'feature_development'
        ? window.confirm(`Unqueue ${unit.title}? This deletes the supervisor workflow/workspace state and returns the planner feature to the queueable pool.`)
        : window.confirm(`Delete ${unit.title}? This deletes the supervisor workflow/workspace state for this work unit.`);
      if (!confirmed) return;
    }
    if (action === 'stage_work_unit') {
      const confirmed = window.confirm(`Stage ${unit.title} to the integration pool? Backend validation requires staged git changes for manual shards.`);
      if (!confirmed) return;
    }
    if (action === 'unstage_work_unit') {
      const confirmed = window.confirm(`Unstage ${unit.title} from the integration pool?`);
      if (!confirmed) return;
    }
    if (action === 'apply_integration') {
      const confirmed = window.confirm('Apply the final integration patch to the root repository?');
      if (!confirmed) return;
    }
    if (action === 'cancel') {
      const confirmed = window.confirm(`Pause integration workflow ${unit.title}?`);
      if (!confirmed) return;
    }

    if (action === 'stage_work_unit' || action === 'unstage_work_unit') {
      await runSupervisorAction(props.supervisor.id, { action: 'stage_work_unit', work_unit_id: unit.id, staged: action === 'stage_work_unit' });
    } else if (action === 'apply_integration' || action === 'cancel') {
      await runSupervisorAction(props.supervisor.id, { action });
    } else {
      await runSupervisorAction(props.supervisor.id, { action, work_unit_id: unit.id });
    }
    props.onActionComplete?.();
  }

  const workflowHeaderState = normalize(telemetryString(unit, 'status') || unit.state);
  const workflowHeaderWaiting = ['waiting', 'waiting_user', 'paused'].includes(workflowHeaderState);
  const workflowHeaderActive = ['queued', 'running', 'active'].includes(workflowHeaderState);
  const workflowHeaderAnimated = workflowHeaderWaiting || workflowHeaderActive;
  const workflowHeaderColor = workflowHeaderWaiting
    ? '250, 176, 5'
    : '34, 139, 230';

  return (
    <Card
      withBorder
      radius="lg"
      p="sm"
      style={{
        background: 'linear-gradient(135deg, rgba(39, 42, 48, 0.96), rgba(31, 34, 39, 0.94))',
        borderColor: `var(--mantine-color-${tone(unit.state)}-7)`,
        marginLeft: props.compact ? 16 : 0,
      }}
    >
      <Stack gap="sm">
        <style>{`
          @keyframes flight-deck-workflow-header-flow {
            0% { background-position: 0% 50%; }
            100% { background-position: 200% 50%; }
          }
        `}</style>
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
            <Box
              px="xs"
              py={6}
              style={{
                border: workflowHeaderAnimated
                  ? `1px solid rgba(${workflowHeaderColor}, 0.46)`
                  : '1px solid transparent',
                borderRadius: 'var(--mantine-radius-sm)',
                backgroundColor: workflowHeaderWaiting
                  ? 'rgba(250, 176, 5, 0.10)'
                  : workflowHeaderActive
                    ? 'rgba(34, 139, 230, 0.10)'
                    : 'transparent',
                backgroundImage: workflowHeaderAnimated
                  ? `linear-gradient(
                      105deg,
                      transparent 0%,
                      rgba(${workflowHeaderColor}, 0.015) 40%,
                      rgba(${workflowHeaderColor}, 0.10) 50%,
                      rgba(${workflowHeaderColor}, 0.015) 60%,
                      transparent 100%
                    )`
                  : undefined,
                backgroundSize: workflowHeaderAnimated ? '220% 100%' : undefined,
                boxShadow: workflowHeaderAnimated
                  ? `inset 2px 0 0 rgba(${workflowHeaderColor}, 0.78)`
                  : 'none',
                animation: workflowHeaderAnimated
                  ? 'flight-deck-workflow-header-flow 3.6s linear infinite'
                  : undefined,
              }}
            >
              <Group justify="space-between" align="center" gap="xs" wrap="nowrap">
                <Group gap="xs" wrap="nowrap" style={{ minWidth: 0 }}>
                  <Badge
                    color={tone(workflowHeaderState)}
                    variant="filled"
                  >
                    {titleCase(workflowHeaderState)}
                  </Badge>
                  {title}
                  {unit.workflow_deleted ? <Badge color="red" variant="outline">Deleted</Badge> : null}
                  {unit.patch_id ? <Badge color="violet" variant="light">Patch {unit.patch_id.slice(0, 8)}</Badge> : null}
                </Group>
                <Group gap={6} wrap="nowrap">
                  {workflowCanUnstageManual(unit) ? (
                    <Button size="compact-xs" color="yellow" variant="outline" onClick={() => void runWorkflowAction('unstage_work_unit')}>Unstage from integration pool</Button>
                  ) : null}
                  {workflowCanPause(unit) ? (
                    <Button size="compact-xs" variant="default" onClick={() => void runWorkflowAction(workflowType(unit) === 'integration' ? 'cancel' : 'pause_work_unit')}>Pause</Button>
                  ) : workflowCanRun(unit) ? (
                    <Button size="compact-xs" variant="default" onClick={() => void runWorkflowAction('start_work_unit')}>Run</Button>
                  ) : null}
                  {workflowCanRegenerate(unit) ? (
                    <Button size="compact-xs" color="yellow" variant="outline" onClick={() => void runWorkflowAction('regenerate_work_unit')}>Regenerate</Button>
                  ) : null}
                  {workflowCanStageManual(unit) ? (
                    <Button size="compact-xs" color="green" variant="outline" onClick={() => void runWorkflowAction('stage_work_unit')}>Stage to integration pool</Button>
                  ) : null}
                  {workflowCanDelete(unit) ? (
                    <Button size="compact-xs" color="red" variant="outline" onClick={() => void runWorkflowAction('delete_work_unit')}>{workflowDeleteLabel(unit)}</Button>
                  ) : null}
                </Group>
              </Group>
            </Box>
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
                  onClick={() => void runWorkflowAction('apply_integration')}
                  style={{ minWidth: 260 }}
                >
                  Apply final patch to root
                </Button>
              </Group>
            ) : null}
          </Stack>
          <Stack gap={6} style={{ minWidth: 0 }}>
            <Text fw={800} size="sm">Capability execution</Text>
            <ScrollArea.Autosize mah={240} offsetScrollbars scrollbarSize={6}>
              <CapabilityStrip capabilities={displayCapabilities} onOpenHistory={openCapabilityHistory} />
            </ScrollArea.Autosize>
          </Stack>
          <Stack gap={6} style={{ minWidth: 0 }}>
            <Text fw={800} size="sm">Stage execution</Text>
            <ScrollArea.Autosize mah={240} offsetScrollbars scrollbarSize={6}>
              <RecentStageStrip unit={unit} fallbackStages={displayFallbackStages} onOpenHistory={openStageHistory} />
            </ScrollArea.Autosize>
          </Stack>
        </div>

        <EventHistoryModal anchor={historyAnchor} onClose={() => setHistoryAnchor(null)} />

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
        value={props.value ?? null}
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
  const templateValue = props.settings.template_id ?? null;
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
  return manualShardIsIntegrationInput(unit);
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
      await runSupervisorAction(props.supervisor.id, { action: 'skip_integration_input', work_unit_id: unit.id });
      props.onActionComplete?.();
    } finally {
      setBusyKey(null);
    }
  }

  async function unskipFeature(unit: FlightDeckWorkUnit) {
    if (!unit.feature_id) return;
    setBusyKey(unit.id);
    try {
      await runSupervisorAction(props.supervisor.id, { action: 'unskip_integration_input', work_unit_id: unit.id });
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
      await runSupervisorAction(props.supervisor.id, { action: 'skip_integration_input', work_unit_id: unit.id });
      props.onActionComplete?.();
    } finally {
      setBusyKey(null);
    }
  }

  async function unskipManualShard(unit: FlightDeckWorkUnit) {
    if (!unit.feature_id) return;
    setBusyKey(unit.id);
    try {
      await runSupervisorAction(props.supervisor.id, { action: 'unskip_integration_input', work_unit_id: unit.id });
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
  const selectedTemplate = settings.template_id ?? null;
  const [manualBusy, setManualBusy] = useState(false);
  const [featurePoolBusy, setFeaturePoolBusy] = useState(false);
  const [manualNameOpen, setManualNameOpen] = useState(false);
  const [manualName, setManualName] = useState('');
  async function updateSettings(patch: FlightDeckPoolSetting) {
    const flightDeckSettings = nextFlightDeckSettings(props.supervisor, props.groupKey, patch);
    await runSupervisorAction(props.supervisor.id, { action: 'update_flight_deck_settings', flight_deck_settings: flightDeckSettings as Record<string, unknown> });
    props.onActionComplete?.();
  }
  const poolControls = <PoolControls groupKey={props.groupKey} templateOptions={props.templateOptions} settings={settings} onSettingsChange={(patch) => void updateSettings(patch)} />;

  async function createManualShard() {
    const name = manualName.trim() || `Manual shard ${props.units.length + 1}`;
    setManualBusy(true);
    props.onManualShardCreating?.(name);
    try {
      await runSupervisorAction(props.supervisor.id, {
        action: 'create_work_unit',
        pool_kind: 'manual_shard',
        name,
        template_id: selectedTemplate
      });
      setManualName('');
      setManualNameOpen(false);
      props.onActionComplete?.();
    } finally {
      setManualBusy(false);
      props.onManualShardCreating?.(null);
    }
  }

  async function runFeaturePoolAction() {
    if (featurePoolBusy) return;

    setFeaturePoolBusy(true);
    try {
      await runSupervisorAction(props.supervisor.id, {
        action: running ? 'pause_feature_pool' : 'resume_feature_pool',
      });
      props.onActionComplete?.();
    } finally {
      setFeaturePoolBusy(false);
    }
  }

  if (props.groupKey === 'integration') {
    const readiness = integrationReadinessModel(props.supervisor);
    const canRunIntegration = readiness.relevantTotal > 0 && readiness.readyTotal === readiness.relevantTotal && !running;
    const integrationWorkUnitId = `${props.supervisor.id}:integration`;
    return (
      <Group gap="xs" align="center" wrap="nowrap">
        {poolControls}
        <Button size="xs" variant="default" disabled={!canRunIntegration} onClick={() => void runSupervisorAction(props.supervisor.id, { action: 'start_work_unit', work_unit_id: integrationWorkUnitId }).then(() => props.onActionComplete?.())}>Run integration</Button>
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
        <Button
          size="xs"
          variant="default"
          loading={featurePoolBusy}
          disabled={featurePoolBusy || (!running && !queued && !waiting && !poolPaused)}
          onClick={() => void runFeaturePoolAction()}
        >
          {featurePoolBusy
            ? running ? 'Pausing…' : 'Starting…'
            : running ? 'Pause' : 'Run'}
        </Button>
      </Group>
    );
  }

  if (props.groupKey === 'refine') {
    return (
      <Group gap="xs" align="center" wrap="nowrap">
        {poolControls}
        <Button size="xs" variant="light" onClick={() => props.onOpenPlanner?.(props.supervisor, { refinementTemplateId: selectedTemplate })}>Refine feature</Button>
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
            background: 'linear-gradient(90deg, rgba(255,255,255,0.045), rgba(255,255,255,0.015), transparent)',
            borderLeft: '3px solid rgba(255,255,255,0.20)',
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

function SupervisorPlannerOptionsModal(props: {
  opened: boolean;
  supervisor: FlightDeckSupervisor | null;
  onClose: () => void;
  onApplied: () => Promise<void> | void;
  onError?: (message: string) => void;
}) {
  const supervisor = props.supervisor;
  const [planners, setPlanners] = useState<PlannerWorkspace[]>([]);
  const [selectedPlannerId, setSelectedPlannerId] = useState<string | null>(null);
  const [newPlannerTitle, setNewPlannerTitle] = useState('');
  const [loading, setLoading] = useState(false);

  const supervisorSelectedPlannerId = supervisor?.selected_planner_id ?? null;

  async function load() {
    if (!supervisor) return;
    setLoading(true);
    try {
      const rows = await listPlannersForRepo(supervisor.root_repo_path);
      setPlanners(rows);
      setSelectedPlannerId((current) => current ?? supervisorSelectedPlannerId ?? rows.find((planner) => planner.is_default)?.id ?? rows[0]?.id ?? null);
      setNewPlannerTitle((current) => current.trim() || `${supervisor.title || 'Supervisor'} Planner`);
    } catch (err) {
      props.onError?.(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    if (!props.opened) return;
    setSelectedPlannerId(supervisorSelectedPlannerId);
    void load();
  }, [props.opened, supervisor?.id, supervisorSelectedPlannerId]);

  async function assignPlanner() {
    if (!supervisor || !selectedPlannerId) return;
    setLoading(true);
    try {
      const settings = {
        ...supervisorFlightDeckSettings(supervisor),
        selected_planner_id: selectedPlannerId,
      } as Record<string, unknown>;
      await runSupervisorAction(supervisor.id, {
        action: 'update_flight_deck_settings',
        flight_deck_settings: settings,
      });
      await props.onApplied();
      props.onClose();
    } catch (err) {
      props.onError?.(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  async function createPlanner() {
    if (!supervisor) return;
    const title = newPlannerTitle.trim() || `${supervisor.title || 'Supervisor'} Planner`;
    setLoading(true);
    try {
      const planner = await createPlannerForRepo({
        root_repo_path: supervisor.root_repo_path,
        title,
        make_default: false,
        features: [],
      });
      const rows = await listPlannersForRepo(supervisor.root_repo_path);
      setPlanners(rows.some((row) => row.id === planner.id) ? rows : [planner, ...rows]);
      setSelectedPlannerId(planner.id);
      setNewPlannerTitle('');
    } catch (err) {
      props.onError?.(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  async function deletePlanner() {
    if (!supervisor || !selectedPlannerId) return;
    const planner = planners.find((item) => item.id === selectedPlannerId);
    if (!planner) return;
    const confirmed = window.confirm(`Delete planner "${planner.title}"? This removes the planner feature log and cannot be undone.`);
    if (!confirmed) return;
    setLoading(true);
    try {
      await deletePlannerForRepo(planner.id);
      const rows = await listPlannersForRepo(supervisor.root_repo_path);
      setPlanners(rows);
      setSelectedPlannerId(rows.find((row) => row.id === supervisorSelectedPlannerId)?.id ?? rows.find((row) => row.is_default)?.id ?? rows[0]?.id ?? null);
      await props.onApplied();
    } catch (err) {
      props.onError?.(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  return (
    <Modal opened={props.opened} onClose={props.onClose} title="Supervisor planner options" centered size="lg" zIndex={360}>
      <Stack gap="sm">
        <Text size="sm" c="dimmed">
          The supervisor owns the active planner assignment. Planner options only create/delete planner logs; assigning one writes to supervisor state.
        </Text>
        <Select
          label="Supervisor selected planner"
          placeholder="Select planner for this supervisor"
          value={selectedPlannerId}
          onChange={setSelectedPlannerId}
          data={planners.map((planner) => {
            const featureCount = planner.feature_count ?? planner.features?.length ?? 0;
            return {
              value: planner.id,
              label: `${planner.title} · ${featureCount} feature${featureCount === 1 ? '' : 's'}${planner.id === supervisorSelectedPlannerId ? ' · current' : ''}`,
            };
          })}
          searchable
          comboboxProps={{ withinPortal: true, zIndex: 700 }}
          disabled={loading || !supervisor}
        />
        <Group justify="space-between" align="end" wrap="nowrap">
          <TextInput
            label="New planner name"
            value={newPlannerTitle}
            onChange={(event) => setNewPlannerTitle(event.currentTarget.value)}
            style={{ flex: 1 }}
          />
          <Button size="xs" variant="light" onClick={() => void createPlanner()} loading={loading} disabled={!supervisor}>Create planner</Button>
        </Group>
        <Group justify="space-between" gap="xs" wrap="nowrap">
          <Button size="xs" color="red" variant="light" onClick={() => void deletePlanner()} loading={loading} disabled={!selectedPlannerId || selectedPlannerId === supervisorSelectedPlannerId}>
            Delete selected planner
          </Button>
          <Group gap="xs" wrap="nowrap">
            <Button size="xs" variant="default" onClick={props.onClose}>Cancel</Button>
            <Button size="xs" onClick={() => void assignPlanner()} loading={loading} disabled={!selectedPlannerId || selectedPlannerId === supervisorSelectedPlannerId}>
              Assign to supervisor
            </Button>
          </Group>
        </Group>
      </Stack>
    </Modal>
  );
}

function SupervisorCockpit(props: { supervisor: FlightDeckSupervisor; templateOptions: TemplateOption[]; navigate?: (path: string) => void; onOpenPlanner?: (supervisor: FlightDeckSupervisor, options?: OpenPlannerOptions) => void; onOpenPlannerOptions?: (supervisor: FlightDeckSupervisor) => void; onActionComplete?: () => void }) {
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
            onClick={() => props.onOpenPlannerOptions?.(supervisor)}
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
  onOpenSupervisorManagement: () => void;
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
          <Group gap="xs" wrap="nowrap">
            <Button variant="light" onClick={() => props.setFiltersOpen(!props.filtersOpen)}>{props.filtersOpen ? 'Hide filters' : 'Show filters'}</Button>
            <Button onClick={props.onOpenSupervisorManagement}>Supervisor management</Button>
          </Group>
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
  feature: { feature_id: string; title: string; current_workflow_run_id?: string | null; development_state?: string | null; has_development_diff?: boolean | null } | null;
  onClose: () => void;
  onConfirm: () => Promise<void>;
}) {
  const [submitting, setSubmitting] = useState(false);
  const hasWorkflow = Boolean(props.feature?.current_workflow_run_id);
  const hasDevelopmentDiff = Boolean(props.feature?.has_development_diff);
  const developmentState = props.feature?.development_state ?? 'queued';

  async function confirm() {
    setSubmitting(true);
    try {
      await props.onConfirm();
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
        {hasDevelopmentDiff ? (
          <Alert color="yellow" title="Workspace changes found">
            Unqueue releases the planner feature from this supervisor. Backend reconciliation removes its workflow/workspace state and discards local file changes. The planner feature itself is not deleted.
          </Alert>
        ) : hasWorkflow ? (
          <Text size="sm" c="dimmed">This releases the planner feature from the supervisor queue. Backend reconciliation will remove the associated workflow state. The planner feature itself is not deleted.</Text>
        ) : (
          <Text size="sm" c="dimmed">Unqueue removes this feature from the supervisor queue. The planner feature itself is not deleted.</Text>
        )}
        <Group gap="xs">
          <Badge size="xs" color="gray">{titleCase(developmentState)}</Badge>
          {props.feature?.current_workflow_run_id ? <Badge size="xs" color="blue">Workflow exists</Badge> : null}
        </Group>
        <Group justify="flex-end" gap="xs">
          <Button size="xs" variant="default" onClick={props.onClose} disabled={submitting}>Cancel</Button>
          <Button size="xs" color="red" onClick={() => void confirm()} loading={submitting}>Unqueue</Button>
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
  const [queuedFeatures, setQueuedFeatures] = useState<SupervisorQueuedFeature[]>([]);
  const [queueLoading, setQueueLoading] = useState(false);
  const [queueError, setQueueError] = useState<string | null>(null);
  const [plannerOptions, setPlannerOptions] = useState<PlannerWorkspace[]>([]);
  const selectedPlannerId = queue?.current_planner_id ?? null;
  const [dequeueFeature, setDequeueFeature] = useState<SupervisorQueueProjection['items'][number] | null>(null);
  const queuedFeaturesRef = useRef<SupervisorQueuedFeature[]>([]);
  const queueWriteRef = useRef<Promise<void>>(Promise.resolve());

  function updateQueuedFeatures(next: SupervisorQueuedFeature[]) {
    queuedFeaturesRef.current = next;
    setQueuedFeatures(next);
  }

  const uniquePlannerOptions = useMemo(() => {
    const seen = new Set<string>();
    return plannerOptions.filter((planner) => {
      const id = planner.id.trim();
      if (!id || seen.has(id)) return false;
      seen.add(id);
      return true;
    });
  }, [plannerOptions]);

  useEffect(() => {
    let cancelled = false;
    if (!props.opened || !supervisorId) {
      setQueue(null);
      updateQueuedFeatures([]);
      setQueueError(null);
      return () => {
        cancelled = true;
      };
    }
    setQueueLoading(true);
    setQueueError(null);
    void getSupervisorQueue(supervisorId)
      .then((next) => {
        if (cancelled) return;
        setQueue(next);
        updateQueuedFeatures(next.queued_features ?? []);
        const nextPlanners = next.planners ?? [];
        setPlannerOptions(nextPlanners as PlannerWorkspace[]);
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
  }, [props.opened, supervisorId]);

  const selectedIds = queuedFeatures.map((item) => item.feature_id);
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

    await persistQueueSelection(
      queuedFeaturesRef.current.filter(
        (queued) => queued.feature_id !== item.feature_id,
      ),
    );
  }

  async function persistQueueSelection(nextQueuedFeatures: SupervisorQueuedFeature[]) {
    if (!supervisor) return;
    const activePlannerId = selectedPlannerId;
    const featureSettings = poolSetting(supervisor, 'feature_development');
    const integrationSettings = poolSetting(supervisor, 'integration');
    const uniqueQueuedFeatures = nextQueuedFeatures.filter((item, index, rows) => item.feature_id && rows.findIndex((candidate) => candidate.feature_id === item.feature_id) === index);
    updateQueuedFeatures(uniqueQueuedFeatures);
    setQueueLoading(true);
    setQueueError(null);

    const write = async () => {
      try {
        await setSupervisorQueue(supervisor.id, uniqueQueuedFeatures, {
          workflow_template_id: featureSettings.template_id ?? null,
          integration_template_id: integrationSettings.template_id ?? null,
          feature_concurrency: featureSettings.concurrency ?? null,
          integration_policy: integrationSettings.mode === 'auto' ? 'auto' : 'manual',
          auto_start: false,
        });
        const next = await getSupervisorQueue(supervisor.id);
        setQueue(next);
        updateQueuedFeatures(next.queued_features ?? []);
        await props.onApplied();
      } catch (err) {
        setQueueError(err instanceof Error ? err.message : String(err));
      }
    };

    queueWriteRef.current = queueWriteRef.current.then(write, write);
    try {
      await queueWriteRef.current;
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
      void persistQueueSelection(queuedFeaturesRef.current.filter((item) => item.feature_id !== featureId));
      return;
    }

    const planner = uniquePlannerOptions.find((item) => item.id === selectedPlannerId);
    if (!selectedPlannerId || !planner) return;
    void persistQueueSelection([
      ...queuedFeaturesRef.current.filter((item) => item.feature_id !== featureId),
      {
        feature_id: featureId,
        planner_id: latestQueueItem(featureId)?.planner_id ?? selectedPlannerId,
        planner_title: latestQueueItem(featureId)?.planner_title ?? planner.title
      }
    ]);
  }

  function moveQueuedFeature(featureId: string, direction: -1 | 1) {
    const currentQueuedFeatures = queuedFeaturesRef.current;
    const index = currentQueuedFeatures.findIndex((item) => item.feature_id === featureId);
    const nextIndex = index + direction;
    if (index < 0 || nextIndex < 0 || nextIndex >= currentQueuedFeatures.length) return;
    const next = [...currentQueuedFeatures];
    const current = next[index];
    next[index] = next[nextIndex];
    next[nextIndex] = current;
    updateQueuedFeatures(next);
    void persistQueueSelection(next);
  }

  async function confirmDequeue() {
    if (!supervisor || !dequeueFeature) return;

    const featureId = dequeueFeature.feature_id;
    setDequeueFeature(null);

    await persistQueueSelection(
      queuedFeaturesRef.current.filter(
        (item) => item.feature_id !== featureId,
      ),
    );
  }

  async function applyQueue() {
    props.onClose();
  }

  return (
    <>
    <Modal opened={props.opened} onClose={props.onClose} title="Manage feature queue" size="calc(100vw - 160px)" centered zIndex={320}>
      <Stack gap="sm">
        <Text size="sm" c="dimmed">Queue and dequeue refined planner features for this supervisor. Planner remains the feature ledger; the supervisor owns queue execution.</Text>
        <Stack gap={2}>
          <Text size="sm" fw={700}>Queueable planner</Text>
          <Text size="sm" c="dimmed">
            {uniquePlannerOptions.find((planner) => planner.id === selectedPlannerId)?.title ?? selectedPlannerId ?? 'No supervisor planner selected'}
          </Text>
          <Text size="xs" c="dimmed">Selected by supervisor state. The queue cannot override it.</Text>
        </Stack>
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
  const [plannerOptionsSupervisor, setPlannerOptionsSupervisor] = useState<FlightDeckSupervisor | null>(null);
  const [newFineChoiceSupervisor, setNewFineChoiceSupervisor] = useState<FlightDeckSupervisor | null>(null);
  const [plannerRefinementTemplateId, setPlannerRefinementTemplateId] = useState<string | null>(null);
  const [creatingRefineFeatureId, setCreatingRefineFeatureId] = useState<string | null>(null);
  const [supervisorManagementOpen, setSupervisorManagementOpen] = useState(false);
  const [newSupervisorTitle, setNewSupervisorTitle] = useState('Supervisor');
  const [newSupervisorRootRepoPath, setNewSupervisorRootRepoPath] = useState('');
  const [creatingSupervisor, setCreatingSupervisor] = useState(false);
  const [deleteSupervisorId, setDeleteSupervisorId] = useState<string | null>(null);
  const [deletingSupervisorId, setDeletingSupervisorId] = useState<string | null>(null);
  const [settingsSupervisorId, setSettingsSupervisorId] = useState<string | null>(null);
  const [executionEventLimit, setExecutionEventLimit] = useState<number | string>(100);
  const [savingExecutionEventLimit, setSavingExecutionEventLimit] = useState(false);
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

  const settingsSupervisor = (deck?.supervisors ?? []).find((supervisor) => supervisor.id === settingsSupervisorId) ?? null;

  async function saveExecutionEventLimit() {
    if (!settingsSupervisor) return;
    const parsedLimit = typeof executionEventLimit === 'number'
      ? executionEventLimit
      : Number.parseInt(executionEventLimit, 10);
    const limit = Number.isFinite(parsedLimit) ? Math.max(10, Math.min(1000, Math.floor(parsedLimit))) : 100;
    setSavingExecutionEventLimit(true);
    try {
      await runSupervisorAction(settingsSupervisor.id, {
        action: 'update_flight_deck_settings',
        flight_deck_settings: {
          ...supervisorFlightDeckSettings(settingsSupervisor),
          execution_event_limit: limit,
        },
      });
      setExecutionEventLimit(limit);
      await refresh();
    } finally {
      setSavingExecutionEventLimit(false);
    }
  }

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

  async function createRefineWorkflowForFeature(featureId: string) {
    const supervisor = plannerSupervisor;
    const templateId = plannerRefinementTemplateId;
    const plannerId = supervisor?.selected_planner_id ?? null;
    if (!supervisor || !templateId || !plannerId) {
      setError('Supervisor planner and refine template are required before creating a fine workflow.');
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

  async function createFlightDeckSupervisor() {
    const title = newSupervisorTitle.trim() || 'Supervisor';
    const rootRepoPath = newSupervisorRootRepoPath.trim();
    if (!rootRepoPath) {
      setError('Root repo path is required.');
      return;
    }

    setCreatingSupervisor(true);
    setError(null);
    try {
      await createSupervisorRun({
        title,
        root_repo_path: rootRepoPath,
        strategy: 'series',
        workflow_template_id: null,
        integration_template_id: null,
        feature_plan_items: [],
        execution_plan_items: [],
        context: {}
      });
      setNewSupervisorTitle('Supervisor');
      setNewSupervisorRootRepoPath('');
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setCreatingSupervisor(false);
    }
  }

  async function removeFlightDeckSupervisor(supervisor: FlightDeckSupervisor) {
    const confirmed = window.confirm(`Delete supervisor "${supervisor.title}"? This removes the supervisor record and cannot be undone from Flight Deck.`);
    if (!confirmed) return;

    setDeletingSupervisorId(supervisor.id);
    setError(null);
    try {
      await deleteSupervisorRun(supervisor.id);
      setDeleteSupervisorId((current) => current === supervisor.id ? null : current);
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingSupervisorId(null);
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
          onOpenSupervisorManagement={() => setSupervisorManagementOpen(true)}
        />


        {error ? <Alert color="red">{error}</Alert> : null}
        {creatingRefineFeatureId ? <Alert color="blue">Creating refine workflow for selected feature…</Alert> : null}
        {loading && !deck ? <Loader /> : null}

        <Stack gap="lg" pr="sm">
          {deck?.supervisors.length ? deck.supervisors.map((supervisor) => (
            <SupervisorCockpit key={supervisor.id} supervisor={supervisor} templateOptions={templateOptions} navigate={props.navigate} onOpenPlanner={openPlanner} onOpenPlannerOptions={setPlannerOptionsSupervisor} onActionComplete={() => void refresh()} />
          )) : !loading ? (
            <Card withBorder radius="xl" p="lg">
              <Text c="dimmed">No supervisors matched the current filters.</Text>
            </Card>
          ) : null}
        </Stack>
      <Modal
        opened={newFineChoiceSupervisor !== null}
        onClose={() => setNewFineChoiceSupervisor(null)}
        title="Refine feature"
        centered
        size="md"
      >
        <Stack gap="sm">
          <Text size="sm" c="dimmed">Start a refinement workflow from an existing planner feature or create a new planner feature first.</Text>
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

            }}>Existing feature</Button>
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
      <Modal opened={supervisorManagementOpen} onClose={() => setSupervisorManagementOpen(false)} title="Supervisor management" centered size="lg">
        <Stack gap="lg">
          <Stack gap="sm">
            <Text fw={700}>Create supervisor</Text>
            <TextInput label="Title" value={newSupervisorTitle} onChange={(event) => setNewSupervisorTitle(event.currentTarget.value)} />
            <TextInput label="Root repo path" value={newSupervisorRootRepoPath} onChange={(event) => setNewSupervisorRootRepoPath(event.currentTarget.value)} />
            <Group justify="flex-end">
              <Button onClick={() => void createFlightDeckSupervisor()} loading={creatingSupervisor}>Create supervisor</Button>
            </Group>
          </Stack>

          <Divider />

          <Stack gap="sm">
            <Text fw={700}>Execution history</Text>
            <Select
              label="Supervisor"
              placeholder="Select supervisor"
              value={settingsSupervisorId}
              onChange={(value) => {
                setSettingsSupervisorId(value);
                const supervisor = (deck?.supervisors ?? []).find((item) => item.id === value);
                setExecutionEventLimit(supervisor ? supervisorExecutionEventLimit(supervisor) : 100);
              }}
              data={(deck?.supervisors ?? []).map((supervisor) => ({ value: supervisor.id, label: supervisor.title }))}
              searchable
              clearable
            />
            <NumberInput
              label="Retained execution events per workflow"
              description="Controls how many recent stage and capability execution events Flight Deck retains in each workflow projection."
              value={executionEventLimit}
              onChange={setExecutionEventLimit}
              min={10}
              max={1000}
              step={10}
              clampBehavior="strict"
              disabled={!settingsSupervisor}
            />
            <Group justify="flex-end">
              <Button
                variant="light"
                disabled={!settingsSupervisor}
                loading={savingExecutionEventLimit}
                onClick={() => void saveExecutionEventLimit()}
              >
                Save execution history setting
              </Button>
            </Group>
          </Stack>

          <Divider />

          <Divider />

          <Stack gap="sm">
            <Text fw={700}>Delete supervisor</Text>
            <Select
              label="Supervisor"
              placeholder="Select supervisor"
              value={deleteSupervisorId}
              onChange={setDeleteSupervisorId}
              data={(deck?.supervisors ?? []).map((supervisor) => ({ value: supervisor.id, label: supervisor.title }))}
              searchable
              clearable
            />
            <Group justify="flex-end">
              <Button
                color="red"
                variant="light"
                disabled={!deleteSupervisorId}
                loading={deleteSupervisorId ? deletingSupervisorId === deleteSupervisorId : false}
                onClick={() => {
                  const supervisor = (deck?.supervisors ?? []).find((item) => item.id === deleteSupervisorId);
                  if (supervisor) void removeFlightDeckSupervisor(supervisor);
                }}
              >
                Delete selected supervisor
              </Button>
            </Group>
          </Stack>
        </Stack>
      </Modal>
      <FeatureQueueModal
        opened={queueSupervisor !== null}
        supervisor={queueSupervisor ? deck?.supervisors.find((item) => item.id === queueSupervisor.id) ?? queueSupervisor : null}
        onClose={() => setQueueSupervisor(null)}
        onApplied={refresh}
      />
      <SupervisorPlannerOptionsModal
        opened={plannerOptionsSupervisor !== null}
        supervisor={plannerOptionsSupervisor ? deck?.supervisors.find((item) => item.id === plannerOptionsSupervisor.id) ?? plannerOptionsSupervisor : null}
        onClose={() => setPlannerOptionsSupervisor(null)}
        onApplied={refresh}
        onError={setError}
      />
      <PlannerModal
        opened={plannerSupervisor !== null}
        rootRepoPath={plannerSupervisor?.root_repo_path ?? ''}
        run={null}
        templates={templates}
        selectedPlannerId={plannerSupervisor?.selected_planner_id ?? null}
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
          if (!selection.feature?.id) return;
          if (plannerRefinementTemplateId) {
            await createRefineWorkflowForFeature(selection.feature.id);
          }
        }}
        onFeatureCreated={async (selection) => {
          if (!selection.feature?.id) return;
          if (plannerRefinementTemplateId) {
            await createRefineWorkflowForFeature(selection.feature.id);
          }
        }}
        onSaved={refresh}
        onError={setError}
      />
      </Stack>
    </Box>
  );
}
