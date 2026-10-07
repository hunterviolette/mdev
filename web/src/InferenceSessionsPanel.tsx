import { useEffect, useState } from 'react';
import { Alert, Badge, Button, Card, Group, Select, SimpleGrid, Stack, Text, TextInput, Title } from '@mantine/core';
import {
  archiveInferenceSession,
  getRun,
  getWorkflowBuilderCatalog,
  listInferenceSessions,
  listOpenAiModels,
  type InferenceConfigPanel,
  type InferenceConfigPanelSession,
  type RepositoryInferenceSession,
} from './api';

type InferenceSessionsPanelProps = {
  opened: boolean;
 globals?: Record<string, unknown> | null;
  value: Record<string, unknown>;
  stageTypes: string[];
  repoRef?: string | null;
  runId?: string | null;
  busy?: boolean;
  status?: string | null;
  onCancel: () => void;
  onSave: (inference: Record<string, unknown>) => Promise<void> | void;
};

function emptyPanel(): InferenceConfigPanel {
  return {
    sessions: [],
    stage_mappings: [],
  };
}

function withSessionEditorIds(panel: InferenceConfigPanel): InferenceConfigPanel {
  return {
    ...panel,
    sessions: panel.sessions.map((session) => ({
      ...session,
      editor_id: session.editor_id ?? crypto.randomUUID(),
    })),
  };
}

function recordValue(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function stringValue(value: unknown): string | null {
  return typeof value === 'string' ? value : null;
}

function inferencePanelFromConfig(
  inference: Record<string, unknown>,
  stageTypes: string[],
): InferenceConfigPanel {
  const configuredSessions = recordValue(inference.sessions);
  const configuredStageSessions = recordValue(inference.stage_sessions);
  const sessionNames = new Set(Object.keys(configuredSessions));

  const sessions: InferenceConfigPanelSession[] = Object.entries(configuredSessions).map(([name, rawConfig]) => {
    const config = recordValue(rawConfig);
    const transport = config.transport === 'browser' ? 'browser' : 'api';
    const lifecycle = config.lifecycle === 'non_persistent' ? 'non_persistent' : 'persistent';
    const api = recordValue(config.api);
    const browser = recordValue(config.browser);
    const source = Object.values(configuredStageSessions)
      .map(recordValue)
      .find((binding) => binding.session === name);

    return {
      editor_id: crypto.randomUUID(),
      name,
      transport,
      lifecycle,
      provider: transport === 'api' ? stringValue(api.provider) : null,
      model: transport === 'api' ? stringValue(api.model) : null,
      endpoint: transport === 'api' ? stringValue(api.endpoint) : null,
      provider_options: transport === 'api' ? recordValue(api.provider_options) : {},
      browser_url: transport === 'browser' ? stringValue(browser.target_url) : null,
      is_default: false,
      source_mode: source?.mode === 'existing' ? 'existing' : 'new',
      repository_session_id: source?.mode === 'existing' ? stringValue(source.session_id) : null,
    };
  });

  const uniqueStageTypes = Array.from(new Set(stageTypes)).sort();
  const stage_mappings = uniqueStageTypes.map((stage_type) => {
    const binding = recordValue(configuredStageSessions[stage_type]);
    const session = stringValue(binding.session) ?? '';

    return {
      stage_type,
      session: sessionNames.has(session) ? session : '',
    };
  });

  return {
    sessions,
    stage_mappings,
  };
}

function inferenceConfigFromPanel(panel: InferenceConfigPanel): Record<string, unknown> {
  const sessions: Record<string, unknown> = {};
  const sessionSources = new Map<string, { mode: 'new' | 'existing'; sessionId: string | null }>();

  for (const session of panel.sessions) {
    const name = session.name.trim();
    if (!name) continue;

    const mode = session.source_mode === 'existing' ? 'existing' : 'new';
    const sessionId = session.repository_session_id?.trim() || null;
    sessionSources.set(name, { mode, sessionId });

    sessions[name] = session.transport === 'browser'
      ? {
          transport: 'browser',
          lifecycle: session.lifecycle,
          browser: {
            target_url: session.browser_url ?? '',
          },
        }
      : {
          transport: 'api',
          lifecycle: session.lifecycle,
          api: {
            provider: session.provider ?? 'openai',
            model: session.model ?? 'gpt-4.1',
            endpoint: session.endpoint ?? '',
            provider_options: session.provider_options ?? {},
          },
        };
  }

  const stage_sessions: Record<string, unknown> = {};
  for (const mapping of panel.stage_mappings) {
    if (!(mapping.session in sessions)) continue;

    const source = sessionSources.get(mapping.session) ?? {
      mode: 'new' as const,
      sessionId: null,
    };

    stage_sessions[mapping.stage_type] = {
      mode: source.mode,
      session: mapping.session,
      session_id: source.sessionId,
    };
  }

  return {
    sessions,
    stage_sessions,
  };
}

function repositorySessionProvider(session: RepositoryInferenceSession) {
  const api = session.config.api;
  if (!api || typeof api !== 'object' || Array.isArray(api)) return null;
  const provider = (api as Record<string, unknown>).provider;
  return typeof provider === 'string' ? provider : null;
}

function repositorySessionModel(session: RepositoryInferenceSession) {
  const api = session.config.api;
  if (!api || typeof api !== 'object' || Array.isArray(api)) return null;
  const model = (api as Record<string, unknown>).model;
  return typeof model === 'string' ? model : null;
}

function repositorySessionEndpoint(session: RepositoryInferenceSession) {
  const api = session.config.api;
  if (!api || typeof api !== 'object' || Array.isArray(api)) return null;
  const endpoint = (api as Record<string, unknown>).endpoint;
  return typeof endpoint === 'string' ? endpoint : null;
}

function repositorySessionBrowserUrl(session: RepositoryInferenceSession) {
  const browser = session.config.browser;
  if (!browser || typeof browser !== 'object' || Array.isArray(browser)) return null;
  const targetUrl = (browser as Record<string, unknown>).target_url;
  return typeof targetUrl === 'string' ? targetUrl : null;
}

export function InferenceSessionsPanel(props: InferenceSessionsPanelProps) {
  const { opened, value, stageTypes, repoRef = null, runId = null, busy = false, status = null, onCancel, onSave } = props;
  const [panel, setPanel] = useState<InferenceConfigPanel>(() => emptyPanel());
  const [structuralStagesLoading, setStructuralStagesLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [repositorySessions, setRepositorySessions] = useState<RepositoryInferenceSession[]>([]);
  const [repositoryActionBusy, setRepositoryActionBusy] = useState<string | null>(null);
  const [openAiModels, setOpenAiModels] = useState<string[]>([]);
  const [openAiModelsLoading, setOpenAiModelsLoading] = useState(false);

  const openAiModelOptions = openAiModels.map((model) => ({ value: model, label: model }));
  const usesOpenAi = panel.sessions.some((session) => session.transport === 'api' && session.provider === 'openai');
  const hasIncompleteExistingBinding = panel.sessions.some((session) => session.source_mode === 'existing' && !session.repository_session_id?.trim());

  useEffect(() => {
    if (!opened) return;

    let cancelled = false;
    setLoadError(null);

    const runtimeRunId = runId?.trim();
    if (!runtimeRunId) {
      setStructuralStagesLoading(false);
      setPanel(withSessionEditorIds(inferencePanelFromConfig(value, stageTypes)));
      return;
    }

    setStructuralStagesLoading(true);
    setPanel(emptyPanel());

    void Promise.all([
      getRun(runtimeRunId),
      getWorkflowBuilderCatalog(),
    ])
      .then(([run, catalog]) => {
        if (cancelled) return;

        const stageCapabilities = catalog.stage_capabilities ?? {};
        const structuralStageTypes = Array.from(new Set(
          (run.definition?.steps ?? [])
            .filter((step) => (stageCapabilities[step.step_type] ?? []).includes('inference'))
            .map((step) => step.step_type)
        )).sort();

        const workflowEngine = recordValue(run.context?.workflow_engine);
        const globalState = recordValue(workflowEngine.global_state);
        const capabilities = recordValue(globalState.capabilities);
        const runtimeInference = recordValue(capabilities.inference);
        const inference = Object.keys(runtimeInference).length > 0
          ? runtimeInference
          : value;

        setPanel(withSessionEditorIds(inferencePanelFromConfig(inference, structuralStageTypes)));
      })
      .catch((err) => {
        if (cancelled) return;
        setPanel(emptyPanel());
        setLoadError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setStructuralStagesLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [opened, value, stageTypes, runId]);

  useEffect(() => {
    if (!opened || !usesOpenAi) {
      setOpenAiModels([]);
      return;
    }

    let cancelled = false;
    setOpenAiModelsLoading(true);
    void listOpenAiModels()
      .then((response) => {
        if (cancelled) return;
        setOpenAiModels(response.models);
      })
      .catch((err) => {
        if (cancelled) return;
        setOpenAiModels([]);
        setLoadError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setOpenAiModelsLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [opened, usesOpenAi]);

  useEffect(() => {
    if (!opened || !repoRef?.trim()) {
      setRepositorySessions([]);
      return;
    }

    let cancelled = false;
    void listInferenceSessions(repoRef.trim(), runId)
      .then((response) => {
        if (cancelled) return;
        setRepositorySessions(response.sessions);
      })
      .catch((err) => {
        if (cancelled) return;
        setLoadError(err instanceof Error ? err.message : String(err));
      });

    return () => {
      cancelled = true;
    };
  }, [opened, repoRef, runId]);

  function updateSession(name: string, patch: Partial<InferenceConfigPanelSession>) {
    setPanel((prev) => ({
      ...prev,
      sessions: prev.sessions.map((session) => session.name === name ? { ...session, ...patch } : session),
    }));
  }

  function addSession() {
    setPanel((prev) => {
      if (prev.sessions.some((session) => session.source_mode === 'new' && !session.name.trim())) {
        return prev;
      }

      const session: InferenceConfigPanelSession = {
        editor_id: crypto.randomUUID(),
        name: '',
        transport: 'api',
        lifecycle: 'persistent',
        provider: 'openai',
        model: null,
        endpoint: '',
        provider_options: {},
        is_default: false,
        source_mode: 'new',
        repository_session_id: null,
      };
      return { ...prev, sessions: [...prev.sessions, session] };
    });
  }

  function removeSession(name: string) {
    setPanel((prev) => ({
      ...prev,
      sessions: prev.sessions.filter((session) => session.name !== name),
      stage_mappings: prev.stage_mappings.map((mapping) => mapping.session === name
        ? { ...mapping, session: '' }
        : mapping),
    }));
  }

  function changeTransport(sessionName: string, value: string | null) {
    const session = panel.sessions.find((item) => item.name === sessionName);
    if (!session) return;

    if (value === 'browser') {
      updateSession(sessionName, {
        transport: 'browser',
        provider: null,
        model: null,
        endpoint: null,
        browser_url: session.browser_url ?? '',
      });
      return;
    }

    updateSession(sessionName, {
      transport: 'api',
      provider: session.provider || 'openai',
      model: session.model || 'gpt-4.1',
      endpoint: session.endpoint || '',
      browser_url: null,
    });
  }

  function useRepositorySession(repositorySession: RepositoryInferenceSession) {
    setPanel((prev) => {
      if (prev.sessions.some((session) => session.repository_session_id === repositorySession.id)) {
        return prev;
      }

      const session: InferenceConfigPanelSession = {
        editor_id: crypto.randomUUID(),
        name: repositorySession.id,
        transport: repositorySession.transport,
        lifecycle: repositorySession.lifecycle,
        provider: repositorySessionProvider(repositorySession),
        model: repositorySessionModel(repositorySession),
        endpoint: repositorySessionEndpoint(repositorySession),
        provider_options: {},
        browser_url: repositorySessionBrowserUrl(repositorySession),
        is_default: prev.sessions.length === 0,
        source_mode: 'existing',
        repository_session_id: repositorySession.id,
      };

      return {
        ...prev,
        sessions: [...prev.sessions, session],
      };
    });
  }

  function removeRepositorySessionFromWorkflow(sessionId: string) {
    setPanel((prev) => {
      const removedNames = new Set(
        prev.sessions
          .filter((session) => session.repository_session_id === sessionId)
          .map((session) => session.name),
      );

      return {
        ...prev,
        sessions: prev.sessions.filter((session) => session.repository_session_id !== sessionId),
        stage_mappings: prev.stage_mappings.map((mapping) => removedNames.has(mapping.session)
          ? { ...mapping, session: '' }
          : mapping),
      };
    });
  }

  async function archiveRepositorySession(sessionId: string) {
    if (!runId?.trim()) return;
    setRepositoryActionBusy(sessionId);
    setLoadError(null);
    try {
      const response = await archiveInferenceSession(runId.trim(), sessionId);
      setRepositorySessions(response.sessions);
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : String(err));
    } finally {
      setRepositoryActionBusy(null);
    }
  }

  async function save() {
    await onSave(inferenceConfigFromPanel(panel));
  }

  return (
    <Stack gap="lg">
      <Group justify="space-between" align="flex-start" wrap="wrap">
        <Stack gap={2}>
          <Title order={4}>Inference</Title>
          <Text size="sm" c="dimmed">Define the inference sessions this workflow uses, then bind stages to them.</Text>
        </Stack>
        <Group gap="xs">
          <Badge variant="light">{panel.stage_mappings.length} stages</Badge>
          <Badge color="blue" variant="light">{panel.sessions.length} sessions</Badge>
          <Button size="xs" variant="light" onClick={addSession}>Add session</Button>
        </Group>
      </Group>

      {loadError ? <Alert color="red">{loadError}</Alert> : null}

      <Card withBorder>
        <Stack gap="md">
          <Group justify="space-between" align="flex-start" wrap="wrap">
            <Stack gap={2}>
              <Title order={5}>Repository sessions</Title>
              <Text size="xs" c="dimmed">Existing inference sessions owned by this repository. Choose which ones are available to this workflow or archive sessions that are no longer needed.</Text>
            </Stack>
            <Badge variant="light">{repositorySessions.length} available</Badge>
          </Group>

          {!repoRef?.trim() ? (
            <Alert color="gray">Select a repository to view its inference sessions.</Alert>
          ) : repositorySessions.length === 0 ? (
            <Alert color="gray">No active inference sessions exist in this repository.</Alert>
          ) : (
            <Stack gap="xs">
              {repositorySessions.map((repositorySession) => {
                const workflowSessions = panel.sessions.filter((session) => session.repository_session_id === repositorySession.id);
                const isUsed = workflowSessions.length > 0;
                const lastUsed = repositorySession.last_used_at || repositorySession.updated_at;

                return (
                  <Card key={repositorySession.id} withBorder padding="sm">
                    <Group justify="space-between" align="center" wrap="wrap">
                      <Stack gap={3}>
                        <Group gap="xs">
                          <Text size="sm" fw={600}>{repositorySession.title}</Text>
                          <Badge variant="light">{repositorySession.transport === 'browser' ? 'Browser' : 'API'}</Badge>
                          <Badge color={repositorySession.lifecycle === 'non_persistent' ? 'orange' : 'gray'} variant="light">
                            {repositorySession.lifecycle === 'non_persistent' ? 'Non-persistent' : 'Persistent'}
                          </Badge>
                          {isUsed ? <Badge color="green" variant="light">Used by workflow</Badge> : null}
                        </Group>
                        <Text size="xs" c="dimmed">{repositorySession.id}</Text>
                        <Text size="xs" c="dimmed">Last used {lastUsed}</Text>
                        {isUsed ? (
                          <Text size="xs" c="dimmed">Workflow session: {workflowSessions.map((session) => session.name).join(', ')}</Text>
                        ) : null}
                      </Stack>

                      <Group gap="xs">
                        {isUsed ? (
                          <Button
                            size="xs"
                            variant="default"
                            onClick={() => removeRepositorySessionFromWorkflow(repositorySession.id)}
                          >
                            Remove from workflow
                          </Button>
                        ) : (
                          <Button
                            size="xs"
                            variant="light"
                            onClick={() => useRepositorySession(repositorySession)}
                          >
                            Use in workflow
                          </Button>
                        )}
                        <Button
                          size="xs"
                          color="red"
                          variant="subtle"
                          onClick={() => void archiveRepositorySession(repositorySession.id)}
                          loading={repositoryActionBusy === repositorySession.id}
                          disabled={isUsed || !runId?.trim() || repositoryActionBusy !== null}
                        >
                          Archive
                        </Button>
                      </Group>
                    </Group>
                  </Card>
                );
              })}
            </Stack>
          )}
        </Stack>
      </Card>

      {panel.sessions.length === 0 ? (
        <Alert color="gray">Add an inference session to configure this workflow.</Alert>
      ) : (
        <Stack gap="md">
          {panel.sessions.map((session, sessionIndex) => {
            const existingSessionId = session.source_mode === 'existing' ? session.repository_session_id ?? null : null;
            const isExisting = Boolean(existingSessionId);
            const repositorySession = isExisting
              ? repositorySessions.find((candidate) => candidate.id === existingSessionId) ?? null
              : null;

            return (
              <Card key={session.editor_id} withBorder>
                <Stack gap="md">
                  <Group justify="space-between" align="flex-start" wrap="wrap">
                    <Stack gap={1}>
                      <Group gap="xs">
                        <Title order={5}>{session.name}</Title>
                        <Badge color={isExisting ? 'violet' : 'blue'} variant="light">
                          {isExisting ? 'Existing' : 'New'}
                        </Badge>
                      </Group>
                      <Text size="xs" c="dimmed">
                        {isExisting ? 'Existing repository session' : 'New workflow session'}
                      </Text>
                    </Stack>
                    {panel.sessions.length > 1 ? (
                      <Button size="xs" color="red" variant="subtle" onClick={() => removeSession(session.name)}>Remove</Button>
                    ) : null}
                  </Group>


                  {isExisting ? (
                    <Stack gap="xs">
                      <Group gap="xs">
                        <Badge color="violet" variant="light">Existing repository session</Badge>
                        <Badge variant="light">{repositorySession?.transport === 'browser' ? 'Browser' : 'API'}</Badge>
                        <Badge variant="light">{repositorySession?.lifecycle === 'non_persistent' ? 'Non-persistent' : 'Persistent'}</Badge>
                      </Group>
                      <Text size="sm" fw={600}>{repositorySession?.title ?? session.name}</Text>
                      <Text size="xs" c="dimmed">{existingSessionId}</Text>
                      <Alert color="gray">Existing session configuration is immutable. Create a new session and remap stages to change inference configuration.</Alert>
                    </Stack>
                  ) : (
                    <Stack gap="md">
                      <TextInput
                        label="Session name"
                        value={session.name}
                        placeholder="Enter a session name"
                        onChange={(event) => {
                          const value = event.currentTarget.value;
                          setPanel((prev) => ({
                            ...prev,
                            sessions: prev.sessions.map((candidate) => candidate === session
                              ? { ...candidate, name: value }
                              : candidate),
                            stage_mappings: prev.stage_mappings.map((mapping) => mapping.session === session.name
                              ? { ...mapping, session: value }
                              : mapping),
                          }));
                        }}
                      />

                      <SimpleGrid cols={{ base: 1, sm: 2 }}>
                        <Select
                          label="Transport"
                          value={session.transport === 'browser' ? 'browser' : 'api'}
                          onChange={(value) => changeTransport(session.name, value)}
                          data={[
                            { value: 'api', label: 'API' },
                            { value: 'browser', label: 'Browser' },
                          ]}
                          allowDeselect={false}
                        />
                        <Select
                          label="Lifecycle"
                          value={session.lifecycle === 'non_persistent' ? 'non_persistent' : 'persistent'}
                          onChange={(value) => updateSession(session.name, {
                            lifecycle: value === 'non_persistent' ? 'non_persistent' : 'persistent',
                          })}
                          data={[
                            { value: 'persistent', label: 'Persistent' },
                            { value: 'non_persistent', label: 'Non-persistent' },
                          ]}
                          allowDeselect={false}
                        />
                      </SimpleGrid>

                      {session.transport === 'browser' ? (
                        <TextInput
                          label="Browser URL"
                          value={session.browser_url ?? ''}
                          onChange={(event) => updateSession(session.name, { browser_url: event.currentTarget.value })}
                          placeholder="https://website.com/"
                        />
                      ) : (
                        <SimpleGrid cols={{ base: 1, sm: 2 }}>
                          <Select
                            label="Provider"
                            value={session.provider || 'openai'}
                            onChange={(value) => {
                              const provider = value || 'openai';
                              updateSession(session.name, {
                                provider,
                                model: provider === 'openai' ? null : session.model,
                                endpoint: provider === 'openai' ? null : session.endpoint,
                              });
                            }}
                            data={[
                              { value: 'openai', label: 'OpenAI' },
                              { value: 'anthropic', label: 'Anthropic' },
                            ]}
                            allowDeselect={false}
                          />
                          {session.provider === 'openai' ? (
                            <Select
                              label="Model"
                              value={session.model ?? null}
                              onChange={(value) => updateSession(session.name, { model: value })}
                              data={openAiModelOptions}
                              searchable
                              disabled={openAiModelsLoading}
                              nothingFoundMessage={openAiModelsLoading ? 'Loading models' : 'No models found'}
                              allowDeselect={false}
                            />
                          ) : (
                            <TextInput
                              label="Model"
                              value={session.model ?? ''}
                              onChange={(event) => updateSession(session.name, { model: event.currentTarget.value })}
                            />
                          )}
                          {session.provider !== 'openai' ? (
                            <TextInput
                              label="Endpoint"
                              value={session.endpoint ?? ''}
                              onChange={(event) => updateSession(session.name, { endpoint: event.currentTarget.value })}
                              placeholder="http://127.0.0.1:11434"
                            />
                          ) : null}
                        </SimpleGrid>
                      )}
                    </Stack>
                  )}
                </Stack>
              </Card>
            );
          })}
        </Stack>
      )}

      <Card withBorder>
        <Stack gap="md">
          <Stack gap={2}>
            <Title order={5}>Stage mappings</Title>
            <Text size="xs" c="dimmed">Map each inference-enabled workflow stage to one of the sessions selected for this workflow.</Text>
          </Stack>

          {panel.stage_mappings.length === 0 ? (
            <Alert color="gray">No inference-enabled stages are present in this workflow.</Alert>
          ) : panel.sessions.length === 0 ? (
            <Alert color="yellow">Add or select a workflow inference session before mapping stages.</Alert>
          ) : (
            <SimpleGrid cols={{ base: 1, md: 2, xl: 3 }} spacing="sm">
              {panel.stage_mappings.map((mapping) => (
                <Select
                  key={mapping.stage_type}
                  label={mapping.stage_type.toUpperCase()}
                  value={mapping.session}
                  onChange={(value) => {
                    if (!value) return;
                    setPanel((prev) => ({
                      ...prev,
                      stage_mappings: prev.stage_mappings.map((candidate) => candidate.stage_type === mapping.stage_type
                        ? { ...candidate, session: value }
                        : candidate),
                    }));
                  }}
                  data={panel.sessions
                    .filter((session) => session.name.trim())
                    .map((session) => {
                      const repositorySession = session.repository_session_id
                        ? repositorySessions.find((candidate) => candidate.id === session.repository_session_id)
                        : null;
                      return {
                        value: session.name,
                        label: repositorySession
                          ? `${repositorySession.title} · existing`
                          : `${session.name} · new`,
                      };
                    })}
                  allowDeselect={false}
                />
              ))}
            </SimpleGrid>
          )}
        </Stack>
      </Card>

      {hasIncompleteExistingBinding ? (
        <Alert color="yellow">Every existing session binding must reference a repository session.</Alert>
      ) : null}

      {status ? <Alert color={status.toLowerCase().includes('saved') ? 'green' : 'red'}>{status}</Alert> : null}

      <Group justify="flex-end">
        <Button size="xs" variant="default" onClick={onCancel}>Cancel</Button>
        <Button
          size="xs"
          onClick={() => void save()}
          loading={busy || structuralStagesLoading}
          disabled={
            structuralStagesLoading
            || hasIncompleteExistingBinding
            || panel.sessions.some((session) => session.source_mode === 'new' && !session.name.trim())
            || panel.stage_mappings.some((mapping) => !mapping.session.trim())
          }
        >
          Save
        </Button>
      </Group>
    </Stack>
  );
}
