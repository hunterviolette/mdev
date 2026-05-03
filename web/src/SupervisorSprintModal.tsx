import { Alert, Badge, Button, Card, Group, JsonInput, Modal, Select, Stack, Table, Text } from '@mantine/core';
import { useEffect, useMemo, useState } from 'react';
import type { WorkflowTemplate } from './api';
import { runSupervisorAction, updateSupervisorPlan, type ExecutionPlanItem, type SupervisorExecutionStrategy, type SupervisorRun } from './supervisor_api';

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

function hasVisibleSprint(status: SupervisorRun['status']): boolean {
  return status !== 'created';
}

function sprintLogValue(run: SupervisorRun): string {
  return JSON.stringify({
    status: run.status,
    sprint_completed_at: run.context?.sprint_completed_at ?? null,
    completed_features: run.context?.completed_features ?? [],
    child_runs: run.child_runs,
    integration_run_id: run.integration_run_id ?? null,
    final_patch_path: run.final_patch_path ?? null
  }, null, 2);
}

export function SupervisorSprintModal({ opened, run, templates, onClose, onChanged }: Props) {
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

  function scheduledItemsForStart(): ExecutionPlanItem[] {
    if (!run) return [];
    return (run.execution_plan_items ?? []).map((item, index) => ({
      ...item,
      workflow_template_id: item.workflow_template_id ?? workflowTemplateId,
      order_index: item.order_index ?? index
    }));
  }

  async function startSprint() {
    if (!run) return;
    setError(null);
    try {
      if (!workflowTemplateId) {
        setError('Select a workflow template before starting the sprint.');
        return;
      }
      if (strategy === 'parallel' && !integrationTemplateId) {
        setError('Select an integration workflow before starting a parallel sprint.');
        return;
      }
      const sprintItems = scheduledItemsForStart();
      if (sprintItems.length === 0) {
        setError('No planner features are scheduled for this sprint.');
        return;
      }
      await updateSupervisorPlan(run.id, run.feature_plan_items, sprintItems, {
        sprint_strategy: strategy,
        workflow_template_id: workflowTemplateId,
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
    <Modal opened={opened} onClose={onClose} title={run ? `${run.title} sprint` : 'Sprint'} size="80%" centered>
      <Stack gap="md">
        {error ? <Alert color="red">{error}</Alert> : null}
        {run ? (
          <>
            <Card withBorder>
              <Stack gap="xs">
                <Group>
                  <Text fw={700}>{run.title}</Text>
                  <Badge>{run.status}</Badge>
                  <Badge variant="light">{strategy}</Badge>
                </Group>
                {!hasVisibleSprint(run.status) ? (
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
                <Text size="sm">Snapshot: {run.snapshot_path ?? ''}</Text>
                <Text size="sm">Integration: {run.integration_path ?? ''}</Text>
                <Text size="sm">Integration run: {run.integration_run_id ?? ''}</Text>
                <Text size="sm">Final patch: {run.final_patch_path ?? ''}</Text>
              </Stack>
            </Card>

            <Card withBorder>
              <Stack gap="sm">
                <Group justify="space-between">
                  <Text fw={700}>Scheduled sprint items</Text>
                  <Badge variant="light">{scheduledItemsForStart().length}</Badge>
                </Group>
                <Table striped>
                  <Table.Thead>
                    <Table.Tr>
                      <Table.Th>Order</Table.Th>
                      <Table.Th>Feature</Table.Th>
                      <Table.Th>Workflow template</Table.Th>
                    </Table.Tr>
                  </Table.Thead>
                  <Table.Tbody>
                    {scheduledItemsForStart().map((item, index) => (
                      <Table.Tr key={`${item.feature_plan_item_id}-${index}`}>
                        <Table.Td>{index + 1}</Table.Td>
                        <Table.Td>{sprintTitle(run, item.feature_plan_item_id)}</Table.Td>
                        <Table.Td>{templates.find((template) => template.id === (item.workflow_template_id ?? workflowTemplateId))?.name ?? item.workflow_template_id ?? workflowTemplateId ?? ''}</Table.Td>
                      </Table.Tr>
                    ))}
                  </Table.Tbody>
                </Table>
              </Stack>
            </Card>

            <Card withBorder>
              <Stack gap="sm">
                <Group justify="space-between">
                  <Text fw={700}>Sprint log</Text>
                  <Badge variant="light">{run.child_runs.length}</Badge>
                </Group>
                <JsonInput value={sprintLogValue(run)} readOnly autosize minRows={8} />
              </Stack>
            </Card>

            <Group justify="flex-end">
              {!hasVisibleSprint(run.status) ? <Button onClick={startSprint}>Start sprint</Button> : null}
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
