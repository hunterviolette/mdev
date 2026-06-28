import { Alert, Anchor, Badge, Button, Card, Group, Modal, NumberInput, Progress, Select, Stack, Table, Text } from '@mantine/core';
import { useEffect, useMemo, useState } from 'react';
import { getRun, pauseWorkflowRun, resumeWorkflowRun, startWorkflowRun, type WorkflowRun, type WorkflowTemplate } from './api';
import { runSupervisorAction, updateSupervisorPlan, type SupervisorChildRun, type SupervisorExecutionStrategy, type SupervisorRun } from './supervisor_api';

type Props = {
  opened: boolean;
  run: SupervisorRun | null | undefined;
  templates: WorkflowTemplate[];
  onClose: () => void;
  onOpenPlanner: () => void;
  onChanged: () => Promise<void> | void;
};

type WorkflowProjection = {
  run_id?: string;
  status?: string;
  current_step_id?: string | null;
  current_stage?: string | null;
  current_stage_name?: string | null;
  latest_event?: {
    kind?: string;
    message?: string;
    created_at?: string;
  } | null;
  latest_message?: string | null;
  summary?: string | null;
};

function sprintTitle(run: SupervisorRun | null | undefined, featurePlanItemId: string): string {
  return run?.feature_plan_items.find((item) => item.id === featurePlanItemId)?.title ?? featurePlanItemId;
}

function canApply(run: SupervisorRun): boolean {
  return run.status === 'ready_to_apply'
    && Boolean(run.integration_run_id)
    && Boolean(run.final_patch_path);
}

function canStartIntegration(run: SupervisorRun, integrationTemplateId: string | null, progress?: { completed: number; total: number }): boolean {
  const developmentComplete = progress ? progress.total > 0 && progress.completed >= progress.total : run.status === 'development_complete';
  return Boolean(integrationTemplateId) && developmentComplete && ['development_complete', 'running_integration', 'ready_to_apply', 'failed'].includes(run.status) && run.child_runs.length > 0;
}

function canCancel(status: string): boolean {
  return ['snapshotting', 'running_children', 'running_integration', 'validating'].includes(status);
}

function canRestartIntegration(run: SupervisorRun, integrationTemplateId: string | null): boolean {
  const hasIntegrationTarget = Boolean(integrationTemplateId) || Boolean(run.integration_run_id) || typeof run.context?.integration_template_id === 'string';
  return hasIntegrationTarget && run.child_runs.length > 0 && ['running_integration', 'validating', 'ready_to_apply', 'failed'].includes(run.status);
}

function canStart(status: string): boolean {
  return ['created', 'cancelled', 'failed'].includes(status);
}

function canStartNextSprint(status: string): boolean {
  return ['applied', 'ready_to_apply', 'failed', 'cancelled'].includes(status);
}

function canRestartSprint(status: string): boolean {
  return ['snapshotting', 'running_children', 'development_complete', 'running_integration', 'validating', 'ready_to_apply', 'failed', 'cancelled'].includes(status);
}

function statusBadgeColor(status: string): string {
  if (['success', 'development_complete', 'ready_to_apply', 'applied', 'completed'].includes(status)) return 'green';
  if (['error', 'failed'].includes(status)) return 'red';
  if (status === 'cancelled') return 'gray';
  if (['snapshotting', 'running', 'running_children', 'running_integration', 'validating', 'queued'].includes(status)) return 'blue';
  if (['waiting', 'paused'].includes(status)) return 'yellow';
  return 'gray';
}

function formatValue(value: string | null | undefined): string {
  return value && value.length > 0 ? value : '—';
}

function shortId(value: string | null | undefined): string {
  if (!value) return '—';
  return value.length > 12 ? `${value.slice(0, 8)}…${value.slice(-4)}` : value;
}

function workflowRoute(runId: string) {
  return `/workflows/${encodeURIComponent(runId)}`;
}

function WorkflowRunAnchor({ runId }: { runId: string | null | undefined }) {
  if (!runId) return <>—</>;
  return <Anchor href={workflowRoute(runId)}>{shortId(runId)}</Anchor>;
}

function workflowName(templates: WorkflowTemplate[], id: string | null | undefined): string {
  if (!id) return '—';
  return templates.find((template) => template.id === id)?.name ?? id;
}

function currentWorkflowStage(run: WorkflowRun | undefined): string {
  if (!run?.current_step_id) return '—';
  const step = run.definition.steps.find((item) => item.id === run.current_step_id);
  if (!step) return shortId(run.current_step_id);
  return step.name || step.label || step.step_type || shortId(step.id);
}

function workflowAutomationLabel(status: string): string {
  if (status === 'paused') return 'Resume automation';
  if (status === 'waiting') return 'Run autonomous';
  if (status === 'draft') return 'Run autonomous';
  return 'Run autonomous';
}

function workflowIsComplete(status: string): boolean {
  return ['success', 'completed'].includes(status);
}

function projectionStage(projection: WorkflowProjection | undefined, workflowRun: WorkflowRun | undefined): string {
  return projection?.current_stage_name
    ?? projection?.current_stage
    ?? currentWorkflowStage(workflowRun);
}

function projectionMessage(projection: WorkflowProjection | undefined): string {
  return projection?.latest_event?.message
    ?? projection?.latest_message
    ?? projection?.summary
    ?? '—';
}

function sprintStageState(run: SupervisorRun, stage: 'development' | 'integration' | 'apply', progress?: { completed: number; total: number }): 'active' | 'complete' | 'up_next' | 'blocked' {
  if (stage === 'development') {
    if (progress && progress.total > 0 && progress.completed < progress.total) return 'active';
    if (['created', 'snapshotting', 'running_children'].includes(run.status)) return 'active';
    if (['development_complete', 'running_integration', 'validating', 'ready_to_apply', 'applied'].includes(run.status)) return 'complete';
    return 'blocked';
  }
  if (stage === 'integration') {
    if (['running_integration', 'validating'].includes(run.status)) return 'active';
    if (['ready_to_apply', 'applied'].includes(run.status)) return 'complete';
    if (progress && progress.total > 0 && progress.completed < progress.total) return 'blocked';
    if (run.status === 'development_complete') return 'up_next';
    return 'blocked';
  }
  if (run.status === 'applied') return 'complete';
  if (run.status === 'ready_to_apply') return 'active';
  return 'up_next';
}

function sprintStageBadgeColor(state: 'active' | 'complete' | 'up_next' | 'blocked'): string {
  if (state === 'complete') return 'green';
  if (state === 'active') return 'blue';
  if (state === 'blocked') return 'red';
  return 'gray';
}

function sprintStageLabel(state: 'active' | 'complete' | 'up_next' | 'blocked'): string {
  if (state === 'complete') return 'COMPLETE';
  if (state === 'active') return 'ACTIVE';
  if (state === 'blocked') return 'BLOCKED';
  return 'UP NEXT';
}

function isChildDone(child: SupervisorChildRun): boolean {
  return ['success', 'completed'].includes(child.status);
}

function isChildFailed(child: SupervisorChildRun): boolean {
  return ['error', 'cancelled'].includes(child.status);
}

function developmentItemDone(run: SupervisorRun, featureId: string, child?: SupervisorChildRun): boolean {
  const feature = run.feature_plan_items.find((item) => item.id === featureId);
  return ['completed', 'applied'].includes(String(feature?.status ?? '')) || Boolean(child && isChildDone(child));
}

function developmentItemStatus(run: SupervisorRun, featureId: string, child: SupervisorChildRun | undefined, liveStatus: string): string {
  if (developmentItemDone(run, featureId, child)) return 'complete';
  if (child) return liveStatus;
  if (run.status === 'running_children') return 'queued';
  return 'scheduled';
}

export function SupervisorSprintModal({ opened, run, templates, onClose, onOpenPlanner, onChanged }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [workflowTemplateId, setWorkflowTemplateId] = useState<string | null>(null);
  const [integrationTemplateId, setIntegrationTemplateId] = useState<string | null>(null);
  const [strategy, setStrategy] = useState<SupervisorExecutionStrategy>('series');
  const [featureConcurrency, setFeatureConcurrency] = useState(1);
  const [integrationPolicy, setIntegrationPolicy] = useState<'auto' | 'manual'>('manual');
  const [workflowRunsById, setWorkflowRunsById] = useState<Record<string, WorkflowRun>>({});
  const [workflowProjectionsById, setWorkflowProjectionsById] = useState<Record<string, WorkflowProjection>>({});

  const templateOptions = useMemo(() => templates.map((template) => ({ value: template.id, label: template.name })), [templates]);

  useEffect(() => {
    if (!opened || !run) return;
    setStrategy(run.strategy);
    const rawFeatureConcurrency = run.context?.feature_concurrency;
    setFeatureConcurrency(typeof rawFeatureConcurrency === 'number' && Number.isFinite(rawFeatureConcurrency) ? Math.max(1, Math.floor(rawFeatureConcurrency)) : 1);
    setIntegrationPolicy(run.context?.integration_policy === 'auto' ? 'auto' : 'manual');
    setWorkflowTemplateId(typeof run.context?.workflow_template_id === 'string' ? run.context.workflow_template_id : null);
    setIntegrationTemplateId(typeof run.context?.integration_template_id === 'string' ? run.context.integration_template_id : null);
    setError(null);
  }, [opened, run?.id]);

  useEffect(() => {
    if (!opened || !run) return;
    if (!['snapshotting', 'running_children', 'development_complete', 'running_integration', 'validating'].includes(run.status)) return;
    const timer = window.setInterval(() => {
      void onChanged();
    }, 1500);
    return () => window.clearInterval(timer);
  }, [opened, run?.id, run?.status, onChanged]);

  useEffect(() => {
    if (!opened || !run) return;
    void refreshWorkflowRunDetails();
    void refreshWorkflowProjections();
    if (run.child_runs.length === 0) return;
    const timer = window.setInterval(() => {
      void refreshWorkflowRunDetails();
      void refreshWorkflowProjections();
    }, 1500);
    return () => window.clearInterval(timer);
  }, [opened, run?.id, run?.child_runs.length]);

  const scheduledItemsForStart = useMemo(() => {
    if (!run) return [];
    return (run.execution_plan_items ?? []).map((item, index) => ({
      ...item,
      workflow_template_id: item.workflow_template_id ?? workflowTemplateId,
      order_index: item.order_index ?? index
    }));
  }, [run, workflowTemplateId]);

  const progress = useMemo(() => {
    if (!run) return { completed: 0, failed: 0, total: 0, percent: 0 };
    const total = scheduledItemsForStart.length;
    const completed = scheduledItemsForStart.filter((item) => {
      const child = run.child_runs.find((entry) => entry.execution_item_id === item.feature_plan_item_id);
      return developmentItemDone(run, item.feature_plan_item_id, child);
    }).length;
    const failed = scheduledItemsForStart.filter((item) => {
      const child = run.child_runs.find((entry) => entry.execution_item_id === item.feature_plan_item_id);
      return Boolean(child && isChildFailed(child));
    }).length;
    return {
      completed,
      failed,
      total,
      percent: total > 0 ? Math.round((completed / total) * 100) : 0
    };
  }, [run, scheduledItemsForStart]);

  useEffect(() => {
    if (!opened || !run) return;
    if (progress.total === 0 || progress.completed >= progress.total) return;
    const missingChild = scheduledItemsForStart.some((item) => !run.child_runs.some((child) => child.execution_item_id === item.feature_plan_item_id));
    if (!missingChild && run.status === 'running_children') return;
    const timer = window.setTimeout(() => {
      void action('tick');
    }, 250);
    return () => window.clearTimeout(timer);
  }, [opened, run?.id, run?.status, progress.completed, progress.total, scheduledItemsForStart, run?.child_runs.length]);

  async function refreshWorkflowRunDetails() {
    const childRunIds = (run?.child_runs ?? [])
      .map((child) => child.workflow_run_id)
      .filter((value): value is string => Boolean(value));
    if (childRunIds.length === 0) {
      setWorkflowRunsById({});
      return;
    }
    const pairs = await Promise.all(
      childRunIds.map(async (runId) => {
        try {
          return [runId, await getRun(runId)] as const;
        } catch {
          return null;
        }
      })
    );
    setWorkflowRunsById(Object.fromEntries(pairs.filter((item): item is readonly [string, WorkflowRun] => Boolean(item))));
  }

  async function refreshWorkflowProjections() {
    const childRunIds = (run?.child_runs ?? [])
      .map((child) => child.workflow_run_id)
      .filter((value): value is string => Boolean(value));
    if (childRunIds.length === 0) {
      setWorkflowProjectionsById({});
      return;
    }
    const pairs = await Promise.all(
      childRunIds.map(async (runId) => {
        try {
          const response = await fetch(`/api/events/projection?run_id=${encodeURIComponent(runId)}`);
          if (!response.ok) return null;
          return [runId, await response.json() as WorkflowProjection] as const;
        } catch {
          return null;
        }
      })
    );
    setWorkflowProjectionsById(Object.fromEntries(pairs.filter((item): item is readonly [string, WorkflowProjection] => Boolean(item))));
  }

  async function startSprint() {
    if (!run) return;
    setError(null);
    try {
      if (!workflowTemplateId) {
        setError('Select a default feature workflow template before starting the sprint.');
        return;
      }
      if (!integrationTemplateId) {
        setError('Select an integration workflow before starting the sprint.');
        return;
      }
      const sprintItems = scheduledItemsForStart;
      if (sprintItems.length === 0) {
        setError('No planner features are scheduled for this sprint.');
        return;
      }
      await saveSprintSettings();
      await runSupervisorAction(run.id, 'start');
      await onChanged();
    } catch (err) {
      setError(String(err));
    }
  }

  async function action(actionName: 'tick' | 'apply' | 'cancel' | 'start_integration' | 'restart_integration' | 'restart_sprint' | 'reopen_development' | 'new_sprint') {
    if (!run) return;
    setError(null);
    try {
      if (actionName === 'start_integration') {
        if (!integrationTemplateId) {
          setError('Select an integration workflow before starting integration.');
          return;
        }
        await updateSupervisorPlan(run.id, run.feature_plan_items, scheduledItemsForStart, {
          sprint_strategy: strategy,
          workflow_template_id: workflowTemplateId,
          integration_template_id: integrationTemplateId,
          feature_concurrency: featureConcurrency,
          integration_policy: integrationPolicy
        });
      }
      if (actionName === 'restart_integration' && integrationTemplateId) {
        await updateSupervisorPlan(run.id, run.feature_plan_items, scheduledItemsForStart, {
          sprint_strategy: strategy,
          workflow_template_id: workflowTemplateId,
          integration_template_id: integrationTemplateId,
          feature_concurrency: featureConcurrency,
          integration_policy: integrationPolicy
        });
      }
      await runSupervisorAction(run.id, actionName);
      await onChanged();
    } catch (err) {
      setError(String(err));
    }
  }

  async function workflowAutomationAction(child: SupervisorChildRun, actionName: 'start' | 'pause' | 'resume') {
    if (!child.workflow_run_id) return;
    setError(null);
    try {
      if (actionName === 'pause') {
        await pauseWorkflowRun(child.workflow_run_id);
      } else if (actionName === 'resume') {
        await resumeWorkflowRun(child.workflow_run_id);
      } else {
        await startWorkflowRun(child.workflow_run_id);
      }
      await onChanged();
      await refreshWorkflowRunDetails();
    } catch (err) {
      setError(String(err));
    }
  }

  async function removeChildWorkflow(child: SupervisorChildRun) {
    if (!run) return;
    const confirmed = window.confirm(`Delete workflow for ${child.title}? This will invalidate any integration workflow for this sprint.`);
    if (!confirmed) return;
    setError(null);
    try {
      await runSupervisorAction(run.id, 'remove_child_workflow', {
        execution_item_id: child.execution_item_id,
        workflow_run_id: child.workflow_run_id
      } as any);
      await onChanged();
      await refreshWorkflowRunDetails();
      await refreshWorkflowProjections();
    } catch (err) {
      setError(String(err));
    }
  }

  function allLiveChildWorkflowsSuccessful(): boolean {
    if (!run || run.child_runs.length === 0) return false;
    return run.child_runs.every((child) => {
      const workflowRun = child.workflow_run_id ? workflowRunsById[child.workflow_run_id] : undefined;
      const projection = child.workflow_run_id ? workflowProjectionsById[child.workflow_run_id] : undefined;
      const liveStatus = projection?.status ?? workflowRun?.status ?? child.status;
      return liveStatus === 'success';
    });
  }

  return (
    <Modal
      opened={opened}
      onClose={onClose}
      title={run ? `${run.title} sprint` : 'Sprint'}
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
        {run ? (
          <>
            <Card withBorder>
              <Stack gap="sm">
                <Group justify="space-between">
                  <Group>
                    <Text fw={700}>{run.title}</Text>
                    <Badge color={statusBadgeColor(run.status)}>{run.status}</Badge>
                    <Badge variant="light">{strategy}</Badge>
                  </Group>
                  <Group gap="xs">
                    <Text size="sm" c="dimmed">
                      Development: {progress.completed}/{progress.total} complete{progress.failed > 0 ? `, ${progress.failed} failed` : ''}
                    </Text>
                  </Group>
                </Group>

                <Group grow align="stretch">
                  {(['development', 'integration', 'apply'] as const).map((stage, index) => {
                    const stageState = sprintStageState(run, stage, progress);
                    return (
                      <Card key={stage} withBorder>
                        <Stack gap={4}>
                          <Badge w="fit-content" color={sprintStageBadgeColor(stageState)}>{index + 1}</Badge>
                          <Text fw={700}>{stage === 'development' ? 'Development' : stage === 'integration' ? 'Integration' : 'Apply'}</Text>
                          <Text size="xs" c="dimmed">
                            {stage === 'development' ? 'Feature workflows' : stage === 'integration' ? 'Live worktree integration' : 'Complete sprint'}
                          </Text>
                          <Badge w="fit-content" color={sprintStageBadgeColor(stageState)}>{sprintStageLabel(stageState)}</Badge>
                          {stage === 'development' ? (
                            <Stack gap={4} mt="xs">
                              <Group justify="space-between" gap="xs">
                                <Text size="xs" c="dimmed">{progress.completed}/{progress.total} complete</Text>
                                <Text size="xs" c="dimmed">{progress.percent}%</Text>
                              </Group>
                              <Progress value={progress.percent} />
                            </Stack>
                          ) : null}
                          {stage === 'development' && ['running_integration', 'ready_to_apply', 'applied', 'failed'].includes(run.status) ? (
                            <Button mt="xs" size="xs" variant="light" onClick={() => action('reopen_development')}>Reopen development</Button>
                          ) : null}
                          {stage === 'development' && run.status === 'applied' ? (
                            <Button mt="xs" size="xs" variant="light" onClick={() => action('new_sprint')}>Start next sprint</Button>
                          ) : null}
                          {stage === 'integration' && stageState === 'up_next' ? (
                            <Button mt="xs" size="xs" disabled={!canStartIntegration(run, integrationTemplateId, progress)} onClick={() => action('start_integration')}>Start integration</Button>
                          ) : null}
                          {stage === 'integration' && ['running_integration', 'ready_to_apply', 'applied', 'failed'].includes(run.status) ? (
                            <Button mt="xs" size="xs" variant="light" disabled={!canRestartIntegration(run, integrationTemplateId)} onClick={() => action('restart_integration')}>Restart integration</Button>
                          ) : null}
                          {stage === 'apply' && stageState === 'active' ? (
                            <Button mt="xs" size="xs" disabled={!canApply(run)} onClick={() => action('apply')}>Apply sprint</Button>
                          ) : null}
                        </Stack>
                      </Card>
                    );
                  })}
                </Group>

                {canStart(run.status) || ['running_children', 'development_complete', 'running_integration', 'validating', 'ready_to_apply', 'failed'].includes(run.status) ? (
                  <Stack gap="sm">
                    <Group justify="space-between">
                      <Group gap="xs">
                        <Text fw={700}>Sprint settings</Text>
                        <Badge variant="light">Integration: {integrationPolicy === 'auto' ? 'Auto-run' : 'Manual'}</Badge>
                      </Group>
                      <Group gap="xs">
                        <Button size="xs" variant="light" onClick={() => void saveSprintSettings()}>Save settings</Button>
                        {canStart(run.status) ? (
                          <Button size="xs" onClick={() => void startSprint()}>Start sprint</Button>
                        ) : null}
                        {allLiveChildWorkflowsSuccessful() && !run.integration_run_id ? (
                          <Button size="xs" disabled={!canStartIntegration(run, integrationTemplateId, progress)} onClick={() => void action('start_integration')}>Start integration</Button>
                        ) : null}
                        {canRestartSprint(run.status) ? (
                          <Button size="xs" color="red" variant="light" onClick={() => action('restart_sprint')}>Restart whole sprint</Button>
                        ) : null}
                      </Group>
                    </Group>
                    <Group grow align="flex-end">
                      <Select
                        label="Workflow template"
                        placeholder="Select workflow template"
                        value={workflowTemplateId}
                        onChange={setWorkflowTemplateId}
                        data={templateOptions}
                        searchable
                      />
                      <Select
                        label="Execution mode"
                        value={strategy}
                        onChange={(value) => setStrategy((value as SupervisorExecutionStrategy) ?? 'series')}
                        data={[
                          { value: 'series', label: 'Series' },
                          { value: 'parallel', label: 'Parallel' }
                        ]}
                        allowDeselect={false}
                      />
                      <NumberInput
                        label="Feature concurrency"
                        min={1}
                        max={64}
                        value={featureConcurrency}
                        onChange={(value) => {
                          const next = typeof value === 'number' ? value : Number(value);
                          setFeatureConcurrency(Number.isFinite(next) ? Math.max(1, Math.floor(next)) : 1);
                        }}
                      />
                      <Select
                        label="Integration"
                        value={integrationPolicy}
                        onChange={(value) => setIntegrationPolicy(value === 'auto' ? 'auto' : 'manual')}
                        data={[
                          { value: 'manual', label: 'Manual start after development' },
                          { value: 'auto', label: 'Auto-run after development' }
                        ]}
                        allowDeselect={false}
                      />
                    </Group>

                    <Select
                      label="Integration workflow"
                      placeholder="Select integration workflow"
                      value={integrationTemplateId}
                      onChange={setIntegrationTemplateId}
                      data={templateOptions}
                      searchable
                    />
                  </Stack>
                ) : null}
              </Stack>
            </Card>

            <Card withBorder>
              <Stack gap="sm">
                <Group justify="space-between">
                  <Group>
                    <Text fw={700}>Workflows</Text>
                    <Badge variant="light">{scheduledItemsForStart.length}</Badge>
                  </Group>
                  <Button size="xs" variant="light" onClick={onOpenPlanner}>Manage features in planner</Button>
                </Group>

                {scheduledItemsForStart.length > 0 ? (
                  <>
                  <Table striped withTableBorder>
                    <Table.Thead>
                      <Table.Tr>
                        <Table.Th>Phase</Table.Th>
                        <Table.Th>Item</Table.Th>
                        <Table.Th>Workflow template</Table.Th>
                        <Table.Th>Status</Table.Th>
                        <Table.Th>Stage</Table.Th>
                        <Table.Th>Live projection</Table.Th>
                        <Table.Th>Controls</Table.Th>
                      </Table.Tr>
                    </Table.Thead>
                    <Table.Tbody>
                      {scheduledItemsForStart.map((item, index) => {
                        const child = run.child_runs.find((entry) => entry.execution_item_id === item.feature_plan_item_id);
                        const workflowRun = child?.workflow_run_id ? workflowRunsById[child.workflow_run_id] : undefined;
                        const projection = child?.workflow_run_id ? workflowProjectionsById[child.workflow_run_id] : undefined;
                        const rawLiveStatus = projection?.status ?? workflowRun?.status ?? child?.status ?? 'scheduled';
                        const liveStatus = developmentItemStatus(run, item.feature_plan_item_id, child, rawLiveStatus);
                        const canPauseWorkflow = Boolean(child) && ['queued', 'running', 'waiting'].includes(liveStatus);
                        const canResumeWorkflow = Boolean(child) && liveStatus === 'paused';
                        const canStartWorkflow = Boolean(child) && ['draft', 'waiting'].includes(liveStatus);
                        return (
                          <Table.Tr key={`${item.feature_plan_item_id}-${index}`}>
                            <Table.Td>Development</Table.Td>
                            <Table.Td>
                              {child?.workflow_run_id ? (
                                <Anchor href={workflowRoute(child.workflow_run_id)}>{sprintTitle(run, item.feature_plan_item_id)}</Anchor>
                              ) : (
                                sprintTitle(run, item.feature_plan_item_id)
                              )}
                            </Table.Td>
                            <Table.Td>{workflowName(templates, item.workflow_template_id ?? workflowTemplateId)}</Table.Td>
                            <Table.Td><Badge color={statusBadgeColor(liveStatus)}>{liveStatus}</Badge></Table.Td>
                            <Table.Td>{projectionStage(projection, workflowRun)}</Table.Td>
                            <Table.Td>
                              <Text size="xs" lineClamp={3}>{projectionMessage(projection)}</Text>
                            </Table.Td>
                            <Table.Td>
                              {workflowIsComplete(liveStatus) && child ? (
                                <Button size="xs" color="red" variant="subtle" onClick={() => void removeChildWorkflow(child)}>Delete</Button>
                              ) : child ? (
                                <Group gap="xs" wrap="nowrap">
                                  {canPauseWorkflow ? (
                                    <Button size="xs" variant="light" color="yellow" onClick={() => void workflowAutomationAction(child, 'pause')}>Pause after stage</Button>
                                  ) : null}
                                  {canResumeWorkflow ? (
                                    <Button size="xs" onClick={() => void workflowAutomationAction(child, 'resume')}>Resume automation</Button>
                                  ) : null}
                                  {canStartWorkflow ? (
                                    <Button size="xs" variant="light" onClick={() => void workflowAutomationAction(child, 'start')}>{workflowAutomationLabel(liveStatus)}</Button>
                                  ) : null}
                                  <Button size="xs" color="red" variant="subtle" onClick={() => void removeChildWorkflow(child)}>Delete</Button>
                                </Group>
                              ) : (
                                <Text size="xs" c="dimmed">Queued for supervisor</Text>
                              )}
                            </Table.Td>
                          </Table.Tr>
                        );
                      })}
                      <Table.Tr>
                        <Table.Td>Integration</Table.Td>
                        <Table.Td>
                          {run.integration_run_id ? (
                            <Anchor href={workflowRoute(run.integration_run_id)}>Integration workflow</Anchor>
                          ) : (
                            'Integration workflow'
                          )}
                        </Table.Td>
                        <Table.Td>{workflowName(templates, integrationTemplateId)}</Table.Td>
                        <Table.Td><Badge color={statusBadgeColor(run.integration_run_id ? run.status : 'scheduled')}>{run.integration_run_id ? run.status : 'scheduled'}</Badge></Table.Td>
                        <Table.Td>{run.integration_run_id ? 'Integration' : '—'}</Table.Td>
                        <Table.Td>
                          <Text size="xs" lineClamp={3}>{run.integration_run_id ? 'Integration workflow is being orchestrated by the supervisor.' : 'Waiting for development completion.'}</Text>
                        </Table.Td>
                        <Table.Td>
                          <Group gap="xs" wrap="nowrap">
                            {allLiveChildWorkflowsSuccessful() && !run.integration_run_id ? (
                              <Button size="xs" disabled={!integrationTemplateId} onClick={() => void action('start_integration')}>Start integration</Button>
                            ) : null}
                            {run.integration_run_id && ['running_integration', 'ready_to_apply', 'failed'].includes(run.status) ? (
                              <Button size="xs" variant="light" onClick={() => void action('restart_integration')}>Restart integration</Button>
                            ) : null}
                            {!run.integration_run_id ? (
                              <Text size="xs" c="dimmed">Uses integration template</Text>
                            ) : null}
                          </Group>
                        </Table.Td>
                      </Table.Tr>
                    </Table.Tbody>
                  </Table>
                  </>
                ) : (
                  <Text size="sm" c="dimmed">No planner features are scheduled for this sprint. Use the planner to add features to the next sprint.</Text>
                )}
              </Stack>
            </Card>

            {Array.isArray(run.context?.sprint_history) && run.context.sprint_history.length > 0 ? (
              <Card withBorder>
                <Stack gap="xs">
                  <Group justify="space-between">
                    <Text fw={700}>Sprint history</Text>
                    <Badge variant="light">{run.context.sprint_history.length}</Badge>
                  </Group>
                  <Stack gap="sm">
                    {(run.context.sprint_history as any[]).map((sprint, index) => {
                      const childRuns = Array.isArray(sprint.child_runs) ? sprint.child_runs : [];
                      const features = Array.isArray(sprint.features) ? sprint.features : [];
                      return (
                        <Card key={String(sprint.sprint_id ?? index)} withBorder>
                          <Stack gap="xs">
                            <Group justify="space-between">
                              <Group>
                                <Text fw={600}>{String(sprint.title ?? sprint.sprint_id ?? `Sprint ${index + 1}`)}</Text>
                                <Badge color={statusBadgeColor(String(sprint.status ?? 'applied'))}>{String(sprint.status ?? 'applied')}</Badge>
                              </Group>
                              <Text size="xs" c="dimmed">{String(sprint.applied_at ?? '—')}</Text>
                            </Group>
                            <Table striped withTableBorder>
                              <Table.Thead>
                                <Table.Tr>
                                  <Table.Th>Feature</Table.Th>
                                  <Table.Th>Applied</Table.Th>
                                  <Table.Th>Workflow run</Table.Th>
                                  <Table.Th>Status</Table.Th>
                                  <Table.Th>Patch path</Table.Th>
                                </Table.Tr>
                              </Table.Thead>
                              <Table.Tbody>
                                {features.map((feature: any, featureIndex: number) => {
                                  const child = childRuns.find((item: any) => item.execution_item_id === feature.id || item.title === feature.title);
                                  return (
                                    <Table.Tr key={String(feature.id ?? featureIndex)}>
                                      <Table.Td>{String(feature.title ?? feature.id ?? `Feature ${featureIndex + 1}`)}</Table.Td>
                                      <Table.Td>{String(feature.applied_at ?? sprint.applied_at ?? '—')}</Table.Td>
                                      <Table.Td><WorkflowRunAnchor runId={child?.workflow_run_id} /></Table.Td>
                                      <Table.Td>{child?.status ? <Badge color={statusBadgeColor(String(child.status))}>{String(child.status)}</Badge> : '—'}</Table.Td>
                                      <Table.Td>{formatValue(child?.patch_path)}</Table.Td>
                                    </Table.Tr>
                                  );
                                })}
                              </Table.Tbody>
                            </Table>
                            <Table withTableBorder>
                              <Table.Tbody>
                                <Table.Tr>
                                  <Table.Th>Integration workflow run</Table.Th>
                                  <Table.Td><WorkflowRunAnchor runId={sprint.integration_run_id} /></Table.Td>
                                </Table.Tr>
                                <Table.Tr>
                                  <Table.Th>Final patch</Table.Th>
                                  <Table.Td>{formatValue(sprint.final_patch_path)}</Table.Td>
                                </Table.Tr>
                              </Table.Tbody>
                            </Table>
                          </Stack>
                        </Card>
                      );
                    })}
                  </Stack>
                </Stack>
              </Card>
            ) : null}

          </>
        ) : null}
      </Stack>
    </Modal>
  );
}
