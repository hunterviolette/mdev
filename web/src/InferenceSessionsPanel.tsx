import { useEffect, useMemo, useState } from 'react';
import { ActionIcon, Alert, Badge, Button, Card, Divider, Group, Select, SimpleGrid, Stack, Table, Text, TextInput, Title } from '@mantine/core';
import { IconTrash } from '@tabler/icons-react';
import {
  buildWorkflowBuilderInferencePanel,
  type InferenceConfigPanel,
  type InferenceConfigPanelSession,
  type WorkflowGlobalConfig,
  type WorkflowTemplateDefinition,
} from './api';

type InferenceSessionsPanelProps = {
  opened: boolean;
  globals: Record<string, unknown> | null | undefined;
  definition: WorkflowTemplateDefinition | null | undefined;
  busy?: boolean;
  status?: string | null;
  onCancel: () => void;
  onSave: (inference: Record<string, unknown>) => Promise<void> | void;
};

function emptyPanel(): InferenceConfigPanel {
  return {
    sessions: [
      {
        name: 'coding',
        transport: 'api',
        provider: 'openai',
        model: 'gpt-4.1',
        endpoint: '',
        is_default: true,
      },
    ],
    stage_mappings: [],
  };
}

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

function normalizeGlobals(globals: Record<string, unknown> | null | undefined): WorkflowGlobalConfig {
  return {
    resources: asRecord(globals?.resources),
    capabilities: asRecord(globals?.capabilities),
    automation: asRecord(globals?.automation),
  };
}

function nextSessionName(sessions: InferenceConfigPanelSession[]) {
  let index = sessions.length + 1;
  const names = new Set(sessions.map((session) => session.name));
  while (names.has(`session-${index}`)) {
    index += 1;
  }
  return `session-${index}`;
}

function sessionLabel(session: InferenceConfigPanelSession) {
  if (session.transport === 'browser') {
    return `${session.name} · ${session.browser_url?.trim() || 'browser'}`;
  }
  return `${session.name} · ${session.provider || 'provider'} / ${session.model || 'model'}`;
}

function sessionOptions(panel: InferenceConfigPanel) {
  return panel.sessions.map((session) => ({ value: session.name, label: sessionLabel(session) }));
}

function selectedOrFirst(panel: InferenceConfigPanel, selected: string) {
  return panel.sessions.some((session) => session.name === selected)
    ? selected
    : panel.sessions[0]?.name ?? 'coding';
}

export function InferenceSessionsPanel(props: InferenceSessionsPanelProps) {
  const { opened, globals, definition, busy = false, status = null, onCancel, onSave } = props;
  const [panel, setPanel] = useState<InferenceConfigPanel>(() => emptyPanel());
  const [selectedSession, setSelectedSession] = useState('coding');
  const [sessionNameDraft, setSessionNameDraft] = useState('coding');
  const [loadError, setLoadError] = useState<string | null>(null);

  const normalizedGlobals = useMemo(() => normalizeGlobals(globals), [globals]);
  const activeSessionName = selectedOrFirst(panel, selectedSession);
  const activeSession = panel.sessions.find((session) => session.name === activeSessionName) ?? panel.sessions[0] ?? emptyPanel().sessions[0];
  const options = sessionOptions(panel);

  useEffect(() => {
    if (!opened || !definition) {
      return;
    }

    let cancelled = false;
    setLoadError(null);
    void buildWorkflowBuilderInferencePanel({ definition, globals: normalizedGlobals })
      .then((response) => {
        if (cancelled) return;
        const nextPanel = response.panel.sessions.length ? response.panel : emptyPanel();
        const defaultSession = nextPanel.sessions.find((session) => session.is_default)?.name ?? nextPanel.sessions[0]?.name ?? 'coding';
        setPanel(nextPanel);
        setSelectedSession(defaultSession);
        setSessionNameDraft(defaultSession);
      })
      .catch((err) => {
        if (cancelled) return;
        setPanel(emptyPanel());
        setSelectedSession('coding');
        setSessionNameDraft('coding');
        setLoadError(err instanceof Error ? err.message : String(err));
      });

    return () => {
      cancelled = true;
    };
  }, [opened, definition, normalizedGlobals]);

  useEffect(() => {
    setSessionNameDraft(activeSessionName);
  }, [activeSessionName]);

  function updateSession(name: string, patch: Partial<InferenceConfigPanelSession>) {
    setPanel((prev) => ({
      ...prev,
      sessions: prev.sessions.map((session) => session.name === name ? { ...session, ...patch } : session),
    }));
  }

  function addSession() {
    const name = nextSessionName(panel.sessions);
    const session: InferenceConfigPanelSession = {
      name,
      transport: 'api',
      provider: 'openai',
      model: 'gpt-4.1',
      endpoint: '',
      is_default: panel.sessions.length === 0,
    };
    setPanel((prev) => ({ ...prev, sessions: [...prev.sessions, session] }));
    setSelectedSession(name);
    setSessionNameDraft(name);
  }

  function deleteSession(name: string) {
    if (panel.sessions.length <= 1) {
      return;
    }
    const fallback = panel.sessions.find((session) => session.name !== name)?.name ?? 'coding';
    setPanel((prev) => ({
      ...prev,
      sessions: prev.sessions.filter((session) => session.name !== name).map((session, index) => ({
        ...session,
        is_default: prev.sessions.find((item) => item.name === name)?.is_default && index === 0 ? true : session.is_default,
      })),
      stage_mappings: prev.stage_mappings.map((mapping) => ({
        ...mapping,
        session: mapping.session === name ? fallback : mapping.session,
      })),
    }));
    setSelectedSession(fallback);
    setSessionNameDraft(fallback);
  }

  function setDefaultSession(name: string) {
    setPanel((prev) => ({
      ...prev,
      sessions: prev.sessions.map((session) => ({ ...session, is_default: session.name === name })),
    }));
  }

  function renameSession() {
    const nextName = sessionNameDraft.trim();
    if (!nextName || nextName === activeSessionName || panel.sessions.some((session) => session.name === nextName)) {
      setSessionNameDraft(activeSessionName);
      return;
    }

    setPanel((prev) => ({
      ...prev,
      sessions: prev.sessions.map((session) => session.name === activeSessionName ? { ...session, name: nextName } : session),
      stage_mappings: prev.stage_mappings.map((mapping) => ({
        ...mapping,
        session: mapping.session === activeSessionName ? nextName : mapping.session,
      })),
    }));
    setSelectedSession(nextName);
    setSessionNameDraft(nextName);
  }

  function changeTransport(value: string | null) {
    if (value === 'browser') {
      updateSession(activeSessionName, {
        transport: 'browser',
        provider: null,
        model: null,
        endpoint: null,
        browser_url: activeSession.browser_url ?? '',
      });
      return;
    }

    updateSession(activeSessionName, {
      transport: 'api',
      provider: activeSession.provider || 'openai',
      model: activeSession.model || 'gpt-4.1',
      endpoint: activeSession.endpoint || '',
      browser_url: null,
    });
  }

  async function save() {
    if (!definition) {
      return;
    }
    const response = await buildWorkflowBuilderInferencePanel({
      definition,
      globals: normalizedGlobals,
      panel,
    });
    await onSave(response.inference);
  }

  return (
    <Stack gap="md">
      <Group justify="space-between" align="flex-start" wrap="wrap">
        <Stack gap={2}>
          <Title order={4}>Inference sessions</Title>
          <Text size="sm" c="dimmed">Manage backend-authored reusable inference sessions and stage mappings.</Text>
        </Stack>
        <Button size="xs" variant="light" onClick={addSession}>Add session</Button>
      </Group>

      {loadError ? <Alert color="red">{loadError}</Alert> : null}

      <Card withBorder>
        <Stack gap="sm">
          <Title order={5}>Sessions</Title>
          <Table striped withTableBorder>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>Session</Table.Th>
                <Table.Th>Transport</Table.Th>
                <Table.Th>Default</Table.Th>
                <Table.Th></Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {panel.sessions.map((session) => {
                const selected = session.name === activeSessionName;
                return (
                  <Table.Tr key={session.name}>
                    <Table.Td>
                      <Button size="xs" variant={selected ? 'filled' : 'subtle'} onClick={() => setSelectedSession(session.name)}>
                        {sessionLabel(session)}
                      </Button>
                    </Table.Td>
                    <Table.Td><Badge variant="light">{session.transport === 'browser' ? 'Browser' : 'API'}</Badge></Table.Td>
                    <Table.Td>{session.is_default ? <Badge color="green">Default</Badge> : null}</Table.Td>
                    <Table.Td>
                      <Group justify="flex-end" gap="xs">
                        <Button size="xs" variant="default" disabled={session.is_default} onClick={() => setDefaultSession(session.name)}>Set default</Button>
                        <ActionIcon color="red" variant="light" disabled={panel.sessions.length <= 1} onClick={() => deleteSession(session.name)}>
                          <IconTrash size={16} />
                        </ActionIcon>
                      </Group>
                    </Table.Td>
                  </Table.Tr>
                );
              })}
            </Table.Tbody>
          </Table>
        </Stack>
      </Card>

      <SimpleGrid cols={{ base: 1, lg: 2 }} spacing="md">
        <Card withBorder>
          <Stack gap="sm">
            <Title order={5}>Edit session</Title>
            <TextInput
              label="Session name"
              value={sessionNameDraft}
              onChange={(event) => setSessionNameDraft(event.currentTarget.value)}
              onBlur={renameSession}
              onKeyDown={(event) => {
                if (event.key === 'Enter') renameSession();
              }}
            />
            <Select
              label="Transport"
              value={activeSession.transport === 'browser' ? 'browser' : 'api'}
              onChange={changeTransport}
              data={[
                { value: 'api', label: 'API' },
                { value: 'browser', label: 'Browser' },
              ]}
              allowDeselect={false}
            />
            {activeSession.transport === 'browser' ? (
              <TextInput
                label="Browser URL"
                value={activeSession.browser_url ?? ''}
                onChange={(event) => updateSession(activeSessionName, { browser_url: event.currentTarget.value })}
                placeholder="https://website.com/"
              />
            ) : (
              <SimpleGrid cols={{ base: 1, sm: 2 }}>
                <TextInput
                  label="Provider"
                  value={activeSession.provider ?? ''}
                  onChange={(event) => updateSession(activeSessionName, { provider: event.currentTarget.value })}
                  placeholder="openai, anthropic, ollama"
                />
                <TextInput
                  label="Model"
                  value={activeSession.model ?? ''}
                  onChange={(event) => updateSession(activeSessionName, { model: event.currentTarget.value })}
                  placeholder="gpt-4.1"
                />
                <TextInput
                  label="Endpoint"
                  value={activeSession.endpoint ?? ''}
                  onChange={(event) => updateSession(activeSessionName, { endpoint: event.currentTarget.value })}
                  placeholder="http://127.0.0.1:11434"
                />
              </SimpleGrid>
            )}
            <Text size="xs" c="dimmed">Browser sessions only expose browser URL; runtime, CDP, and process allocation remain backend-owned.</Text>
          </Stack>
        </Card>

        <Card withBorder>
          <Stack gap="sm">
            <Title order={5}>Stage mappings</Title>
            <Divider />
            {panel.stage_mappings.length === 0 ? (
              <Alert color="gray">No inference-enabled stages are present in this workflow definition.</Alert>
            ) : (
              <Table striped withTableBorder>
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>Stage type</Table.Th>
                    <Table.Th>Inference session</Table.Th>
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {panel.stage_mappings.map((mapping) => (
                    <Table.Tr key={mapping.stage_type}>
                      <Table.Td><Badge variant="light">{mapping.stage_type}</Badge></Table.Td>
                      <Table.Td>
                        <Select
                          value={mapping.session}
                          onChange={(value) => setPanel((prev) => ({
                            ...prev,
                            stage_mappings: prev.stage_mappings.map((item) => item.stage_type === mapping.stage_type ? { ...item, session: value || mapping.session } : item),
                          }))}
                          data={options}
                          allowDeselect={false}
                        />
                      </Table.Td>
                    </Table.Tr>
                  ))}
                </Table.Tbody>
              </Table>
            )}
          </Stack>
        </Card>
      </SimpleGrid>

      {status ? <Alert color={status.toLowerCase().includes('saved') ? 'green' : 'red'}>{status}</Alert> : null}

      <Group justify="flex-end">
        <Button size="xs" variant="default" onClick={onCancel}>Cancel</Button>
        <Button size="xs" onClick={() => void save()} loading={busy}>Save</Button>
      </Group>
    </Stack>
  );
}
