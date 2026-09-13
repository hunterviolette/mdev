import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  Alert,
  Anchor,
  Badge,
  Button,
  Card,
  Code,
  Group,
  Modal,
  ScrollArea,
  Stack,
  Text,
} from '@mantine/core';
import {
  IconExternalLink,
  IconRotateClockwise,
  IconTerminal2,
} from '@tabler/icons-react';

import {
  getRuntimeProcesses,
  restartWorkflowStage,
  type QaServiceSpec,
  type RuntimeProcessRecord,
  type WorkflowStepDefinition,
} from '../api';

const ACTIVE_STATUSES = new Set([
  'starting',
  'running',
  'stopping',
  'killing',
]);

export type DeployQARuntimeProps = {
  runId: string | null;
  step: WorkflowStepDefinition;
  disabled?: boolean;
};

function statusColor(status: string): string {
  if (status === 'running') return 'green';
  if (status === 'starting') return 'blue';
  if (status === 'succeeded') return 'teal';
  if (status === 'failed' || status === 'timed_out') return 'red';
  if (status === 'stopped' || status === 'killed') return 'orange';
  return 'gray';
}

function serviceUrl(
  process: RuntimeProcessRecord | undefined
): string | null {
  if (!process || !ACTIVE_STATUSES.has(process.status)) return null;

  const port =
    process.environment?.SERVICE_PORT
    ?? process.environment?.MDEV_QA_PORT
    ?? process.environment?.PORT;

  if (typeof port !== 'string' || !/^\d+$/.test(port.trim())) {
    return null;
  }

  return `http://localhost:${port.trim()}`;
}

export function DeployQARuntime({
  runId,
  step,
  disabled = false,
}: DeployQARuntimeProps) {
  const [processes, setProcesses] = useState<RuntimeProcessRecord[]>([]);
  const [logExecutionId, setLogExecutionId] = useState<string | null>(null);
  const [busy, setBusy] = useState<'restart' | ''>('');
  const [error, setError] = useState('');

  const refresh = useCallback(async () => {
    if (!runId) {
      setProcesses([]);
      return;
    }

    try {
      const response = await getRuntimeProcesses();
      const runtimeProcesses = Array.isArray(response.processes)
        ? response.processes
        : [];

      setProcesses(
        runtimeProcesses.filter(
          (process) =>
            process?.owner?.run_id === runId &&
            process?.owner?.step_id === step.id
        )
      );
      setError('');
    } catch (nextError) {
      setError(
        nextError instanceof Error ? nextError.message : String(nextError)
      );
    }
  }, [runId, step.id]);

  useEffect(() => {
    void refresh();

    if (!runId) return;

    const timer = window.setInterval(() => {
      void refresh();
    }, 1500);

    return () => window.clearInterval(timer);
  }, [refresh, runId]);

  const serviceProcesses = useMemo(() => {
    const latestByService = new Map<string, RuntimeProcessRecord>();

    for (const process of processes) {
      if (
        process.mode !== 'service' ||
        process.owner.capability !== 'qa_environment'
      ) {
        continue;
      }

      const serviceId = process.owner.service_id ?? process.execution_id;
      const current = latestByService.get(serviceId);

      if (!current) {
        latestByService.set(serviceId, process);
        continue;
      }

      const processActive = ACTIVE_STATUSES.has(process.status);
      const currentActive = ACTIVE_STATUSES.has(current.status);

      if (processActive && !currentActive) {
        latestByService.set(serviceId, process);
        continue;
      }

      if (processActive === currentActive) {
        const processStartedAt = Date.parse(process.started_at ?? '') || 0;
        const currentStartedAt = Date.parse(current.started_at ?? '') || 0;

        if (processStartedAt >= currentStartedAt) {
          latestByService.set(serviceId, process);
        }
      }
    }

    return Array.from(latestByService.values());
  }, [processes]);

  const qaServices = useMemo<QaServiceSpec[]>(() => {
    return serviceProcesses.map((process, index): QaServiceSpec => ({
      id: process.owner.service_id || `service-${index + 1}`,
      label: process.owner.service_id || process.label || `Service ${index + 1}`,
      command: {
        id: process.command_id,
        label: process.label,
        command: process.command,
        arguments: process.arguments ?? [],
        working_directory: process.working_directory,
        environment: process.environment ?? {},
        shell: 'system',
        mode: 'service',
        timeout_seconds: null,
        continue_on_error: false,
      },
      port: {
        environment_variable: 'PORT',
        preferred: null,
      },
      readiness: {},
      public: Boolean(
        process.environment?.MDEV_QA_PUBLIC_URL
        ?? process.environment?.SERVICE_PUBLIC_URL
      ),
    }));
  }, [serviceProcesses]);

  const prepareProcesses = useMemo(
    () =>
      processes.filter(
        (process) => process.owner.capability === 'qa_environment.prepare'
      ),
    [processes]
  );

  const active = serviceProcesses.some((process) =>
    ACTIVE_STATUSES.has(process.status)
  );

  const failed = serviceProcesses.some(
    (process) => process.status === 'failed' || process.status === 'timed_out'
  );

  const deploymentStatus = active
    ? 'RUNNING'
    : failed
      ? 'FAILED'
      : serviceProcesses.length > 0
        ? 'STOPPED'
        : 'NOT STARTED';

  async function restart() {
    if (!runId) return;

    setBusy('restart');
    try {
      const deployment = restartWorkflowStage(runId, step.id);
      window.setTimeout(() => {
        setBusy((current) => current === 'restart' ? '' : current);
        void refresh();
      }, 1500);
      await deployment;
      await refresh();
      setError('');
    } catch (nextError) {
      setError(
        nextError instanceof Error ? nextError.message : String(nextError)
      );
    } finally {
      setBusy('');
    }
  }

  const logProcess = logExecutionId
    ? processes.find((process) => process.execution_id === logExecutionId) ?? null
    : null;

  return (
    <>
      <Modal
        opened={logProcess !== null}
        onClose={() => setLogExecutionId(null)}
        title={logProcess ? `${logProcess.label || logProcess.command} logs` : 'Service logs'}
        size="xl"
      >
        {logProcess ? (
          <Stack gap="sm">
            <Group gap="lg">
              <Text size="xs" c="dimmed">PID: {logProcess.pid ?? '—'}</Text>
              <Text size="xs" c="dimmed">Execution: {logProcess.execution_id.slice(0, 8)}</Text>
              <Badge size="sm" color={statusColor(logProcess.status)}>
                {logProcess.status}
              </Badge>
            </Group>

            <Text size="xs" fw={600}>Command</Text>
            <Code block>{logProcess.command}</Code>

            <Text size="xs" fw={600}>stdout</Text>
            <ScrollArea h={320} offsetScrollbars>
              <Code block>{logProcess.stdout || 'No stdout captured.'}</Code>
            </ScrollArea>

            <Text size="xs" fw={600}>stderr</Text>
            <ScrollArea h={220} offsetScrollbars>
              <Code block c="red">{logProcess.stderr || 'No stderr captured.'}</Code>
            </ScrollArea>
          </Stack>
        ) : null}
      </Modal>

      <Card withBorder padding="sm">
        <Stack gap="sm">
        <Group justify="space-between" align="flex-start">
          <Stack gap={3}>
            <Group gap="xs">
              <Text fw={700}>DeployQA runtime</Text>
              <Badge color={active ? 'green' : failed ? 'red' : 'gray'}>
                {deploymentStatus}
              </Badge>
            </Group>
            <Text size="xs" c="dimmed">
              {serviceProcesses.length} service process
              {serviceProcesses.length === 1 ? '' : 'es'} tracked by the API.
            </Text>
          </Stack>


        </Group>

        {error ? <Alert color="red">{error}</Alert> : null}

        {active ? (
          <Group gap="xs">
            <Button
              size="xs"
              variant="light"
              leftSection={<IconRotateClockwise size={15} />}
              disabled={disabled || !runId || busy !== ''}
              loading={busy === 'restart'}
              onClick={() => void restart()}
            >
              Restart
            </Button>
          </Group>
        ) : null}

        {qaServices.map((service, index) => {
          const serviceRecord = service as unknown as Record<string, unknown>;
          const serviceId =
            typeof serviceRecord.id === 'string'
              ? serviceRecord.id
              : `service-${index + 1}`;
          const process = serviceProcesses.find(
            (candidate) => candidate.owner.service_id === serviceId
          );
          const url = serviceUrl(process);
          return (
            <Card key={serviceId} withBorder padding="xs">
              <Stack gap="xs">
                <Group justify="space-between" align="flex-start">
                  <Stack gap={2}>
                    <Group gap="xs">
                      <Text size="sm" fw={600}>
                        {typeof serviceRecord.label === 'string'
                          ? serviceRecord.label
                          : serviceId}
                      </Text>
                      <Badge
                        size="sm"
                        color={statusColor(process?.status ?? 'not_started')}
                      >
                        {process?.status ?? 'not started'}
                      </Badge>
                    </Group>

                    {url ? (
                      <Anchor
                        href={url}
                        target="_blank"
                        rel="noreferrer"
                        size="sm"
                      >
                        <Group gap={4}>
                          <span>{url}</span>
                          <IconExternalLink size={13} />
                        </Group>
                      </Anchor>
                    ) : (
                      <Text size="xs" c="dimmed">
                        Local service URL unavailable.
                      </Text>
                    )}
                  </Stack>

                  {process ? (
                    <Button
                      size="compact-xs"
                      variant="subtle"
                      leftSection={<IconTerminal2 size={14} />}
                      onClick={() => setLogExecutionId(process.execution_id)}
                    >
                      Logs
                    </Button>
                  ) : null}
                </Group>

                {process ? (
                  <Group gap="lg">
                    <Text size="xs" c="dimmed">
                      PID: {process.pid ?? '—'}
                    </Text>
                    <Text size="xs" c="dimmed">
                      Execution: {process.execution_id.slice(0, 8)}
                    </Text>
                    <Text size="xs" c="dimmed">
                      Exit: {process.exit_code ?? '—'}
                    </Text>
                  </Group>
                ) : null}

              </Stack>
            </Card>
          );
        })}

        {prepareProcesses.length > 0 ? (
          <Stack gap="xs">
            <Text size="sm" fw={600}>Preparation commands</Text>
            {prepareProcesses.map((process) => (
              <Group key={process.execution_id} justify="space-between">
                <Text size="xs">{process.label || process.command}</Text>
                <Badge size="sm" color={statusColor(process.status)}>
                  {process.status}
                </Badge>
              </Group>
            ))}
          </Stack>
        ) : null}
        </Stack>
      </Card>
    </>
  );
}
