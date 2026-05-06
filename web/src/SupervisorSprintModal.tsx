import { Alert, Badge, Button, Card, Group, Modal, Select, Stack, Table, Text } from '@mantine/core';
import { useEffect, useMemo, useState } from 'react';
import type { WorkflowTemplate } from './api';
import { runSupervisorAction, updateSupervisorPlan, type ExecutionPlanItem, type SupervisorChildRun, type SupervisorExecutionStrategy, type SupervisorRun } from './supervisor_api';

type Props = {
  opened: boolean;
  run: SupervisorRun | null | undefined;
  templates: WorkflowTemplate[];
  onClose: () => void;
  onChanged: () => Promise<void> | void;
};

function sprintTitle(run: SupervisorRun | null | undefined, featurePlanItemId: string): string {
  return run?.feature_plan_items.find((item) => item.id === featurePlanItemId)?.title ?? featurePlanItemId;
}

function canApply(status: string): boolean {
  return status === 'ready_to_apply';
}

function canCancel(status: string): boolean {
  return ['snapshotting', 'running_children', 'running_integration', 'validating'].includes(status);
}

function canStart(status: string): boolean {
  return ['created', 'cancelled', 'failed'].includes(status);
}

function statusBadgeColor(status: string): string {
  if (['success', 'ready_to_apply', 'applied', 'completed'].includes(status)) return 'green';
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

function workflowName(templates: WorkflowTemplate[], id: string | null | undefined): string {
  if (!id) return '—';
  return templates.find((template) => template.id === id)?.name ?? id;
}

function isChildDone(child: SupervisorChildRun): boolean {
  return ['success', 'waiting', 'paused'].includes(child.status);
}

function isChildFailed(child: SupervisorChildRun): boolean {
  return ['error', 'cancelled'].includes(child.status);
}

export function SupervisorSprintModal({ opened, run, templates, onClose, onChanged }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [workflowTemplateId, setWorkflowTemplateId] = useState<string | null>(null);
  const [workflowStartStepId, setWorkflowStartStepId] = useState<string | null>(null);
  const [integrationTemplateId, setIntegrationTemplateId] = useState<string | null>(null);
  const [strategy, setStrategy] = useState<SupervisorExecutionStrategy>('series');

  const templateOptions = useMemo(() => templates.map((template) => ({ value: template.id, label: template.name })), [templates]);
  const selectedWorkflowTemplate = useMemo(() => templates.find((template) => template.id === workflowTemplateId) ?? null, [templates, workflowTemplateId]);
  const workflowStartStepOptions = useMemo(() => {
    return (selectedWorkflowTemplate?.definition.steps ?? []).map((step) => ({
      value: step.id,
      label: `${step.name} (${step.step_type})`
    }));
  }, [selectedWorkflowTemplate]);

  useEffect(() => {
    if (!opened || !run) return;
    setStrategy(run.strategy);
    setWorkflowTemplateId(typeof run.context?.workflow_template_id === 'string' ? run.context.workflow_template_id : null);
    setWorkflowStartStepId(typeof run.context?.workflow_start_step_id === 'string' ? run.context.workflow_start_step_id : null);
    setIntegrationTemplateId(typeof run.context?.integration_template_id === 'string' ? run.context.integration_template_id : null);
    setError(null);
  }, [opened, run?.id]);

  useEffect(() => {
    if (!opened) return;
    if (!selectedWorkflowTemplate) {
      setWorkflowStartStepId(null);
      return;
    }
    const steps = selectedWorkflowTemplate.definition.steps ?? [];
    if (workflowStartStepId && steps.some((step) => step.id === workflowStartStepId)) return;
    const preferred = steps.find((step) => step.step_type === 'code') ?? steps[0];
    setWorkflowStartStepId(preferred?.id ?? null);
  }, [opened, selectedWorkflowTemplate, workflowStartStepId]);

  const scheduledItemsForStart = useMemo<ExecutionPlanItem[]>(() => {
    if (!run) return [];
    return (run.execution_plan_items ?? []).map((item, index) => ({
      ...item,
      workflow_template_id: item.workflow_template_id ?? workflowTemplateId,
      order_index: item.order_index ?? index
    }));
  }, [run, workflowTemplateId]);

  const progress = useMemo(() => {
    if (!run) return { completed: 0, failed: 0, total: 0 };
    const total = scheduledItemsForStart.length;
    if (run.child_runs.length > 0) {
      return {
        completed: run.child_runs.filter(isChildDone).length,
        failed: run.child_runs.filter(isChildFailed).length,
        total
      };
    }
    if (['ready_to_apply', 'applied'].includes(run.status)) return { completed: total, failed: 0, total };
    if (run.status === 'failed') return { completed: 0, failed: total > 0 ? 1 : 0, total };
    return { completed: 0, failed: 0, total };
  }, [run, scheduledItemsForStart]);

  async function startSprint() {
    if (!run) return;
    setError(null);
    try {
      if (!workflowTemplateId) {
        setError('Select a workflow template before starting the sprint.');
        return;
      }
      if (!workflowStartStepId) {
        setError('Select a workflow start stage before starting the sprint.');
        return;
      }
      if (strategy === 'parallel' && !integrationTemplateId) {
        setError('Select an integration workflow before starting a parallel sprint.');
        return;
      }
      const sprintItems = scheduledItemsForStart;
      if (sprintItems.length === 0) {
        setError('No planner features are scheduled for this sprint.');
        return;
      }
      await updateSupervisorPlan(run.id, run.feature_plan_items, sprintItems, {
        sprint_strategy: strategy,
        workflow_template_id: workflowTemplateId,
        workflow_start_step_id: workflowStartStepId,
        integration_template_id: strategy === 'parallel' ? integrationTemplateId : null
      });
      await runSupervisorAction(run.id, 'start');
      await onChanged();
    } catch (err) {
      setError(String(err));
    }
  }

  async function action(actionName: 'tick' | 'apply' | 'cancel') {
    if (!run) return;
    setError(null);
    try {
      await runSupervisorAction(run.id, actionName);
      await onChanged();
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <Modal opened={opened} onClose={onClose} title={run ? `${run.title} sprint` : 'Sprint'} size="90%" centered>
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
                  <Text size="sm" c="dimmed">
                    Progress: {progress.completed}/{progress.total}{progress.failed > 0 ? `, ${progress.failed} failed` : ''}
                  </Text>
                </Group>

                {canStart(run.status) ? (
                  <Stack gap="sm">
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
                    </Group>
                    <Select
                      label="Start stage"
                      placeholder="Select workflow start stage"
                      value={workflowStartStepId}
                      onChange={setWorkflowStartStepId}
                      data={workflowStartStepOptions}
                      searchable
                      disabled={!workflowTemplateId}
                    />
                    {strategy === 'parallel' ? (
                      <Select
                        label="Integration workflow"
                        placeholder="Select integration workflow"
                        value={integrationTemplateId}
                        onChange={setIntegrationTemplateId}
                        data={templateOptions}
                        searchable
                      />
                    ) : null}
                  </Stack>
                ) : null}

                <Table withTableBorder>
                  <Table.Tbody>
                    <Table.Tr>
                      <Table.Th>Status</Table.Th>
                      <Table.Td><Badge color={statusBadgeColor(run.status)}>{run.status}</Badge></Table.Td>
                    </Table.Tr>
                    <Table.Tr>
                      <Table.Th>Snapshot path</Table.Th>
                      <Table.Td>{formatValue(run.snapshot_path)}</Table.Td>
                    </Table.Tr>
                    <Table.Tr>
                      <Table.Th>Integration path</Table.Th>
                      <Table.Td>{formatValue(run.integration_path)}</Table.Td>
                    </Table.Tr>
                    <Table.Tr>
                      <Table.Th>Integration workflow run</Table.Th>
                      <Table.Td>{shortId(run.integration_run_id)}</Table.Td>
                    </Table.Tr>
                    <Table.Tr>
                      <Table.Th>Final patch</Table.Th>
                      <Table.Td>{formatValue(run.final_patch_path)}</Table.Td>
                    </Table.Tr>
                  </Table.Tbody>
                </Table>
              </Stack>
            </Card>

            <Card withBorder>
              <Stack gap="sm">
                <Group justify="space-between">
                  <Text fw={700}>Scheduled sprint items</Text>
                  <Badge variant="light">{scheduledItemsForStart.length}</Badge>
                </Group>
                <Table striped withTableBorder>
                  <Table.Thead>
                    <Table.Tr>
                      <Table.Th>Order</Table.Th>
                      <Table.Th>Feature</Table.Th>
                      <Table.Th>Workflow template</Table.Th>
                    </Table.Tr>
                  </Table.Thead>
                  <Table.Tbody>
                    {scheduledItemsForStart.map((item, index) => (
                      <Table.Tr key={`${item.feature_plan_item_id}-${index}`}>
                        <Table.Td>{index + 1}</Table.Td>
                        <Table.Td>{sprintTitle(run, item.feature_plan_item_id)}</Table.Td>
                        <Table.Td>{workflowName(templates, item.workflow_template_id ?? workflowTemplateId)}</Table.Td>
                      </Table.Tr>
                    ))}
                  </Table.Tbody>
                </Table>
              </Stack>
            </Card>

            <Card withBorder>
              <Stack gap="sm">
                <Group justify="space-between">
                  <Text fw={700}>Workflow runs</Text>
                  <Badge variant="light">{run.child_runs.length + (run.integration_run_id ? 1 : 0)}</Badge>
                </Group>

                {run.child_runs.length > 0 ? (
                  <Table striped withTableBorder>
                    <Table.Thead>
                      <Table.Tr>
                        <Table.Th>Feature</Table.Th>
                        <Table.Th>Workflow run</Table.Th>
                        <Table.Th>Status</Table.Th>
                        <Table.Th>Shard path</Table.Th>
                        <Table.Th>Patch path</Table.Th>
                      </Table.Tr>
                    </Table.Thead>
                    <Table.Tbody>
                      {run.child_runs.map((child) => (
                        <Table.Tr key={child.execution_item_id}>
                          <Table.Td>{child.title}</Table.Td>
                          <Table.Td>{shortId(child.workflow_run_id)}</Table.Td>
                          <Table.Td><Badge color={statusBadgeColor(child.status)}>{child.status}</Badge></Table.Td>
                          <Table.Td>{formatValue(child.shard_path)}</Table.Td>
                          <Table.Td>{formatValue(child.patch_path)}</Table.Td>
                        </Table.Tr>
                      ))}
                    </Table.Tbody>
                  </Table>
                ) : run.integration_run_id ? (
                  <Table striped withTableBorder>
                    <Table.Thead>
                      <Table.Tr>
                        <Table.Th>Workflow</Table.Th>
                        <Table.Th>Workflow run</Table.Th>
                        <Table.Th>Status</Table.Th>
                      </Table.Tr>
                    </Table.Thead>
                    <Table.Tbody>
                      <Table.Tr>
                        <Table.Td>{strategy === 'series' ? 'Series execution' : 'Integration'}</Table.Td>
                        <Table.Td>{shortId(run.integration_run_id)}</Table.Td>
                        <Table.Td><Badge color={statusBadgeColor(run.status)}>{run.status}</Badge></Table.Td>
                      </Table.Tr>
                    </Table.Tbody>
                  </Table>
                ) : (
                  <Text size="sm" c="dimmed">
                    No workflow runs have been spawned yet. Current sprint phase: {run.status}.
                  </Text>
                )}
              </Stack>
            </Card>

            <Group justify="flex-end">
              {canStart(run.status) ? (
                <Button onClick={startSprint}>{run.status === 'created' ? 'Start sprint' : 'Restart sprint'}</Button>
              ) : null}
              <Button variant="light" onClick={() => action('tick')}>Refresh sprint status</Button>
              <Button disabled={!canApply(run.status)} onClick={() => action('apply')}>Apply sprint</Button>
              <Button color="red" variant="light" disabled={!canCancel(run.status)} onClick={() => action('cancel')}>Cancel sprint</Button>
            </Group>
          </>
        ) : null}
      </Stack>
    </Modal>
  );
}
