import { Alert, Badge, Button, Card, Group, Modal, Select, Stack, Table, Text } from '@mantine/core';
import { useEffect, useMemo, useState } from 'react';
import type { WorkflowTemplate } from './api';
import { runSupervisorAction, updateSupervisorPlan, type SupervisorChildRun, type SupervisorExecutionStrategy, type SupervisorRun } from './supervisor_api';

type Props = {
  opened: boolean;
  run: SupervisorRun | null | undefined;
  templates: WorkflowTemplate[];
  onClose: () => void;
  onOpenPlanner: () => void;
  onChanged: () => Promise<void> | void;
};

function sprintTitle(run: SupervisorRun | null | undefined, featurePlanItemId: string): string {
  return run?.feature_plan_items.find((item) => item.id === featurePlanItemId)?.title ?? featurePlanItemId;
}

function canApply(status: string): boolean {
  return status === 'ready_to_apply';
}

function canStartIntegration(run: SupervisorRun, integrationTemplateId: string | null): boolean {
  return Boolean(integrationTemplateId) && ['development_complete', 'running_integration', 'ready_to_apply', 'failed'].includes(run.status) && run.child_runs.length > 0;
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

function workflowName(templates: WorkflowTemplate[], id: string | null | undefined): string {
  if (!id) return '—';
  return templates.find((template) => template.id === id)?.name ?? id;
}

function sprintStageState(run: SupervisorRun, stage: 'development' | 'integration' | 'apply'): 'active' | 'complete' | 'up_next' | 'blocked' {
  if (stage === 'development') {
    if (['created', 'snapshotting', 'running_children'].includes(run.status)) return 'active';
    if (['development_complete', 'running_integration', 'validating', 'ready_to_apply', 'applied'].includes(run.status)) return 'complete';
    return 'blocked';
  }
  if (stage === 'integration') {
    if (['running_integration', 'validating'].includes(run.status)) return 'active';
    if (['ready_to_apply', 'applied'].includes(run.status)) return 'complete';
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

export function SupervisorSprintModal({ opened, run, templates, onClose, onOpenPlanner, onChanged }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [workflowTemplateId, setWorkflowTemplateId] = useState<string | null>(null);
  const [integrationTemplateId, setIntegrationTemplateId] = useState<string | null>(null);
  const [strategy, setStrategy] = useState<SupervisorExecutionStrategy>('series');

  const templateOptions = useMemo(() => templates.map((template) => ({ value: template.id, label: template.name })), [templates]);

  useEffect(() => {
    if (!opened || !run) return;
    setStrategy(run.strategy);
    setWorkflowTemplateId(typeof run.context?.workflow_template_id === 'string' ? run.context.workflow_template_id : null);
    setIntegrationTemplateId(typeof run.context?.integration_template_id === 'string' ? run.context.integration_template_id : null);
    setError(null);
  }, [opened, run?.id]);

  useEffect(() => {
    if (!opened || !run) return;
    if (!['snapshotting', 'running_children', 'running_integration', 'validating'].includes(run.status)) return;
    const timer = window.setInterval(() => {
      void onChanged();
    }, 1500);
    return () => window.clearInterval(timer);
  }, [opened, run?.id, run?.status, onChanged]);

  const scheduledItemsForStart = useMemo(() => {
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
      if (!integrationTemplateId) {
        setError('Select an integration workflow before starting the sprint.');
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
        integration_template_id: integrationTemplateId
      });
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
          integration_template_id: integrationTemplateId
        });
      }
      if (actionName === 'restart_integration' && integrationTemplateId) {
        await updateSupervisorPlan(run.id, run.feature_plan_items, scheduledItemsForStart, {
          sprint_strategy: strategy,
          workflow_template_id: workflowTemplateId,
          integration_template_id: integrationTemplateId
        });
      }
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
                  <Group gap="xs">
                    <Text size="sm" c="dimmed">
                      Progress: {progress.completed}/{progress.total}{progress.failed > 0 ? `, ${progress.failed} failed` : ''}
                    </Text>
                    {canRestartSprint(run.status) ? (
                      <Button size="xs" color="red" variant="light" onClick={() => action('restart_sprint')}>Restart sprint</Button>
                    ) : null}
                  </Group>
                </Group>

                <Group grow align="stretch">
                  {(['development', 'integration', 'apply'] as const).map((stage, index) => {
                    const stageState = sprintStageState(run, stage);
                    return (
                      <Card key={stage} withBorder>
                        <Stack gap={4}>
                          <Badge w="fit-content" color={sprintStageBadgeColor(stageState)}>{index + 1}</Badge>
                          <Text fw={700}>{stage === 'development' ? 'Development' : stage === 'integration' ? 'Integration' : 'Apply'}</Text>
                          <Text size="xs" c="dimmed">
                            {stage === 'development' ? 'Feature workflows' : stage === 'integration' ? 'Live worktree integration' : 'Complete sprint'}
                          </Text>
                          <Badge w="fit-content" color={sprintStageBadgeColor(stageState)}>{sprintStageLabel(stageState)}</Badge>
                          {stage === 'development' && stageState === 'active' && canStart(run.status) ? (
                            <Button mt="xs" size="xs" onClick={startSprint}>{run.status === 'created' ? 'Start sprint' : 'Restart sprint'}</Button>
                          ) : null}
                          {stage === 'development' && ['running_integration', 'ready_to_apply', 'applied', 'failed'].includes(run.status) ? (
                            <Button mt="xs" size="xs" variant="light" onClick={() => action('reopen_development')}>Reopen development</Button>
                          ) : null}
                          {stage === 'development' && run.status === 'applied' ? (
                            <Button mt="xs" size="xs" variant="light" onClick={() => action('new_sprint')}>Start next sprint</Button>
                          ) : null}
                          {stage === 'integration' && stageState === 'up_next' ? (
                            <Button mt="xs" size="xs" disabled={!canStartIntegration(run, integrationTemplateId)} onClick={() => action('start_integration')}>Start integration</Button>
                          ) : null}
                          {stage === 'integration' && ['running_integration', 'ready_to_apply', 'applied', 'failed'].includes(run.status) ? (
                            <Button mt="xs" size="xs" variant="light" disabled={!canRestartIntegration(run, integrationTemplateId)} onClick={() => action('restart_integration')}>Restart integration</Button>
                          ) : null}
                          {stage === 'apply' && stageState === 'active' ? (
                            <Button mt="xs" size="xs" disabled={!canApply(run.status)} onClick={() => action('apply')}>Apply sprint</Button>
                          ) : null}
                        </Stack>
                      </Card>
                    );
                  })}
                </Group>

                {canStart(run.status) || ['development_complete', 'running_integration', 'validating', 'ready_to_apply', 'failed'].includes(run.status) ? (
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
                      label="Integration workflow"
                      placeholder="Select integration workflow"
                      value={integrationTemplateId}
                      onChange={setIntegrationTemplateId}
                      data={templateOptions}
                      searchable
                    />
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
                  <Group>
                    <Text fw={700}>Scheduled sprint items</Text>
                    <Badge variant="light">{scheduledItemsForStart.length}</Badge>
                  </Group>
                  <Button size="xs" variant="light" onClick={onOpenPlanner}>Manage features in planner</Button>
                </Group>
                {scheduledItemsForStart.length > 0 ? (
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
                ) : (
                  <Text size="sm" c="dimmed">No planner features are scheduled for this sprint. Use the planner to add features to the next sprint.</Text>
                )}
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
                        <Table.Th>Backend entrypoint</Table.Th>
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
                                      <Table.Td>{shortId(child?.workflow_run_id)}</Table.Td>
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
                                  <Table.Td>{shortId(sprint.integration_run_id)}</Table.Td>
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
