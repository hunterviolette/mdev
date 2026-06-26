import { useEffect, useMemo, useState } from 'react';
import { Alert, Anchor, Badge, Button, Card, Group, Stack, Table, Text, TextInput, Title } from '@mantine/core';
import { listTemplates, type WorkflowTemplate } from './api';
import { SupervisorPlannerModal } from './SupervisorPlannerModal';
import { SupervisorSprintModal } from './SupervisorSprintModal';
import { createSupervisorRun, deleteSupervisorRun, ensureSupervisorPlannerRun, listSupervisorRuns, runSupervisorAction, type SupervisorRun } from './supervisor_api';

type Props = {
  onOpenWorkflowRun?: (workflowRunId: string) => Promise<void> | void;
  supervisorRunId?: string | null;
  navigate?: (path: string) => void;
};

function supervisorRoute(supervisorRunId: string) {
  return `/supervisors/${encodeURIComponent(supervisorRunId)}`;
}

function shouldUseBrowserNavigation(event: { defaultPrevented: boolean; button: number; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean }) {
  return event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey;
}

export function SupervisorPanel(props: Props) {
  const [runs, setRuns] = useState<SupervisorRun[]>([]);
  const [templates, setTemplates] = useState<WorkflowTemplate[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [plannerOpen, setPlannerOpen] = useState(false);
  const [sprintOpen, setSprintOpen] = useState(false);
  const [title, setTitle] = useState('Supervisor');
  const [rootRepoPath, setRootRepoPath] = useState('');
  const [error, setError] = useState<string | null>(null);

  const selected = useMemo(() => runs.find((run) => run.id === selectedId) ?? runs[0], [runs, selectedId]);

  useEffect(() => {
    if (!props.supervisorRunId) return;
    setSelectedId(props.supervisorRunId);
  }, [props.supervisorRunId]);

  function handleSupervisorLinkClick(event: { defaultPrevented: boolean; button: number; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; preventDefault: () => void }, supervisorRunId: string) {
    if (shouldUseBrowserNavigation(event)) return;
    event.preventDefault();
    setSelectedId(supervisorRunId);
    props.navigate?.(supervisorRoute(supervisorRunId));
  }

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
      setPlannerOpen(true);
    } catch (err) {
      setError(String(err));
    }
  }

  async function openSprint() {
    if (!selected) return;
    setError(null);
    try {
      setSprintOpen(true);
    } catch (err) {
      setError(String(err));
    }
  }

  async function openPlannerForSelectedRepo() {
    if (!selected) return;
    setError(null);
    try {
      const response = await ensureSupervisorPlannerRun({
        root_repo_path: selected.root_repo_path,
        title: selected.title
      });
      await refresh();
      setSelectedId(response.supervisor_run.id);
      setPlannerOpen(true);
    } catch (err) {
      setError(String(err));
    }
  }

  function openPlannerFromSprint() {
    setSprintOpen(false);
    setPlannerOpen(true);
  }

  async function removeRun() {
    if (!selected) return;
    setError(null);
    try {
      await deleteSupervisorRun(selected.id);
      setSelectedId(null);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <Stack gap="md">
      <Title order={3}>Supervisors</Title>
      {error ? <Alert color="red">{error}</Alert> : null}

      <Card withBorder>
        <Stack gap="sm">
          <Text fw={700}>Create supervisor</Text>
          <Group grow align="flex-end">
            <TextInput label="Title" value={title} onChange={(event) => setTitle(event.currentTarget.value)} />
            <TextInput label="Root repo path" value={rootRepoPath} onChange={(event) => setRootRepoPath(event.currentTarget.value)} />
          </Group>
          <Group>
            <Button onClick={createEmptySupervisor}>Create supervisor</Button>
          </Group>
        </Stack>
      </Card>

      <Card withBorder>
        <Table striped highlightOnHover>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Title</Table.Th>
              <Table.Th>Status</Table.Th>
              <Table.Th>Current sprint strategy</Table.Th>
              <Table.Th>Planner ideas</Table.Th>
              <Table.Th>Next sprint items</Table.Th>
              <Table.Th>Updated</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {runs.map((run) => (
              <Table.Tr key={run.id}>
                <Table.Td>
                  <Anchor href={supervisorRoute(run.id)} onClick={(event) => handleSupervisorLinkClick(event, run.id)}>
                    {run.title}
                  </Anchor>
                </Table.Td>
                <Table.Td><Badge>{run.status}</Badge></Table.Td>
                <Table.Td>{run.strategy}</Table.Td>
                <Table.Td>{run.feature_plan_items.length}</Table.Td>
                <Table.Td>{run.execution_plan_items.length}</Table.Td>
                <Table.Td>{run.updated_at}</Table.Td>
              </Table.Tr>
            ))}
          </Table.Tbody>
        </Table>
      </Card>

      {selected ? (
        <Card withBorder>
          <Stack gap="sm">
            <Group justify="space-between">
              <Group>
                <Text fw={700}>{selected.title}</Text>
                <Badge>{selected.status}</Badge>
                <Badge variant="light">{selected.strategy}</Badge>
              </Group>
              <Group>
                <Button variant="light" onClick={() => void openPlannerForSelectedRepo()}>Planner</Button>
                <Button onClick={openSprint}>Sprint</Button>
                <Button color="red" variant="subtle" onClick={removeRun}>Delete</Button>
              </Group>
            </Group>
            <Text size="sm">Planner ideas: {selected.feature_plan_items.length}</Text>
            <Text size="sm">Next sprint items: {selected.execution_plan_items.length}</Text>
          </Stack>
        </Card>
      ) : null}

      <SupervisorPlannerModal
        opened={plannerOpen}
        run={selected}
        templates={templates}
        onClose={() => setPlannerOpen(false)}
        onSaved={refresh}
        onWorkflowRunCreated={props.onOpenWorkflowRun}
      />

      <SupervisorSprintModal
        opened={sprintOpen}
        run={selected}
        templates={templates}
        onClose={() => setSprintOpen(false)}
        onOpenPlanner={openPlannerFromSprint}
        onChanged={refresh}
      />
    </Stack>
  );
}
