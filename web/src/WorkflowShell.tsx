import { memo, useEffect, useMemo, useRef, useState } from 'react';
import {
  ActionIcon,
  Alert,
  AppShell,
  Badge,
  Box,
  Button,
  Card,
  Code,
  Divider,
  Grid,
  Group,
  JsonInput,
  Loader,
  Modal,
  ScrollArea,
  SegmentedControl,
  Select,
  SimpleGrid,
  Stack,
  Switch,
  Table,
  Tabs,
  Text,
  TextInput,
  Textarea,
  Title
} from '@mantine/core';
import { IconPlayerPause, IconPlayerPlay, IconRefresh, IconTrash } from '@tabler/icons-react';
import {
  createRun,
  createTemplate,
  deleteRun,
  getEventChainSummary,
  getChangesetSchema,
  getRun,
  getStageExecutionChain,
  getWorkflowBuilderContract,
  listRepoTree,
  listRunEvents,
  listRuns,
  listTemplates,
  nextWorkflowStep,
  openEventStream,
  patchWorkflowGlobalState,
  patchWorkflowStageState,
  pauseWorkflowRun,
  previousWorkflowStep,
  resumeWorkflowRun,
  runCurrentWorkflowStep,
  selectWorkflowStep,
  startWorkflowRun,
  type AutomationMode,
  type BrowserProbeResult,
  type EventChainSummaryItem,
  type EventChainSummaryResponse,
  type InferenceTransport,
  type RepoTreeResponse,
  type StageExecutionChain,
  type StageExecutionEvent,
  type WorkflowBuilderContract,
  type WorkflowBuilderFieldContract,
  type WorkflowBuilderStageContract,
  type WorkflowEvent,
  type WorkflowRun,
  type WorkflowRunStatus,
  type WorkflowStepDefinition,
  type WorkflowTemplate,
  type WorkflowTemplateDefinition,
  type WorkflowTransition
} from './api';
import { GlobalCapabilitiesPanel } from './GlobalCapabilitiesPanel';
import { RepoTree, type RepoTreeEntry } from './RepoTree';

type TransitionEditorValue = {
  successTarget: string;
  errorTarget: string;
  pausedTarget: string;
};

type BuilderStep = {
  id: string;
  name: string;
  stepType: string;
  automationMode: AutomationMode;
  autoAdvanceOnSuccess: boolean;
  fields: Record<string, boolean | number | string>;
  transitions: TransitionEditorValue;
};

type BuilderMode = 'builder' | 'json';
type ShellView = 'builder' | 'monitor';
type MonitorView = 'workflow_list' | 'workflow_detail';
type OperatorMode = 'auto' | 'manual';

type EventTone = { color: string; label: string };

type InferenceConnectionStatus = { color: string; label: string };

type EventStreamStatus = { color: string; label: string };


type LiveCapabilityTrail = {
  key: string;
  capabilityId: string;
  name: string;
  statusColor: string;
  statusLabel: string;
  message: string;
  startedAtText: string;
  startedAtRaw: string | null;
  durationText: string;
  durationMs: number | null;
  latestCreatedAt: string;
  isActive: boolean;
  isNew: boolean;
  eventCount: number;
  latestLevel: string;
  latestKind: string;
  latestPayload: unknown;
  inputPayload: unknown;
  outputPayload: unknown;
};

type LiveStageTrail = {
  key: string;
  stepId: string;
  label: string;
  stageExecutionId: string;
  latestCreatedAt: string;
  durationMs: number | null;
  isActive: boolean;
  isCurrent: boolean;
  capabilities: LiveCapabilityTrail[];
};

type LiveExecutionChainState = {
  loading: boolean;
  error: string | null;
  chain: StageExecutionChain | null;
  latestCreatedAt: string | null;
};

function collectLoadedFilePaths(parentPath: string, childrenByParent: Record<string, RepoTreeEntry[]>): string[] {
  const children = childrenByParent[parentPath] ?? [];
  const out: string[] = [];
  for (const child of children) {
    if (child.kind === 'file') {
      out.push(child.path);
    } else {
      out.push(...collectLoadedFilePaths(child.path, childrenByParent));
    }
  }
  return out;
}

function getLiveExecutionDefaultExpanded(trail: LiveStageTrail): boolean {
  return trail.isActive || trail.isCurrent;
}

function extractInferenceTextFromPayload(payload: unknown): string {
  const objectPayload = (payload ?? {}) as Record<string, unknown>;
  const result = objectPayload.result as Record<string, unknown> | undefined;
  const nestedResult = result?.result as Record<string, unknown> | undefined;
  const directText = typeof nestedResult?.text === 'string' ? nestedResult.text : undefined;
  if (directText && directText.trim()) return directText;

  const capabilityResults = Array.isArray(objectPayload.capability_results)
    ? (objectPayload.capability_results as Array<Record<string, unknown>>)
    : [];

  for (let i = capabilityResults.length - 1; i >= 0; i -= 1) {
    const entry = capabilityResults[i];
    const entryResult = entry?.result as Record<string, unknown> | undefined;
    const entryNestedResult = entryResult?.result as Record<string, unknown> | undefined;
    const text = typeof entryNestedResult?.text === 'string' ? entryNestedResult.text : undefined;
    if (text && text.trim()) return text;
  }

  return '';
}

function extractCompileResultsFromPayload(payload: unknown): Array<Record<string, unknown>> {
  const objectPayload = (payload ?? {}) as Record<string, unknown>;
  const directResult = objectPayload.result as Record<string, unknown> | undefined;
  const directRows = Array.isArray(directResult?.results)
    ? (directResult?.results as Array<Record<string, unknown>>)
    : null;
  if (directRows && directRows.length > 0) return directRows;

  const capabilityResults = Array.isArray(objectPayload.capability_results)
    ? (objectPayload.capability_results as Array<Record<string, unknown>>)
    : [];

  for (let i = capabilityResults.length - 1; i >= 0; i -= 1) {
    const entry = capabilityResults[i];
    const entryKey = typeof entry?.key === 'string' ? entry.key : '';
    const entryResult = entry?.result as Record<string, unknown> | undefined;
    const rows = Array.isArray(entryResult?.results)
      ? (entryResult?.results as Array<Record<string, unknown>>)
      : [];
    if (entryKey === 'compile_commands' && rows.length > 0) {
      return rows;
    }
  }

  return [];
}

function formatCompileStageStream(commandResults: Array<Record<string, unknown>>): string {
  const parts: string[] = ['### COMPILE RESULTS'];

  for (const row of commandResults) {
    const label = typeof row.label === 'string' && row.label.trim()
      ? row.label.trim()
      : (typeof row.command === 'string' ? row.command.trim() : 'compile command');
    const command = typeof row.command === 'string' ? row.command : '';
    const status = typeof row.status === 'number' ? row.status : Number(row.status ?? -1);
    const stdout = typeof row.stdout === 'string' ? row.stdout.trim() : '';
    const stderr = typeof row.stderr === 'string' ? row.stderr.trim() : '';

    parts.push(`#### ${label}`);
    if (command) parts.push(`COMMAND: ${command}`);
    parts.push(`STATUS: ${Number.isFinite(status) ? status : -1}`);
    parts.push(`STDOUT:\n${stdout || '(empty)'}`);
    parts.push(`STDERR:\n${stderr || '(empty)'}`);
  }

  return parts.join('\n\n');
}

function builderStepFromContract(stage: WorkflowBuilderStageContract): BuilderStep {
  return {
    id: stage.step_type,
    name: stage.label,
    stepType: stage.step_type,
    automationMode: stage.automation_mode_default,
    autoAdvanceOnSuccess: stage.automation_mode_default === 'automatic',
    fields: Object.fromEntries(stage.fields.map((field) => [field.key, field.default])),
    transitions: {
      successTarget: stage.transition_defaults.on_success,
      errorTarget: stage.transition_defaults.on_error,
      pausedTarget: stage.transition_defaults.on_paused
    }
  };
}

function readBool(step: BuilderStep, key: string, fallback = false) {
  const value = step.fields[key];
  return typeof value === 'boolean' ? value : fallback;
}

function readNumber(step: BuilderStep, key: string, fallback = 0) {
  const value = step.fields[key];
  return typeof value === 'number' ? value : fallback;
}

function readString(step: BuilderStep, key: string, fallback = '') {
  const value = step.fields[key];
  return typeof value === 'string' ? value : fallback;
}

function buildTransitions(step: BuilderStep): WorkflowTransition[] {
  const transitions: WorkflowTransition[] = [];
  if (step.transitions.successTarget) transitions.push({ when: { type: 'success' }, target_step_id: step.transitions.successTarget });
  if (step.transitions.errorTarget) transitions.push({ when: { type: 'error' }, target_step_id: step.transitions.errorTarget });
  if (step.transitions.pausedTarget) transitions.push({ when: { type: 'paused' }, target_step_id: step.transitions.pausedTarget });
  return transitions;
}

function buildBranchDisposition(targetStepId: string, fallback: 'success' | 'error' | 'paused') {
  if (!targetStepId) {
    return fallback === 'paused' ? 'paused' : 'success';
  }
  return 'move_to_step';
}

function buildBranchConfig(targetStepId: string, fallback: 'success' | 'error' | 'paused') {
  const disposition = buildBranchDisposition(targetStepId, fallback);
  return targetStepId
    ? { disposition, target_step_id: targetStepId }
    : { disposition };
}

function buildStepDefinition(step: BuilderStep): WorkflowStepDefinition {
  const compileCommands = readString(step, 'execution.compile_checks.commands_text', '')
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)
    .map((command) => ({ command, label: command }));

  const executionLogic: Record<string, unknown> = step.id === 'code'
    ? {
        kind: 'code_stage_policy',
        automation: {
          inject_context: readBool(step, 'automation.inject_context', true),
          inject_changeset_schema: readBool(step, 'automation.inject_changeset_schema', true),
          auto_apply_changeset: readBool(step, 'automation.auto_apply_changeset', true),
          max_consecutive_apply_failures: readNumber(step, 'automation.max_consecutive_apply_failures', 1)
        },
        on_success: buildBranchConfig(step.transitions.successTarget, 'success'),
        on_error: buildBranchConfig(step.transitions.errorTarget, 'error'),
        on_paused: buildBranchConfig(step.transitions.pausedTarget, 'paused')
      }
    : step.id === 'compile'
      ? {
          kind: 'compile_stage_policy',
          automation: {
            run_compile_checks: true
          },
          on_success: buildBranchConfig(step.transitions.successTarget, 'success'),
          on_error: buildBranchConfig(step.transitions.errorTarget, 'error'),
          on_paused: buildBranchConfig(step.transitions.pausedTarget, 'paused')
        }
      : step.id === 'review'
        ? {
            kind: 'review_stage_policy',
            require_manual_approval: readBool(step, 'execution_logic.require_manual_approval', true),
            on_success: buildBranchConfig(step.transitions.successTarget, 'success'),
            on_error: buildBranchConfig(step.transitions.errorTarget, 'error'),
            on_paused: buildBranchConfig(step.transitions.pausedTarget, 'paused')
          }
        : {
            kind: 'design_stage_policy',
            automation: {
              inject_context: readBool(step, 'automation.inject_context', true)
            },
            on_success: buildBranchConfig(step.transitions.successTarget, 'success'),
            on_error: buildBranchConfig(step.transitions.errorTarget, 'error'),
            on_paused: buildBranchConfig(step.transitions.pausedTarget, 'paused')
          };

  const executionPlan = step.id === 'compile'
    ? [
        {
          kind: 'capability' as const,
          key: 'compile_commands',
          enabled: true,
          config: {},
          input_mapping: {},
          output_mapping: {},
          run_after: [],
          condition: null
        }
      ]
    : step.id === 'design' || step.id === 'code'
      ? [
          {
            kind: 'capability' as const,
            key: 'context_export',
            enabled: true,
            config: {},
            input_mapping: {},
            output_mapping: {},
            run_after: [],
            condition: null
          },
          {
            kind: 'capability' as const,
            key: 'inference',
            enabled: true,
            config: {},
            input_mapping: {},
            output_mapping: {},
            run_after: ['context_export'],
            condition: null
          }
        ]
      : [
          {
            kind: 'capability' as const,
            key: 'inference',
            enabled: true,
            config: {},
            input_mapping: {},
            output_mapping: {},
            run_after: [],
            condition: null
          }
        ];

  return {
    id: step.id,
    name: step.name,
    step_type: step.stepType,
    automation_mode: step.automationMode,
    execution: {
      changeset_apply: step.id === 'code'
        ? {
            enabled: readBool(step, 'automation.auto_apply_changeset', true),
            max_consecutive_failures: readNumber(step, 'automation.max_consecutive_apply_failures', 1)
          }
        : {},
      compile_checks: step.id === 'compile' ? { commands: compileCommands } : {}
    },
    prompt: {
      include_repo_context: readBool(step, 'automation.inject_context', false),
      include_changeset_schema: readBool(step, 'automation.inject_changeset_schema', false),
      include_user_context: true
    },
    config: {
      pause_policy: {
        pause_on_enter: readBool(step, 'config.pause_policy.pause_on_enter', false)
      }
    },
    capabilities: [],
    execution_logic: executionLogic,
    execution_plan: executionPlan,
    advancement: {
      mode: step.automationMode,
      auto_run_on_enter: step.automationMode === 'automatic',
      auto_advance_on_success: step.autoAdvanceOnSuccess,
      auto_advance_on_error: false,
      auto_advance_on_paused: false
    },
    transitions: buildTransitions(step)
  };
}

function buildTemplateDefinition(steps: BuilderStep[]): WorkflowTemplateDefinition {
  return {
    version: 1,
    globals: {
      resources: {
        repo: {
          repo_ref: '',
          git_ref: 'WORKTREE'
        }
      },
      capabilities: {
        inference: {},
        context_export: {
          save_path: '/tmp/repo_context.txt'
        },
        changeset_schema: {},
        'gateway_model/changeset': {},
        compile_commands: {}
      }
    },
    steps: steps.map(buildStepDefinition)
  };
}

function formatTimestamp(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
}

function statusColor(status: WorkflowRunStatus) {
  switch (status) {
    case 'success': return 'green';
    case 'error': return 'red';
    case 'running': return 'blue';
    case 'queued': return 'yellow';
    case 'waiting': return 'grape';
    case 'paused': return 'orange';
    case 'cancelled': return 'gray';
    default: return 'dark';
  }
}

const InferenceConnectionCard = memo(function InferenceConnectionCard(props: {
  inferenceConnectionStatus: InferenceConnectionStatus;
  inferenceReady: boolean;
  inferenceSummaryText: string;
  inferenceTransport: InferenceTransport;
  inferenceModel: string;
  browserTargetUrl: string;
  browserCdpUrl: string;
  inferenceBusy: boolean;
  inferenceStatus: string | null;
  inferenceConfigOpen: boolean;
  onOpenConfig: () => void;
  onCloseConfig: () => void;
  onTransportChange: (value: InferenceTransport) => void;
  onModelChange: (value: string) => void;
  onBrowserTargetUrlChange: (value: string) => void;
  onBrowserCdpUrlChange: (value: string) => void;
  onSaveConfig: () => void;
}) {
  const {
    inferenceConnectionStatus,
    inferenceReady,
    inferenceTransport,
    inferenceModel,
    browserTargetUrl,
    browserCdpUrl,
    inferenceBusy,
    inferenceStatus,
    inferenceConfigOpen,
    onOpenConfig,
    onCloseConfig,
    onTransportChange,
    onModelChange,
    onBrowserTargetUrlChange,
    onBrowserCdpUrlChange,
    onSaveConfig
  } = props;

  const showInlineConfig = !inferenceReady;

  return (
    <>
      <Stack gap="md">
        <Group justify="space-between" align="center" wrap="nowrap">
          <Group gap="xs" wrap="nowrap">
            <Text size="sm" fw={600}>Inference status</Text>
            <Badge color={inferenceConnectionStatus.color} variant="light">
              {inferenceConnectionStatus.label}
            </Badge>
          </Group>
          {!showInlineConfig ? <Button size="xs" variant="subtle" onClick={onOpenConfig}>Connector</Button> : null}
        </Group>

        {showInlineConfig ? (
          <Stack gap="md">
            <SimpleGrid cols={{ base: 1, md: 2 }}>
              <Select
                label="Mode"
                value={inferenceTransport}
                onChange={(value) => onTransportChange((value as InferenceTransport) ?? 'api')}
                data={[
                  { value: 'api', label: 'API' },
                  { value: 'browser', label: 'Browser' }
                ]}
                allowDeselect={false}
              />
              <TextInput
                label="Model"
                value={inferenceModel}
                onChange={(e) => onModelChange(e.currentTarget.value)}
                disabled={inferenceTransport !== 'api'}
              />
            </SimpleGrid>

            {inferenceTransport === 'browser' ? (
              <Stack gap="md">
                <SimpleGrid cols={{ base: 1, md: 2 }}>
                  <TextInput
                    label="Browser URL"
                    value={browserTargetUrl}
                    onChange={(e) => onBrowserTargetUrlChange(e.currentTarget.value)}
                    placeholder="https://website.com/"
                  />
                  <TextInput
                    label="CDP URL"
                    value={browserCdpUrl}
                    onChange={(e) => onBrowserCdpUrlChange(e.currentTarget.value)}
                    placeholder="http://127.0.0.1:9222"
                  />
                </SimpleGrid>
                <Alert color="blue">Browser lifecycle is managed automatically by the backend when this stage runs.</Alert>
                <Group>
                  <Button variant="default" onClick={onSaveConfig} loading={inferenceBusy}>Save config</Button>
                </Group>
              </Stack>
            ) : (
              <Group>
                <Button variant="default" onClick={onSaveConfig} loading={inferenceBusy}>Save config</Button>
              </Group>
            )}

            {inferenceStatus ? <Alert color="blue">{inferenceStatus}</Alert> : null}
          </Stack>
        ) : null}
      </Stack>

      <Modal opened={inferenceConfigOpen} onClose={onCloseConfig} title="Inference connector" size="70%" centered>
        <Stack gap="md">
          <SimpleGrid cols={{ base: 1, md: 2 }}>
            <Select
              label="Mode"
              value={inferenceTransport}
              onChange={(value) => onTransportChange((value as InferenceTransport) ?? 'api')}
              data={[
                { value: 'api', label: 'API' },
                { value: 'browser', label: 'Browser' }
              ]}
              allowDeselect={false}
            />
            <TextInput
              label="Model"
              value={inferenceModel}
              onChange={(e) => onModelChange(e.currentTarget.value)}
              disabled={inferenceTransport !== 'api'}
            />
          </SimpleGrid>

          {inferenceTransport === 'browser' ? (
            <Stack gap="md">
              <SimpleGrid cols={{ base: 1, md: 2 }}>
                <TextInput
                  label="Browser URL"
                  value={browserTargetUrl}
                  onChange={(e) => onBrowserTargetUrlChange(e.currentTarget.value)}
                  placeholder="https://website.com/"
                />
                <TextInput
                  label="CDP URL"
                  value={browserCdpUrl}
                  onChange={(e) => onBrowserCdpUrlChange(e.currentTarget.value)}
                  placeholder="http://127.0.0.1:9222"
                />
              </SimpleGrid>
              <Alert color="blue">Browser lifecycle is managed automatically by the backend when this stage runs.</Alert>
              <Group>
                <Button variant="default" onClick={onSaveConfig} loading={inferenceBusy}>Save config</Button>
              </Group>
            </Stack>
          ) : (
            <Group>
              <Button variant="default" onClick={onSaveConfig} loading={inferenceBusy}>Save config</Button>
            </Group>
          )}

          {inferenceStatus ? <Alert color="blue">{inferenceStatus}</Alert> : null}
        </Stack>
      </Modal>
    </>
  );
});

const DesignStageInputsPanel = memo(function DesignStageInputsPanel(props: {
  stageUserInput: string;
  stageIncludeRepoContext: boolean;
  repoFragmentSummary: string | null;
  onStageUserInputChange: (value: string) => void;
  onStageIncludeRepoContextChange: (checked: boolean) => void;
  onOpenRepoConfig: () => void;
}) {
  const {
    stageUserInput,
    stageIncludeRepoContext,
    repoFragmentSummary,
    onStageUserInputChange,
    onStageIncludeRepoContextChange,
    onOpenRepoConfig
  } = props;

  return (
    <Stack>
      <Title order={6}>Stage inputs</Title>
      <Textarea label="User input" value={stageUserInput} onChange={(e) => onStageUserInputChange(e.currentTarget.value)} minRows={3} autosize />
      <Group>
        <Switch label="Include repo fragment" checked={stageIncludeRepoContext} onChange={(e) => onStageIncludeRepoContextChange(e.currentTarget.checked)} />
        <Button size="xs" variant="light" onClick={onOpenRepoConfig}>Configure repo fragment</Button>
        {stageIncludeRepoContext && repoFragmentSummary ? <Badge variant="light">{repoFragmentSummary}</Badge> : null}
      </Group>
    </Stack>
  );
});

const CodeStageInputsPanel = memo(function CodeStageInputsPanel(props: {
  stageUserInput: string;
  stageIncludeRepoContext: boolean;
  stageIncludeChangesetSchema: boolean;
  stageAutoApplyChangeset: boolean;
  stageIncludeApplyError: boolean;
  stageApplyError: string;
  stageIncludeCompileError: boolean;
  stageCompileError: string;
  repoFragmentSummary: string | null;
  onStageUserInputChange: (value: string) => void;
  onStageIncludeRepoContextChange: (checked: boolean) => void;
  onStageIncludeChangesetSchemaChange: (checked: boolean) => void;
  onStageAutoApplyChangesetChange: (checked: boolean) => void;
  onStageIncludeApplyErrorChange: (checked: boolean) => void;
  onStageIncludeCompileErrorChange: (checked: boolean) => void;
  onOpenRepoConfig: () => void;
  onOpenSchemaConfig: () => void;
  onOpenApplyErrorConfig: () => void;
  onOpenCompileErrorConfig: () => void;
}) {
  const {
    stageUserInput,
    stageIncludeRepoContext,
    stageIncludeChangesetSchema,
    stageAutoApplyChangeset,
    stageIncludeApplyError,
    stageApplyError,
    stageIncludeCompileError,
    stageCompileError,
    repoFragmentSummary,
    onStageUserInputChange,
    onStageIncludeRepoContextChange,
    onStageIncludeChangesetSchemaChange,
    onStageAutoApplyChangesetChange,
    onStageIncludeApplyErrorChange,
    onStageIncludeCompileErrorChange,
    onOpenRepoConfig,
    onOpenSchemaConfig,
    onOpenApplyErrorConfig,
    onOpenCompileErrorConfig
  } = props;

  return (
    <Stack>
      <Title order={6}>Stage inputs</Title>
      <Textarea label="User input" value={stageUserInput} onChange={(e) => onStageUserInputChange(e.currentTarget.value)} minRows={3} autosize />
      <Group>
        <Switch label="Include repo fragment" checked={stageIncludeRepoContext} onChange={(e) => onStageIncludeRepoContextChange(e.currentTarget.checked)} />
        <Button size="xs" variant="light" onClick={onOpenRepoConfig}>Configure repo fragment</Button>
        {stageIncludeRepoContext && repoFragmentSummary ? <Badge variant="light">{repoFragmentSummary}</Badge> : null}
      </Group>
      <Group>
        <Switch label="Include changeset schema fragment" checked={stageIncludeChangesetSchema} onChange={(e) => onStageIncludeChangesetSchemaChange(e.currentTarget.checked)} />
        <Button size="xs" variant="light" onClick={onOpenSchemaConfig}>Configure schema</Button>
      </Group>
      <Group>
        <Switch label="Auto apply changeset" checked={stageAutoApplyChangeset} onChange={(e) => onStageAutoApplyChangesetChange(e.currentTarget.checked)} />
      </Group>
      {stageApplyError.trim() ? (
        <Group>
          <Switch label="Include apply error fragment" checked={stageIncludeApplyError} onChange={(e) => onStageIncludeApplyErrorChange(e.currentTarget.checked)} />
          <Badge variant="light" color="orange">Apply error available</Badge>
          <Button size="xs" variant="light" onClick={onOpenApplyErrorConfig}>View apply error</Button>
        </Group>
      ) : null}
      {stageCompileError.trim() ? (
        <Group>
          <Switch label="Include compile error fragment" checked={stageIncludeCompileError} onChange={(e) => onStageIncludeCompileErrorChange(e.currentTarget.checked)} />
          <Badge variant="light" color="yellow">Compile error available</Badge>
          <Button size="xs" variant="light" onClick={onOpenCompileErrorConfig}>View compile error</Button>
        </Group>
      ) : null}
    </Stack>
  );
});

const CompileStageInputsPanel = memo(function CompileStageInputsPanel(props: {
  stageCompileCommandsText: string;
  onStageCompileCommandsTextChange: (value: string) => void;
}) {
  const {
    stageCompileCommandsText,
    onStageCompileCommandsTextChange
  } = props;

  return (
    <Stack>
      <Title order={6}>Stage inputs</Title>
      <Textarea
        label="Compile commands"
        description="One command per line. The compile stage should only patch backend compile checks."
        value={stageCompileCommandsText}
        onChange={(e) => onStageCompileCommandsTextChange(e.currentTarget.value)}
        minRows={6}
        autosize
        placeholder={"cargo check\nnpm run build"}
      />
    </Stack>
  );
});

const ReviewStageInputsPanel = memo(function ReviewStageInputsPanel(props: {
  stageReviewNotes: string;
  onStageReviewNotesChange: (value: string) => void;
}) {
  const {
    stageReviewNotes,
    onStageReviewNotesChange
  } = props;

  return (
    <Stack>
      <Title order={6}>Stage inputs</Title>
      <Textarea label="Review notes" value={stageReviewNotes} onChange={(e) => onStageReviewNotesChange(e.currentTarget.value)} minRows={4} autosize />
    </Stack>
  );
});

const StageStreamPanel = memo(function StageStreamPanel(props: {
  renderStageStreamPanel: (emptyText: string) => JSX.Element;
}) {
  return (
    <Box h="100%">
      {props.renderStageStreamPanel('No stage stream yet.')}
    </Box>
  );
});

const workflowLiveBarKeyframes = `
@keyframes workflow-live-bar {
  0% { background-position: 0 0; }
  100% { background-position: 34px 0; }
}
`;

export function WorkflowShell() {
  const [view, setView] = useState<ShellView>('monitor');
  const [builderMode, setBuilderMode] = useState<BuilderMode>('builder');
  const [monitorView, setMonitorView] = useState<MonitorView>('workflow_list');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [templates, setTemplates] = useState<WorkflowTemplate[]>([]);
  const [runs, setRuns] = useState<WorkflowRun[]>([]);
  const [events, setEvents] = useState<WorkflowEvent[]>([]);
  const [allWorkflowEvents, setAllWorkflowEvents] = useState<Record<string, WorkflowEvent[]>>({});
  const [recentEventIds, setRecentEventIds] = useState<Set<string>>(new Set());
  const [eventStreamConnected, setEventStreamConnected] = useState(false);
  const [eventStreamStatusText, setEventStreamStatusText] = useState('Disconnected');
  const [selectedTemplateId, setSelectedTemplateId] = useState<string | null>(null);
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);

  const selectedRunIdRef = useRef<string | null>(null);
  const allWorkflowEventsRef = useRef<Record<string, WorkflowEvent[]>>({});
  const runEventStreamsRef = useRef<Record<string, EventSource>>({});
  const runRefreshTimersRef = useRef<Record<string, number>>({});

  const [workflowName, setWorkflowName] = useState('Default workflow');
  const [workflowDescription, setWorkflowDescription] = useState('Design, code, and review workflow');
  const [runTitle, setRunTitle] = useState('New workflow run');
  const [repoRef, setRepoRef] = useState('');
  const [builderContract, setBuilderContract] = useState<WorkflowBuilderContract | null>(null);
  const [builderSteps, setBuilderSteps] = useState<BuilderStep[]>([]);
  const [jsonDraft, setJsonDraft] = useState('');
  const [createRunAfterSave, setCreateRunAfterSave] = useState(true);
  const [templateModalOpen, setTemplateModalOpen] = useState(false);
  const [globalCapabilitiesOpen, setGlobalCapabilitiesOpen] = useState(false);

  useEffect(() => {
    void getWorkflowBuilderContract()
      .then((contract) => {
        setBuilderContract(contract);
        setBuilderSteps(contract.stages.map(builderStepFromContract));
      })
      .catch((err) => {
        setError(err instanceof Error ? err.message : String(err));
      });
  }, []);

  const [operatorMode, setOperatorMode] = useState<OperatorMode>('auto');
  const [selectedStepId, setSelectedStepId] = useState<string | null>(null);
  const [manualCapabilityStatus, setManualCapabilityStatus] = useState<string | null>(null);
  const [manualCapabilityBusy, setManualCapabilityBusy] = useState(false);
  const [manualCapabilityResponse, setManualCapabilityResponse] = useState('');

  const [inferenceTransport, setInferenceTransport] = useState<InferenceTransport>('api');
  const [inferenceModel, setInferenceModel] = useState('gpt-5');
  const [browserTargetUrl, setBrowserTargetUrl] = useState('https://website.com/');
  const [browserCdpUrl, setBrowserCdpUrl] = useState('http://127.0.0.1:9222');
  const [browserSessionId, setBrowserSessionId] = useState('');
  const [browserProbe, setBrowserProbe] = useState<BrowserProbeResult | null>(null);
  const [inferenceBusy, setInferenceBusy] = useState(false);
  const [inferenceStatus, setInferenceStatus] = useState<string | null>(null);
  const [inferencePollBusy, setInferencePollBusy] = useState(false);
  const [inferenceConnected, setInferenceConnected] = useState(false);

  const [stageUserInput, setStageUserInput] = useState('');
  const [stageIncludeRepoContext, setStageIncludeRepoContext] = useState(false);
  const [stageRepoContextGitRef, setStageRepoContextGitRef] = useState('WORKTREE');
  const [stageRepoContextIncludeFilesText, setStageRepoContextIncludeFilesText] = useState('');
  const [stageRepoContextExcludeRegexText, setStageRepoContextExcludeRegexText] = useState('');
  const [stageRepoContextSavePath, setStageRepoContextSavePath] = useState('/tmp/repo_context.txt');
  const [stageRepoContextSkipBinary, setStageRepoContextSkipBinary] = useState(true);
  const [stageRepoContextSkipGitignore, setStageRepoContextSkipGitignore] = useState(true);
  const [stageRepoContextIncludeStagedDiff, setStageRepoContextIncludeStagedDiff] = useState(false);
  const [stageRepoContextIncludeUnstagedDiff, setStageRepoContextIncludeUnstagedDiff] = useState(false);
  const [stageIncludeChangesetSchema, setStageIncludeChangesetSchema] = useState(true);
  const [stageChangesetSchemaText, setStageChangesetSchemaText] = useState('');
  const [stageApplyError, setStageApplyError] = useState('');
  const [stageIncludeApplyError, setStageIncludeApplyError] = useState(true);
  const [stageReviewNotes, setStageReviewNotes] = useState('');
  const [stageCompileError, setStageCompileError] = useState('');
  const [stageIncludeCompileError, setStageIncludeCompileError] = useState(true);
  const [stageAutoApplyChangeset, setStageAutoApplyChangeset] = useState(true);
  const [stageCompileCommandsText, setStageCompileCommandsText] = useState('');
  const [stageApproved, setStageApproved] = useState(false);
  const [stageRejected, setStageRejected] = useState(false);

  const [repoContextConfigOpen, setRepoContextConfigOpen] = useState(false);
  const [changesetSchemaBusy, setChangesetSchemaBusy] = useState(false);
  const [changesetSchemaConfigOpen, setChangesetSchemaConfigOpen] = useState(false);
  const [applyErrorConfigOpen, setApplyErrorConfigOpen] = useState(false);
  const [responseViewerOpen, setResponseViewerOpen] = useState(false);
  const [compileErrorConfigOpen, setCompileErrorConfigOpen] = useState(false);
  const [runContextOpen, setRunContextOpen] = useState(false);
  const [inferenceConfigOpen, setInferenceConfigOpen] = useState(false);
  const [previewViewerMode, setPreviewViewerMode] = useState<'prompt' | 'response' | 'stream'>('stream');

  const [treeRootData, setTreeRootData] = useState<RepoTreeResponse | null>(null);
  const [treeChildrenByParent, setTreeChildrenByParent] = useState<Record<string, RepoTreeEntry[]>>({});
  const [loadingTreeDirs, setLoadingTreeDirs] = useState<Set<string>>(new Set());
  const [treeBusy, setTreeBusy] = useState(false);
  const [treeError, setTreeError] = useState<string | null>(null);
  const [selectedRepoPaths, setSelectedRepoPaths] = useState<string[]>([]);
  const [selectedRepoDirs, setSelectedRepoDirs] = useState<Set<string>>(new Set());

  const [expandedStageIds, setExpandedStageIds] = useState<Set<string>>(new Set());
  const [collapsedStageIds, setCollapsedStageIds] = useState<Set<string>>(new Set());
  const [manuallyExpandedLiveExecutionIds, setManuallyExpandedLiveExecutionIds] = useState<Set<string>>(new Set());
  const [manuallyCollapsedLiveExecutionIds, setManuallyCollapsedLiveExecutionIds] = useState<Set<string>>(new Set());
  const [expandedLiveEventIds, setExpandedLiveEventIds] = useState<Set<string>>(new Set());
  const [liveExecutionChains, setLiveExecutionChains] = useState<Record<string, LiveExecutionChainState>>({});
  const [liveExecutionTrails, setLiveExecutionTrails] = useState<LiveStageTrail[]>([]);
  const [stickyCompletedLiveExecutionId, setStickyCompletedLiveExecutionId] = useState<string | null>(null);
  const [liveNow, setLiveNow] = useState(() => Date.now());

  useEffect(() => {
    const styleId = 'workflow-live-bar-keyframes';
    if (document.getElementById(styleId)) return;
    const style = document.createElement('style');
    style.id = styleId;
    style.textContent = workflowLiveBarKeyframes;
    document.head.appendChild(style);
    return () => {
      style.remove();
    };
  }, []);

  const selectedRun = useMemo(() => runs.find((run) => run.id === selectedRunId) ?? null, [runs, selectedRunId]);
  const selectedRunTemplate = selectedRun?.template_id ? templates.find((template) => template.id === selectedRun.template_id) ?? null : null;
  const selectedWorkflowStep = selectedRunTemplate?.definition.steps.find((step) => step.id === (selectedStepId ?? selectedRun?.current_step_id ?? '')) ?? null;
  const sharedInferenceState = useMemo(() => {
    const context = (selectedRun?.context as Record<string, unknown> | undefined) ?? undefined;
    const inference = (context?.model_inference ?? null) as Record<string, unknown> | null;
    return inference;
  }, [selectedRun?.context]);
  const selectedStageState = useMemo(() => {
    const workflowEngine = (selectedRun?.context as Record<string, unknown> | undefined)?.workflow_engine as Record<string, unknown> | undefined;
    const stageState = (workflowEngine?.stage_state ?? {}) as Record<string, unknown>;
    const globalState = (workflowEngine?.global_state ?? {}) as Record<string, unknown>;
    const globalRepoContext = (globalState.repo_context ?? null) as Record<string, unknown> | null;
    const stepId = selectedStepId ?? selectedRun?.current_step_id ?? '';
    const localStageState = (stageState[stepId] ?? null) as Record<string, unknown> | null;
    if (!localStageState && !globalRepoContext && !sharedInferenceState) {
      return null;
    }
    return {
      ...(globalRepoContext ? { repo_context: globalRepoContext } : {}),
      ...(sharedInferenceState ? { inference: sharedInferenceState } : {}),
      ...(localStageState ?? {})
    } as Record<string, unknown>;
  }, [selectedRun?.context, selectedRun?.current_step_id, selectedStepId, sharedInferenceState]);
  const rootTreeEntries = useMemo(() => treeChildrenByParent[''] ?? [], [treeChildrenByParent]);
  const selectedRepoPathSet = useMemo(() => new Set(selectedRepoPaths), [selectedRepoPaths]);
  const repoFragmentSummary = useMemo(() => {
    const includeFiles = Array.from(new Set(selectedRepoPaths.map((value) => value.trim()).filter(Boolean)));
    if (includeFiles.length === 0) {
      return '0 files selected';
    }
    return `${includeFiles.length} file${includeFiles.length === 1 ? '' : 's'} selected`;
  }, [selectedRepoPaths]);
  const selectedStageHydrationKey = `${selectedRun?.id ?? ''}:${selectedStepId ?? selectedRun?.current_step_id ?? ''}`;
  const definition = useMemo(() => buildTemplateDefinition(builderSteps), [builderSteps]);

  const inferenceConnectionStatus = useMemo<InferenceConnectionStatus>(() => {
    if (inferenceTransport === 'api') {
      return {
        color: 'blue',
        label: 'API MODE'
      };
    }

    return {
      color: 'violet',
      label: 'BROWSER MODE'
    };
  }, [inferenceTransport]);

  const inferenceRequiresConnection = false;
  const inferenceReady = true;
  const showStageStream = true;

  const shouldPollBrowserInference = false;

  const inferenceSummaryText = inferenceConnectionStatus.label;

  useEffect(() => {
    setJsonDraft(JSON.stringify(definition, null, 2));
  }, [definition]);

  useEffect(() => {
    selectedRunIdRef.current = selectedRunId;
  }, [selectedRunId]);

  useEffect(() => {
    allWorkflowEventsRef.current = allWorkflowEvents;
  }, [allWorkflowEvents]);

  useEffect(() => {
    return () => {
      for (const source of Object.values(runEventStreamsRef.current)) {
        source.close();
      }
      runEventStreamsRef.current = {};
      for (const timer of Object.values(runRefreshTimersRef.current)) {
        window.clearTimeout(timer);
      }
      runRefreshTimersRef.current = {};
    };
  }, []);

  useEffect(() => {
    void refreshRunsAndTemplates();
  }, []);

  useEffect(() => {
    if (!selectedRunId) {
      setEvents([]);
      setLiveExecutionTrails([]);
      return;
    }
    void refreshRunDetails(selectedRunId);
  }, [selectedRunId]);

  useEffect(() => {
    if (!selectedRunId) {
      setEventStreamConnected(false);
      setEventStreamStatusText('Disconnected');
      return;
    }

    connectRunEventStream(selectedRunId);

    return () => {
      if (selectedRunIdRef.current !== selectedRunId) {
        setEventStreamConnected(false);
        setEventStreamStatusText('Disconnected');
      }
    };
  }, [selectedRunId]);

  useEffect(() => {
    const desired = new Set(
      runs
        .filter((run) => run.status === 'queued' || run.status === 'running' || run.status === 'waiting')
        .map((run) => run.id)
    );

    if (selectedRunId) {
      desired.add(selectedRunId);
    }

    for (const runId of Object.keys(runEventStreamsRef.current)) {
      if (!desired.has(runId)) {
        disconnectRunEventStream(runId);
      }
    }

    for (const runId of desired) {
      connectRunEventStream(runId);
    }
  }, [runs, selectedRunId]);

  useEffect(() => {
    setSelectedStepId(selectedRun?.current_step_id ?? null);
    setManualCapabilityStatus(null);
    setManualCapabilityResponse('');
  }, [selectedRun?.id, selectedRun?.current_step_id]);

  useEffect(() => {
    const inference = (sharedInferenceState ?? null) as Record<string, unknown> | null;
    if (!inference) {
      setInferenceTransport('api');
      setInferenceModel('gpt-5');
      setBrowserTargetUrl('https://website.com/');
      setBrowserCdpUrl('http://127.0.0.1:9222');
      setBrowserSessionId('');
      setBrowserProbe(null);
      return;
    }

    setInferenceTransport((inference.transport as InferenceTransport) ?? 'api');
    setInferenceModel(typeof inference.model === 'string' && inference.model.trim() ? inference.model : 'gpt-5');

    const browser = (inference.browser ?? {}) as Record<string, unknown>;
    setBrowserTargetUrl(typeof browser.target_url === 'string' ? browser.target_url : 'https://website.com/');
    setBrowserCdpUrl(typeof browser.cdp_url === 'string' ? browser.cdp_url : 'http://127.0.0.1:9222');
    setBrowserSessionId(typeof browser.session_id === 'string' ? browser.session_id : '');
    setBrowserProbe(null);
  }, [sharedInferenceState, selectedRun?.id]);

  useEffect(() => {
    const step = selectedWorkflowStep;
    if (!step) return;

    const globalState = ((selectedRun?.context?.workflow_engine as Record<string, unknown> | undefined)?.global_state ?? {}) as Record<string, unknown>;
    const globalCapabilities = (globalState.capabilities ?? {}) as Record<string, unknown>;
    const inferenceConfig = (globalCapabilities.inference ?? {}) as Record<string, unknown>;
    const promptFragments = ((inferenceConfig.prompt_fragments ?? {}) as Record<string, unknown>);
    const promptFragmentEnabled = ((inferenceConfig.prompt_fragment_enabled ?? {}) as Record<string, unknown>);
    const repoContext = (globalCapabilities.context_export ?? {}) as Record<string, unknown>;
    const compileConfig = (globalCapabilities.compile_commands ?? {}) as Record<string, unknown>;
    const review = (selectedStageState?.review ?? {}) as Record<string, unknown>;
    const includeFiles = Array.isArray(repoContext.include_files)
      ? repoContext.include_files.filter((value): value is string => typeof value === 'string')
      : [];

    if (step.id === 'code' && typeof promptFragments.changeset_schema !== 'string') {
      void loadCanonicalChangesetSchema(false);
    }

    setStageUserInput(typeof promptFragments.user_input === 'string' ? promptFragments.user_input : '');
    setStageChangesetSchemaText(typeof promptFragments.changeset_schema === 'string' ? promptFragments.changeset_schema : '');
    setStageApplyError(typeof promptFragments.apply_error === 'string' ? promptFragments.apply_error : '');
    setStageReviewNotes(typeof review.notes === 'string' ? review.notes : '');
    setStageCompileError(typeof promptFragments.compile_error === 'string' ? promptFragments.compile_error : '');
    const compileCommands = compileConfig.commands;
    setStageCompileCommandsText(
      Array.isArray(compileCommands)
        ? compileCommands
            .map((item) => {
              if (typeof item === 'string') return item;
              if (item && typeof item === 'object' && typeof (item as Record<string, unknown>).command === 'string') {
                return String((item as Record<string, unknown>).command);
              }
              return '';
            })
            .filter(Boolean)
            .join('\n')
        : ''
    );
    setStageApproved(Boolean(review.approved));
    setStageRejected(Boolean(review.rejected));
    setStageIncludeRepoContext(
      typeof promptFragmentEnabled.repo_context === 'boolean'
        ? promptFragmentEnabled.repo_context
        : Boolean(step.prompt?.include_repo_context)
    );
    setStageIncludeChangesetSchema(
      typeof promptFragmentEnabled.changeset_schema === 'boolean'
        ? promptFragmentEnabled.changeset_schema
        : (step.prompt?.include_changeset_schema ?? step.id === 'code')
    );
    setStageIncludeApplyError(
      typeof promptFragmentEnabled.apply_error === 'boolean'
        ? promptFragmentEnabled.apply_error
        : false
    );
    setStageIncludeCompileError(
      typeof promptFragmentEnabled.compile_error === 'boolean'
        ? promptFragmentEnabled.compile_error
        : false
    );
    const selectedExecutionLogic = (selectedStageState?.execution_logic ?? step.execution_logic ?? {}) as Record<string, unknown>;
    const selectedAutomation = (selectedExecutionLogic.automation ?? {}) as Record<string, unknown>;
    setStageAutoApplyChangeset(
      typeof selectedAutomation.auto_apply_changeset === 'boolean'
        ? Boolean(selectedAutomation.auto_apply_changeset)
        : Boolean((step.execution?.changeset_apply as Record<string, unknown> | undefined)?.enabled ?? step.id === 'code')
    );
    setInferenceTransport(inferenceConfig.transport === 'browser' ? 'browser' : 'api');
    setInferenceModel(typeof inferenceConfig.model === 'string' ? String(inferenceConfig.model) : 'gpt-5');
    setBrowserTargetUrl(
      typeof ((inferenceConfig.browser as Record<string, unknown> | undefined)?.target_url) === 'string'
        ? String((inferenceConfig.browser as Record<string, unknown>).target_url)
        : 'https://chatgpt.com/?temporary-chat=true'
    );
    setBrowserCdpUrl(
      typeof ((inferenceConfig.browser as Record<string, unknown> | undefined)?.cdp_url) === 'string'
        ? String((inferenceConfig.browser as Record<string, unknown>).cdp_url)
        : 'http://127.0.0.1:9222'
    );
    setInferenceTransport(inferenceConfig.transport === 'browser' ? 'browser' : 'api');
    setInferenceModel(typeof inferenceConfig.model === 'string' ? String(inferenceConfig.model) : 'gpt-5');
    setBrowserTargetUrl(
      typeof ((inferenceConfig.browser as Record<string, unknown> | undefined)?.target_url) === 'string'
        ? String((inferenceConfig.browser as Record<string, unknown>).target_url)
        : 'https://chatgpt.com/?temporary-chat=true'
    );
    setBrowserCdpUrl(
      typeof ((inferenceConfig.browser as Record<string, unknown> | undefined)?.cdp_url) === 'string'
        ? String((inferenceConfig.browser as Record<string, unknown>).cdp_url)
        : 'http://127.0.0.1:9222'
    );
    setStageRepoContextGitRef(typeof repoContext.git_ref === 'string' && repoContext.git_ref.trim() ? repoContext.git_ref : 'WORKTREE');
    setStageRepoContextIncludeFilesText(includeFiles.join('\n'));
    setSelectedRepoPaths(includeFiles);
    setSelectedRepoDirs(new Set());
    setStageRepoContextExcludeRegexText(
      Array.isArray(repoContext.exclude_regex)
        ? repoContext.exclude_regex.filter((value): value is string => typeof value === 'string').join('\n')
        : ''
    );
    setStageRepoContextSavePath(
      typeof repoContext.save_path === 'string' && repoContext.save_path.trim()
        ? repoContext.save_path
        : '/tmp/repo_context.txt'
    );
    setStageRepoContextSkipBinary(typeof repoContext.skip_binary === 'boolean' ? repoContext.skip_binary : true);
    setStageRepoContextSkipGitignore(typeof repoContext.skip_gitignore === 'boolean' ? repoContext.skip_gitignore : true);
    setStageRepoContextIncludeStagedDiff(Boolean(repoContext.include_staged_diff));
    setStageRepoContextIncludeUnstagedDiff(Boolean(repoContext.include_unstaged_diff));
  }, [selectedStageHydrationKey]);

  function buildInteractiveGlobalStatePayload() {
    const includeFiles = stageRepoContextIncludeFilesText
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean);
    const excludeRegex = stageRepoContextExcludeRegexText
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean);
    const compileCommands = stageCompileCommandsText
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .map((command) => ({ command, label: command }));
    const promptFragments: Record<string, unknown> = {
      user_input: stageUserInput,
      changeset_schema: stageChangesetSchemaText,
      apply_error: stageApplyError,
      compile_error: stageCompileError
    };
    const promptFragmentEnabled: Record<string, unknown> = {
      user_input: true,
      repo_context: stageIncludeRepoContext,
      changeset_schema: stageIncludeChangesetSchema,
      apply_error: stageIncludeApplyError && Boolean(stageApplyError.trim()),
      compile_error: stageIncludeCompileError && Boolean(stageCompileError.trim())
    };

    return {
      resources: {
        repo: {
          repo_ref: resolveRepoRefForRun(selectedRun),
          git_ref: stageRepoContextGitRef || 'WORKTREE'
        }
      },
      capabilities: {
        inference: {
          transport: inferenceTransport,
          model: inferenceModel,
          prompt_fragments: promptFragments,
          prompt_fragment_enabled: promptFragmentEnabled,
          browser: {
            profile: 'default',
            bridge_dir: 'bridge',
            cdp_url: browserCdpUrl || 'http://127.0.0.1:9222',
            page_url_contains: browserTargetUrl,
            target_url: browserTargetUrl,
            edge_executable: '',
            user_data_dir: '',
            session_id: null,
            auto_launch_edge: true,
            response_timeout_ms: 120000,
            response_poll_ms: 1000,
            dom_poll_ms: 1000
          }
        },
        context_export: {
          git_ref: stageRepoContextGitRef || 'WORKTREE',
          include_files: includeFiles,
          exclude_regex: excludeRegex,
          save_path: stageRepoContextSavePath || '/tmp/repo_context.txt',
          skip_binary: stageRepoContextSkipBinary,
          skip_gitignore: stageRepoContextSkipGitignore,
          include_staged_diff: stageRepoContextIncludeStagedDiff,
          include_unstaged_diff: stageRepoContextIncludeUnstagedDiff
        },
        compile_commands: {
          commands: compileCommands
        }
      }
    } as Record<string, unknown>;
  }




  useEffect(() => {
    if (!repoContextConfigOpen || !selectedRun) return;
    if (treeRootData) return;
    void loadTreeDir(selectedRun, '', true);
  }, [repoContextConfigOpen, selectedRun?.id, stageRepoContextGitRef, stageRepoContextSkipBinary, stageRepoContextSkipGitignore]);



  useEffect(() => {
    const desired = new Set(
      runs
        .filter((run) => run.status === 'queued' || run.status === 'running' || run.status === 'waiting')
        .map((run) => run.id)
    );

    if (selectedRunId) {
      desired.add(selectedRunId);
    }

    for (const runId of Object.keys(runEventStreamsRef.current)) {
      if (!desired.has(runId)) {
        disconnectRunEventStream(runId);
      }
    }

    for (const runId of desired) {
      connectRunEventStream(runId);
    }
  }, [runs, selectedRunId]);

  useEffect(() => {
    const validTrailKeys = new Set(liveExecutionTrails.map((trail) => trail.key));

    setManuallyExpandedLiveExecutionIds((prev) => {
      const next = new Set(Array.from(prev).filter((key) => validTrailKeys.has(key)));
      return next.size === prev.size ? prev : next;
    });

    setManuallyCollapsedLiveExecutionIds((prev) => {
      const next = new Set(Array.from(prev).filter((key) => validTrailKeys.has(key)));
      return next.size === prev.size ? prev : next;
    });

    setLiveExecutionChains((prev) => {
      const nextEntries = Object.entries(prev).filter(([key]) => validTrailKeys.has(key));
      if (nextEntries.length === Object.keys(prev).length) return prev;
      return Object.fromEntries(nextEntries);
    });
  }, [liveExecutionTrails]);

  useEffect(() => {
    const validEventIds = new Set(
      Object.values(liveExecutionChains)
        .flatMap((state) => state.chain?.items ?? [])
        .map((item) => item.id)
    );

    setExpandedLiveEventIds((prev) => {
      const next = new Set(Array.from(prev).filter((id) => validEventIds.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [liveExecutionChains]);

  useEffect(() => {
    const activeTrail = liveExecutionTrails.find((trail) => trail.isActive || trail.isCurrent);
    if (activeTrail) {
      setStickyCompletedLiveExecutionId(null);
      return;
    }

    const mostRecentCompletedTrail = liveExecutionTrails[0] ?? null;
    setStickyCompletedLiveExecutionId((prev) => {
      if (!mostRecentCompletedTrail) return null;
      if (manuallyCollapsedLiveExecutionIds.has(mostRecentCompletedTrail.key)) return prev;
      return mostRecentCompletedTrail.key;
    });
  }, [liveExecutionTrails, manuallyCollapsedLiveExecutionIds]);

  useEffect(() => {
    for (const trail of liveExecutionTrails) {
      if (isLiveExecutionExpanded(trail)) {
        void ensureLiveExecutionChainLoaded(trail, trail.isActive || trail.isCurrent);
      }
    }
  }, [liveExecutionTrails, selectedRunId, stickyCompletedLiveExecutionId]);

  useEffect(() => {
    setLiveExecutionChains({});
    setExpandedLiveEventIds(new Set());
    setManuallyExpandedLiveExecutionIds(new Set());
    setManuallyCollapsedLiveExecutionIds(new Set());
    setStickyCompletedLiveExecutionId(null);
  }, [selectedRunId]);

  useEffect(() => {
    const hasActiveCapability = liveExecutionTrails.some((trail) =>
      trail.capabilities.some((capability) => capability.isActive)
    );
    if (!hasActiveCapability) return;
    const timer = window.setInterval(() => {
      setLiveNow(Date.now());
    }, 500);
    return () => window.clearInterval(timer);
  }, [liveExecutionTrails]);


  async function refreshRunsAndTemplates(nextSelectedRunId?: string | null) {
    const [runsRes, templatesRes] = await Promise.all([listRuns(), listTemplates()]);
    setRuns(runsRes);
    setTemplates(templatesRes);
    const resolvedRunId = nextSelectedRunId ?? selectedRunId ?? runsRes[0]?.id ?? null;
    setSelectedRunId(resolvedRunId);
    if (!selectedTemplateId && templatesRes[0]) setSelectedTemplateId(templatesRes[0].id);
  }

  function mapLiveExecutionTrails(summary: EventChainSummaryResponse): LiveStageTrail[] {
    return summary.stages
      .map((stage: EventChainSummaryItem) => ({
        key: stage.key,
        stepId: stage.step_id,
        label: stage.label,
        stageExecutionId: stage.stage_execution_id,
        latestCreatedAt: stage.latest_created_at,
        durationMs: stage.duration_ms,
        isActive: stage.is_active,
        isCurrent: stage.is_current,
        capabilities: stage.capabilities
          .map((capability) => ({
            key: capability.key,
            capabilityId: capability.capability_id,
            name: capability.name,
            statusColor: capability.status_color,
            statusLabel: capability.status_label,
            message: capability.message,
            startedAtText: capability.started_at ? formatTimestamp(capability.started_at) : '—',
            startedAtRaw: capability.started_at ?? null,
            durationText: formatDurationMs(capability.duration_ms, capability.started_at, capability.is_active ? null : capability.latest_created_at),
            durationMs: capability.duration_ms,
            latestCreatedAt: capability.latest_created_at,
            isActive: capability.is_active,
            isNew: false,
            eventCount: capability.event_count,
            latestLevel: capability.status_color === 'red' ? 'error' : capability.status_color === 'yellow' ? 'warn' : 'info',
            latestKind: '',
            latestPayload: null,
            inputPayload: null,
            outputPayload: null
          }))
          .sort((a, b) => b.latestCreatedAt.localeCompare(a.latestCreatedAt))
      }))
      .sort((a, b) => b.latestCreatedAt.localeCompare(a.latestCreatedAt));
  }

  function isLiveExecutionExpanded(trail: LiveStageTrail): boolean {
    if (manuallyExpandedLiveExecutionIds.has(trail.key)) return true;
    if (manuallyCollapsedLiveExecutionIds.has(trail.key)) return false;
    if (stickyCompletedLiveExecutionId === trail.key) return true;
    return getLiveExecutionDefaultExpanded(trail);
  }

  function toggleLiveExecutionExpanded(trail: LiveStageTrail) {
    const defaultExpanded = getLiveExecutionDefaultExpanded(trail);
    const currentlyExpanded = isLiveExecutionExpanded(trail);

    if (currentlyExpanded) {
      setManuallyExpandedLiveExecutionIds((prev) => {
        const next = new Set(prev);
        next.delete(trail.key);
        return next;
      });
      setManuallyCollapsedLiveExecutionIds((prev) => {
        const next = new Set(prev);
        if (defaultExpanded) next.add(trail.key);
        else next.delete(trail.key);
        return next;
      });
      return;
    }

    setManuallyCollapsedLiveExecutionIds((prev) => {
      const next = new Set(prev);
      next.delete(trail.key);
      return next;
    });
    setManuallyExpandedLiveExecutionIds((prev) => {
      const next = new Set(prev);
      if (!defaultExpanded) next.add(trail.key);
      else next.delete(trail.key);
      return next;
    });
  }

  async function ensureLiveExecutionChainLoaded(trail: LiveStageTrail, force = false) {
    if (!selectedRunId) return;

    const shouldRefreshLiveTrail = trail.isActive || trail.isCurrent;
    const existing = liveExecutionChains[trail.key];
    const summaryAdvanced = existing?.latestCreatedAt !== trail.latestCreatedAt;
    if (existing?.loading) return;
    if (!force && existing?.chain && !shouldRefreshLiveTrail && !summaryAdvanced) return;

    setLiveExecutionChains((prev) => ({
      ...prev,
      [trail.key]: {
        loading: true,
        error: null,
        chain: prev[trail.key]?.chain ?? null,
        latestCreatedAt: prev[trail.key]?.latestCreatedAt ?? null
      }
    }));

    try {
      const chain = await getStageExecutionChain(selectedRunId, trail.stepId, trail.stageExecutionId);
      setLiveExecutionChains((prev) => ({
        ...prev,
        [trail.key]: {
          loading: false,
          error: null,
          chain,
          latestCreatedAt: trail.latestCreatedAt
        }
      }));
    } catch (err) {
      setLiveExecutionChains((prev) => ({
        ...prev,
        [trail.key]: {
          loading: false,
          error: err instanceof Error ? err.message : String(err),
          chain: prev[trail.key]?.chain ?? null,
          latestCreatedAt: prev[trail.key]?.latestCreatedAt ?? null
        }
      }));
    }
  }

  function toggleLiveEventExpanded(eventId: string) {
    setExpandedLiveEventIds((prev) => {
      const next = new Set(prev);
      if (next.has(eventId)) next.delete(eventId);
      else next.add(eventId);
      return next;
    });
  }

  const eventStreamStatus = useMemo<EventStreamStatus>(() => {
    if (eventStreamConnected) return { color: 'teal', label: 'Live' };
    if (selectedRunId) return { color: 'yellow', label: eventStreamStatusText || 'Reconnecting' };
    return { color: 'gray', label: 'Idle' };
  }, [eventStreamConnected, eventStreamStatusText, selectedRunId]);

  function liveStageTone(trail: LiveStageTrail): string {
    const latestCapability = trail.capabilities[0] ?? null;
    if (trail.isCurrent && trail.isActive) return 'blue';
    if (trail.isActive) return 'yellow';
    if (!latestCapability) return 'gray';
    return capabilityTone(latestCapability);
  }

  function capabilityTone(capability: LiveCapabilityTrail): string {
    if (capability.isActive) return 'blue';
    if (capability.statusColor === 'red' || capability.latestLevel === 'error') return 'red';
    if (capability.statusColor === 'yellow' || capability.latestLevel === 'warn') return 'yellow';
    return 'green';
  }

  function livePulseStyle(active: boolean, recent: boolean): React.CSSProperties {
    return {
      position: 'relative',
      overflow: 'hidden',
      transition: 'box-shadow 160ms ease, transform 160ms ease, border-color 160ms ease',
      boxShadow: active
        ? '0 0 0 1px rgba(59,130,246,0.5), 0 0 22px rgba(59,130,246,0.22)'
        : recent
          ? '0 0 0 1px rgba(34,197,94,0.35), 0 0 18px rgba(34,197,94,0.16)'
          : undefined,
      transform: active ? 'translateY(-1px)' : undefined
    };
  }

  function liveProgressBar(active: boolean, tone: string): React.CSSProperties {
    if (!active) {
      return { display: 'none' };
    }
    const stripe = tone === 'red'
      ? 'rgba(250,82,82,0.22)'
      : tone === 'yellow'
        ? 'rgba(250,176,5,0.22)'
        : tone === 'green'
          ? 'rgba(64,192,87,0.22)'
          : 'rgba(34,139,230,0.24)';
    const highlight = tone === 'red'
      ? 'rgba(255,255,255,0.10)'
      : tone === 'yellow'
        ? 'rgba(255,255,255,0.12)'
        : tone === 'green'
          ? 'rgba(255,255,255,0.10)'
          : 'rgba(255,255,255,0.12)';
    return {
      position: 'absolute',
      inset: 0,
      borderRadius: 8,
      backgroundImage: `repeating-linear-gradient(-45deg, ${stripe} 0px, ${stripe} 12px, ${highlight} 12px, ${highlight} 24px)`,
      backgroundSize: '34px 34px',
      animation: 'workflow-live-bar 900ms linear infinite',
      pointerEvents: 'none',
      opacity: 0.55,
      zIndex: 0
    };
  }

  function capabilityIoPayload(capability: LiveCapabilityTrail): Record<string, unknown> {
    return {
      capability_id: capability.capabilityId,
      name: capability.name,
      status: capability.statusLabel,
      latest_kind: capability.latestKind,
      latest_level: capability.latestLevel,
      input: capability.inputPayload ?? null,
      output: capability.outputPayload ?? capability.latestPayload ?? null
    };
  }

  function deriveCapabilityStatusLabel(event: StageExecutionEvent | null, fallback: string): string {
    if (!event) return fallback;
    if (event.level === 'error' || event.kind.endsWith('_failed')) return 'FAILED';
    if (event.kind.endsWith('_completed')) return 'COMPLETE';
    if (event.kind.endsWith('_started')) return 'RUNNING';
    return fallback;
  }

  function deriveCapabilityStatusColor(event: StageExecutionEvent | null, fallback: string): string {
    if (!event) return fallback;
    if (event.level === 'error' || event.kind.endsWith('_failed')) return 'red';
    if (event.level === 'warn') return 'yellow';
    if (event.kind.endsWith('_started')) return 'blue';
    if (event.kind.endsWith('_completed')) return 'green';
    return fallback;
  }

  function deriveCapabilityPayload(role: 'input' | 'output', payload: unknown): unknown {
    const objectPayload = payload && typeof payload === 'object' ? payload as Record<string, unknown> : null;
    if (!objectPayload) return payload ?? null;
    if (role === 'input') {
      return objectPayload.input ?? objectPayload.inputs ?? objectPayload.request ?? objectPayload.args ?? objectPayload.payload ?? objectPayload;
    }
    return objectPayload.output ?? objectPayload.result ?? objectPayload.response ?? objectPayload.error ?? objectPayload.payload ?? objectPayload;
  }

  function capabilityNameFromKind(kind: string): string {
    const value = kind
      .replace(/_started$/i, '')
      .replace(/_completed$/i, '')
      .replace(/_failed$/i, '');
    const prefixes = ['stage_execution_', 'capability_', 'workflow_'];
    for (const prefix of prefixes) {
      if (value.startsWith(prefix)) {
        return value.slice(prefix.length);
      }
    }
    return value;
  }

  function formatCapabilityLabel(value: string): string {
    return value
      .split(/[\/_-]+/)
      .filter(Boolean)
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join('/');
  }

  function formatDuration(startedAt?: string | null, endedAt?: string | null): string {
    if (!startedAt) return 'elapsed —';
    const start = new Date(startedAt).getTime();
    const end = endedAt ? new Date(endedAt).getTime() : Date.now();
    if (!Number.isFinite(start) || !Number.isFinite(end) || end < start) return 'elapsed —';
    return formatDurationMs(end - start, startedAt, endedAt);
  }

  function buildLiveCapabilitiesFromEvents(trail: LiveStageTrail, rawEvents: StageExecutionEvent[]): LiveCapabilityTrail[] {
    const eventsAsc = rawEvents.slice().sort((a, b) => a.sequence_no - b.sequence_no);
    const grouped = new Map<string, StageExecutionEvent[]>();

    for (const event of eventsAsc) {
      const capabilityId = event.capability_invocation_id;
      if (!capabilityId) continue;
      const bucket = grouped.get(capabilityId) ?? [];
      bucket.push(event);
      grouped.set(capabilityId, bucket);
    }

    const mapped = trail.capabilities.map((capability) => {
      const capabilityEvents = grouped.get(capability.capabilityId) ?? [];
      const firstEvent = capabilityEvents[0] ?? null;
      const lastEvent = capabilityEvents[capabilityEvents.length - 1] ?? null;
      const startedEvent = capabilityEvents.find((event) => event.kind.endsWith('_started')) ?? firstEvent;
      const completedEvent = capabilityEvents.find((event) => event.kind.endsWith('_completed') || event.kind.endsWith('_failed')) ?? lastEvent;
      return {
        ...capability,
        statusColor: deriveCapabilityStatusColor(lastEvent, capability.statusColor),
        statusLabel: deriveCapabilityStatusLabel(lastEvent, capability.statusLabel),
        message: lastEvent?.message ?? capability.message,
        latestCreatedAt: lastEvent?.created_at ?? capability.latestCreatedAt,
        isActive: capabilityEvents.length > 0 ? !capabilityEvents.some((event) => event.kind.endsWith('_completed') || event.kind.endsWith('_failed')) : capability.isActive,
        isNew: capabilityEvents.some((event) => recentEventIds.has(event.id)),
        eventCount: capabilityEvents.length > 0 ? capabilityEvents.length : capability.eventCount,
        latestLevel: lastEvent?.level ?? capability.latestLevel,
        latestKind: lastEvent?.kind ?? capability.latestKind,
        latestPayload: lastEvent?.payload ?? capability.latestPayload,
        startedAtRaw: startedEvent?.created_at ?? capability.startedAtRaw,
        inputPayload: startedEvent ? deriveCapabilityPayload('input', startedEvent.payload) : capability.inputPayload,
        outputPayload: completedEvent ? deriveCapabilityPayload('output', completedEvent.payload) : capability.outputPayload
      };
    });

    for (const [capabilityId, capabilityEvents] of grouped.entries()) {
      if (mapped.some((capability) => capability.capabilityId === capabilityId)) continue;
      const firstEvent = capabilityEvents[0] ?? null;
      const lastEvent = capabilityEvents[capabilityEvents.length - 1] ?? null;
      const capabilityName = formatCapabilityLabel(capabilityNameFromKind(lastEvent?.kind ?? firstEvent?.kind ?? capabilityId));
      mapped.push({
        key: capabilityId,
        capabilityId,
        name: capabilityName,
        statusColor: deriveCapabilityStatusColor(lastEvent, 'gray'),
        statusLabel: deriveCapabilityStatusLabel(lastEvent, 'INFO'),
        message: lastEvent?.message ?? capabilityName,
        startedAtText: firstEvent ? formatTimestamp(firstEvent.created_at) : '—',
        startedAtRaw: firstEvent?.created_at ?? null,
        durationText: formatDuration(firstEvent?.created_at ?? null, lastEvent && (lastEvent.kind.endsWith('_completed') || lastEvent.kind.endsWith('_failed')) ? lastEvent.created_at : null),
        durationMs: null,
        latestCreatedAt: lastEvent?.created_at ?? firstEvent?.created_at ?? '',
        isActive: !capabilityEvents.some((event) => event.kind.endsWith('_completed') || event.kind.endsWith('_failed')),
        isNew: capabilityEvents.some((event) => recentEventIds.has(event.id)),
        eventCount: capabilityEvents.length,
        latestLevel: lastEvent?.level ?? 'info',
        latestKind: lastEvent?.kind ?? '',
        latestPayload: lastEvent?.payload ?? null,
        inputPayload: firstEvent ? deriveCapabilityPayload('input', firstEvent.payload) : null,
        outputPayload: lastEvent ? deriveCapabilityPayload('output', lastEvent.payload) : null
      });
    }

    return mapped.sort((a, b) => b.latestCreatedAt.localeCompare(a.latestCreatedAt));
  }

  function mergeWorkflowEvents(existing: WorkflowEvent[], incoming: WorkflowEvent & { sequence_no?: number }): WorkflowEvent[] {
    const deduped = existing.filter((item) => item.id !== incoming.id);
    return [...deduped, incoming].sort((a, b) => a.created_at.localeCompare(b.created_at));
  }

  function getAfterSequence(runId: string): number {
    const currentEvents = allWorkflowEventsRef.current[runId] ?? [];
    return currentEvents.reduce((max, event) => {
      const candidate = event as WorkflowEvent & { sequence_no?: number };
      const value = typeof candidate.sequence_no === 'number' ? candidate.sequence_no : 0;
      return Math.max(max, value);
    }, 0);
  }

  async function refreshRunRecord(runId: string) {
    const run = await getRun(runId);
    setRuns((prev) => [run, ...prev.filter((item) => item.id !== run.id)]);
  }

  function scheduleRunRefresh(runId: string) {
    const existing = runRefreshTimersRef.current[runId];
    if (typeof existing === 'number') {
      window.clearTimeout(existing);
    }
    runRefreshTimersRef.current[runId] = window.setTimeout(() => {
      delete runRefreshTimersRef.current[runId];
      void refreshRunRecord(runId);
    }, 150);
  }

  function applyIncomingWorkflowEvent(runId: string, incoming: WorkflowEvent & { sequence_no?: number }) {
    setRecentEventIds((prev) => {
      const next = new Set(prev);
      next.add(incoming.id);
      return next;
    });
    window.setTimeout(() => {
      setRecentEventIds((prev) => {
        const next = new Set(prev);
        next.delete(incoming.id);
        return next;
      });
    }, 1800);
    setAllWorkflowEvents((prev) => ({
      ...prev,
      [runId]: mergeWorkflowEvents(prev[runId] ?? [], incoming)
    }));
    if (selectedRunIdRef.current === runId) {
      setEvents((prev) => mergeWorkflowEvents(prev, incoming));
    }
    scheduleRunRefresh(runId);
  }

  function connectRunEventStream(runId: string) {
    if (runEventStreamsRef.current[runId]) {
      return;
    }

    const source = openEventStream(runId, getAfterSequence(runId));
    runEventStreamsRef.current[runId] = source;

    if (selectedRunIdRef.current === runId) {
      setEventStreamConnected(false);
      setEventStreamStatusText('Connecting');
    }

    source.onopen = () => {
      if (selectedRunIdRef.current === runId) {
        setEventStreamConnected(true);
        setEventStreamStatusText('Live');
      }
    };

    source.addEventListener('workflow_event', (raw) => {
      try {
        const incoming = JSON.parse((raw as MessageEvent<string>).data) as WorkflowEvent & { sequence_no?: number };
        applyIncomingWorkflowEvent(runId, incoming);
        if (selectedRunIdRef.current === runId) {
          setEventStreamConnected(true);
          setEventStreamStatusText('Live');
        }
      } catch {
      }
    });

    source.addEventListener('monitor_snapshot', (raw) => {
      if (selectedRunIdRef.current !== runId) {
        return;
      }
      try {
        const summary = JSON.parse((raw as MessageEvent<string>).data) as EventChainSummaryResponse;
        setLiveExecutionTrails(mapLiveExecutionTrails(summary));
        setEventStreamConnected(true);
        setEventStreamStatusText('Live');
      } catch {
      }
    });

    source.onerror = () => {
      if (selectedRunIdRef.current === runId) {
        setEventStreamConnected(false);
        setEventStreamStatusText('Reconnecting');
      }
    };
  }

  function disconnectRunEventStream(runId: string) {
    const source = runEventStreamsRef.current[runId];
    if (source) {
      source.close();
      delete runEventStreamsRef.current[runId];
    }
    const timer = runRefreshTimersRef.current[runId];
    if (typeof timer === 'number') {
      window.clearTimeout(timer);
      delete runRefreshTimersRef.current[runId];
    }
  }

  async function refreshRunDetails(runId: string) {
    const [run, runEvents] = await Promise.all([getRun(runId), listRunEvents(runId)]);
    setRuns((prev) => [run, ...prev.filter((item) => item.id !== run.id)]);
    setEvents(runEvents);
    setAllWorkflowEvents((prev) => ({ ...prev, [run.id]: runEvents }));
    setSelectedRunId(run.id);
  }

  async function refreshLiveMonitor(runId: string) {
    const summary = await getEventChainSummary(runId);
    setLiveExecutionTrails(mapLiveExecutionTrails(summary));
  }


  async function openWorkflow(runId: string) {
    await refreshRunDetails(runId);
    setView('monitor');
    setMonitorView('workflow_detail');
  }

  function backToWorkflowList() {
    setMonitorView('workflow_list');
  }

  function updateStep(stepId: string, patch: Partial<BuilderStep>) {
    setBuilderSteps((prev) => prev.map((step) => (step.id === stepId ? { ...step, ...patch } : step)));
  }

  function updateStepTransitions(stepId: string, patch: Partial<TransitionEditorValue>) {
    setBuilderSteps((prev) => prev.map((step) => step.id !== stepId ? step : { ...step, transitions: { ...step.transitions, ...patch } }));
  }

  async function handleSaveTemplate() {
    try {
      setBusy(true);
      setError(null);
      const parsed = builderMode === 'json' ? (JSON.parse(jsonDraft) as WorkflowTemplateDefinition) : definition;
      const template = await createTemplate({ name: workflowName, description: workflowDescription, definition: parsed });
      await refreshRunsAndTemplates();
      setSelectedTemplateId(template.id);
      setTemplateModalOpen(false);
      if (createRunAfterSave) {
        const run = await createRun({ template_id: template.id, title: runTitle, repo_ref: repoRef, context: {} });
        await refreshRunsAndTemplates(run.id);
        setView('monitor');
        setMonitorView('workflow_detail');
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleCreateRunFromTemplate(templateId?: string | null) {
    if (!templateId) {
      setError('Select a template first.');
      return;
    }
    try {
      setBusy(true);
      setError(null);
      const run = await createRun({ template_id: templateId, title: runTitle, repo_ref: repoRef, context: {} });
      await refreshRunsAndTemplates(run.id);
      setView('monitor');
      setMonitorView('workflow_detail');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleStartRun() {
    if (!selectedRunId) return;
    try {
      setBusy(true);
      setError(null);
      await startWorkflowRun(selectedRunId);
      await refreshRunDetails(selectedRunId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleResumeRun() {
    if (!selectedRunId) return;
    try {
      setBusy(true);
      setError(null);
      await resumeWorkflowRun(selectedRunId);
      await refreshRunDetails(selectedRunId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handlePauseRun() {
    if (!selectedRunId) return;
    try {
      setBusy(true);
      setError(null);
      await pauseWorkflowRun(selectedRunId);
      await refreshRunDetails(selectedRunId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleDeleteRun(runId: string) {
    try {
      setBusy(true);
      setError(null);
      await deleteRun(runId);
      const nextId = selectedRunId === runId ? null : selectedRunId;
      await refreshRunsAndTemplates(nextId);
      if (selectedRunId === runId) {
        setEvents([]);
        setMonitorView('workflow_list');
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function refreshSelectedRunArtifacts() {
    if (!selectedRunId) return;
    await refreshRunDetails(selectedRunId);
  }

  async function loadCanonicalChangesetSchema(forceOverride = true) {
    try {
      setChangesetSchemaBusy(true);
      const json = await getChangesetSchema();
      if (forceOverride || !stageChangesetSchemaText.trim()) {
        setStageChangesetSchemaText(typeof json.schema === 'string' ? json.schema : '');
      }
    } finally {
      setChangesetSchemaBusy(false);
    }
  }

  function syncRepoSelectionState(nextPaths: string[]) {
    const normalized = Array.from(new Set(nextPaths.map((path) => path.trim()).filter(Boolean))).sort();
    setSelectedRepoPaths(normalized);
    setStageRepoContextIncludeFilesText(normalized.join('\n'));
  }

  function resolveRepoRefForRun(run: WorkflowRun | null): string {
    const workflowEngine = (run?.context as Record<string, unknown> | undefined)?.workflow_engine as Record<string, unknown> | undefined;
    const globalState = (workflowEngine?.global_state ?? {}) as Record<string, unknown>;
    const resources = (globalState.resources ?? {}) as Record<string, unknown>;
    const repo = (resources.repo ?? {}) as Record<string, unknown>;
    if (typeof repo.repo_ref === 'string' && repo.repo_ref.trim()) {
      return String(repo.repo_ref);
    }
    if (typeof run?.repo_ref === 'string' && run.repo_ref.trim()) {
      return run.repo_ref;
    }
    return repoRef;
  }

  function setPaths(paths: string[], checked: boolean) {
    const next = new Set(selectedRepoPaths);
    for (const path of paths) {
      if (checked) next.add(path);
      else next.delete(path);
    }
    syncRepoSelectionState(Array.from(next));
  }

  function toggleFile(path: string) {
    setPaths([path], !selectedRepoPathSet.has(path));
  }

  async function loadTreeSubtree(run: WorkflowRun, basePath: string): Promise<{ children: Record<string, RepoTreeEntry[]>; files: string[] }> {
    const repoRefForTree = resolveRepoRefForRun(run);
    const data = await listRepoTree(repoRefForTree, stageRepoContextGitRef, {
      basePath,
      skipBinary: stageRepoContextSkipBinary,
      skipGitignore: stageRepoContextSkipGitignore
    });

    const children: Record<string, RepoTreeEntry[]> = {
      [basePath]: data.entries
    };
    const files: string[] = [];

    for (const entry of data.entries) {
      if (entry.kind === 'file') {
        files.push(entry.path);
      } else if (entry.has_children) {
        const nested = await loadTreeSubtree(run, entry.path);
        Object.assign(children, nested.children);
        files.push(...nested.files);
      }
    }

    return { children, files };
  }

  async function loadTreeDir(run: WorkflowRun, basePath: string, replaceRoot = false) {
    if (loadingTreeDirs.has(basePath)) return;

    setTreeError(null);
    if (replaceRoot) setTreeBusy(true);
    setLoadingTreeDirs((prev) => {
      const next = new Set(prev);
      next.add(basePath);
      return next;
    });

    try {
      const repoRefForTree = resolveRepoRefForRun(run);
      const data = await listRepoTree(repoRefForTree, stageRepoContextGitRef, {
        basePath,
        skipBinary: stageRepoContextSkipBinary,
        skipGitignore: stageRepoContextSkipGitignore
      });

      if (replaceRoot) {
        setTreeRootData(data);
        setTreeChildrenByParent({ '': data.entries });
        const visiblePaths = new Set<string>(data.entries.filter((entry) => entry.kind === 'file').map((entry) => entry.path));
        syncRepoSelectionState(selectedRepoPaths.filter((path) => visiblePaths.has(path)));
        setSelectedRepoDirs(new Set());
      } else {
        setTreeChildrenByParent((prev) => ({ ...prev, [basePath]: data.entries }));
      }
    } catch (err) {
      setTreeError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoadingTreeDirs((prev) => {
        const next = new Set(prev);
        next.delete(basePath);
        return next;
      });
      if (replaceRoot) setTreeBusy(false);
    }
  }

  async function toggleDirectory(entry: RepoTreeEntry, checked: boolean) {
    if (!selectedRun) return;

    if (checked) {
      const nested = await loadTreeSubtree(selectedRun, entry.path);
      setTreeChildrenByParent((prev) => ({ ...prev, ...nested.children }));
      setSelectedRepoDirs((prev) => {
        const next = new Set(prev);
        next.add(entry.path);
        return next;
      });
      setPaths(nested.files, true);
      return;
    }

    const descendantFiles = collectLoadedFilePaths(entry.path, treeChildrenByParent);
    setSelectedRepoDirs((prev) => {
      const next = new Set(prev);
      next.delete(entry.path);
      return next;
    });
    setPaths(descendantFiles, false);
  }

  const composedInferencePrompt = useMemo(() => {
    if (selectedWorkflowStep?.id === 'compile') {
      return stageCompileCommandsText.trim()
        ? `### COMPILE COMMANDS\n${stageCompileCommandsText.trim()}`
        : '';
    }

    const parts: string[] = [];
    if (stageIncludeRepoContext) parts.push('### REPO CONTEXT\nAttached repo context from backend export');
    if (selectedWorkflowStep?.id === 'code' && stageIncludeChangesetSchema) {
      parts.push(`### CHANGESET SCHEMA\n${stageChangesetSchemaText.trim() || 'Use ChangeSet JSON version 1. Return only the JSON payload.'}`);
    }
    if (selectedWorkflowStep?.id === 'code' && stageIncludeApplyError && stageApplyError.trim()) parts.push(`### APPLY ERROR\n${stageApplyError.trim()}`);
    if (selectedWorkflowStep?.id === 'code' && stageIncludeCompileError && stageCompileError.trim()) parts.push(`### COMPILE ERROR\n${stageCompileError.trim()}`);
    if (selectedWorkflowStep?.id === 'review' && stageReviewNotes.trim()) parts.push(`### REVIEW NOTES\n${stageReviewNotes.trim()}`);
    if (stageUserInput.trim()) parts.push(`### USER INPUT\n${stageUserInput.trim()}`);
    return parts.join('\n\n');
  }, [selectedWorkflowStep?.id, stageCompileCommandsText, stageIncludeRepoContext, stageIncludeChangesetSchema, stageChangesetSchemaText, stageIncludeApplyError, stageApplyError, stageIncludeCompileError, stageCompileError, stageReviewNotes, stageUserInput]);

  const selectedLiveStageTrail = useMemo(() => {
    const scopedTrails = selectedStepId
      ? liveExecutionTrails.filter((trail) => trail.stepId === selectedStepId)
      : liveExecutionTrails;

    if (scopedTrails.length === 0) return null;
    return scopedTrails.find((trail) => trail.isCurrent || trail.isActive) ?? scopedTrails[0];
  }, [liveExecutionTrails, selectedStepId]);

  const selectedLiveExecutionState = selectedLiveStageTrail ? (liveExecutionChains[selectedLiveStageTrail.key] ?? null) : null;

  useEffect(() => {
    if (!selectedLiveStageTrail) return;
    void ensureLiveExecutionChainLoaded(selectedLiveStageTrail, true);
  }, [
    selectedLiveStageTrail?.key,
    selectedLiveStageTrail?.isActive,
    selectedLiveStageTrail?.isCurrent,
    selectedLiveStageTrail?.latestCreatedAt,
    selectedRunId
  ]);

  const inferenceResponse = useMemo(() => {
    const executionItems = selectedLiveExecutionState?.chain?.items ?? [];
    for (let i = executionItems.length - 1; i >= 0; i -= 1) {
      const text = extractInferenceTextFromPayload(executionItems[i].payload);
      if (text.trim()) return text;
    }

    if (selectedLiveStageTrail) {
      if (selectedLiveExecutionState?.loading) {
        return 'Loading current execution output…';
      }
      if (selectedLiveExecutionState?.error) {
        return `Unable to load current execution chain: ${selectedLiveExecutionState.error}`;
      }
      return '';
    }

    const stageEvents = selectedStepId ? events.filter((event) => event.step_id === selectedStepId) : events;
    for (let i = stageEvents.length - 1; i >= 0; i -= 1) {
      const text = extractInferenceTextFromPayload(stageEvents[i].payload);
      if (text.trim()) return text;
    }
    return '';
  }, [events, selectedStepId, selectedLiveExecutionState, selectedLiveStageTrail]);

  const stageStreamContent = useMemo(() => {
    const parts: string[] = [];
    if (composedInferencePrompt.trim()) parts.push(`### INPUT\n${composedInferencePrompt}`);

    if (selectedWorkflowStep?.id === 'compile') {
      const executionItems = selectedLiveExecutionState?.chain?.items ?? [];
      let compileResults: Array<Record<string, unknown>> = [];

      for (let i = executionItems.length - 1; i >= 0; i -= 1) {
        const rows = extractCompileResultsFromPayload(executionItems[i].payload);
        if (rows.length > 0) {
          compileResults = rows;
          break;
        }
      }

      if (compileResults.length === 0) {
        const stageEvents = selectedStepId ? events.filter((event) => event.step_id === selectedStepId) : events;
        for (let i = stageEvents.length - 1; i >= 0; i -= 1) {
          const rows = extractCompileResultsFromPayload(stageEvents[i].payload);
          if (rows.length > 0) {
            compileResults = rows;
            break;
          }
        }
      }

      if (compileResults.length > 0) {
        parts.push(formatCompileStageStream(compileResults));
      } else if (selectedLiveExecutionState?.loading) {
        parts.push('### COMPILE RESULTS\nLoading current execution output…');
      } else if (selectedLiveExecutionState?.error) {
        parts.push(`### COMPILE RESULTS\nUnable to load current execution chain: ${selectedLiveExecutionState.error}`);
      }

      return parts.join('\n\n');
    }

    if (inferenceResponse.trim()) parts.push(`### OUTPUT\n${inferenceResponse}`);
    return parts.join('\n\n');
  }, [composedInferencePrompt, events, inferenceResponse, selectedLiveExecutionState, selectedStepId, selectedWorkflowStep?.id]);

  function renderPreviewPanel(title: string, content: string, emptyText: string, mode: 'prompt' | 'response' | 'stream') {
    return (
      <Stack gap="xs" h="100%">
        <Group justify="space-between" align="center">
          <Text fw={600}>{title}</Text>
          <Group gap="xs">
            <Badge variant="light">{content ? `${content.length.toLocaleString()} chars` : 'empty'}</Badge>
            <Button size="xs" variant="light" onClick={() => { setPreviewViewerMode(mode); setResponseViewerOpen(true); }}>
              Full screen
            </Button>
          </Group>
        </Group>
        <Box p="md" h="100%" style={{ flex: 1, border: '1px solid var(--mantine-color-dark-4)', borderRadius: 12, minHeight: 220, background: 'linear-gradient(180deg, rgba(255,255,255,0.02), rgba(255,255,255,0.01))' }}>
          <Text size="sm" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', wordBreak: 'break-word', lineHeight: 1.7, fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, Liberation Mono, monospace' }}>
            {content || emptyText}
          </Text>
        </Box>
      </Stack>
    );
  }

  function renderStageStreamPanel(emptyText: string) {
    return renderPreviewPanel('Stage stream', stageStreamContent, emptyText, 'stream');
  }

  function buildInteractiveStagePayload() {
    const step = selectedWorkflowStep;
    const stepId = step?.id ?? null;

    if (stepId === 'compile') {
      return {
        execution_logic: {
          kind: 'compile_stage_policy'
        }
      } as Record<string, unknown>;
    }

    if (stepId === 'review') {
      return {
        review: { approved: stageApproved, rejected: stageRejected, notes: stageReviewNotes }
      } as Record<string, unknown>;
    }

    const payload: Record<string, unknown> = {};

    if (stepId === 'design') {
      payload.execution_logic = {
        kind: 'design_stage_policy',
        automation: {
          inject_context: stageIncludeRepoContext
        }
      };
    }

    if (stepId === 'code') {
      payload.execution_logic = {
        kind: 'code_stage_policy',
        automation: {
          inject_context: stageIncludeRepoContext,
          inject_changeset_schema: stageIncludeChangesetSchema,
          auto_apply_changeset: stageAutoApplyChangeset,
          max_consecutive_apply_failures: 1
        }
      };
    }

    return payload;
  }

  async function runManualCapability(action: () => Promise<Record<string, unknown>>, successMessage: string) {
    try {
      setManualCapabilityBusy(true);
      setManualCapabilityStatus(null);
      const json = await action();
      setManualCapabilityResponse(JSON.stringify(json, null, 2));
      setManualCapabilityStatus(successMessage);
      await refreshSelectedRunArtifacts();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setManualCapabilityStatus(message);
      setManualCapabilityResponse('');
    } finally {
      setManualCapabilityBusy(false);
    }
  }

  async function handleManualSelectStep(stepId: string | null) {
    if (!selectedRun || !stepId) return;
    await runManualCapability(async () => {
      const json = await selectWorkflowStep(selectedRun.id, stepId);
      return json as Record<string, unknown>;
    }, `Selected step ${stepId}.`);
    setSelectedStepId(stepId);
  }

  async function handleManualRunCurrentStep() {
    if (!selectedRun) return;
    await runManualCapability(async () => {
      const json = await runCurrentWorkflowStep(selectedRun.id, selectedStepId ?? selectedRun.current_step_id);
      return json as Record<string, unknown>;
    }, 'Executed current workflow step.');
  }

  async function handleManualPreviousStep() {
    if (!selectedRun) return;
    await runManualCapability(async () => {
      const json = await previousWorkflowStep(selectedRun.id);
      return json as Record<string, unknown>;
    }, 'Moved to previous workflow step.');
  }

  async function handleManualNextStep() {
    if (!selectedRun) return;
    await runManualCapability(async () => {
      const json = await nextWorkflowStep(selectedRun.id);
      return json as Record<string, unknown>;
    }, 'Moved to next workflow step.');
  }

  async function syncInteractiveGlobalState() {
    if (!selectedRun) return;
    const globalPayload = buildInteractiveGlobalStatePayload();
    await patchWorkflowGlobalState(selectedRun.id, globalPayload);
  }

  async function handleManualPatchStageState() {
    if (!selectedRun || !selectedStepId) return;
    await runManualCapability(async () => {
      await syncInteractiveGlobalState();
      const payload = buildInteractiveStagePayload();
      const json = await patchWorkflowStageState(selectedRun.id, selectedStepId, payload);
      return json as Record<string, unknown>;
    }, 'Patched stage state.');
  }

  async function handleManualRunWithPatchedState() {
    if (!selectedRun || !selectedStepId) return;
    await runManualCapability(async () => {
      await syncInteractiveGlobalState();
      const payload = buildInteractiveStagePayload();
      const json = await runCurrentWorkflowStep(selectedRun.id, selectedStepId, payload);
      return json as Record<string, unknown>;
    }, 'Executed current stage with interactive local state through backend workflow engine.');
  }

  async function configureInference() {
    if (!selectedRun) return;
    try {
      setInferenceBusy(true);
      setInferenceStatus(null);
      await syncInteractiveGlobalState();
      setInferenceStatus('Global capability configuration saved.');
      await refreshSelectedRunArtifacts();
    } catch (err) {
      setInferenceStatus(err instanceof Error ? err.message : String(err));
    } finally {
      setInferenceBusy(false);
    }
  }






  function eventTone(event: WorkflowEvent): EventTone {
    if (event.level === 'error') return { color: 'red', label: 'ERROR' };
    if (event.level === 'warn') return { color: 'yellow', label: 'WARN' };
    if (event.kind.includes('success') || event.kind.includes('completed')) return { color: 'green', label: 'SUCCESS' };
    if (event.kind.includes('running') || event.kind.includes('executed')) return { color: 'blue', label: 'RUNNING' };
    return { color: 'gray', label: 'INFO' };
  }

  function summarizeEvent(event: WorkflowEvent): string {
    if (event.kind === 'stage_executed') {
      const disposition = typeof event.payload?.disposition === 'string' ? event.payload.disposition : null;
      return disposition ? `Stage executed: ${disposition}` : 'Stage executed';
    }
    if (event.kind === 'capability_executed') {
      const node = typeof event.payload?.node === 'string' ? event.payload.node : null;
      return node ? `Capability ${node} executed` : 'Capability executed';
    }
    if (event.kind === 'run_paused') return 'Run paused';
    if (event.kind === 'run_created') return 'Run created';
    return event.message;
  }


  function formatDurationMs(durationMs: number | null, fallbackStartedAt?: string | null, fallbackCompletedAt?: string | null): string {
    if (typeof durationMs === 'number') {
      if (durationMs < 1000) return `${durationMs} ms`;
      const seconds = durationMs / 1000;
      if (seconds < 60) return `${seconds.toFixed(seconds < 10 ? 1 : 0)} s`;
      const minutes = Math.floor(seconds / 60);
      const remainingSeconds = Math.round(seconds % 60);
      return `${minutes}m ${remainingSeconds}s`;
    }
    return formatDuration(fallbackStartedAt, fallbackCompletedAt);
  }

  function toggleExpandedSet(setter: React.Dispatch<React.SetStateAction<Set<string>>>, key: string) {
    setter((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key); else next.add(key);
      return next;
    });
  }

  function toggleStageExpanded(stepId: string, isCurrent: boolean) {
    if (isCurrent) {
      toggleExpandedSet(setCollapsedStageIds, stepId);
      return;
    }
    toggleExpandedSet(setExpandedStageIds, stepId);
  }

  const stepOptions = builderSteps.map((step) => ({ value: step.id, label: `${step.name} (${step.id})` }));
  const stageContractByType = new Map((builderContract?.stages ?? []).map((stage) => [stage.step_type, stage]));

  function renderBuilderField(step: BuilderStep, field: WorkflowBuilderFieldContract) {
    const value = step.fields[field.key];
    if (field.type === 'boolean') {
      return (
        <Switch
          key={field.key}
          label={field.label}
          checked={typeof value === 'boolean' ? value : false}
          onChange={(e) => updateStep(step.id, { fields: { ...step.fields, [field.key]: e.currentTarget.checked } })}
        />
      );
    }
    if (field.type === 'integer') {
      return (
        <TextInput
          key={field.key}
          label={field.label}
          value={String(typeof value === 'number' ? value : 0)}
          onChange={(e) => updateStep(step.id, { fields: { ...step.fields, [field.key]: Number(e.currentTarget.value || '0') } })}
        />
      );
    }
    if (field.type === 'multiline_text') {
      return (
        <Textarea
          key={field.key}
          label={field.label}
          minRows={4}
          autosize
          value={typeof value === 'string' ? value : ''}
          onChange={(e) => updateStep(step.id, { fields: { ...step.fields, [field.key]: e.currentTarget.value } })}
        />
      );
    }
    return (
      <TextInput
        key={field.key}
        label={field.label}
        value={typeof value === 'string' ? value : ''}
        onChange={(e) => updateStep(step.id, { fields: { ...step.fields, [field.key]: e.currentTarget.value } })}
      />
    );
  }

  return (
    <AppShell padding="md">
      <AppShell.Main>
        <Stack>
          <Group justify="space-between">
            <div>
              <Title order={2}>Workflow Shell</Title>
              <Text c="dimmed">Build templates declaratively and monitor background workflow runs.</Text>
            </div>
            <Group>
              <Button onClick={() => setView('builder')}>New workflow</Button>
              <Button variant="light" onClick={() => setTemplateModalOpen(true)} disabled={view !== 'builder'}>Save template</Button>
              <Button variant="default" leftSection={<IconRefresh size={16} />} onClick={() => void refreshRunsAndTemplates()}>Refresh</Button>
            </Group>
          </Group>

          {error ? <Alert color="red">{error}</Alert> : null}

          {view === 'builder' ? (
            <Modal opened={view === 'builder'} onClose={() => setView('monitor')} title="Create workflow" size="min(1400px, 96vw)" centered>
              <Stack>
                <SimpleGrid cols={{ base: 1, xl: 2 }}>
                <Card withBorder>
                  <Stack>
                    <Title order={4}>Workflow metadata</Title>
                    <TextInput label="Workflow name" value={workflowName} onChange={(e) => setWorkflowName(e.currentTarget.value)} />
                    <Textarea label="Description" minRows={2} value={workflowDescription} onChange={(e) => setWorkflowDescription(e.currentTarget.value)} />
                    <TextInput label="Run title" value={runTitle} onChange={(e) => setRunTitle(e.currentTarget.value)} />
                    <TextInput label="Repo path" placeholder="C:/repo or /home/user/repo" value={repoRef} onChange={(e) => setRepoRef(e.currentTarget.value)} />
                    <Group>
                      <Button onClick={() => void handleCreateRunFromTemplate(selectedTemplateId)} loading={busy} variant="default">Create run from selected template</Button>
                      <Select style={{ flex: 1 }} label="Existing template" placeholder="Select saved template" data={templates.map((template) => ({ value: template.id, label: template.name }))} value={selectedTemplateId} onChange={setSelectedTemplateId} searchable clearable />
                    </Group>
                  </Stack>
                </Card>

                <Card withBorder>
                  <Stack>
                    <Group justify="space-between">
                      <Title order={4}>Builder mode</Title>
                      <SegmentedControl value={builderMode} onChange={(value) => setBuilderMode(value as BuilderMode)} data={[{ label: 'Visual', value: 'builder' }, { label: 'JSON', value: 'json' }]} />
                    </Group>
                    <Text c="dimmed" size="sm">Visual mode edits stages and transitions directly. JSON mode lets you inspect or override the template definition sent to the backend.</Text>
                    {builderMode === 'json' ? (
                      <JsonInput autosize minRows={20} value={jsonDraft} onChange={setJsonDraft} formatOnBlur />
                    ) : (
                      <Stack gap="md">
                        {builderSteps.map((step, index) => (
                          <Card withBorder key={step.id}>
                            <Stack>
                              <Group justify="space-between">
                                <Group>
                                  <Badge variant="light">{index + 1}</Badge>
                                  <Text fw={600}>{step.name}</Text>
                                  <Code>{step.id}</Code>
                                </Group>
                                <Badge color={step.automationMode === 'automatic' ? 'blue' : 'gray'}>{step.automationMode}</Badge>
                              </Group>
                              <Grid>
                                <Grid.Col span={{ base: 12, md: 6 }}>
                                  <TextInput label="Name" value={step.name} onChange={(e) => updateStep(step.id, { name: e.currentTarget.value })} />
                                </Grid.Col>
                                <Grid.Col span={{ base: 12, md: 6 }}>
                                  <Select label="Automation" value={step.automationMode} onChange={(value) => value && updateStep(step.id, { automationMode: value as AutomationMode })} data={[{ value: 'manual', label: 'manual' }, { value: 'assisted', label: 'assisted' }, { value: 'automatic', label: 'automatic' }]} />
                                </Grid.Col>
                              </Grid>
                              <SimpleGrid cols={{ base: 1, md: 2 }}>
                                {(stageContractByType.get(step.stepType)?.fields ?? []).map((field) => renderBuilderField(step, field))}
                                <Switch label="Auto-advance on success" checked={step.autoAdvanceOnSuccess} onChange={(e) => updateStep(step.id, { autoAdvanceOnSuccess: e.currentTarget.checked })} />
                              </SimpleGrid>
                              <Divider label="Transitions" />
                              <SimpleGrid cols={{ base: 1, md: 3 }}>
                                <Select label="On success →" value={step.transitions.successTarget} data={[{ value: '', label: 'End workflow' }, ...stepOptions]} onChange={(value) => updateStepTransitions(step.id, { successTarget: value ?? '' })} />
                                <Select label="On error →" value={step.transitions.errorTarget} data={[{ value: '', label: 'Stop on error' }, ...stepOptions]} onChange={(value) => updateStepTransitions(step.id, { errorTarget: value ?? '' })} />
                                <Select label="On paused →" value={step.transitions.pausedTarget} data={[{ value: '', label: 'Wait on current step' }, ...stepOptions]} onChange={(value) => updateStepTransitions(step.id, { pausedTarget: value ?? '' })} />
                              </SimpleGrid>
                            </Stack>
                          </Card>
                        ))}
                      </Stack>
                    )}
                  </Stack>
                </Card>
                </SimpleGrid>
                <Group justify="flex-end">
                  <Button variant="default" onClick={() => setView('monitor')}>Close</Button>
                  <Button variant="light" onClick={() => setTemplateModalOpen(true)}>Save template</Button>
                </Group>
              </Stack>
            </Modal>
          ) : monitorView === 'workflow_list' ? (
            <Stack>
              <Card withBorder>
                <Stack>
                  <Group justify="space-between">
                    <Title order={4}>Workflow list</Title>
                    <Text c="dimmed" size="sm">Open a workflow to inspect logs and control execution.</Text>
                  </Group>
                  <Table striped highlightOnHover>
                    <Table.Thead>
                      <Table.Tr>
                        <Table.Th>Workflow</Table.Th>
                        <Table.Th>Status</Table.Th>
                        <Table.Th>Current step</Table.Th>
                        <Table.Th>Repo</Table.Th>
                        <Table.Th>Updated</Table.Th>
                        <Table.Th>Actions</Table.Th>
                      </Table.Tr>
                    </Table.Thead>
                    <Table.Tbody>
                      {runs.map((run) => (
                        <Table.Tr key={run.id} onClick={() => void openWorkflow(run.id)} style={{ cursor: 'pointer' }}>
                          <Table.Td>{run.title}</Table.Td>
                          <Table.Td><Badge color={statusColor(run.status)}>{run.status}</Badge></Table.Td>
                          <Table.Td><Code>{run.current_step_id ?? '—'}</Code></Table.Td>
                          <Table.Td><Code>{run.repo_ref}</Code></Table.Td>
                          <Table.Td>{formatTimestamp(run.updated_at)}</Table.Td>
                          <Table.Td>
                            <Group gap="xs">
                              <Button size="xs" variant="light" onClick={(e) => { e.stopPropagation(); void openWorkflow(run.id); }}>Open</Button>
                              <ActionIcon color="red" variant="subtle" onClick={(e) => { e.stopPropagation(); void handleDeleteRun(run.id); }}><IconTrash size={16} /></ActionIcon>
                            </Group>
                          </Table.Td>
                        </Table.Tr>
                      ))}
                    </Table.Tbody>
                  </Table>
                </Stack>
              </Card>

              <Card withBorder>
                <Stack>
                  <Group justify="space-between">
                    <Title order={4}>Global summary</Title>
                    <Button variant="light" size="xs" onClick={() => void refreshRunsAndTemplates()}>Refresh summary</Button>
                  </Group>
                  {Object.keys(allWorkflowEvents).length === 0 ? (
                    <Text c="dimmed">No active workflow summaries yet.</Text>
                  ) : (
                    <Stack>
                      {runs.filter((run) => allWorkflowEvents[run.id]?.length).map((run) => {
                        const latestEvent = allWorkflowEvents[run.id][allWorkflowEvents[run.id].length - 1] ?? null;
                        return (
                          <Card key={run.id} withBorder>
                            <Group justify="space-between" align="flex-start">
                              <Stack gap={4}>
                                <Text fw={600}>{run.title}</Text>
                                <Text size="sm" c="dimmed">{run.repo_ref}</Text>
                                <Group gap="xs">
                                  <Badge color={statusColor(run.status)}>{run.status}</Badge>
                                  <Code>{run.current_step_id ?? '-'}</Code>
                                </Group>
                              </Stack>
                              <Stack gap={4} align="flex-end">
                                <Text size="xs" c="dimmed">{latestEvent ? formatTimestamp(latestEvent.created_at) : '-'}</Text>
                                <Text size="sm">{latestEvent ? summarizeEvent(latestEvent) : 'No events'}</Text>
                              </Stack>
                            </Group>
                          </Card>
                        );
                      })}
                    </Stack>
                  )}
                </Stack>
              </Card>
            </Stack>
          ) : (
            <Grid align="start">
              <Grid.Col span={{ base: 12, xl: 7 }}>
                <Stack>
                  <Card withBorder>
                    {selectedRun ? (
                      <Stack>
                        <Group justify="space-between">
                          <Group>
                            <Button variant="light" onClick={backToWorkflowList}>Back to workflows</Button>
                            <div>
                              <Title order={4}>{selectedRun.title}</Title>
                              <Text c="dimmed">{selectedRun.repo_ref}</Text>
                            </div>
                          </Group>
                          <Badge color={statusColor(selectedRun.status)}>{selectedRun.status}</Badge>
                        </Group>
                        <Stack gap="md">
                          <Group>
                            <Button leftSection={<IconPlayerPlay size={16} />} onClick={() => void handleStartRun()} loading={busy}>Start</Button>
                            <Button variant="light" leftSection={<IconPlayerPlay size={16} />} onClick={() => void handleResumeRun()} loading={busy}>Resume</Button>
                            <Button variant="default" leftSection={<IconPlayerPause size={16} />} onClick={() => void handlePauseRun()} loading={busy}>Pause</Button>
                            <Button variant="default" leftSection={<IconRefresh size={16} />} onClick={() => selectedRunId && void refreshRunDetails(selectedRunId)}>Refresh run</Button>
                          </Group>
                          <Card withBorder>
                            <Stack gap="md">
                              <Group justify="space-between" align="center">
                                <Title order={6}>Workflow controls</Title>
                                <SegmentedControl value={operatorMode} onChange={(value) => setOperatorMode(value as OperatorMode)} data={[{ label: 'Auto', value: 'auto' }, { label: 'Manual', value: 'manual' }]} />
                              </Group>
                              <SimpleGrid cols={{ base: 1, md: 3 }}>
                                <Select label="Selected step" data={selectedRunTemplate?.definition.steps.map((step) => ({ value: step.id, label: `${step.name} (${step.id})` })) ?? []} value={selectedStepId} onChange={(value) => setSelectedStepId(value)} clearable={false} />
                                <Stack gap="xs">
                                  <Text size="sm">Current template step</Text>
                                  <Code>{selectedWorkflowStep?.id ?? selectedRun?.current_step_id ?? '-'}</Code>
                                </Stack>
                                <Stack gap="xs">
                                  <Text size="sm">Automation</Text>
                                  <Badge>{selectedWorkflowStep?.automation_mode ?? '-'}</Badge>
                                </Stack>
                              </SimpleGrid>
                              <Group>
                                <Button variant="default" onClick={() => void handleManualPreviousStep()} disabled={operatorMode !== 'manual' || manualCapabilityBusy}>Previous step</Button>
                                <Button variant="default" onClick={() => selectedStepId && void handleManualSelectStep(selectedStepId)} disabled={operatorMode !== 'manual' || !selectedStepId || manualCapabilityBusy}>Select step</Button>
                                <Button variant="default" onClick={() => void handleManualNextStep()} disabled={operatorMode !== 'manual' || manualCapabilityBusy}>Next step</Button>
                                <Button onClick={() => void handleManualRunCurrentStep()} disabled={operatorMode !== 'manual' || manualCapabilityBusy} loading={manualCapabilityBusy}>Run current step</Button>
                              </Group>
                            </Stack>
                          </Card>
                        </Stack>
                        <Card withBorder>
                          <Stack gap="md">
                            <Group justify="space-between" align="flex-start">
                              <Stack gap="xs">
                                <Text fw={600}>Run overview</Text>
                                <Group gap="xs" wrap="wrap">
                                  <Badge color={statusColor(selectedRun.status)}>{selectedRun.status}</Badge>
                                  <Badge variant="light">Current: {selectedRun.current_step_id ?? '-'}</Badge>
                                  <Badge variant="light">Polling: {(selectedRun.status === 'queued' || selectedRun.status === 'running') ? 'active' : 'idle'}</Badge>
                                </Group>
                              </Stack>
                              <Code>{selectedRun.id}</Code>
                            </Group>
                            <SimpleGrid cols={{ base: 1, md: 3 }}>
                              <Box>
                                <Text size="xs" c="dimmed">Template</Text>
                                <Text size="sm"><Code>{selectedRun.template_id ?? '-'}</Code></Text>
                              </Box>
                              <Box>
                                <Text size="xs" c="dimmed">Created</Text>
                                <Text size="sm">{formatTimestamp(selectedRun.created_at)}</Text>
                              </Box>
                              <Box>
                                <Text size="xs" c="dimmed">Updated</Text>
                                <Text size="sm">{formatTimestamp(selectedRun.updated_at)}</Text>
                              </Box>
                            </SimpleGrid>
                            <Divider />
                            <Stack gap="md">
                              <Text fw={600}>Workflow progress</Text>
                              {selectedRunTemplate ? (
                                <Group gap="sm" wrap="wrap" align="stretch">
                                  {selectedRunTemplate.definition.steps.map((step, index) => {
                                    const isCurrent = selectedRun?.current_step_id === step.id;
                                    const currentIndex = selectedRunTemplate.definition.steps.findIndex((item) => item.id === selectedRun?.current_step_id);
                                    const isCompleted = currentIndex >= 0 && index < currentIndex;
                                    const color = isCurrent ? 'blue' : isCompleted ? 'green' : 'gray';
                                    return (
                                      <Group key={step.id} gap="sm" wrap="nowrap" align="center">
                                        <Box
                                          p="md"
                                          style={{
                                            minWidth: 180,
                                            borderRadius: 12,
                                            border: `1px solid var(--mantine-color-${color}-6)`,
                                            background: isCurrent
                                              ? 'rgba(34, 139, 230, 0.14)'
                                              : isCompleted
                                                ? 'rgba(64, 192, 87, 0.12)'
                                                : 'rgba(255,255,255,0.02)'
                                          }}
                                        >
                                          <Stack gap={6}>
                                            <Badge color={color} variant={isCurrent ? 'filled' : 'light'} style={{ alignSelf: 'flex-start' }}>
                                              {index + 1}
                                            </Badge>
                                            <Text fw={600}>{step.name}</Text>
                                            <Text size="xs" c="dimmed">{step.automation_mode}</Text>
                                            <Badge color={color} variant={isCurrent ? 'filled' : 'light'} style={{ alignSelf: 'flex-start' }}>
                                              {isCurrent ? 'ACTIVE' : isCompleted ? 'DONE' : 'UP NEXT'}
                                            </Badge>
                                          </Stack>
                                        </Box>
                                        {index < selectedRunTemplate.definition.steps.length - 1 ? <Text c="dimmed" fw={700}>→</Text> : null}
                                      </Group>
                                    );
                                  })}
                                </Group>
                              ) : (
                                <Text c="dimmed">The selected run is not linked to a loaded template.</Text>
                              )}
                            </Stack>
                          </Stack>
                        </Card>
                      </Stack>
                    ) : (
                      <Text c="dimmed">No workflow selected.</Text>
                    )}
                  </Card>

                  <Card withBorder>
                    <Stack>
                      <Grid align="stretch">
                        <Grid.Col span={{ base: 12, xl: 4 }}>
                          <Stack>
                            {selectedWorkflowStep?.id === 'design' || selectedWorkflowStep?.id === 'code' ? (
                              <Card withBorder>
                                <InferenceConnectionCard
                                  inferenceConnectionStatus={inferenceConnectionStatus}
                                  inferenceReady={inferenceReady}
                                  inferenceSummaryText={inferenceSummaryText}
                                  inferenceTransport={inferenceTransport}
                                  inferenceModel={inferenceModel}
                                  browserTargetUrl={browserTargetUrl}
                                  browserCdpUrl={browserCdpUrl}
                                  inferenceBusy={inferenceBusy}
                                  inferenceStatus={inferenceStatus}
                                  inferenceConfigOpen={inferenceConfigOpen}
                                  onOpenConfig={() => setInferenceConfigOpen(true)}
                                  onCloseConfig={() => setInferenceConfigOpen(false)}
                                  onTransportChange={(value) => setInferenceTransport(value)}
                                  onModelChange={setInferenceModel}
                                  onBrowserTargetUrlChange={setBrowserTargetUrl}
                                  onBrowserCdpUrlChange={setBrowserCdpUrl}
                                  onSaveConfig={() => void configureInference()}
                                />
                              </Card>
                            ) : null}

                            {selectedWorkflowStep?.id === 'compile' || !inferenceRequiresConnection || inferenceReady ? (
                              <>
                                {selectedWorkflowStep?.id === 'design' ? (
                                  <DesignStageInputsPanel
                                    stageUserInput={stageUserInput}
                                    stageIncludeRepoContext={stageIncludeRepoContext}
                                    repoFragmentSummary={repoFragmentSummary}
                                    onStageUserInputChange={setStageUserInput}
                                    onStageIncludeRepoContextChange={setStageIncludeRepoContext}
                                    onOpenRepoConfig={() => setRepoContextConfigOpen(true)}
                                  />
                                ) : selectedWorkflowStep?.id === 'code' ? (
                                  <CodeStageInputsPanel
                                    stageUserInput={stageUserInput}
                                    stageIncludeRepoContext={stageIncludeRepoContext}
                                    stageIncludeChangesetSchema={stageIncludeChangesetSchema}
                                    stageAutoApplyChangeset={stageAutoApplyChangeset}
                                    stageIncludeApplyError={stageIncludeApplyError}
                                    stageApplyError={stageApplyError}
                                    stageIncludeCompileError={stageIncludeCompileError}
                                    stageCompileError={stageCompileError}
                                    repoFragmentSummary={repoFragmentSummary}
                                    onStageUserInputChange={setStageUserInput}
                                    onStageIncludeRepoContextChange={setStageIncludeRepoContext}
                                    onStageIncludeChangesetSchemaChange={setStageIncludeChangesetSchema}
                                    onStageAutoApplyChangesetChange={setStageAutoApplyChangeset}
                                    onStageIncludeApplyErrorChange={setStageIncludeApplyError}
                                    onStageIncludeCompileErrorChange={setStageIncludeCompileError}
                                    onOpenRepoConfig={() => setRepoContextConfigOpen(true)}
                                    onOpenSchemaConfig={() => setChangesetSchemaConfigOpen(true)}
                                    onOpenApplyErrorConfig={() => setApplyErrorConfigOpen(true)}
                                    onOpenCompileErrorConfig={() => setCompileErrorConfigOpen(true)}
                                  />
                                ) : selectedWorkflowStep?.id === 'compile' ? (
                                  <CompileStageInputsPanel
                                    stageCompileCommandsText={stageCompileCommandsText}
                                    onStageCompileCommandsTextChange={setStageCompileCommandsText}
                                  />
                                ) : (
                                  <ReviewStageInputsPanel
                                    stageReviewNotes={stageReviewNotes}
                                    onStageReviewNotesChange={setStageReviewNotes}
                                  />
                                )}

                                <Group>
                                  <Button variant="default" onClick={() => void handleManualPatchStageState()} disabled={operatorMode !== 'manual' || !selectedStepId || manualCapabilityBusy}>Save stage inputs</Button>
                                  <Button size="md" onClick={() => void handleManualRunWithPatchedState()} disabled={operatorMode !== 'manual' || !selectedStepId || manualCapabilityBusy} loading={manualCapabilityBusy}>Run stage</Button>
                                  <Button variant="light" onClick={() => setRunContextOpen(true)} disabled={!selectedRun}>View run context</Button>
                                  <Button variant="light" onClick={() => setGlobalCapabilitiesOpen(true)} disabled={!selectedRun}>Global capabilities</Button>
                                </Group>
                              </>
                            ) : null}
                          </Stack>
                        </Grid.Col>
                        <Grid.Col span={{ base: 12, xl: 8 }} style={{ display: 'flex' }}>
                          <Box style={{ flex: 1 }}>
                            {showStageStream ? <StageStreamPanel renderStageStreamPanel={renderStageStreamPanel} /> : null}
                          </Box>
                        </Grid.Col>
                      </Grid>


                      {manualCapabilityStatus ? <Alert color={manualCapabilityStatus.toLowerCase().includes('error') ? 'red' : 'blue'}>{manualCapabilityStatus}</Alert> : null}
                    </Stack>
                  </Card>
                  </Stack>
                </Grid.Col>

                <Grid.Col span={{ base: 12, xl: 5 }}>
                  <Card withBorder style={{ height: '100%' }}>
                    <Stack h="100%">
                      <Group justify="space-between">
                        <Group gap="xs">
                          <Title order={5}>Live workflow events</Title>
                          <Badge color={eventStreamStatus.color} variant="light">Stream {eventStreamStatus.label}</Badge>
                        </Group>
                        <Button variant="light" size="xs" onClick={() => selectedRunId && void refreshLiveMonitor(selectedRunId)}>Refresh events</Button>
                      </Group>
                      {liveExecutionTrails.length > 0 ? (
                        <Stack gap="xs">
                          {liveExecutionTrails.map((trail, index) => {
                            const trailExpanded = isLiveExecutionExpanded(trail);
                            const executionState = liveExecutionChains[trail.key] ?? { loading: false, error: null, chain: null, latestCreatedAt: null };
                            const rawEvents = (executionState.chain?.items ?? []).slice().sort((a, b) => b.sequence_no - a.sequence_no);
                            return (
                              <Box
                                key={trail.key}
                                p="sm"
                                style={{
                                  border: `1px solid var(--mantine-color-${liveStageTone(trail)}-4)`,
                                  borderRadius: 10,
                                  background: trail.isCurrent
                                    ? 'rgba(34, 139, 230, 0.08)'
                                    : trail.isActive
                                      ? 'rgba(250, 176, 5, 0.08)'
                                      : liveStageTone(trail) === 'green'
                                        ? 'rgba(64, 192, 87, 0.08)'
                                        : liveStageTone(trail) === 'red'
                                          ? 'rgba(250, 82, 82, 0.08)'
                                          : liveStageTone(trail) === 'yellow'
                                            ? 'rgba(250, 176, 5, 0.08)'
                                            : 'rgba(255,255,255,0.02)'
                                }}
                              >
                                <Group justify="space-between" align="center" wrap="nowrap">
                                  <Group gap="xs" wrap="wrap" style={{ flex: 1 }}>
                                    <Badge color={trail.isCurrent ? 'blue' : trail.isActive ? 'yellow' : liveStageTone(trail)}>
                                      {trail.isCurrent ? 'RUNNING' : trail.isActive ? 'ACTIVE' : liveStageTone(trail) === 'red' ? 'FAILED' : liveStageTone(trail) === 'yellow' ? 'WARN' : 'COMPLETE'}
                                    </Badge>
                                    <Badge variant="light">{trail.stepId !== '__ungrouped__' ? trail.stepId : trail.label}</Badge>
                                  </Group>
                                  <Group gap="md" align="center" wrap="nowrap">
                                    <Stack gap={0} align="flex-end">
                                      <Text size="sm" fw={600}>{formatTimestamp(trail.latestCreatedAt)}</Text>
                                      <Text size="sm" fw={600}>{formatDurationMs(trail.durationMs, null, null)}</Text>
                                    </Stack>
                                    <Button
                                      size="xs"
                                      variant="subtle"
                                      onClick={() => {
                                        toggleLiveExecutionExpanded(trail);
                                        if (!trailExpanded) {
                                          void ensureLiveExecutionChainLoaded(trail);
                                        }
                                      }}
                                    >
                                      {trailExpanded ? 'Collapse execution' : 'Expand execution'}
                                    </Button>
                                  </Group>
                                </Group>
                                {trailExpanded ? (
                                  <Stack gap="xs" mt="sm">
                                    <Divider label="Capabilities" labelPosition="left" />

                                    {executionState.loading ? <Loader size="sm" /> : null}
                                    {executionState.error ? <Alert color="red">{executionState.error}</Alert> : null}
                                    {!executionState.loading && !executionState.error && rawEvents.length === 0 ? (
                                      <Text size="sm" c="dimmed">No execution events loaded.</Text>
                                    ) : null}

                                    {(() => {
                                      const capabilityCards = buildLiveCapabilitiesFromEvents(trail, rawEvents);
                                      if (!executionState.loading && !executionState.error && capabilityCards.length === 0) {
                                        return <Text size="sm" c="dimmed">No capability executions found.</Text>;
                                      }
                                      return capabilityCards.map((capability) => {
                                        const eventExpanded = expandedLiveEventIds.has(capability.key);
                                        return (
                                          <Box
                                            key={capability.key}
                                            p="sm"
                                            style={{
                                              ...livePulseStyle(capability.isActive, capability.isNew),
                                              border: `1px solid var(--mantine-color-${capabilityTone(capability)}-4)`,
                                              borderRadius: 8,
                                              background: capability.isActive
                                                ? 'rgba(34, 139, 230, 0.08)'
                                                : capabilityTone(capability) === 'green'
                                                  ? 'rgba(64, 192, 87, 0.08)'
                                                  : capabilityTone(capability) === 'red'
                                                    ? 'rgba(250, 82, 82, 0.08)'
                                                    : capabilityTone(capability) === 'yellow'
                                                      ? 'rgba(250, 176, 5, 0.08)'
                                                      : 'rgba(255,255,255,0.02)'
                                            }}
                                          >
                                            <Box style={liveProgressBar(capability.isActive, capabilityTone(capability))} />
                                            <Group justify="space-between" align="flex-start" wrap="nowrap" style={{ position: 'relative', zIndex: 1 }}>
                                              <Group align="flex-start" justify="space-between" wrap="nowrap" style={{ flex: 1 }}>
                                                <Stack gap={4} style={{ flex: 1 }}>
                                                  <Group gap="xs" wrap="wrap">
                                                    <Badge color={capabilityTone(capability)}>{capability.statusLabel}</Badge>
                                                    <Badge variant="light">{capability.name}</Badge>
                                                    <Text size="xs" c="dimmed">events {capability.eventCount}</Text>
                                                  </Group>
                                                  <Text size="sm">{capability.message}</Text>
                                                </Stack>
                                                <Stack gap={2} align="flex-end" style={{ minWidth: 220 }}>
                                                  <Text size="sm" fw={600}>{capability.startedAtText}</Text>
                                                  <Text size="sm" fw={600}>{capability.isActive ? formatDuration(capability.startedAtRaw ?? capability.latestCreatedAt, new Date(liveNow).toISOString()) : capability.durationText}</Text>
                                                </Stack>
                                              </Group>
                                              <Button size="xs" variant="subtle" onClick={() => toggleLiveEventExpanded(capability.key)}>
                                                {eventExpanded ? 'Hide raw JSON' : 'Show raw JSON'}
                                              </Button>
                                            </Group>
                                            {eventExpanded ? (
                                              <ScrollArea mt="sm" offsetScrollbars>
                                                <Code block>{JSON.stringify(capabilityIoPayload(capability), null, 2)}</Code>
                                              </ScrollArea>
                                            ) : null}
                                          </Box>
                                        );
                                      });
                                    })()}
                                  </Stack>
                                ) : null}
                              </Box>
                            );
                          })}
                        </Stack>
                      ) : null}
                      {liveExecutionTrails.length === 0 ? (
                        <Text c="dimmed">No live executions yet.</Text>
                      ) : null}
                    </Stack>
                  </Card>
                </Grid.Col>
              </Grid>
          )}
        </Stack>

        <Modal opened={repoContextConfigOpen} onClose={() => setRepoContextConfigOpen(false)} title="Repo fragment" size="80%" centered>
          <Stack>
            <TextInput label="Git ref" value={stageRepoContextGitRef} onChange={(e) => setStageRepoContextGitRef(e.currentTarget.value)} placeholder="WORKTREE" />
            <TextInput label="Save path" value={stageRepoContextSavePath} onChange={(e) => setStageRepoContextSavePath(e.currentTarget.value)} placeholder="/tmp/repo_context.txt" />
            <SimpleGrid cols={{ base: 1, md: 2 }}>
              <Switch label="Skip binary" checked={stageRepoContextSkipBinary} onChange={(e) => setStageRepoContextSkipBinary(e.currentTarget.checked)} />
              <Switch label="Skip .gitignore" checked={stageRepoContextSkipGitignore} onChange={(e) => setStageRepoContextSkipGitignore(e.currentTarget.checked)} />
              <Switch label="Include staged diff" checked={stageRepoContextIncludeStagedDiff} onChange={(e) => setStageRepoContextIncludeStagedDiff(e.currentTarget.checked)} />
              <Switch label="Include unstaged diff" checked={stageRepoContextIncludeUnstagedDiff} onChange={(e) => setStageRepoContextIncludeUnstagedDiff(e.currentTarget.checked)} />
            </SimpleGrid>
            <Group justify="space-between">
              <Group>
                <Button size="xs" variant="light" onClick={() => { if (selectedRun) void loadTreeDir(selectedRun, '', true); }} disabled={!selectedRun}>
                  Refresh tree
                </Button>
                <Button size="xs" variant="light" onClick={() => { syncRepoSelectionState([]); setSelectedRepoDirs(new Set()); }}>
                  Clear selection
                </Button>
                <Button size="xs" variant="light" onClick={() => {
                  const allVisibleFiles = collectLoadedFilePaths('', treeChildrenByParent);
                  setSelectedRepoDirs(new Set(rootTreeEntries.filter((entry) => entry.kind === 'dir').map((entry) => entry.path)));
                  setPaths(allVisibleFiles, true);
                }}>
                  Select loaded files
                </Button>
              </Group>
              <Text size="sm">Selected files: <Code>{selectedRepoPaths.length}</Code></Text>
            </Group>
            {treeError ? <Alert color="red">{treeError}</Alert> : null}
            {treeRootData ? <Text size="sm" c="dimmed">Refreshed {treeRootData.refreshed_at}</Text> : null}
            {treeBusy && !treeRootData ? (
              <Group><Loader size="sm" /><Text size="sm">Scanning repository…</Text></Group>
            ) : (
              <RepoTree
                rootEntries={rootTreeEntries}
                childrenByParent={treeChildrenByParent}
                loadingDirs={loadingTreeDirs}
                selected={selectedRepoPathSet}
                selectedDirs={selectedRepoDirs}
                onLoadDir={(path) => {
                  if (selectedRun) {
                    void loadTreeDir(selectedRun, path, false);
                  }
                }}
                onToggleFile={toggleFile}
                onToggleDir={(entry, checked) => {
                  void toggleDirectory(entry, checked);
                }}
                onSetPaths={setPaths}
                height={360}
              />
            )}
            <Textarea label="Include files" minRows={8} value={stageRepoContextIncludeFilesText} onChange={(e) => {
              const value = e.currentTarget.value;
              syncRepoSelectionState(value.split('\n').map((item) => item.trim()).filter(Boolean));
            }} placeholder={"src/main.rs\nsrc/lib.rs"} />
            <Textarea label="Exclude regex" minRows={6} value={stageRepoContextExcludeRegexText} onChange={(e) => setStageRepoContextExcludeRegexText(e.currentTarget.value)} placeholder={"target/.*\nnode_modules/.*"} />
            <Group justify="flex-end"><Button size="xs" onClick={() => setRepoContextConfigOpen(false)}>Done</Button></Group>
          </Stack>
        </Modal>

        <Modal opened={changesetSchemaConfigOpen} onClose={() => setChangesetSchemaConfigOpen(false)} title="Changeset schema fragment" size="70%" centered>
          <Stack>
            <Group justify="space-between">
              <Text size="sm" c="dimmed">Use the canonical changeset schema example from the backend capability or paste custom guidance.</Text>
              <Button size="xs" variant="light" loading={changesetSchemaBusy} onClick={() => void loadCanonicalChangesetSchema(true)}>
                Reload from API
              </Button>
            </Group>
            <Textarea label="Changeset schema guidance" minRows={18} value={stageChangesetSchemaText} onChange={(e) => setStageChangesetSchemaText(e.currentTarget.value)} placeholder="Canonical backend changeset schema will populate here by default; you can override it." />
            <Group justify="flex-end"><Button size="xs" onClick={() => setChangesetSchemaConfigOpen(false)}>Done</Button></Group>
          </Stack>
        </Modal>

        <Modal opened={applyErrorConfigOpen} onClose={() => setApplyErrorConfigOpen(false)} title="Apply error fragment" size="70%" centered>
          <Stack>
            <Textarea label="Apply error" minRows={12} value={stageApplyError} onChange={(e) => setStageApplyError(e.currentTarget.value)} placeholder="Paste apply failures for the next retry prompt" />
            <Group justify="flex-end"><Button size="xs" onClick={() => setApplyErrorConfigOpen(false)}>Done</Button></Group>
          </Stack>
        </Modal>

        <Modal opened={compileErrorConfigOpen} onClose={() => setCompileErrorConfigOpen(false)} title="Compile error fragment" size="70%" centered>
          <Stack>
            <Textarea label="Compile error" minRows={12} value={stageCompileError} onChange={(e) => setStageCompileError(e.currentTarget.value)} placeholder="Compile failures persisted by the backend for the next code retry prompt" />
            <Group justify="flex-end"><Button size="xs" onClick={() => setCompileErrorConfigOpen(false)}>Done</Button></Group>
          </Stack>
        </Modal>


        <Modal opened={responseViewerOpen} onClose={() => setResponseViewerOpen(false)} title={previewViewerMode === 'stream' ? 'Stage stream' : previewViewerMode === 'prompt' ? 'Composed prompt preview' : 'Inference response'} size="min(1200px, 96vw)" centered>
          <Stack gap="md">
            <Group justify="space-between" align="center">
              <Group gap="xs">
                <Badge variant="light">{(previewViewerMode === 'stream' ? stageStreamContent : previewViewerMode === 'prompt' ? composedInferencePrompt : inferenceResponse) ? `${(previewViewerMode === 'stream' ? stageStreamContent : previewViewerMode === 'prompt' ? composedInferencePrompt : inferenceResponse).length.toLocaleString()} chars` : 'empty'}</Badge>
                <Text size="sm" c="dimmed">Wrapped and formatted for review</Text>
              </Group>
              <Button size="xs" variant="light" onClick={() => { void navigator.clipboard.writeText(previewViewerMode === 'stream' ? stageStreamContent : previewViewerMode === 'prompt' ? composedInferencePrompt : inferenceResponse); }} disabled={!(previewViewerMode === 'stream' ? stageStreamContent : previewViewerMode === 'prompt' ? composedInferencePrompt : inferenceResponse).trim()}>
                {previewViewerMode === 'stream' ? 'Copy stream' : previewViewerMode === 'prompt' ? 'Copy prompt' : 'Copy response'}
              </Button>
            </Group>
            <Box p="lg" style={{ border: '1px solid var(--mantine-color-dark-4)', borderRadius: 12, background: 'linear-gradient(180deg, rgba(255,255,255,0.02), rgba(255,255,255,0.01))' }}>
              <ScrollArea h="82vh" offsetScrollbars>
                <Box maw={920} mx="auto">
                  <Text size="sm" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', wordBreak: 'break-word', lineHeight: 1.8, letterSpacing: '0.01em', fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, Liberation Mono, monospace' }}>
                    {previewViewerMode === 'stream' ? (stageStreamContent || 'No stage stream yet.') : previewViewerMode === 'prompt' ? (composedInferencePrompt || 'No prompt fragments enabled yet.') : (inferenceResponse || 'No inference response yet.')}
                  </Text>
                </Box>
              </ScrollArea>
            </Box>
          </Stack>
        </Modal>

        <Modal opened={globalCapabilitiesOpen} onClose={() => setGlobalCapabilitiesOpen(false)} title="Global capabilities" size="min(1000px, 96vw)" centered>
          <GlobalCapabilitiesPanel
            value={((selectedRun?.context?.workflow_engine as Record<string, unknown> | undefined)?.global_state as Record<string, unknown> | undefined) ?? {}}
            busy={busy}
            onSave={async (payload) => {
              if (!selectedRun?.id) return;
              await patchWorkflowGlobalState(selectedRun.id, payload);
              await refreshRunDetails(selectedRun.id);
            }}
          />
        </Modal>

        <Modal opened={runContextOpen} onClose={() => setRunContextOpen(false)} title="Run context" size="min(1100px, 96vw)" centered>
          <Stack>
            <JsonInput value={JSON.stringify(selectedRun?.context ?? {}, null, 2)} readOnly autosize minRows={20} />
          </Stack>
        </Modal>

        <Modal opened={templateModalOpen} onClose={() => setTemplateModalOpen(false)} title="Save workflow template" size="lg">
          <Stack>
            <TextInput label="Name" value={workflowName} onChange={(e) => setWorkflowName(e.currentTarget.value)} />
            <Textarea label="Description" minRows={2} value={workflowDescription} onChange={(e) => setWorkflowDescription(e.currentTarget.value)} />
            <Switch label="Create a run immediately after saving" checked={createRunAfterSave} onChange={(e) => setCreateRunAfterSave(e.currentTarget.checked)} />
            <JsonInput value={builderMode === 'json' ? jsonDraft : JSON.stringify(definition, null, 2)} readOnly autosize minRows={12} />
            <Group justify="flex-end">
              <Button variant="default" onClick={() => setTemplateModalOpen(false)}>Cancel</Button>
              <Button onClick={() => void handleSaveTemplate()} loading={busy}>Save template</Button>
            </Group>
          </Stack>
        </Modal>
      </AppShell.Main>
    </AppShell>
  );
}
