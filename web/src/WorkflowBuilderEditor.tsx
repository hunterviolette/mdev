import { useEffect, useMemo, useState } from 'react';
import {
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Code,
  Divider,
  Group,
  Loader,
  Menu,
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
  type WorkflowStageDescriptor,
  type WorkflowStageField,
  type WorkflowTemplateDefinition,
} from './api';
import { buildBuilderDocument, builderStepFromDescriptor, builderStepsFromDefinition, descriptorMap, type BuilderStep } from './workflow_builder';

type WorkflowBuilderEditorProps = {
  initialDefinition?: WorkflowTemplateDefinition | null;
  onCompiledDefinitionChange: (definition: WorkflowTemplateDefinition) => void;
  onError?: (message: string | null) => void;
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


export function WorkflowBuilderEditor({ initialDefinition, onCompiledDefinitionChange, onError }: WorkflowBuilderEditorProps) {
  const [catalog, setCatalog] = useState<Record<string, WorkflowStageDescriptor>>({});
  const [stageDescriptors, setStageDescriptors] = useState<WorkflowStageDescriptor[]>([]);
  const [steps, setSteps] = useState<BuilderStep[]>([]);
  const [selectedStepId, setSelectedStepId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [compileState, setCompileState] = useState<CompileState>('idle');
  const [compileMessage, setCompileMessage] = useState('');
  const [compileRevision, setCompileRevision] = useState(0);

  const selectedStep = useMemo(
    () => steps.find((step) => step.id === selectedStepId) ?? null,
    [steps, selectedStepId]
  );
  const selectedDescriptor = useMemo(
    () => (selectedStep ? catalog[selectedStep.stepType] ?? null : null),
    [catalog, selectedStep]
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

        const hydratedSteps = builderStepsFromDefinition(initialDefinition, loadedCatalog);
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
          onError?.(error instanceof Error ? error.message : String(error));
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
  }, [initialDefinition, onError]);

  function markDirty(message = 'Unsaved visual changes') {
    setCompileState('dirty');
    setCompileMessage(message);
    setCompileRevision((prev) => prev + 1);
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


  async function compileDocument() {
    try {
      setCompileState('compiling');
      setCompileMessage('Compiling backend-defined workflow');
      const result = await compileWorkflowBuilderDocument(
        buildBuilderDocument(steps)
      );
      if (!result.ok) {
        const message = result.errors.length > 0 ? result.errors.join('\n') : 'Workflow compilation failed.';
        setCompileState('error');
        setCompileMessage(message);
        onError?.(message);
        return;
      }
      onCompiledDefinitionChange(result.definition);
      setCompileState('compiled');
      setCompileMessage(result.warnings.length > 0 ? result.warnings.join('\n') : 'Compiled successfully');
      onError?.(null);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setCompileState('error');
      setCompileMessage(message);
      onError?.(message);
    }
  }

  useEffect(() => {
    if (steps.length === 0) {
      return;
    }
    void compileDocument();
  }, [compileRevision]);

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

            {compileMessage ? <Alert color={compileState === 'error' ? 'red' : 'blue'}>{compileMessage}</Alert> : null}

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
                  <Divider label="Editable parameters" />
                  {selectedDescriptor.editable_fields.map((group) => (
                    <Stack key={group.key} gap="xs">
                      <Text fw={600} size="sm">
                        {group.label}
                      </Text>
                      {group.fields.map((field) => renderField(field, selectedStep.fields[field.key]))}
                    </Stack>
                  ))}

                </Stack>
              </ScrollArea>
            )}
          </Stack>
        </Card>
      </Box>
    </Box>
  );
}
