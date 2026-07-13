import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  Alert,
  Badge,
  Button,
  Code,
  Card,
  Group,
  ScrollArea,
  SegmentedControl,
  Stack,
  Table,
  Text,
} from '@mantine/core';
import { IconRefresh, IconTrash, IconX } from '@tabler/icons-react';

import {
  clearCompletedRuntimeProcesses,
  getRuntimeProcesses,
  terminateRuntimeProcess,
  type RuntimeProcessRecord,
} from '../api';

const ACTIVE = new Set(['starting', 'running', 'stopping', 'killing']);

function statusColor(status: string): string {
  if (status === 'running') return 'green';
  if (status === 'succeeded') return 'blue';
  if (status === 'failed' || status === 'timed_out') return 'red';
  if (status === 'killed' || status === 'stopped') return 'orange';
  return 'gray';
}

export function RuntimeAdmin() {
  const [processes, setProcesses] = useState<RuntimeProcessRecord[]>([]);
  const [filter, setFilter] = useState<'active' | 'deployments' | 'all'>('active');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState('');

  const refresh = useCallback(async () => {
    try {
      const response = await getRuntimeProcesses();
      setProcesses(Array.isArray(response.processes) ? response.processes : []);
      setError('');
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : String(nextError));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 1500);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const visible = useMemo(() => {
    if (filter === 'deployments') {
      return processes.filter((process) => process.mode === 'service');
    }
    if (filter === 'active') {
      return processes.filter((process) => ACTIVE.has(process.status));
    }
    return processes;
  }, [filter, processes]);

  const activeCount = processes.filter((process) => ACTIVE.has(process.status)).length;

  async function terminate(process: RuntimeProcessRecord, force: boolean) {
    setBusy(process.execution_id);
    try {
      await terminateRuntimeProcess(process.execution_id, force);
      await refresh();
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : String(nextError));
    } finally {
      setBusy('');
    }
  }

  return (
    <Card withBorder padding="md">
      <Stack gap="sm">
        <Group justify="space-between" align="center">
          <Group gap="xs">
            <Text fw={700}>Runtime processes</Text>
            <Badge color={activeCount > 0 ? 'green' : 'gray'}>
              {activeCount} active
            </Badge>
          </Group>
          <Button
            variant="default"
            leftSection={<IconRefresh size={16} />}
            onClick={() => void refresh()}
          >
            Refresh
          </Button>
        </Group>
          {error ? <Alert color="red">{error}</Alert> : null}

          <Group justify="space-between">
            <SegmentedControl
              value={filter}
              onChange={(value) => setFilter(value as typeof filter)}
              data={[
                { value: 'active', label: 'Active' },
                { value: 'deployments', label: 'Deployments' },
                { value: 'all', label: 'All' },
              ]}
            />
            <Button
              variant="default"
              leftSection={<IconTrash size={16} />}
              onClick={async () => {
                await clearCompletedRuntimeProcesses();
                await refresh();
              }}
            >
              Clear completed
            </Button>
          </Group>

          <ScrollArea h="70vh">
            <Table striped highlightOnHover withTableBorder>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Status</Table.Th>
                  <Table.Th>Command</Table.Th>
                  <Table.Th>Owner</Table.Th>
                  <Table.Th>PID</Table.Th>
                  <Table.Th>Execution</Table.Th>
                  <Table.Th>Started</Table.Th>
                  <Table.Th>Output</Table.Th>
                  <Table.Th>Controls</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {visible.map((process) => (
                  <Table.Tr key={process.execution_id}>
                    <Table.Td>
                      <Stack gap={3}>
                        <Badge color={statusColor(process.status)}>{process.status}</Badge>
                        <Text size="xs" c="dimmed">{process.mode}</Text>
                      </Stack>
                    </Table.Td>
                    <Table.Td maw={320}>
                      <Text fw={600} size="sm">{process.label || process.command_id}</Text>
                      <Code block>{process.command}</Code>
                      <Text size="xs" c="dimmed">{process.working_directory}</Text>
                    </Table.Td>
                    <Table.Td>
                      <Text size="sm">{process.owner.capability}</Text>
                      <Text size="xs" c="dimmed">run {process.owner.run_id.slice(0, 8)}</Text>
                      <Text size="xs" c="dimmed">step {process.owner.step_id}</Text>
                      {process.owner.service_id ? (
                        <Text size="xs" c="dimmed">service {process.owner.service_id}</Text>
                      ) : null}
                    </Table.Td>
                    <Table.Td>{process.pid ?? '—'}</Table.Td>
                    <Table.Td>
                      <Text size="xs">{process.execution_id.slice(0, 8)}</Text>
                    </Table.Td>
                    <Table.Td>
                      <Text size="xs">{new Date(process.started_at).toLocaleString()}</Text>
                      {process.duration_ms != null ? (
                        <Text size="xs" c="dimmed">{process.duration_ms} ms</Text>
                      ) : null}
                    </Table.Td>
                    <Table.Td maw={420}>
                      <ScrollArea h={130}>
                        {process.stdout ? <Code block>{process.stdout}</Code> : null}
                        {process.stderr ? <Code block c="red">{process.stderr}</Code> : null}
                        {!process.stdout && !process.stderr ? (
                          <Text size="xs" c="dimmed">No captured output.</Text>
                        ) : null}
                      </ScrollArea>
                    </Table.Td>
                    <Table.Td>
                      {ACTIVE.has(process.status) ? (
                        <Stack gap="xs">
                          <Button
                            size="xs"
                            variant="light"
                            color="orange"
                            loading={busy === process.execution_id}
                            onClick={() => void terminate(process, false)}
                          >
                            Stop
                          </Button>
                          <Button
                            size="xs"
                            color="red"
                            leftSection={<IconX size={14} />}
                            loading={busy === process.execution_id}
                            onClick={() => void terminate(process, true)}
                          >
                            Kill
                          </Button>
                        </Stack>
                      ) : null}
                    </Table.Td>
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>

            {visible.length === 0 ? (
              <Text ta="center" c="dimmed" py="xl">No matching processes.</Text>
            ) : null}
          </ScrollArea>
      </Stack>
    </Card>
  );
}
