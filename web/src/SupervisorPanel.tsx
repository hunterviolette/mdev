import { useEffect, useMemo, useRef, useState } from 'react';
import { Alert, Badge, Button, Card, Group, Modal, Stack, Table, Text, TextInput } from '@mantine/core';
import { listTemplates, type WorkflowTemplate } from './api';
import { SupervisorPlannerModal } from './SupervisorPlannerModal';
import { SupervisorSprintModal } from './SupervisorSprintModal';
import { createSupervisorRun, deleteSupervisorRun, ensureSupervisorPlannerRun, listSupervisorRuns, type SupervisorRun } from './supervisor_api';

type Props = {
  onOpenWorkflowRun?: (workflowRunId: string) => Promise<void> | void;
  supervisorRunId?: string | null;
  supervisorView?: 'planner' | 'sprint' | null;
  navigate?: (path: string) => void;
};

type SupervisorPanelProps = Props & {
  createRequestedToken?: number;
  refreshRequestedToken?: number;
};

function statusBadgeColor(status: string): string {
  if (['running_children', 'running_integration', 'validating', 'ready_to_apply'].includes(status)) return 'blue';
  if (['development_complete', 'applied', 'completed', 'success'].includes(status)) return 'green';
  if (['failed', 'error'].includes(status)) return 'red';
  if (status === 'cancelled') return 'gray';
  return 'gray';
}

function formatUpdatedAt(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
}

function supervisorPlannerPath(runId: string): string {
  return `/supervisors/${encodeURIComponent(runId)}/planner`;
}

function supervisorSprintPath(runId: string): string {
  return `/supervisors/${encodeURIComponent(runId)}/sprint`;
}

function shouldHandleLinkInApp(event: React.MouseEvent<HTMLAnchorElement>): boolean {
  return event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;
}


export function SupervisorPanel(props: SupervisorPanelProps) {
  const [runs, setRuns] = useState<SupervisorRun[]>([]);
  const [templates, setTemplates] = useState<WorkflowTemplate[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [plannerOpen, setPlannerOpen] = useState(false);
  const [sprintOpen, setSprintOpen] = useState(false);
  const [title, setTitle] = useState('Supervisor');
  const [rootRepoPath, setRootRepoPath] = useState('');
  const [error, setError] = useState<string | null>(null);
  const lastCreateRequestTokenRef = useRef(props.createRequestedToken ?? 0);
  const lastRefreshRequestTokenRef = useRef(props.refreshRequestedToken ?? 0);

  const selected = useMemo(() => runs.find((run) => run.id === selectedId) ?? null, [runs, selectedId]);

  useEffect(() => {
    if (!selected) return;
    if (props.supervisorView === 'planner') {
      document.title = `Supervisor · ${selected.title} · planner`;
      return;
    }
    if (props.supervisorView === 'sprint') {
      document.title = `Supervisor · ${selected.title} · sprint`;
      return;
    }
    if (props.supervisorRunId) {
      document.title = `Supervisor · ${selected.title}`;
    }
  }, [selected, props.supervisorRunId, props.supervisorView]);

  useEffect(() => {
    if (!props.supervisorRunId) {
      setPlannerOpen(false);
      setSprintOpen(false);
      return;
    }
    setSelectedId(props.supervisorRunId);
    setPlannerOpen(props.supervisorView === 'planner');
    setSprintOpen(props.supervisorView === 'sprint');
  }, [props.supervisorRunId, props.supervisorView]);

  useEffect(() => {
    const nextToken = props.createRequestedToken ?? 0;
    if (nextToken === lastCreateRequestTokenRef.current) return;
    lastCreateRequestTokenRef.current = nextToken;
    setCreateOpen(true);
  }, [props.createRequestedToken]);

  useEffect(() => {
    const nextToken = props.refreshRequestedToken ?? 0;
    if (nextToken === lastRefreshRequestTokenRef.current) return;
    lastRefreshRequestTokenRef.current = nextToken;
    refresh().catch((err) => setError(String(err)));
  }, [props.refreshRequestedToken]);

  async function refresh() {
    const [nextRuns, nextTemplates] = await Promise.all([listSupervisorRuns(), listTemplates()]);
    setRuns(nextRuns);
    setTemplates(nextTemplates);
  }

  useEffect(() => {
    refresh().catch((err) => setError(String(err)));
  }, []);

  async function createEmptySupervisor() {
    setError(null);
    try {
      const run = await createSupervisorRun({
        title,
        root_repo_path: rootRepoPath,
        strategy: 'series',
        workflow_template_id: null,
        integration_template_id: null,
        feature_plan_items: [],
        execution_plan_items: [],
        context: {}
      });
      await refresh();
      setSelectedId(run.id);
      setCreateOpen(false);
      setPlannerOpen(true);
      setSprintOpen(false);
      props.navigate?.(supervisorPlannerPath(run.id));
    } catch (err) {
      setError(String(err));
    }
  }

  function openSprint(run: SupervisorRun) {
    setError(null);
    setSelectedId(run.id);
    setPlannerOpen(false);
    setSprintOpen(true);
    props.navigate?.(supervisorSprintPath(run.id));
  }

  async function openPlanner(run: SupervisorRun) {
    setError(null);
    try {
      const response = await ensureSupervisorPlannerRun({
        root_repo_path: run.root_repo_path,
        title: run.title
      });
      await refresh();
      setSelectedId(response.supervisor_run.id);
      setSprintOpen(false);
      setPlannerOpen(true);
      props.navigate?.(supervisorPlannerPath(response.supervisor_run.id));
    } catch (err) {
      setError(String(err));
    }
  }

  function openPlannerFromSprint() {
    setSprintOpen(false);
    setPlannerOpen(true);
    if (selectedId) {
      props.navigate?.(supervisorPlannerPath(selectedId));
    }
  }

  async function removeRun(run: SupervisorRun) {
    setError(null);
    try {
      await deleteSupervisorRun(run.id);
      if (selectedId === run.id) {
        setSelectedId(null);
      }
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  function closeSupervisorModal() {
    setPlannerOpen(false);
    setSprintOpen(false);
    props.navigate?.('/supervisors');
  }

  return (
    <Stack gap="md">
      {error ? <Alert color="red">{error}</Alert> : null}

      <Card withBorder>
        <Stack gap="sm">
          <Group justify="space-between" align="center">
            <Text fw={700} size="lg">Supervisors</Text>
          </Group>

          <Table striped highlightOnHover>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Title</Table.Th>
              <Table.Th>Status</Table.Th>
              <Table.Th>Current sprint strategy</Table.Th>
              <Table.Th>Planner ideas</Table.Th>
              <Table.Th>Next sprint items</Table.Th>
              <Table.Th>Updated</Table.Th>
              <Table.Th>Actions</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {runs.map((run) => (
              <Table.Tr key={run.id}>
                <Table.Td>
                  <Text>{run.title}</Text>
                </Table.Td>
                <Table.Td><Badge color={statusBadgeColor(run.status)}>{run.status}</Badge></Table.Td>
                <Table.Td>{run.strategy}</Table.Td>
                <Table.Td>{run.feature_plan_items.length}</Table.Td>
                <Table.Td>{run.execution_plan_items.length}</Table.Td>
                <Table.Td>{formatUpdatedAt(run.updated_at)}</Table.Td>
                <Table.Td>
                  <Group gap="xs" wrap="nowrap">
                    <Button
                      component="a"
                      href={supervisorPlannerPath(run.id)}
                      size="xs"
                      variant="light"
                      onClick={(event) => {
                        if (!shouldHandleLinkInApp(event)) return;
                        event.preventDefault();
                        void openPlanner(run);
                      }}
                    >
                      Planner
                    </Button>
                    <Button
                      component="a"
                      href={supervisorSprintPath(run.id)}
                      size="xs"
                      onClick={(event) => {
                        if (!shouldHandleLinkInApp(event)) return;
                        event.preventDefault();
                        openSprint(run);
                      }}
                    >
                      Sprint
                    </Button>
                    <Button size="xs" color="red" variant="subtle" onClick={() => void removeRun(run)}>Delete</Button>
                  </Group>
                </Table.Td>
              </Table.Tr>
            ))}
          </Table.Tbody>
          </Table>
        </Stack>
      </Card>

      <Modal opened={createOpen} onClose={() => setCreateOpen(false)} title="Create supervisor" centered>
        <Stack gap="sm">
          <TextInput label="Title" value={title} onChange={(event) => setTitle(event.currentTarget.value)} />
          <TextInput label="Root repo path" value={rootRepoPath} onChange={(event) => setRootRepoPath(event.currentTarget.value)} />
          <Group justify="flex-end">
            <Button variant="subtle" onClick={() => setCreateOpen(false)}>Cancel</Button>
            <Button onClick={createEmptySupervisor}>Create supervisor</Button>
          </Group>
        </Stack>
      </Modal>

      <SupervisorPlannerModal
        opened={plannerOpen}
        run={selected}
        templates={templates}
        onClose={closeSupervisorModal}
        onSaved={refresh}
        onWorkflowRunCreated={props.onOpenWorkflowRun}
      />

      <SupervisorSprintModal
        opened={sprintOpen}
        run={selected}
        templates={templates}
        onClose={closeSupervisorModal}
        onOpenPlanner={openPlannerFromSprint}
        onChanged={refresh}
      />
    </Stack>
  );
}
