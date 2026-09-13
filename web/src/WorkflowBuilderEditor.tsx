import { useEffect, useMemo, useRef, useState } from 'react';
import {
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Code,
  Divider,
  Group,
  JsonInput,
  Loader,
  Menu,
  Modal,
  ScrollArea,
  Select,
  Stack,
  Switch,
  Text,
  TextInput,
  Textarea,
  Title,
} from '@mantine/core';
import {
  compileWorkflowBuilderDocument,
  getWorkflowBuilderCatalog,
  type CompileWorkflowBuilderResponse,
  type WorkflowCapabilitySummaryItem,
  type WorkflowAutomationControlDescriptor,
  type WorkflowStageDescriptor,
  type WorkflowStageField,
  type SharedDependenciesConfig,
  type WorkflowGlobalConfig,
  type WorkflowTemplateDefinition,
} from './api';
import { buildBuilderDocument, builderStepFromDescriptor, builderStepsFromDefinition, capabilityDisplayLabel, defaultGlobals, descriptorMap, type BuilderStep } from './workflow_builder';

import {
  DeployQA,
  deployQACapabilityFromValues,
  deployQAValuesFromCapability,
  type DeployQAValues,
} from './Capabilities/DeployQA';
import { SharedDependencies } from './Capabilities/SharedDependencies';
import {
  Automation,
  type AutomationProfile,
} from './Capabilities/Automation';

type WorkflowBuilderEditorProps = {
  initialDefinition?: WorkflowTemplateDefinition | null;
  builderGlobals?: WorkflowTemplateDefinition['globals'] | null;
  loadRevision?: number;
  onCompiledDefinitionChange: (definition: WorkflowTemplateDefinition) => void;
  onError?: (message: string | null) => void;
  onOpenCapabilityConfig?: (
    capabilityKey: string,
    onChange: (value: Record<string, unknown>) => void
  ) => void;
};

type CompileState = 'idle' | 'dirty' | 'compiling' | 'compiled' | 'error';

function compileBadgeColor(state: CompileState) {
  switch (state) {
    case 'compiled':
      return 'green';
    case 'compiling':
      return 'yellow';
    case 'dirty':
      return 'orange';
    case 'error':
      return 'red';
    default:
      return 'gray';
  }
}

function stageCardTone(selected: boolean) {
  if (selected) {
    return {
      border: '2px solid var(--mantine-color-blue-5)',
      background: 'rgba(34, 139, 230, 0.10)',
    };
  }
  return {
    border: '1px solid var(--mantine-color-dark-4)',
    background: 'var(--mantine-color-body)',
  };
}

function fieldControl(field: WorkflowStageField) {
  if (field.ui?.control) {
    return field.ui.control;
  }
  if (field.type === 'boolean') {
    return 'switch';
  }
  if (field.type === 'integer') {
    return 'number';
  }
  if (field.type === 'multiline_text') {
    return 'textarea';
  }
  return 'text';
}

function capabilitySummaryDetail(item: WorkflowCapabilitySummaryItem) {
  if (item.stage_types.length === 0) {
    return 'Used by workflow';
  }
  return `Used by ${item.stage_types.join(', ')}`;
}

function normalizedCapabilityKey(capabilityKey: string) {
  return capabilityKey.trim().toLowerCase().replace(/[\s/-]+/g, '_');
}

function isDeployQACapability(capabilityKey: string) {
  const normalized = normalizedCapabilityKey(capabilityKey);
  return normalized === 'qa_environment' || normalized === 'deployqa' || normalized === 'deploy_qa';
}

function isSharedDependenciesCapability(capabilityKey: string) {
  const normalized = normalizedCapabilityKey(capabilityKey);
  return normalized === 'shared_dependencies' || normalized === 'shareddependencies';
}

function canonicalCapabilityConfigKey(capabilityKey: string) {
  const normalized = normalizedCapabilityKey(capabilityKey);

  switch (normalized) {
    case 'changeset':
    case 'changeset_apply':
    case 'gateway_model_changeset':
      return 'changeset';
    case 'compile':
    case 'compile_commands':
      return 'compile_commands';
    case 'context_export':
    case 'repo_context':
      return 'context_export';
    case 'qa_environment':
    case 'deployqa':
    case 'deploy_qa':
      return 'qa_environment';
    case 'shareddependencies':
      return 'shared_dependencies';
    default:
      return normalized;
  }
}

function isGlobalEditableCapability(capabilityKey: string) {
  const normalized = normalizedCapabilityKey(capabilityKey);

  return normalized === 'context_export'
    || normalized === 'inference'
    || isDeployQACapability(capabilityKey)
    || isSharedDependenciesCapability(capabilityKey);
}

function workflowBuilderFieldVisible(field: WorkflowStageField, fields: Record<string, unknown>) {
  return (field.visible_when ?? []).every((condition) => {
    const value = fields[condition.path];
    return value === condition.equals;
  });
}


export function WorkflowBuilderEditor({ initialDefinition, builderGlobals, loadRevision = 0, onCompiledDefinitionChange, onError, onOpenCapabilityConfig }: WorkflowBuilderEditorProps) {
  const [catalog, setCatalog] = useState<Record<string, WorkflowStageDescriptor>>({});
  const [stageDescriptors, setStageDescriptors] = useState<WorkflowStageDescriptor[]>([]);
  const [automationControls, setAutomationControls] = useState<WorkflowAutomationControlDescriptor[]>([]);
  const [steps, setSteps] = useState<BuilderStep[]>([]);
  const [selectedStepId, setSelectedStepId] = useState<string | null>(null);
  const [globalsRevision, setGlobalsRevision] = useState(0);
  const [loading, setLoading] = useState(true);
  const [compileState, setCompileState] = useState<CompileState>('idle');
  const [compileMessage, setCompileMessage] = useState('');
  const [compileRevision, setCompileRevision] = useState(0);
  const [compiledCapabilitySummary, setCompiledCapabilitySummary] = useState<WorkflowCapabilitySummaryItem[]>([]);
  const [deployQAOpen, setDeployQAOpen] = useState(false);
  const [sharedDependenciesOpen, setSharedDependenciesOpen] = useState(false);
  const [automationOpen, setAutomationOpen] = useState(false);
  const [automationSaving, setAutomationSaving] = useState(false);
  const [automationStatus, setAutomationStatus] = useState<string | null>(null);
  const [capabilityConfigKey, setCapabilityConfigKey] = useState<string | null>(null);
  const [capabilityConfigDraft, setCapabilityConfigDraft] = useState('{}');
  const [capabilityConfigError, setCapabilityConfigError] = useState<string | null>(null);
  const [editableGlobals, setEditableGlobals] = useState<WorkflowGlobalConfig>(() =>
    structuredClone(initialDefinition?.globals ?? builderGlobals ?? defaultGlobals())
  );
  const latestCompiledDefinitionRef = useRef<WorkflowTemplateDefinition | null>(
    initialDefinition ? structuredClone(initialDefinition) : null
  );
  const compileRequestRef = useRef(0);
  const onErrorRef = useRef(onError);

  useEffect(() => {
    onErrorRef.current = onError;
  }, [onError]);

  useEffect(() => {
    setEditableGlobals(
      structuredClone(initialDefinition?.globals ?? builderGlobals ?? defaultGlobals())
    );
    latestCompiledDefinitionRef.current = initialDefinition
      ? structuredClone(initialDefinition)
      : null;
    setGlobalsRevision((current) => current + 1);
  }, [loadRevision]);

  const selectedStep = useMemo(
    () => steps.find((step) => step.id === selectedStepId) ?? null,
    [steps, selectedStepId]
  );
  const selectedDescriptor = useMemo(
    () => (selectedStep ? catalog[selectedStep.stepType] ?? null : null),
    [catalog, selectedStep]
  );

  const selectedStageCapabilitySummary = useMemo(
    () => compiledCapabilitySummary.find((item) => item.stage_ids.includes(selectedStep?.id ?? '')) ?? null,
    [compiledCapabilitySummary, selectedStep?.id]
  );

  const invokedAutomationCapabilities = useMemo(
    () => compiledCapabilitySummary.map((item) => item.key),
    [compiledCapabilitySummary]
  );

  const sharedDependencies = useMemo<SharedDependenciesConfig>(() => {
    const capabilities = editableGlobals.capabilities ?? {};
    const configured = capabilities.shared_dependencies;
    const legacy = editableGlobals.shared_dependencies;

    if (configured && typeof configured === 'object' && !Array.isArray(configured)) {
      return configured as SharedDependenciesConfig;
    }

    return legacy ?? {
      enabled: false,
      providers: [],
    };
  }, [editableGlobals.capabilities, editableGlobals.shared_dependencies]);

  const automationProfile = useMemo<AutomationProfile>(() => {
    const configured = editableGlobals.automation;
    const value = configured && typeof configured === 'object' && !Array.isArray(configured)
      ? configured as Partial<AutomationProfile>
      : {};
    const threshold = (candidate: unknown, fallback: number) =>
      typeof candidate === 'number' && Number.isFinite(candidate)
        ? Math.max(1, Math.floor(candidate))
        : fallback;

    return {
      new_session: Array.isArray(value.new_session)
        ? value.new_session.filter((item): item is string => typeof item === 'string')
        : ['repo_context', 'changeset_schema', 'planner_fragment'],
      selected: Array.isArray(value.selected)
        ? value.selected.filter((item): item is string => typeof item === 'string')
        : automationControls.map((descriptor) => descriptor.key),
      inject_file_context_after_changeset_failures: threshold(
        value.inject_file_context_after_changeset_failures,
        2,
      ),
      inject_broad_context_after_changeset_failures: threshold(
        value.inject_broad_context_after_changeset_failures,
        4,
      ),
      pause_after_changeset_failures: threshold(
        value.pause_after_changeset_failures,
        6,
      ),
      inject_changeset_schema_after_errors: threshold(
        value.inject_changeset_schema_after_errors,
        3,
      ),
      pause_after_compile_errors: threshold(
        value.pause_after_compile_errors,
        4,
      ),
    };
  }, [editableGlobals.automation, automationControls]);

  const deployQAValues = useMemo(
    () => deployQAValuesFromCapability(editableGlobals.capabilities?.qa_environment),
    [editableGlobals.capabilities]
  );

  useEffect(() => {
    let cancelled = false;

    async function load() {
      setLoading(true);
      try {
        const loadedCatalog = await getWorkflowBuilderCatalog();
        if (cancelled) {
          return;
        }
        const descriptors = loadedCatalog.stage_descriptors ?? [];
        const byType = descriptorMap(loadedCatalog);
        setCatalog(byType);
        setStageDescriptors(descriptors);
        setAutomationControls(loadedCatalog.automation_controls ?? []);

        const hydratedSteps = builderStepsFromDefinition(initialDefinition, loadedCatalog);
        setGlobalsRevision((prev) => prev + 1);
        if (hydratedSteps.length > 0) {
          setSteps(hydratedSteps);
          setSelectedStepId((prev) => {
            if (prev && hydratedSteps.some((step) => step.id === prev)) {
              return prev;
            }
            return hydratedSteps[0]?.id ?? null;
          });
        } else if (descriptors.length > 0) {
          const first = builderStepFromDescriptor(descriptors[0]);
          setSteps([first]);
          setSelectedStepId(first.id);
        } else {
          setSteps([]);
          setSelectedStepId(null);
        }
      } catch (error) {
        if (!cancelled) {
          onErrorRef.current?.(error instanceof Error ? error.message : String(error));
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    }

    void load();
    return () => {
      cancelled = true;
    };
  }, [loadRevision]);

  function markDirty(message = 'Unsaved visual changes') {
    compileRequestRef.current += 1;
    setCompileState('dirty');
    setCompileMessage(message);
    setCompileRevision((prev) => prev + 1);
  }

  async function updateBuilderGlobals(
    nextGlobals: WorkflowGlobalConfig,
    message: string
  ): Promise<boolean> {
    const requestId = ++compileRequestRef.current;
    setEditableGlobals(nextGlobals);
    setCompileState('compiling');
    setCompileMessage(message);

    try {
      const result = await compileWorkflowBuilderDocument(
        buildBuilderDocument(steps, nextGlobals)
      );

      if (requestId !== compileRequestRef.current) {
        return false;
      }

      if (!result.ok) {
        const errorMessage = result.errors.length > 0
          ? result.errors.join('\n')
          : 'Workflow compilation failed.';
        setCompiledCapabilitySummary([]);
        setCompileState('error');
        setCompileMessage(errorMessage);
        onErrorRef.current?.(errorMessage);
        return false;
      }

      const nextDefinition = structuredClone(result.definition);
      setCompiledCapabilitySummary(result.capability_summary ?? []);
      setEditableGlobals(structuredClone(nextDefinition.globals));
      latestCompiledDefinitionRef.current = structuredClone(nextDefinition);
      onCompiledDefinitionChange(structuredClone(nextDefinition));
      setCompileState('compiled');
      setCompileMessage(
        result.warnings.length > 0
          ? result.warnings.join('\n')
          : 'Compiled successfully'
      );
      onErrorRef.current?.(null);
      return true;
    } catch (error) {
      if (requestId !== compileRequestRef.current) {
        return false;
      }

      const errorMessage = error instanceof Error ? error.message : String(error);
      setCompiledCapabilitySummary([]);
      setCompileState('error');
      setCompileMessage(errorMessage);
      onErrorRef.current?.(errorMessage);
      return false;
    }
  }

  function updateCapabilityConfig(
    capabilityKey: string,
    value: Record<string, unknown>,
    message = `${capabilityDisplayLabel(capabilityKey)} capability configuration changed`
  ) {
    const configKey = canonicalCapabilityConfigKey(capabilityKey);
    const nextGlobals: WorkflowGlobalConfig = {
      ...structuredClone(editableGlobals),
      capabilities: {
        ...structuredClone(editableGlobals.capabilities ?? {}),
        [configKey]: structuredClone(value),
      },
    };

    void updateBuilderGlobals(nextGlobals, message);
  }

  function addStep(stepType: string) {
    const descriptor = catalog[stepType];
    if (!descriptor) {
      return;
    }
    const step = builderStepFromDescriptor(descriptor);
    setSteps((prev) => [...prev, step]);
    setSelectedStepId(step.id);
    markDirty();
  }

  function removeStep(stepId: string) {
    setSteps((prev) => prev.filter((step) => step.id !== stepId));
    setSelectedStepId((prev) => (prev === stepId ? null : prev));
    markDirty();
  }

  function moveStep(stepId: string, direction: -1 | 1) {
    setSteps((prev) => {
      const index = prev.findIndex((step) => step.id === stepId);
      if (index < 0) {
        return prev;
      }
      const nextIndex = index + direction;
      if (nextIndex < 0 || nextIndex >= prev.length) {
        return prev;
      }
      const copy = [...prev];
      const [item] = copy.splice(index, 1);
      copy.splice(nextIndex, 0, item);
      return copy;
    });
    markDirty();
  }

  function updateStep(stepId: string, patch: Partial<BuilderStep>) {
    setSteps((prev) => prev.map((step) => (step.id === stepId ? { ...step, ...patch } : step)));
    markDirty();
  }

  function updateStepField(stepId: string, key: string, value: unknown) {
    setSteps((prev) =>
      prev.map((step) =>
        step.id === stepId
          ? {
              ...step,
              fields: {
                ...step.fields,
                [key]: value,
              },
            }
          : step
      )
    );
    markDirty();
  }


  function renderField(field: WorkflowStageField, value: unknown) {
    if (!selectedStep) {
      return null;
    }
    const control = fieldControl(field);
    if (control === 'switch') {
      return (
        <Switch
          key={field.key}
          label={field.label}
          description={field.description}
          checked={Boolean(value)}
          onChange={(event) => updateStepField(selectedStep.id, field.key, event.currentTarget.checked)}
        />
      );
    }
    if (control === 'number') {
      return (
        <TextInput
          key={field.key}
          label={field.label}
          description={field.description}
          placeholder={field.ui?.placeholder}
          value={String(typeof value === 'number' ? value : Number(value ?? 0) || 0)}
          onChange={(event) => updateStepField(selectedStep.id, field.key, Number(event.currentTarget.value || '0'))}
        />
      );
    }
    if (control === 'textarea') {
      return (
        <Textarea
          key={field.key}
          label={field.label}
          description={field.description}
          placeholder={field.ui?.placeholder}
          minRows={field.ui?.min_rows ?? 4}
          autosize
          value={typeof value === 'string' ? value : ''}
          onChange={(event) => updateStepField(selectedStep.id, field.key, event.currentTarget.value)}
        />
      );
    }
    if (control === 'string_list' || control === 'dependency_providers') {
      const values = Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
      return (
        <TextInput
          key={field.key}
          label={field.label}
          description={field.description}
          placeholder={field.ui?.placeholder}
          value={values.join(', ')}
          onChange={(event) => {
            const nextValue = event.currentTarget.value
              .split(',')
              .map((item) => item.trim())
              .filter(Boolean);
            updateStepField(selectedStep.id, field.key, nextValue);
          }}
        />
      );
    }
    if (control === 'select') {
      return (
        <Select
          key={field.key}
          label={field.label}
          description={field.description}
          placeholder={field.ui?.placeholder}
          data={(field.options ?? []).map((option) => ({ value: option.value, label: option.label }))}
          value={typeof value === 'string' ? value : ''}
          onChange={(nextValue) => updateStepField(selectedStep.id, field.key, nextValue ?? '')}
          clearable={!field.required}
        />
      );
    }
    return (
      <TextInput
        key={field.key}
        label={field.label}
        description={field.description}
        placeholder={field.ui?.placeholder}
        value={typeof value === 'string' ? value : ''}
        onChange={(event) => updateStepField(selectedStep.id, field.key, event.currentTarget.value)}
      />
    );
  }


  async function compileDocument(): Promise<WorkflowTemplateDefinition | null> {
    const requestId = ++compileRequestRef.current;

    try {
      setCompileState('compiling');
      setCompileMessage('Compiling backend-defined workflow');
      const result: CompileWorkflowBuilderResponse = await compileWorkflowBuilderDocument(
        buildBuilderDocument(steps, editableGlobals)
      );

      if (requestId !== compileRequestRef.current) {
        return null;
      }

      if (!result.ok) {
        const message = result.errors.length > 0 ? result.errors.join('\n') : 'Workflow compilation failed.';
        setCompiledCapabilitySummary([]);
        setCompileState('error');
        setCompileMessage(message);
        onError?.(message);
        return null;
      }

      setCompiledCapabilitySummary(result.capability_summary ?? []);
      setEditableGlobals(structuredClone(result.definition.globals));
      latestCompiledDefinitionRef.current = structuredClone(result.definition);
      onCompiledDefinitionChange(structuredClone(result.definition));
      setCompileState('compiled');
      setCompileMessage(result.warnings.length > 0 ? result.warnings.join('\n') : 'Compiled successfully');
      onError?.(null);
      return result.definition;
    } catch (error) {
      if (requestId !== compileRequestRef.current) {
        return null;
      }

      const message = error instanceof Error ? error.message : String(error);
      setCompiledCapabilitySummary([]);
      setCompileState('error');
      setCompileMessage(message);
      onError?.(message);
      return null;
    }
  }

  async function closeDeployQA() {
    const definition = await compileDocument();
    if (!definition) {
      return;
    }

    setDeployQAOpen(false);
  }

  async function openDeployQA() {
    if (deployQAValues) {
      setDeployQAOpen(true);
      return;
    }

    const definition = await compileDocument();
    const values = deployQAValuesFromCapability(
      definition?.globals.capabilities?.qa_environment
    );

    if (!values) {
      onError?.(
        `Invalid DeployQA capability configuration: ${JSON.stringify(
          definition?.globals.capabilities?.qa_environment ?? null
        )}`
      );
      return;
    }

    setDeployQAOpen(true);
  }

  useEffect(() => {
    if (steps.length === 0) {
      return;
    }
    void compileDocument();
  }, [compileRevision, globalsRevision]);

  if (loading) {
    return (
      <Box p="xl">
        <Group>
          <Loader size="sm" />
          <Text size="sm">Loading builder catalog…</Text>
        </Group>
      </Box>
    );
  }

  return (
    <Box h="100%" p="md">
      <Box
        style={{
          display: 'grid',
          gridTemplateColumns: 'minmax(0, 1fr) 360px',
          gap: 16,
          height: '100%',
          minHeight: 0,
        }}
      >
        <Card withBorder h="100%" p="sm">
          <Stack h="100%" gap="sm">
            <Group justify="space-between" align="center">
              <Group gap="xs">
                <Title order={4}>Workflow pipeline</Title>
                <Badge color={compileBadgeColor(compileState)} variant="light">
                  {compileState.toUpperCase()}
                </Badge>
              </Group>
              <Group gap="xs">
                <Menu shadow="md" width={260}>
                  <Menu.Target>
                    <Button variant="light">Add stage</Button>
                  </Menu.Target>
                  <Menu.Dropdown>
                    {stageDescriptors.map((descriptor) => (
                      <Menu.Item key={descriptor.step_type} onClick={() => addStep(descriptor.step_type)}>
                        {descriptor.label}
                      </Menu.Item>
                    ))}
                  </Menu.Dropdown>
                </Menu>
              </Group>
            </Group>

            {compileMessage ? <Alert color={compileState === 'error' ? 'red' : 'blue'}>{compileMessage}</Alert> : null}

            {compiledCapabilitySummary.length > 0 ? (
              <Card withBorder p="sm">
                <Stack gap="xs">
                  <Text fw={600} size="sm">Capabilities invoked</Text>
                  <Group gap="xs">
                    <Button
                      variant="light"
                      size="compact-xs"
                      title="Configure workflow automation policies"
                      onClick={() => {
                        setAutomationStatus(null);
                        setAutomationOpen(true);
                      }}
                    >
                      Automation
                    </Button>
                    {compiledCapabilitySummary.map((item) => {
                      const editable = isGlobalEditableCapability(item.key);
                      return (
                        <Button
                          key={item.key}
                          variant={editable ? 'light' : 'default'}
                          size="compact-xs"
                          title={editable ? `${capabilitySummaryDetail(item)} · Click to configure` : capabilitySummaryDetail(item)}
                          style={{ cursor: editable ? 'pointer' : 'default' }}
                          onClick={(event) => {
                            event.preventDefault();
                            event.stopPropagation();

                            if (!editable) {
                              return;
                            }

                            if (isDeployQACapability(item.key)) {
                              const qaStep = steps.find((step) =>
                                item.stage_ids.includes(step.id) && step.stepType === 'qa'
                              ) ?? steps.find((step) => step.stepType === 'qa');

                              if (qaStep) {
                                setSelectedStepId(qaStep.id);
                                void openDeployQA();
                              }
                              return;
                            }

                            if (isSharedDependenciesCapability(item.key)) {
                              setSharedDependenciesOpen(true);
                              return;
                            }

                            const configKey = canonicalCapabilityConfigKey(item.key);

                            if (configKey === 'context_export' || configKey === 'inference') {
                              onOpenCapabilityConfig?.(
                                configKey,
                                (value) => updateCapabilityConfig(configKey, value)
                              );
                              return;
                            }

                            const currentConfig = editableGlobals.capabilities?.[configKey] ?? {};

                            setCapabilityConfigKey(configKey);
                            setCapabilityConfigDraft(JSON.stringify(currentConfig, null, 2));
                            setCapabilityConfigError(null);
                            onOpenCapabilityConfig?.(
                              configKey,
                              (value) => updateCapabilityConfig(configKey, value)
                            );
                          }}
                        >
                          {capabilityDisplayLabel(item.key)}
                        </Button>
                      );
                    })}
                  </Group>
                </Stack>
              </Card>
            ) : null}

            <ScrollArea h="100%" type="auto">
              <Stack gap="sm">
                {steps.map((step, index) => {
                  const descriptor = catalog[step.stepType];
                  const selected = step.id === selectedStepId;
                  return (
                    <Card
                      key={step.id}
                      withBorder
                      padding="sm"
                      style={{ cursor: 'pointer', ...stageCardTone(selected) }}
                      onClick={() => setSelectedStepId(step.id)}
                    >
                      <Group justify="space-between" align="start">
                        <Stack gap={4}>
                          <Group gap={8}>
                            <Badge variant="light">{index + 1}</Badge>
                            <Text fw={600}>{step.name}</Text>
                          </Group>
                          <Group gap={8}>
                            <Code>{step.stepType}</Code>
                            {descriptor?.category ? <Badge variant="dot">{descriptor.category}</Badge> : null}
                          </Group>
                        </Stack>
                        <Group gap="xs">
                          <Button
                            variant="subtle"
                            size="xs"
                            onClick={(event) => {
                              event.stopPropagation();
                              moveStep(step.id, -1);
                            }}
                          >
                            Left
                          </Button>
                          <Button
                            variant="subtle"
                            size="xs"
                            onClick={(event) => {
                              event.stopPropagation();
                              moveStep(step.id, 1);
                            }}
                          >
                            Right
                          </Button>
                          <Button
                            color="red"
                            variant="subtle"
                            size="xs"
                            onClick={(event) => {
                              event.stopPropagation();
                              removeStep(step.id);
                            }}
                          >
                            Remove
                          </Button>
                        </Group>
                      </Group>
                    </Card>
                  );
                })}
              </Stack>
            </ScrollArea>
          </Stack>
        </Card>

        <Card withBorder h="100%" p="sm">
          <Stack h="100%" gap="sm">
            {!selectedStep || !selectedDescriptor ? (
              <Text c="dimmed" size="sm">Select a stage.</Text>
            ) : (
              <ScrollArea h="100%" type="auto">
                <Stack gap="sm">
                  <TextInput
                    label="Stage name"
                    value={selectedStep.name}
                    onChange={(event) => updateStep(selectedStep.id, { name: event.currentTarget.value })}
                  />
                  <Divider label="Stage type" />
                  <Group>
                    <Text size="sm">{selectedDescriptor.label}</Text>
                    <Code>{selectedDescriptor.step_type}</Code>
                    <Badge variant="light">{selectedDescriptor.category || 'stage'}</Badge>
                  </Group>
                  <Text size="sm" c="dimmed">
                    {selectedDescriptor.description || 'No description provided.'}
                  </Text>
                  {selectedStep.stepType === 'qa' ? (
                    <Button variant="light" onClick={() => void openDeployQA()}>
                      Configure DeployQA
                    </Button>
                  ) : null}
                  {selectedStep.stepType !== 'qa' ? <Divider label="Editable parameters" /> : null}
                  {selectedStep.stepType !== 'qa' ? selectedDescriptor.editable_fields.map((group) => (
                    <Stack key={group.key} gap="xs">
                      <Text fw={600} size="sm">
                        {group.label}
                      </Text>
                      {group.fields
                        .filter((field) => workflowBuilderFieldVisible(field, selectedStep.fields))
                        .map((field) => renderField(field, selectedStep.fields[field.key]))}
                    </Stack>
                  )) : null}

                  {selectedStageCapabilitySummary ? <Divider label="Execution plan summary" /> : null}
                  {selectedStageCapabilitySummary ? (
                    <Alert color="blue" variant="light">
                      {selectedStageCapabilitySummary.stage_types.length > 0
                        ? `This stage contributes to the workflow-level capability summary through ${selectedStageCapabilitySummary.stage_types.join(', ')}.`
                        : 'This stage contributes to the workflow-level capability summary.'}
                    </Alert>
                  ) : null}

                </Stack>
              </ScrollArea>
            )}
          </Stack>
        </Card>
      </Box>
      <Modal
        opened={capabilityConfigKey !== null}
        onClose={() => {
          setCapabilityConfigKey(null);
          setCapabilityConfigError(null);
        }}
        title={capabilityConfigKey
          ? `${capabilityDisplayLabel(capabilityConfigKey)} capability`
          : 'Capability'}
        size="lg"
        centered
      >
        <Stack gap="md">
          <Text size="sm" c="dimmed">
            Configure the workflow-level state for this capability. Stage definitions continue to determine which stages invoke or consume it.
          </Text>

          <JsonInput
            label="Capability configuration"
            value={capabilityConfigDraft}
            onChange={setCapabilityConfigDraft}
            validationError="Configuration must be valid JSON"
            formatOnBlur
            autosize
            minRows={12}
          />

          {capabilityConfigError ? (
            <Alert color="red">{capabilityConfigError}</Alert>
          ) : null}

          <Group justify="flex-end">
            <Button
              size="xs"
              variant="default"
              onClick={() => {
                setCapabilityConfigKey(null);
                setCapabilityConfigError(null);
              }}
            >
              Cancel
            </Button>
            <Button
              size="xs"
              onClick={() => {
                if (!capabilityConfigKey) {
                  return;
                }

                try {
                  const parsed = JSON.parse(capabilityConfigDraft) as unknown;
                  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
                    throw new Error('Capability configuration must be a JSON object.');
                  }

                  const nextCapabilityConfig = structuredClone(parsed) as Record<string, unknown>;
                  const configKey = capabilityConfigKey;

                  updateCapabilityConfig(configKey, nextCapabilityConfig);
                  setCapabilityConfigKey(null);
                  setCapabilityConfigError(null);
                } catch (error) {
                  setCapabilityConfigError(
                    error instanceof Error ? error.message : String(error)
                  );
                }
              }}
            >
              Save
            </Button>
          </Group>
        </Stack>
      </Modal>

      <Modal
        opened={automationOpen}
        onClose={() => {
          if (!automationSaving) {
            setAutomationOpen(false);
            setAutomationStatus(null);
          }
        }}
        title={
          <Group gap="sm" wrap="nowrap">
            <Text fw={600}>Automation</Text>
            <Text size="sm" c="dimmed" fw={400}>
              Choose which behaviors run automatically and when they should activate.
            </Text>
          </Group>
        }
        size="min(1120px, 94vw)"
        centered
        styles={{
          content: { maxHeight: '92dvh' },
          body: { overflow: 'hidden' },
        }}
      >
        <Automation
          value={automationProfile}
          controls={automationControls}
          invokedCapabilities={invokedAutomationCapabilities}
          busy={automationSaving}
          status={automationStatus}
          onCancel={() => {
            if (!automationSaving) {
              setAutomationOpen(false);
              setAutomationStatus(null);
            }
          }}
          onSave={async (profile) => {
            setAutomationSaving(true);
            setAutomationStatus(null);

            try {
              const nextGlobals: WorkflowGlobalConfig = {
                ...structuredClone(editableGlobals),
                automation: structuredClone(profile),
              };

              const saved = await updateBuilderGlobals(
                nextGlobals,
                'Automation configuration changed'
              );

              if (!saved) {
                setAutomationStatus('Unable to compile automation configuration.');
                return;
              }

              setAutomationStatus('Saved');
              setAutomationOpen(false);
            } catch (error) {
              setAutomationStatus(error instanceof Error ? error.message : String(error));
            } finally {
              setAutomationSaving(false);
            }
          }}
        />
      </Modal>

      <SharedDependencies
        opened={sharedDependenciesOpen}
        value={sharedDependencies}
        onClose={() => setSharedDependenciesOpen(false)}
        onChange={(next) => {
          updateCapabilityConfig(
            'shared_dependencies',
            structuredClone(next) as unknown as Record<string, unknown>,
            'Shared dependency configuration changed'
          );
        }}
      />

      {selectedStep?.stepType === 'qa' && deployQAValues ? (
        <DeployQA
          opened={deployQAOpen}
          onClose={() => {
            void closeDeployQA();
          }}
          values={deployQAValues}
          onChange={(key, value) => {
            const nextValues: DeployQAValues = {
              ...deployQAValues,
              [key]: value,
            };

            updateCapabilityConfig(
              'qa_environment',
              deployQACapabilityFromValues(
                nextValues,
                editableGlobals.capabilities?.qa_environment
              ),
              'DeployQA capability configuration changed'
            );
          }}
        />
      ) : null}
    </Box>
  );
}
