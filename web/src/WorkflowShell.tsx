import { Suspense, lazy, memo, useEffect, useMemo, useRef, useState } from 'react';
import {
  ActionIcon,
  Alert,
  AppShell,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  Code,
  Divider,
  Grid,
  Group,
  JsonInput,
  Loader,
  Modal,
  ScrollArea,
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
  applyWorkflowChangeset,
  executeWorkflowCapability,
  createTemplate,
  deleteRun,
  deleteTemplate,
  getEventChainSummary,
  getWorkflowChangeset,
  getChangesetSchema,
  getRun,
  openWorkflowRun,
  getStageExecutionChain,
  getWorkflowBuilderCatalog,
  listRepoTree,
  listWorkflowRepoTree,
  listRunEvents,
  validateRepoRef,
  listRuns,
  listWorkflowChangesets,
  listTemplates,
  openEventStream,
  patchWorkflowGlobalState,
  patchWorkflowStageState,
  pauseWorkflowRun,
  prepareWorkflowStage,
  forceWaitWorkflowRun,
  resumeWorkflowRun,
  resolveWorkflowDispositionReview,
  sapScanExportCandidates,
  sapSearchObjects,
  runCurrentWorkflowStep,
  selectWorkflowStep,
  startWorkflowRun,
  type AutomationMode,
  type BrowserProbeResult,
  type ApplyChangesetResponse,
  type ChangesetAttemptSummary,
  type EventChainSummaryItem,
  type EventChainSummaryResponse,
  type InferenceTransport,
  type RepoTreeResponse,
  type SapExportScanItem,
  type SapSearchObject,
  type StageExecutionChain,
  type StageExecutionEvent,
  type WorkflowBuilderCatalog,
  type WorkflowEvent,
  type WorkflowRun,
  type WorkflowRunStatus,
  type WorkflowStageDescriptor,
  type WorkflowStageField,
  type WorkflowStepDefinition,
  type WorkflowTemplate,
  type WorkflowTemplateDefinition,
  type WorkflowTransition
} from './api';
import { GlobalCapabilitiesPanel } from './GlobalCapabilitiesPanel';
import { InferenceSessionsPanel } from './InferenceSessionsPanel';
import { RepoTree, type RepoTreeEntry } from './RepoTree';
import type { ReviewSourceControlState } from './ReviewDiffViewerPanel';
import { ensureSupervisorPlannerRun, getSupervisorRun, listSupervisorRuns, type FeaturePlanItem, type SupervisorRun } from './supervisor_api';
import { WorkflowBuilderEditor } from './WorkflowBuilderEditor';
import { SupervisorPanel } from './SupervisorPanel';
import { SupervisorPlannerModal } from './SupervisorPlannerModal';
import { defaultGlobals, descriptorMap, flattenStageFields } from './workflow_builder';

const ReviewDiffViewerPanel = lazy(async () => {
  const mod = await import('./ReviewDiffViewerPanel');
  return { default: mod.ReviewDiffViewerPanel };
});

const CommitSummaryPanel = lazy(async () => {
  const mod = await import('./CommitSummaryPanel');
  return { default: mod.CommitSummaryPanel };
});

const RepoMonacoFileEditorPanel = lazy(async () => {
  const mod = await import('./RepoMonacoFileEditorPanel');
  return { default: mod.RepoMonacoFileEditorPanel };
});

function openBuilderCapabilityConfig(
  capabilityKey: string,
  handlers: {
    openRepo: () => void;
    openInference: () => void;
    openSchema: () => void;
    openApplyChangeset: () => void;
    openGitPatchPayload: () => void;
  }
) {
  const normalized = capabilityKey.trim().toLowerCase();

  switch (normalized) {
    case 'context_export':
      handlers.openRepo();
      return;
    case 'inference':
      handlers.openInference();
      return;
    case 'changeset_schema':
      handlers.openSchema();
      return;
    case 'gateway_model/changeset':
    case 'changeset_apply':
    case 'changeset apply':
      handlers.openApplyChangeset();
      return;
    case 'git_patch_payload':
    case 'git patch payload':
    case 'git_patch':
      handlers.openGitPatchPayload();
      return;
    case 'compile_commands':
      return;
    default:
      return;
  }
}


type BuilderMode = 'builder' | 'json';
type ShellView = 'builder' | 'monitor';
type MonitorView = 'workflow_list' | 'workflow_detail';
type MonitorHomeView = 'workflows' | 'supervisors';
type WorkspaceTabKey = 'workflows' | 'supervisor' | 'diff' | 'commits' | 'files' | 'capabilities';
type EventTone = { color: string; label: string };

type InferenceConnectionStatus = { color: string; label: string };

type EventStreamStatus = { color: string; label: string };

type StageModifierAction = {
  key: string;
  label: string;
  status?: string;
  color?: string;
  buttonLabel: string;
  onOpen?: () => void;
  toggleLabel?: string;
  onToggle?: () => void;
  toggleColor?: string;
  helperText?: string;
};


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

function stepUsesCapability(step: WorkflowStepDefinition | null | undefined, capabilityKey: string): boolean {
  if (!step) return false;
  return (step.execution_plan ?? []).some((node) => node.kind === 'capability' && node.enabled !== false && node.key === capabilityKey);
}

function readNestedValue(root: unknown, path: string, fallback: unknown = ''): unknown {
  const parts = path.split('.').filter(Boolean);
  let current: unknown = root;
  for (const part of parts) {
    if (!current || typeof current !== 'object' || !(part in (current as Record<string, unknown>))) {
      return fallback;
    }
    current = (current as Record<string, unknown>)[part];
  }
  return current ?? fallback;
}

function readStringValue(root: unknown, path: string, fallback = ''): string {
  const value = readNestedValue(root, path, fallback);
  return typeof value === 'string' ? value : String(value ?? fallback);
}

function readBooleanValue(root: unknown, path: string, fallback = false): boolean {
  return Boolean(readNestedValue(root, path, fallback));
}


const StageModifierActions = memo(function StageModifierActions(props: {
  actions: StageModifierAction[];
}) {
  const { actions } = props;
  if (actions.length === 0) return null;

  return (
    <Stack gap="xs">
      <Text fw={600} size="sm">Stage tools</Text>
      {actions.map((action) => (
        <Group key={action.key} justify="space-between" wrap="nowrap" align="flex-start" style={{ border: '1px solid var(--mantine-color-dark-4)', borderRadius: 8, padding: 10 }}>
          <Stack gap={4} style={{ minWidth: 0, flex: 1 }}>
            <Group gap="xs" wrap="nowrap">
              <Text size="sm" fw={500}>{action.label}</Text>
              {action.status ? (
                <Badge variant="light" color={action.color ?? 'blue'}>{action.status}</Badge>
              ) : null}
            </Group>
            {action.helperText ? <Text size="xs" c="dimmed">{action.helperText}</Text> : null}
          </Stack>
          <Group gap="xs" wrap="nowrap">
            {action.onToggle ? (
              <Button size="xs" variant="light" color={action.toggleColor ?? 'blue'} onClick={action.onToggle}>
                {action.toggleLabel ?? 'Toggle'}
              </Button>
            ) : null}
            {action.onOpen ? (
              <Button size="xs" variant="light" onClick={action.onOpen}>{action.buttonLabel}</Button>
            ) : null}
          </Group>
        </Group>
      ))}
    </Stack>
  );
});

const BackendDrivenStageInputsPanel = memo(function BackendDrivenStageInputsPanel(props: {
  descriptor: WorkflowStageDescriptor | null;
  selectedWorkflowStep: WorkflowStepDefinition | null;
  repoFragmentSummary: string | null;
  stageApplyError: string;
  stageCompileError: string;
  stageCompileCommandsText: string;
  stageUserInput: string;
  inferenceConnectionStatus: InferenceConnectionStatus;
  inferenceTransport: InferenceTransport;
  sharedInferenceState: Record<string, unknown> | null;
  sharedPlannerFragmentState: Record<string, unknown> | null;
  plannerAvailableForRepo: boolean;
  activePlannerFeatureTitle: string | null;
  stageIncludeRepoContext: boolean;
  stageIncludeChangesetSchema: boolean;
  disabled: boolean;
  onToggleSharedRepoContext: () => void;
  onToggleSharedChangesetSchema: () => void;
  onTogglePlanningFragment: () => void;
  onOpenPlanner: () => void;
  onPatchSelectedStepConfig: (key: string, value: unknown) => void;
  onOpenInferenceConfig: () => void;
  onOpenRepoConfig: () => void;
  onOpenSchemaConfig: () => void;
  onOpenApplyErrorConfig: () => void;
  onOpenCompileErrorConfig: () => void;
  onOpenChanges: () => void;
}) {
  const {
    descriptor,
    selectedWorkflowStep,
    repoFragmentSummary,
    stageApplyError,
    stageCompileError,
    stageCompileCommandsText,
    stageUserInput,
    inferenceConnectionStatus,
    inferenceTransport,
    sharedInferenceState,
    sharedPlannerFragmentState,
    plannerAvailableForRepo,
    activePlannerFeatureTitle,
    stageIncludeRepoContext,
    stageIncludeChangesetSchema,
    disabled,
    onToggleSharedRepoContext,
    onToggleSharedChangesetSchema,
    onTogglePlanningFragment,
    onOpenPlanner,
    onPatchSelectedStepConfig,
    onOpenInferenceConfig,
    onOpenRepoConfig,
    onOpenSchemaConfig,
    onOpenApplyErrorConfig,
    onOpenCompileErrorConfig,
    onOpenChanges
  } = props;

  const fields = useMemo(() => descriptor ? flattenStageFields(descriptor) : [], [descriptor]);
  const [fieldDrafts, setFieldDrafts] = useState<Record<string, unknown>>({});
  const usesInference = stepUsesCapability(selectedWorkflowStep, 'inference');
  const usesRepoContext = !!selectedWorkflowStep && (
    usesInference
    || selectedWorkflowStep.step_type === 'design'
    || selectedWorkflowStep.step_type === 'code'
    || !!selectedWorkflowStep.prompt?.include_repo_context
    || stepUsesCapability(selectedWorkflowStep, 'context_export')
  );
  const usesChangesetSchema = !!selectedWorkflowStep && (
    selectedWorkflowStep.step_type === 'code'
    || !!selectedWorkflowStep.prompt?.include_changeset_schema
  );
  const usesCompileCommands = stepUsesCapability(selectedWorkflowStep, 'compile_commands');
  const designModeDraftValue = fieldDrafts['config.design_mode'];
  const designMode = typeof designModeDraftValue === 'string'
    ? designModeDraftValue
    : readStringValue(selectedWorkflowStep, 'config.design_mode', 'v1');
  const plannerCapabilityState = (sharedPlannerFragmentState ?? {}) as Record<string, unknown>;
  const [plannerSchemaArmedDraft, setPlannerSchemaArmedDraft] = useState<boolean | null>(null);
  const [plannerAutoApplyDraft, setPlannerAutoApplyDraft] = useState<boolean | null>(null);
  const selectedPlannerFeatureId = typeof plannerCapabilityState.selected_feature_id === 'string' && plannerCapabilityState.selected_feature_id.trim()
    ? plannerCapabilityState.selected_feature_id
    : null;
  const fineFeatureFormatArmed = plannerSchemaArmedDraft ?? Boolean(plannerCapabilityState.schema_armed && selectedPlannerFeatureId);
  const autoNormalizeAndApplyToPlanner = plannerAutoApplyDraft ?? Boolean(plannerCapabilityState.auto_apply_armed && selectedPlannerFeatureId);
  const hasBackendPlanningFragment = Boolean(sharedPlannerFragmentState);
  const planningFragmentArmed = Boolean(plannerCapabilityState.fragment_armed && selectedPlannerFeatureId);
  const plannerSupportedStep = selectedWorkflowStep?.step_type === 'design' || selectedWorkflowStep?.step_type === 'code' || selectedWorkflowStep?.step_type === 'review';
  const showPlannerControls = Boolean(hasBackendPlanningFragment || planningFragmentArmed || selectedPlannerFeatureId || (plannerAvailableForRepo && plannerSupportedStep));

  useEffect(() => {
    setPlannerSchemaArmedDraft(null);
    setPlannerAutoApplyDraft(null);
  }, [
    selectedWorkflowStep?.id,
    plannerCapabilityState.schema_armed,
    plannerCapabilityState.auto_apply_armed
  ]);

  const modifierActions = useMemo<StageModifierAction[]>(() => {

    const actions: StageModifierAction[] = [];

    if (usesRepoContext) {
      const repoContextArmed = !!sharedInferenceState?.repo_context_armed;
      actions.push({
        key: 'repo_fragment',
        label: 'Repo fragment',
        buttonLabel: 'Configure',
        onOpen: onOpenRepoConfig,
        toggleLabel: repoContextArmed ? 'Disarm' : 'Arm',
        toggleColor: repoContextArmed ? 'orange' : 'green',
        onToggle: onToggleSharedRepoContext,
        helperText: repoFragmentSummary ?? '0 files selected'
      });
    }

    if (usesChangesetSchema) {
      const changesetSchemaArmed = !!sharedInferenceState?.changeset_schema_armed;
      actions.push({
        key: 'changeset_schema',
        label: 'Schema',
        buttonLabel: 'Configure',
        onOpen: onOpenSchemaConfig,
        toggleLabel: changesetSchemaArmed ? 'Disarm' : 'Arm',
        toggleColor: changesetSchemaArmed ? 'orange' : 'green',
        onToggle: onToggleSharedChangesetSchema,
        helperText: 'Shared global capability surfaced in this stage.'
      });
    }

    if (showPlannerControls) {
      actions.push({
        key: 'planning_fragment',
        label: 'Planner fragment',
        buttonLabel: 'Open planner',
        onOpen: onOpenPlanner,
        toggleLabel: planningFragmentArmed ? 'Disarm' : 'Arm',
        toggleColor: planningFragmentArmed ? 'orange' : 'green',
        onToggle: onTogglePlanningFragment,
        helperText: activePlannerFeatureTitle
          ? `Selected feature: ${activePlannerFeatureTitle}`
          : 'No planner feature selected.'
      });
    }

    if (showPlannerControls && selectedWorkflowStep?.step_type === 'design') {
      actions.push({
        key: 'planner_schema',
        label: 'Planner schema',
        buttonLabel: '',
        toggleLabel: fineFeatureFormatArmed ? 'Disarm' : 'Arm',
        toggleColor: fineFeatureFormatArmed ? 'orange' : 'green',
        onToggle: () => {
          const next = !fineFeatureFormatArmed;
          setPlannerSchemaArmedDraft(next);
          onPatchSelectedStepConfig('capabilities.planner.schema_armed', next);
        },
        helperText: 'Inject planner schema into the next prompt.'
      });

      actions.push({
        key: 'planner_auto_apply',
        label: 'Planner apply',
        buttonLabel: '',
        toggleLabel: autoNormalizeAndApplyToPlanner ? 'Disarm' : 'Arm',
        toggleColor: autoNormalizeAndApplyToPlanner ? 'orange' : 'green',
        onToggle: () => {
          const next = !autoNormalizeAndApplyToPlanner;
          setPlannerAutoApplyDraft(next);
          onPatchSelectedStepConfig('capabilities.planner.auto_apply_armed', next);
        },
        helperText: 'Apply valid design-stage planner output back to the selected planner feature.'
      });
    }

    if (usesInference) {
      actions.push({
        key: 'inference',
        label: 'Inference',
        status: inferenceTransport === 'browser' ? 'Browser' : 'API',
        color: inferenceConnectionStatus.color,
        buttonLabel: 'Configure',
        onOpen: onOpenInferenceConfig
      });
    }

    if (stageApplyError.trim()) {
      actions.push({
        key: 'apply_error',
        label: 'Apply error',
        status: 'Available',
        color: 'orange',
        buttonLabel: 'View',
        onOpen: onOpenApplyErrorConfig
      });
    }

    if (stageCompileError.trim()) {
      actions.push({
        key: 'compile_error',
        label: 'Compile error',
        status: 'Available',
        color: 'yellow',
        buttonLabel: 'View',
        onOpen: onOpenCompileErrorConfig
      });
    }

    return actions;
  }, [
    usesRepoContext,
    repoFragmentSummary,
    sharedInferenceState,
    onOpenRepoConfig,
    onToggleSharedRepoContext,
    usesChangesetSchema,
    onOpenSchemaConfig,
    onToggleSharedChangesetSchema,
    hasBackendPlanningFragment,
    plannerAvailableForRepo,
    showPlannerControls,
    planningFragmentArmed,
    onTogglePlanningFragment,
    onOpenPlanner,
    selectedPlannerFeatureId,
    activePlannerFeatureTitle,
    usesInference,
    designMode,
    fineFeatureFormatArmed,
    autoNormalizeAndApplyToPlanner,
    inferenceConnectionStatus,
    inferenceTransport,
    onOpenInferenceConfig,
    stageApplyError,
    onOpenApplyErrorConfig,
    stageCompileError,
    onOpenCompileErrorConfig
  ]);

  function valueForField(field: WorkflowStageField): unknown {
    if (field.bind_to === 'prompt.user_input') {
      return stageUserInput;
    }

    if (field.bind_to === 'execution.compile_checks.commands_text') {
      return stageCompileCommandsText;
    }

    const parts = field.bind_to.split('.');
    let current: unknown = selectedWorkflowStep ?? {};
    for (const part of parts) {
      if (!current || typeof current !== 'object' || !(part in (current as Record<string, unknown>))) {
        return field.default;
      }
      current = (current as Record<string, unknown>)[part];
    }
    return current ?? field.default;
  }

  useEffect(() => {
    setFieldDrafts(
      Object.fromEntries(
        fields.map((field) => [field.key, valueForField(field)])
      )
    );
  }, [fields, selectedWorkflowStep?.id, stageCompileCommandsText, stageUserInput]);

  function updateField(field: WorkflowStageField, value: unknown) {
    setFieldDrafts((prev) => ({
      ...prev,
      [field.key]: value
    }));
    onPatchSelectedStepConfig(field.bind_to, value);
  }

  function valueAtPath(root: unknown, path: string): unknown {
    return path.split('.').filter(Boolean).reduce<unknown>((cursor, part) => {
      if (cursor && typeof cursor === 'object' && part in cursor) {
        return (cursor as Record<string, unknown>)[part];
      }
      return undefined;
    }, root);
  }

  function fieldVisible(field: WorkflowStageField) {
    return (field.visible_when ?? []).every((condition) => {
      const value = condition.path in fieldDrafts
        ? fieldDrafts[condition.path]
        : valueAtPath(selectedWorkflowStep, condition.path);
      return value === condition.equals;
    });
  }

  function renderField(field: WorkflowStageField) {
    const value = field.key in fieldDrafts ? fieldDrafts[field.key] : valueForField(field);

    if (field.type === 'boolean') {
      return (
        <Switch
          key={field.key}
          label={field.label}
          checked={typeof value === 'boolean' ? value : Boolean(field.default)}
          onChange={(event) => updateField(field, event.currentTarget.checked)}
          disabled={disabled}
        />
      );
    }

    if (field.type === 'integer') {
      return (
        <TextInput
          key={field.key}
          label={field.label}
          value={String(typeof value === 'number' ? value : Number(value ?? field.default ?? 0) || 0)}
          onChange={(event) => updateField(field, Number(event.currentTarget.value || '0'))}
          disabled={disabled}
        />
      );
    }

    if (field.ui?.control === 'select') {
      return (
        <Select
          key={field.key}
          label={field.label}
          description={field.description}
          data={(field.options ?? []).map((option) => ({ value: option.value, label: option.label }))}
          value={typeof value === 'string' ? value : String(field.default ?? '')}
          onChange={(nextValue) => updateField(field, nextValue ?? field.default ?? '')}
          disabled={disabled}
          clearable={!field.required}
        />
      );
    }

    if (field.type === 'multiline_text') {
      return (
        <Textarea
          key={field.key}
          label={field.label}
          description={field.description}
          value={typeof value === 'string' ? value : String(value ?? field.default ?? '')}
          onChange={(event) => updateField(field, event.currentTarget.value)}
          minRows={4}
          autosize
          disabled={disabled}
        />
      );
    }

    return (
      <TextInput
        key={field.key}
        label={field.label}
        description={field.description}
        value={typeof value === 'string' ? value : String(value ?? field.default ?? '')}
        onChange={(event) => updateField(field, event.currentTarget.value)}
        disabled={disabled}
      />
    );
  }

  return (
    <Stack>
      <Title order={6}>{descriptor?.label ?? selectedWorkflowStep?.name ?? 'Stage'} inputs</Title>
      {!descriptor ? (
        <Textarea
          label="User input"
          value={stageUserInput}
          onChange={(event) => onPatchSelectedStepConfig('prompt.user_input', event.currentTarget.value)}
          disabled={disabled}
          minRows={2}
          autosize
        />
      ) : null}
      {descriptor?.editable_fields.map((group) => (
        <Stack key={group.key} gap="xs">
          {descriptor?.editable_fields.length > 1 ? <Text fw={600} size="sm">{group.label}</Text> : null}
          {group.fields.filter((field) => fieldVisible(field)).map((field) => renderField(field))}
        </Stack>
      ))}

      {selectedWorkflowStep?.step_type === 'review' ? (
        <Group>
          <Button variant="light" onClick={onOpenChanges} disabled={disabled}>
            Open changes
          </Button>
        </Group>
      ) : null}
      <StageModifierActions actions={modifierActions} />
    </Stack>
  );
});

const SapImportStageControlsPanel = memo(function SapImportStageControlsPanel(props: {
  status: string | null;
  packageName: string;
  includeSubpackages: boolean;
  includeXmlArtifacts: boolean;
  searchBusy: boolean;
  checkedCount: number;
  onLoad: () => void;
  onApplySelection: () => void;
  onPackageNameChange: (value: string) => void;
  onIncludeSubpackagesChange: (value: boolean) => void;
  onIncludeXmlArtifactsChange: (value: boolean) => void;
}) {
  const {
    status,
    packageName,
    includeSubpackages,
    includeXmlArtifacts,
    searchBusy,
    checkedCount,
    onLoad,
    onApplySelection,
    onPackageNameChange,
    onIncludeSubpackagesChange,
    onIncludeXmlArtifactsChange
  } = props;

  return (
    <Stack>
      <Title order={6}>SAP Import inputs</Title>
      {status ? <Alert color="blue">{status}</Alert> : null}
      <Stack gap="md" style={{ minWidth: 0 }}>
        <Group align="end" wrap="wrap">
          <Button size="xs" variant="default" onClick={onLoad} loading={searchBusy}>
            Load
          </Button>
          <Button size="xs" variant="light" onClick={onApplySelection} disabled={checkedCount === 0}>
            Import selected
          </Button>
        </Group>

        <TextInput
          label="Package"
          value={packageName}
          onChange={(event) => onPackageNameChange(event.currentTarget.value)}
        />

        <Stack gap="xs">
          <Switch
            label="Include subpackages"
            checked={includeSubpackages}
            onChange={(event) => onIncludeSubpackagesChange(event.currentTarget.checked)}
          />
          <Switch
            label="Include XML artifacts"
            checked={includeXmlArtifacts}
            onChange={(event) => onIncludeXmlArtifactsChange(event.currentTarget.checked)}
          />
        </Stack>
      </Stack>
    </Stack>
  );
});

const SapImportObjectBrowserPanel = memo(function SapImportObjectBrowserPanel(props: {
  objects: SapSearchObject[];
  visibleObjects: SapSearchObject[];
  groupedObjects: Array<{ group: string; items: SapSearchObject[] }>;
  checkedUris: Set<string>;
  objectFilter: string;
  onObjectFilterChange: (value: string) => void;
  onClearFilter: () => void;
  onToggleUri: (uri: string, checked: boolean) => void;
  onToggleGroup: (items: SapSearchObject[], checked: boolean) => void;
}) {
  const {
    objects,
    visibleObjects,
    groupedObjects,
    checkedUris,
    objectFilter,
    onObjectFilterChange,
    onClearFilter,
    onToggleUri,
    onToggleGroup
  } = props;

  return (
    <Stack h="100%" gap="sm">
      <Group justify="space-between" align="center" wrap="wrap">
        <Text fw={600}>Package objects</Text>
        <Group gap="xs">
          <Text size="sm" c="dimmed">
            {visibleObjects.length} objects / {groupedObjects.length} groups
          </Text>
          <Button size="compact-xs" variant="subtle" onClick={onClearFilter} disabled={!objectFilter.trim()}>
            Clear filter
          </Button>
        </Group>
      </Group>

      <TextInput
        label="Filter loaded objects"
        placeholder="type, name, URI, package..."
        value={objectFilter}
        onChange={(event) => onObjectFilterChange(event.currentTarget.value)}
      />

      {objects.length === 0 ? (
        <Text c="dimmed" size="sm">No SAP objects loaded yet.</Text>
      ) : groupedObjects.length === 0 ? (
        <Text c="dimmed" size="sm">No selectable SAP objects match the current filter.</Text>
      ) : (
        <ScrollArea h="100%" type="auto">
          <Stack gap="xs" style={{ minWidth: 0, width: '100%' }}>
            {groupedObjects.map(({ group, items }) => {
              const selectedCount = items.filter((item) => checkedUris.has(item.source_uri || item.uri)).length;
              const allSelected = items.length > 0 && selectedCount === items.length;
              return (
                <Box key={group} style={{ width: '100%', border: '1px solid var(--mantine-color-dark-4)', borderRadius: 8, padding: 10 }}>
                  <Stack gap="xs">
                    <Group justify="space-between" align="center" wrap="wrap">
                      <Group gap="xs">
                        <Text fw={600}>{group}</Text>
                        <Badge variant="light">{items.length}</Badge>
                        {selectedCount > 0 ? <Text size="sm" c="dimmed">{selectedCount} selected</Text> : null}
                      </Group>
                      <Group gap="xs">
                        <Button size="compact-xs" variant="subtle" onClick={() => onToggleGroup(items, true)} disabled={allSelected}>
                          Select all
                        </Button>
                        <Button size="compact-xs" variant="subtle" color="gray" onClick={() => onToggleGroup(items, false)} disabled={selectedCount === 0}>
                          Clear
                        </Button>
                      </Group>
                    </Group>

                    <Stack gap={2}>
                      {items.map((item) => {
                        const effectiveUri = item.source_uri || item.uri;
                        const displayName = sapObjectDisplayName(item);
                        return (
                          <Group key={effectiveUri} align="flex-start" wrap="nowrap" style={{ borderTop: '1px solid var(--mantine-color-dark-4)', paddingTop: 6, minWidth: 0 }}>
                            <Checkbox
                              mt={2}
                              checked={checkedUris.has(effectiveUri)}
                              onChange={(event) => onToggleUri(effectiveUri, event.currentTarget.checked)}
                            />
                            <Stack gap={1} style={{ flex: 1, minWidth: 0 }}>
                              <Group gap="xs" wrap="nowrap" style={{ minWidth: 0 }}>
                                <Text fw={500} truncate style={{ flex: 1, minWidth: 0 }}>{displayName}</Text>
                                <Code>{item.object_type}</Code>
                              </Group>
                              <Text size="xs" c="dimmed" truncate>
                                {effectiveUri}
                              </Text>
                            </Stack>
                          </Group>
                        );
                      })}
                    </Stack>
                  </Stack>
                </Box>
              );
            })}
          </Stack>
        </ScrollArea>
      )}
    </Stack>
  );
});

function sapObjectGroupKey(objectType: string): string {
  const value = objectType.trim().toUpperCase();
  if (!value) return 'UNKNOWN';
  const slash = value.indexOf('/');
  return slash >= 0 ? value.slice(0, slash) : value;
}

function sapObjectDisplayName(item: SapSearchObject): string {
  const raw = item.name.trim();
  if (raw && !/^=+$/.test(raw)) return raw;
  const fallback = (item.source_uri || item.uri).split('/').filter(Boolean).pop()?.trim();
  return fallback && fallback.length > 0 ? fallback : '(unnamed object)';
}

function sapObjectGroupLabel(objectType: string): string {
  const key = sapObjectGroupKey(objectType);
  return key === 'UNKNOWN' ? 'Unknown' : key;
}

function isStructuralSapObject(item: SapSearchObject): boolean {
  const type = item.object_type.trim().toUpperCase();
  const display = sapObjectDisplayName(item);
  if (type === 'DEVC/P' || type === 'DEVC/K') return true;
  if (!item.source_uri && type.startsWith('DEVC/')) return true;
  if (display === '(unnamed object)') return true;
  if (/^=+$/.test(item.name.trim())) return true;
  return false;
}

const SapExportStageInputsPanel = memo(function SapExportStageInputsPanel(props: {
  selectedWorkflowStep: WorkflowStepDefinition | null;
  repoRef: string;
  onPatchSelectedStepConfig: (key: string, value: unknown) => void;
}) {
  const { selectedWorkflowStep, repoRef, onPatchSelectedStepConfig } = props;
  const [manifestPathsText, setManifestPathsText] = useState('');
  const [autoActivate, setAutoActivate] = useState(true);

  const selectedManifestPaths = useMemo(
    () => new Set(manifestPathsText.split(/\r?\n/).map((item) => item.trim()).filter(Boolean)),
    [manifestPathsText]
  );

  const [scanBusy, setScanBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [manifests, setManifests] = useState<SapExportScanItem[]>([]);
  const [checkedManifestPaths, setCheckedManifestPaths] = useState<Set<string>>(new Set());

  useEffect(() => {
    const nextManifestPathsText = readStringValue(selectedWorkflowStep, 'config.sap_export.manifest_paths_text', '');
    setManifestPathsText(nextManifestPathsText);
    setAutoActivate(readBooleanValue(selectedWorkflowStep, 'config.sap_export.auto_activate', true));
    setCheckedManifestPaths(new Set(nextManifestPathsText.split(/\r?\n/).map((item) => item.trim()).filter(Boolean)));
  }, [selectedWorkflowStep?.id]);

  async function handleScan() {
    try {
      setScanBusy(true);
      setStatus(null);
      const result = await sapScanExportCandidates(repoRef);
      setManifests(result.manifests);
      const nextChecked = new Set<string>(selectedManifestPaths);
      for (const item of result.manifests) {
        if (selectedManifestPaths.has(item.manifest_path)) {
          nextChecked.add(item.manifest_path);
        }
      }
      setCheckedManifestPaths(nextChecked);
      setStatus(`Found ${result.count} exportable SAP manifest(s).`);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setScanBusy(false);
    }
  }

  function toggleManifest(path: string, checked: boolean) {
    setCheckedManifestPaths((prev) => {
      const next = new Set(prev);
      if (checked) next.add(path); else next.delete(path);
      return next;
    });
  }

  function applySelection() {
    const next = Array.from(checkedManifestPaths).join('\n');
    setManifestPathsText(next);
    onPatchSelectedStepConfig('config.sap_export.manifest_paths_text', next);
    setStatus(`Selected ${checkedManifestPaths.size} manifest(s) for export.`);
  }

  return (
    <Stack>
      <Title order={6}>SAP Export inputs</Title>
      <Group align="end" wrap="wrap">
        <Button
          size="xs"
          variant="default"
          onClick={() => void handleScan()}
          loading={scanBusy}
          disabled={!repoRef.trim()}
        >
          Scan local SAP manifests
        </Button>
        <Button size="xs" variant="light" onClick={applySelection} disabled={checkedManifestPaths.size === 0}>
          Export selected
        </Button>
      </Group>
      {status ? <Alert color="blue">{status}</Alert> : null}
      <Group>
        <Switch
          label="Auto activate"
          checked={autoActivate}
          onChange={(event) => {
            const next = event.currentTarget.checked;
            setAutoActivate(next);
            onPatchSelectedStepConfig('config.sap_export.auto_activate', next);
          }}
        />
      </Group>
      <Divider label="Local export candidates" />
      {manifests.length === 0 ? (
        <Text c="dimmed" size="sm">No export candidates scanned yet.</Text>
      ) : (
        <ScrollArea h={420} type="auto">
          <Table striped highlightOnHover>
            <Table.Thead>
              <Table.Tr>
                <Table.Th></Table.Th>
                <Table.Th>Object</Table.Th>
                <Table.Th>Type</Table.Th>
                <Table.Th>Package</Table.Th>
                <Table.Th>Manifest</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {manifests.map((item) => (
                <Table.Tr key={item.manifest_path}>
                  <Table.Td>
                    <Checkbox
                      checked={checkedManifestPaths.has(item.manifest_path)}
                      onChange={(event) => toggleManifest(item.manifest_path, event.currentTarget.checked)}
                    />
                  </Table.Td>
                  <Table.Td>{item.object_name}</Table.Td>
                  <Table.Td><Code>{item.object_type}</Code></Table.Td>
                  <Table.Td>{item.package_name ?? ''}</Table.Td>
                  <Table.Td>
                    <Text size="xs">{item.manifest_path}</Text>
                    <Text size="xs" c="dimmed">{item.candidate_count} resource(s)</Text>
                  </Table.Td>
                </Table.Tr>
              ))}
            </Table.Tbody>
          </Table>
        </ScrollArea>
      )}
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
  const [monitorHomeView, setMonitorHomeView] = useState<MonitorHomeView>('workflows');
  const [activeWorkspaceTab, setActiveWorkspaceTab] = useState<WorkspaceTabKey>('workflows');
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
  const [workflowBuilderCatalog, setWorkflowBuilderCatalog] = useState<WorkflowBuilderCatalog | null>(null);

  const selectedRunIdRef = useRef<string | null>(null);
  const allWorkflowEventsRef = useRef<Record<string, WorkflowEvent[]>>({});
  const runEventStreamsRef = useRef<Record<string, EventSource>>({});
  const runRefreshTimersRef = useRef<Record<string, number>>({});


  function patchSelectedStepDescriptorField(bindTo: string, value: unknown) {
    if (!selectedRunId || !selectedWorkflowStep) return;

    if (bindTo === 'prompt.user_input') {
      setStageUserInput(typeof value === 'string' ? value : String(value ?? ''));
    } else if (bindTo === 'execution.compile_checks.commands_text') {
      const text = typeof value === 'string' ? value : String(value ?? '');
      setStageCompileCommandsText(text);
      const compileCommands = text
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean)
        .map((command) => ({ command, label: command }));
      const currentGlobalState = ((selectedRun?.context?.workflow_engine as Record<string, unknown> | undefined)?.global_state as Record<string, unknown> | undefined) ?? {};
      const currentCapabilities = (currentGlobalState.capabilities as Record<string, unknown> | undefined) ?? {};
      const currentCompileCommands = (currentCapabilities.compile_commands as Record<string, unknown> | undefined) ?? {};
      void patchWorkflowGlobalState(selectedRunId, {
        ...currentGlobalState,
        capabilities: {
          ...currentCapabilities,
          compile_commands: {
            ...currentCompileCommands,
            commands: compileCommands
          }
        }
      });
    } else if (bindTo === 'capabilities.planner.schema_armed' || bindTo === 'capabilities.planner.auto_apply_armed') {
      const plannerKey = bindTo === 'capabilities.planner.schema_armed' ? 'schema_armed' : 'auto_apply_armed';
      void patchPlannerCapabilityState({
        [plannerKey]: Boolean(value)
      });
      return;
    } else if (bindTo === 'execution_logic.automation.inject_context') {
      setStageIncludeRepoContext(Boolean(value));
    } else if (bindTo === 'execution_logic.automation.inject_changeset_schema') {
      setStageIncludeChangesetSchema(Boolean(value));
    } else if (bindTo === 'execution_logic.automation.include_apply_error') {
      setStageIncludeApplyError(Boolean(value));
    } else if (bindTo === 'execution_logic.automation.include_compile_error') {
      setStageIncludeCompileError(Boolean(value));
    } else if (bindTo === 'execution_logic.automation.auto_apply_changeset') {
      setStageAutoApplyChangeset(Boolean(value));
    } else if (bindTo === 'review.notes') {
      setStageReviewNotes(typeof value === 'string' ? value : String(value ?? ''));
    } else if (bindTo === 'review.approved') {
      const checked = Boolean(value);
      setStageApproved(checked);
      if (checked) {
        setStageRejected(false);
      }
    } else if (bindTo === 'review.rejected') {
      const checked = Boolean(value);
      setStageRejected(checked);
      if (checked) {
        setStageApproved(false);
      }
    }

    const payload: Record<string, unknown> = {};
    const parts = bindTo.split('.').filter(Boolean);
    let cursor: Record<string, unknown> = payload;
    for (let index = 0; index < parts.length; index += 1) {
      const part = parts[index]!;
      if (index === parts.length - 1) {
        cursor[part] = value;
      } else {
        const next: Record<string, unknown> = {};
        cursor[part] = next;
        cursor = next;
      }
    }
    void patchWorkflowStageState(selectedRunId, selectedWorkflowStep.id, payload);
  }

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const next = await getWorkflowBuilderCatalog();
        if (!cancelled) {
          setWorkflowBuilderCatalog(next);
        }
      } catch {
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const [workflowName, setWorkflowName] = useState('Default workflow');
  const [workflowDescription, setWorkflowDescription] = useState('Design, code, and review workflow');
  const [repoRef, setRepoRef] = useState('');
  const [jsonDraft, setJsonDraft] = useState('');
  const [compiledBuilderDefinition, setCompiledBuilderDefinition] = useState<WorkflowTemplateDefinition | null>(null);
  const [loadedTemplateDefinition, setLoadedTemplateDefinition] = useState<WorkflowTemplateDefinition | null>(null);
  const [builderLoadRevision, setBuilderLoadRevision] = useState(0);
  const [builderGlobals, setBuilderGlobals] = useState<WorkflowTemplateDefinition['globals'] | null>(null);
  const [createRunAfterSave, setCreateRunAfterSave] = useState(true);
  const [templateModalOpen, setTemplateModalOpen] = useState(false);
  const [loadTemplateOpen, setLoadTemplateOpen] = useState(false);
  const [globalCapabilitiesOpen, setGlobalCapabilitiesOpen] = useState(false);

  const [selectedStepId, setSelectedStepId] = useState<string | null>(null);
  const [pendingStageSelectionId, setPendingStageSelectionId] = useState<string | null>(null);
  const [pendingDispositionAutoRun, setPendingDispositionAutoRun] = useState<{ runId: string; stepId: string; runAutomatically: boolean } | null>(null);
  const [pauseRequestBusy, setPauseRequestBusy] = useState(false);
  const [manualCapabilityStatus, setManualCapabilityStatus] = useState<string | null>(null);
  const [manualCapabilityBusy, setManualCapabilityBusy] = useState(false);
  const [manualCapabilityResponse, setManualCapabilityResponse] = useState('');

  const [inferenceTransport, setInferenceTransport] = useState<InferenceTransport>('api');
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
  const [stageRepoContextInlinePrompt, setStageRepoContextInlinePrompt] = useState(false);
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
  const [globalInferenceConfigOpen, setGlobalInferenceConfigOpen] = useState(false);
  const [changesetSchemaBusy, setChangesetSchemaBusy] = useState(false);
  const [changesetSchemaConfigOpen, setChangesetSchemaConfigOpen] = useState(false);
  const [plannerFragmentConfigOpen, setPlannerFragmentConfigOpen] = useState(false);
  const [plannerSelectedFeatureIdDraft, setPlannerSelectedFeatureIdDraft] = useState<string | null>(null);
  const [plannerFeatureSearch, setPlannerFeatureSearch] = useState('');
  const [remotePlannerFeatureItems, setRemotePlannerFeatureItems] = useState<FeaturePlanItem[]>([]);
  const [repoPlannerAvailable, setRepoPlannerAvailable] = useState(false);
  const [plannerFeatureViewItem, setPlannerFeatureViewItem] = useState<Record<string, unknown> | null>(null);
  const [supervisorPlannerOpen, setSupervisorPlannerOpen] = useState(false);
  const [supervisorPlannerRun, setSupervisorPlannerRun] = useState<SupervisorRun | null>(null);
  const [applyErrorConfigOpen, setApplyErrorConfigOpen] = useState(false);
  const [globalApplyChangesetOpen, setGlobalApplyChangesetOpen] = useState(false);
  const [globalApplyChangesetText, setGlobalApplyChangesetText] = useState('');
  const [globalApplyChangesetResult, setGlobalApplyChangesetResult] = useState<ApplyChangesetResponse | null>(null);
  const [globalApplyChangesetPanelMode, setGlobalApplyChangesetPanelMode] = useState<'input' | 'output'>('input');
  const [globalApplyChangesetHistory, setGlobalApplyChangesetHistory] = useState<ChangesetAttemptSummary[]>([]);
  const [globalApplyChangesetHistoryBusy, setGlobalApplyChangesetHistoryBusy] = useState(false);
  const [gitPatchPayloadOpen, setGitPatchPayloadOpen] = useState(false);
  const [gitPatchPayloadMode, setGitPatchPayloadMode] = useState<'generate' | 'apply'>('generate');
  const [gitPatchPayloadScope, setGitPatchPayloadScope] = useState<'staged' | 'unstaged' | 'both'>('both');
  const [gitPatchPayloadText, setGitPatchPayloadText] = useState('');
  const [gitPatchPayloadReverse, setGitPatchPayloadReverse] = useState(false);
  const [gitPatchPayloadBusy, setGitPatchPayloadBusy] = useState(false);
  const [gitPatchPayloadStatus, setGitPatchPayloadStatus] = useState<string | null>(null);
  const [responseViewerOpen, setResponseViewerOpen] = useState(false);
  const [compileErrorConfigOpen, setCompileErrorConfigOpen] = useState(false);
  const [runContextOpen, setRunContextOpen] = useState(false);
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
  const isInteractiveMode = selectedRun?.status === 'paused' || selectedRun?.status === 'waiting' || selectedRun?.status === 'draft' || selectedRun?.status === 'success';
  const isManualMode = isInteractiveMode;
  const isBackendRunLocked = Boolean(
    busy
    || manualCapabilityBusy
    || selectedRun?.status === 'queued'
    || selectedRun?.status === 'running'
  );
  const canRequestRunPause = Boolean(
    selectedRunId
    && !pauseRequestBusy
    && (
      selectedRun?.status === 'queued'
      || selectedRun?.status === 'running'
      || selectedRun?.status === 'waiting'
      || busy
      || manualCapabilityBusy
    )
  );
  const selectedRunTemplate = selectedRun?.template_id ? templates.find((template) => template.id === selectedRun.template_id) ?? null : null;

  const selectedRunDefinition = useMemo<WorkflowTemplateDefinition | null>(() => {
    return selectedRun?.definition ?? null;
  }, [selectedRun?.definition]);

  const pendingDispositionReview = useMemo(() => {
    const workflowEngine = ((selectedRun?.context as Record<string, unknown> | undefined)?.workflow_engine ?? undefined) as Record<string, unknown> | undefined;
    const runState = (workflowEngine?.run_state ?? {}) as Record<string, unknown>;
    const blockedOn = (runState.blocked_on ?? null) as Record<string, unknown> | null;
    if (!blockedOn || blockedOn.kind !== 'disposition_review') return null;
    const available = Array.isArray(blockedOn.available_dispositions)
      ? blockedOn.available_dispositions.filter((item): item is string => item === 'move_next' || item === 'pause')
      : ['move_next', 'pause'];
    return {
      stageId: typeof blockedOn.stage_id === 'string' ? blockedOn.stage_id : selectedRun?.current_step_id ?? '',
      stageType: typeof blockedOn.stage_type === 'string' ? blockedOn.stage_type : '',
      recommendedDisposition: typeof blockedOn.recommended_disposition === 'string' ? blockedOn.recommended_disposition : '',
      nextStepId: typeof blockedOn.next_step_id === 'string' ? blockedOn.next_step_id : '',
      message: typeof blockedOn.message === 'string' ? blockedOn.message : '',
      availableDispositions: available.length > 0 ? available : ['move_next', 'pause']
    };
  }, [selectedRun?.context, selectedRun?.current_step_id]);

  const hasPendingDispositionReview = Boolean(pendingDispositionReview);

  const selectedRunStepId = selectedStepId ?? selectedRun?.current_step_id ?? selectedRunDefinition?.steps[0]?.id ?? null;

  const selectedWorkflowStep = useMemo(() => {
    return selectedRunDefinition?.steps.find((step) => step.id === selectedRunStepId) ?? null;
  }, [selectedRunDefinition, selectedRunStepId]);


  const [sapImportPackageName, setSapImportPackageName] = useState('');
  const [sapImportIncludeSubpackages, setSapImportIncludeSubpackages] = useState(true);
  const [sapImportIncludeXmlArtifacts, setSapImportIncludeXmlArtifacts] = useState(false);
  const [sapImportSelectedObjectUrisText, setSapImportSelectedObjectUrisText] = useState('');
  const [sapImportSearchBusy, setSapImportSearchBusy] = useState(false);
  const [sapImportApplyBusy, setSapImportApplyBusy] = useState(false);
  const [sapImportStatus, setSapImportStatus] = useState<string | null>(null);
  const [sapImportObjects, setSapImportObjects] = useState<SapSearchObject[]>([]);
  const [sapImportCheckedUris, setSapImportCheckedUris] = useState<Set<string>>(new Set());
  const [sapImportObjectFilter, setSapImportObjectFilter] = useState('');

  const sapImportSelectedObjectUris = useMemo(
    () => new Set(sapImportSelectedObjectUrisText.split(/\r?\n/).map((item) => item.trim()).filter(Boolean)),
    [sapImportSelectedObjectUrisText]
  );

  const sapImportVisibleObjects = useMemo(() => {
    const needle = sapImportObjectFilter.trim().toLowerCase();
    return sapImportObjects.filter((item) => {
      if (isStructuralSapObject(item)) return false;
      if (!needle) return true;
      const effectiveUri = (item.source_uri || item.uri).toLowerCase();
      const displayName = sapObjectDisplayName(item).toLowerCase();
      const objectType = item.object_type.toLowerCase();
      const packageNameValue = (item.package_name ?? '').toLowerCase();
      return displayName.includes(needle)
        || objectType.includes(needle)
        || packageNameValue.includes(needle)
        || effectiveUri.includes(needle);
    });
  }, [sapImportObjects, sapImportObjectFilter]);

  const sapImportGroupedObjects = useMemo(() => {
    const grouped = new Map<string, SapSearchObject[]>();
    for (const item of sapImportVisibleObjects) {
      const key = sapObjectGroupLabel(item.object_type);
      const bucket = grouped.get(key) ?? [];
      bucket.push(item);
      grouped.set(key, bucket);
    }
    return Array.from(grouped.entries())
      .map(([group, items]) => ({
        group,
        items: items.slice().sort((a, b) => {
          const typeCompare = a.object_type.localeCompare(b.object_type);
          if (typeCompare !== 0) return typeCompare;
          return sapObjectDisplayName(a).localeCompare(sapObjectDisplayName(b));
        })
      }))
      .sort((a, b) => a.group.localeCompare(b.group));
  }, [sapImportVisibleObjects]);

  useEffect(() => {
    if (selectedWorkflowStep?.step_type !== 'sap_import') {
      return;
    }
    setSapImportPackageName(readStringValue(selectedWorkflowStep, 'config.sap_import.package_name', ''));
    setSapImportIncludeSubpackages(readBooleanValue(selectedWorkflowStep, 'config.sap_import.include_subpackages', true));
    setSapImportIncludeXmlArtifacts(readBooleanValue(selectedWorkflowStep, 'config.sap_import.include_xml_artifacts', false));
    const nextUrisText = readStringValue(selectedWorkflowStep, 'config.sap_import.object_uris_text', '');
    setSapImportSelectedObjectUrisText(nextUrisText);
    setSapImportCheckedUris(new Set(nextUrisText.split(/\r?\n/).map((item) => item.trim()).filter(Boolean)));
    setSapImportObjectFilter('');
    setSapImportStatus(null);
  }, [selectedWorkflowStep?.id, selectedWorkflowStep?.step_type]);

  async function handleSapImportSearch() {
    const packageName = sapImportPackageName.trim();
    if (!packageName) {
      setSapImportStatus(null);
      return;
    }

    try {
      setSapImportSearchBusy(true);
      setSapImportStatus(null);
      const result = await sapSearchObjects(packageName, sapImportIncludeSubpackages);
      setSapImportObjects(result.objects);
      const nextChecked = new Set<string>(sapImportSelectedObjectUris);
      for (const item of result.objects) {
        const effectiveUri = item.source_uri || item.uri;
        if (sapImportSelectedObjectUris.has(effectiveUri)) {
          nextChecked.add(effectiveUri);
        }
      }
      setSapImportCheckedUris(nextChecked);
      const selectableCount = result.objects.filter((item) => !isStructuralSapObject(item)).length;
      const hiddenCount = result.count - selectableCount;
      setSapImportStatus(
        hiddenCount > 0
          ? `Loaded ${selectableCount} SAP object(s). Hid ${hiddenCount} structural package node(s).`
          : `Loaded ${selectableCount} SAP object(s).`
      );
    } catch (error) {
      setSapImportStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setSapImportSearchBusy(false);
    }
  }

  function toggleSapImportUri(uri: string, checked: boolean) {
    setSapImportCheckedUris((prev) => {
      const next = new Set(prev);
      if (checked) next.add(uri); else next.delete(uri);
      return next;
    });
  }

  function toggleSapImportGroup(items: SapSearchObject[], checked: boolean) {
    setSapImportCheckedUris((prev) => {
      const next = new Set(prev);
      for (const item of items) {
        const effectiveUri = item.source_uri || item.uri;
        if (checked) next.add(effectiveUri); else next.delete(effectiveUri);
      }
      return next;
    });
  }

  async function applySapImportSelection() {
    const nextChecked = Array.from(sapImportCheckedUris);
    const selectedObjects = sapImportObjects
      .filter((item) => nextChecked.includes(item.uri) || nextChecked.includes(item.source_uri || ''))
      .map((item) => ({
        object_uri: item.uri,
        object_name: item.name,
        object_type: item.object_type,
        package_name: item.package_name ?? null,
        source_uri: item.source_uri ?? null
      }));
    const next = selectedObjects.map((item) => item.object_uri).join('\n');

    setSapImportSelectedObjectUrisText(next);
    patchSelectedStepDescriptorField('config.sap_import.object_uris_text', next);
    patchSelectedStepDescriptorField('config.sap_import.selected_objects', selectedObjects);
    patchSelectedStepDescriptorField('config.sap_import.package_name', sapImportPackageName);
    patchSelectedStepDescriptorField('config.sap_import.include_xml_artifacts', sapImportIncludeXmlArtifacts);

    if (selectedObjects.length === 0) {
      setSapImportStatus('Select at least one SAP object to import.');
      return;
    }

    try {
      setSapImportApplyBusy(true);
      setSapImportStatus(null);
      setSapImportStatus(`Prepared ${selectedObjects.length} SAP object(s) for workflow import.`);
    } catch (error) {
      setSapImportStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setSapImportApplyBusy(false);
    }
  }

  const workflowStageDescriptors = useMemo(
    () => workflowBuilderCatalog ? descriptorMap(workflowBuilderCatalog) : {},
    [workflowBuilderCatalog]
  );

  const selectedStageDescriptor = useMemo(() => {
    if (!selectedWorkflowStep) return null;
    const stepType = selectedWorkflowStep.step_type;
    return workflowStageDescriptors[stepType] ?? workflowStageDescriptors[stepType.trim().toLowerCase()] ?? null;
  }, [selectedWorkflowStep, workflowStageDescriptors]);

  const inferenceRequiredForSelectedStep = useMemo(
    () => stepUsesCapability(selectedWorkflowStep, 'inference'),
    [selectedWorkflowStep]
  );

  const pendingStageSelection = pendingStageSelectionId
    ? selectedRunDefinition?.steps.find((step) => step.id === pendingStageSelectionId) ?? null
    : null;
  const sharedInferenceState = useMemo(() => {
    const workflowEngine = (selectedRun?.context as Record<string, unknown> | undefined)?.workflow_engine as Record<string, unknown> | undefined;
    const globalState = (workflowEngine?.global_state ?? {}) as Record<string, unknown>;
    const capabilities = (globalState.capabilities ?? {}) as Record<string, unknown>;
    const inference = (capabilities.inference ?? null) as Record<string, unknown> | null;
    const runState = (workflowEngine?.run_state ?? {}) as Record<string, unknown>;
    const lastPreparedStage = (runState.last_prepared_stage ?? null) as Record<string, unknown> | null;
    const preparedStepId = typeof lastPreparedStage?.step_id === 'string' ? lastPreparedStage.step_id : null;
    const preparedInference = (lastPreparedStage?.inference ?? null) as Record<string, unknown> | null;

    if (preparedStepId && preparedStepId === selectedRunStepId && preparedInference) {
      return {
        ...preparedInference,
        ...(inference ?? {}),
        last_prepared_stage: lastPreparedStage
      };
    }

    return inference;
  }, [selectedRun?.context, selectedRunStepId]);
  const sharedPlannerFragmentState = useMemo(() => {
    const workflowEngine = (selectedRun?.context as Record<string, unknown> | undefined)?.workflow_engine as Record<string, unknown> | undefined;
    const globalState = (workflowEngine?.global_state ?? {}) as Record<string, unknown>;
    const capabilities = (globalState.capabilities ?? {}) as Record<string, unknown>;
    return (capabilities.planner ?? null) as Record<string, unknown> | null;
  }, [selectedRun?.context]);
  const supervisorContext = useMemo(() => {
    return ((selectedRun?.context as Record<string, unknown> | undefined)?.supervisor ?? null) as Record<string, unknown> | null;
  }, [selectedRun?.context]);

  const plannerSupervisorRunId = useMemo(() => {
    const fromPlanner = sharedPlannerFragmentState?.supervisor_run_id;
    if (typeof fromPlanner === 'string' && fromPlanner.trim()) return fromPlanner;
    const fromSupervisor = supervisorContext?.supervisor_run_id;
    if (typeof fromSupervisor === 'string' && fromSupervisor.trim()) return fromSupervisor;
    return null;
  }, [sharedPlannerFragmentState, supervisorContext]);

  useEffect(() => {
    let cancelled = false;
    const selectedFeatureId = typeof sharedPlannerFragmentState?.selected_feature_id === 'string'
      ? sharedPlannerFragmentState.selected_feature_id
      : null;
    const repoRef = typeof selectedRun?.repo_ref === 'string' ? selectedRun.repo_ref : '';

    async function loadPlannerFeatures() {
      if (plannerSupervisorRunId) {
        const run = await getSupervisorRun(plannerSupervisorRunId);
        if (!cancelled) setRepoPlannerAvailable(true);
        return run.feature_plan_items ?? [];
      }

      if (!selectedFeatureId && !repoRef.trim()) {
        return [];
      }

      const runs = await listSupervisorRuns();
      const normalizedRepoRef = repoRef.replace(/\\/g, '/').toLowerCase();
      const matchingRun = runs.find((run) => {
        const normalizedRoot = run.root_repo_path.replace(/\\/g, '/').toLowerCase();
        const hasSelectedFeature = selectedFeatureId
          ? run.feature_plan_items.some((item) => item.id === selectedFeatureId)
          : false;
        const repoMatches = normalizedRepoRef && normalizedRoot && normalizedRepoRef.startsWith(normalizedRoot);
        return hasSelectedFeature || repoMatches;
      });
      if (!cancelled) setRepoPlannerAvailable(Boolean(matchingRun));
      return matchingRun?.feature_plan_items ?? [];
    }

    loadPlannerFeatures()
      .then((items) => {
        if (!cancelled) setRemotePlannerFeatureItems(items);
      })
      .catch(() => {
        if (!cancelled) setRemotePlannerFeatureItems([]);
      });
    if (!plannerSupervisorRunId && !selectedRun?.repo_ref?.trim()) {
      setRepoPlannerAvailable(false);
    }

    return () => {
      cancelled = true;
    };
  }, [plannerSupervisorRunId, selectedRun?.repo_ref, sharedPlannerFragmentState]);

  const plannerFeatureItems = useMemo(() => {
    const remoteItems = remotePlannerFeatureItems;
    const supervisorItems = Array.isArray(supervisorContext?.feature_plan_items)
      ? supervisorContext.feature_plan_items
      : [];
    const items = remoteItems.length > 0
      ? remoteItems
      : supervisorItems;
    const seen = new Set<string>();
    return items.filter((item): item is Record<string, unknown> => {
      if (!item || typeof item !== 'object') return false;
      const id = typeof item.id === 'string' ? item.id : '';
      if (!id) return true;
      if (seen.has(id)) return false;
      seen.add(id);
      return true;
    });
  }, [remotePlannerFeatureItems, sharedPlannerFragmentState, supervisorContext]);

  const plannerFeatureOptions = useMemo(() => plannerFeatureItems
    .map((item) => {
      const id = typeof item.id === 'string' ? item.id : '';
      const title = typeof item.title === 'string' && item.title.trim()
        ? item.title.trim()
        : typeof item.summary === 'string' && item.summary.trim()
          ? item.summary.trim()
          : id;
      return id ? { value: id, label: title } : null;
    })
    .filter((item): item is { value: string; label: string } => item !== null), [plannerFeatureItems]);

  const selectedPlannerFeatureId = plannerSelectedFeatureIdDraft
    ?? (typeof sharedPlannerFragmentState?.selected_feature_id === 'string' && sharedPlannerFragmentState.selected_feature_id.trim() ? sharedPlannerFragmentState.selected_feature_id : null)
    ?? (typeof supervisorContext?.feature_id === 'string' && supervisorContext.feature_id.trim() ? supervisorContext.feature_id : null);

  const selectedPlannerFeatureIds = selectedPlannerFeatureId ? [selectedPlannerFeatureId] : [];
  const selectedPlannerFeature = useMemo(() => {
    if (!selectedPlannerFeatureId) return null;
    return plannerFeatureItems.find((item) => item.id === selectedPlannerFeatureId) ?? null;
  }, [plannerFeatureItems, selectedPlannerFeatureId]);

  const filteredPlannerFeatureItems = useMemo(() => {
    const needle = plannerFeatureSearch.trim().toLowerCase();
    if (!needle) return plannerFeatureItems;
    return plannerFeatureItems.filter((item) => {
      const title = typeof item.title === 'string' ? item.title : '';
      const summary = typeof item.summary === 'string' ? item.summary : '';
      const status = typeof item.status === 'string' ? item.status : '';
      return title.toLowerCase().includes(needle)
        || summary.toLowerCase().includes(needle)
        || status.toLowerCase().includes(needle);
    });
  }, [plannerFeatureItems, plannerFeatureSearch]);
  const selectedStageState = useMemo(() => {
    const workflowEngine = (selectedRun?.context as Record<string, unknown> | undefined)?.workflow_engine as Record<string, unknown> | undefined;
    const stageOverrides = (workflowEngine?.stage_overrides ?? {}) as Record<string, unknown>;
    const globalState = (workflowEngine?.global_state ?? {}) as Record<string, unknown>;
    const capabilities = (globalState.capabilities ?? {}) as Record<string, unknown>;
    const globalRepoContext = (capabilities.context_export ?? null) as Record<string, unknown> | null;
    const stepId = selectedStepId ?? selectedRun?.current_step_id ?? '';
    const localStageOverride = (stageOverrides[stepId] ?? null) as Record<string, unknown> | null;
    if (!localStageOverride && !globalRepoContext && !sharedInferenceState) {
      return null;
    }
    return {
      ...(globalRepoContext ? { repo_context: globalRepoContext } : {}),
      ...(sharedInferenceState ? { inference: sharedInferenceState } : {}),
      ...(localStageOverride ?? {})
    } as Record<string, unknown>;
  }, [selectedRun?.context, selectedRun?.current_step_id, selectedStepId, sharedInferenceState]);

  useEffect(() => {
    if (!pendingDispositionAutoRun) return;
    if (!selectedRun || selectedRun.id !== pendingDispositionAutoRun.runId) return;
    if (selectedRun.current_step_id !== pendingDispositionAutoRun.stepId) return;
    if (selectedRunStepId !== pendingDispositionAutoRun.stepId) return;
    if (!selectedWorkflowStep || selectedWorkflowStep.id !== pendingDispositionAutoRun.stepId) return;
    if (hasPendingDispositionReview || isBackendRunLocked) return;

    const pending = pendingDispositionAutoRun;
    setPendingDispositionAutoRun(null);
    window.setTimeout(() => {
      const action = pending.runAutomatically
        ? startWorkflowRun(pending.runId)
        : runCurrentWorkflowStep(pending.runId, pending.stepId);
      void action
        .catch((err) => {
          setError(err instanceof Error ? err.message : String(err));
        })
        .finally(() => {
          void refreshRunDetails(pending.runId);
        });
    }, 0);
  }, [pendingDispositionAutoRun, selectedRun?.id, selectedRun?.current_step_id, selectedRunStepId, selectedWorkflowStep?.id, hasPendingDispositionReview, isBackendRunLocked]);

  const persistedReviewSourceControlState = useMemo<ReviewSourceControlState>(() => {
    const review = (selectedStageState?.review ?? {}) as Record<string, unknown>;
    const sourceControl = (review.source_control ?? {}) as Record<string, unknown>;
    return {
      selected_scope: sourceControl.selected_scope === 'staged' ? 'staged' : 'unstaged',
      selected_path: typeof sourceControl.selected_path === 'string' && sourceControl.selected_path.trim()
        ? sourceControl.selected_path
        : null,
      diff_style: sourceControl.diff_style === 'split' ? 'split' : 'unified',
      only_changes: sourceControl.only_changes !== false,
      context_lines: typeof sourceControl.context_lines === 'number' ? sourceControl.context_lines : 10,
      whole_file: Boolean(sourceControl.whole_file)
    };
  }, [selectedStageState]);
  const [localReviewSourceControlState, setLocalReviewSourceControlState] = useState<ReviewSourceControlState>({
    selected_scope: 'unstaged',
    selected_path: null,
    diff_style: 'unified',
    only_changes: true,
    context_lines: 10,
    whole_file: false
  });
  useEffect(() => {
    if (selectedWorkflowStep?.step_type === 'review') {
      setLocalReviewSourceControlState(persistedReviewSourceControlState);
    }
  }, [persistedReviewSourceControlState, selectedWorkflowStep?.step_type]);
  const reviewSourceControlState = localReviewSourceControlState;

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
  const definition = useMemo<WorkflowTemplateDefinition>(() => compiledBuilderDefinition ?? ({
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
        compile_commands: {},
        'sap/import': {},
        'sap/export': {}
      },
      automation: {
        guardrails: {
          changeset_context_inject_after_failures: 3,
          changeset_pause_after_failures: 6,
          compile_pause_after_failures: 5
        }
      }
    },
    steps: []
  }), [compiledBuilderDefinition]);



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
    if (!selectedRunId) return;
    const shouldPoll = Boolean(
      busy
      || manualCapabilityBusy
      || pauseRequestBusy
      || selectedRun?.status === 'queued'
      || selectedRun?.status === 'running'
    );
    if (!shouldPoll) return;

    let cancelled = false;
    const runId = selectedRunId;

    async function pollRun() {
      if (cancelled) return;
      try {
        await refreshRunDetails(runId);
        await refreshLiveMonitor(runId);
      } catch {
      }
    }

    void pollRun();
    const timer = window.setInterval(() => {
      void pollRun();
    }, 1000);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [selectedRunId, selectedRun?.status, busy, manualCapabilityBusy, pauseRequestBusy]);

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
    const desired = new Set<string>();

    if (monitorView === 'workflow_detail' && selectedRunId) {
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
  }, [monitorView, selectedRunId]);

  useEffect(() => {
    setSelectedStepId(selectedRun?.current_step_id ?? null);
    setManualCapabilityStatus(null);
    setManualCapabilityResponse('');
  }, [selectedRun?.id, selectedRun?.current_step_id]);

  useEffect(() => {
    const inference = (sharedInferenceState ?? null) as Record<string, unknown> | null;
    if (!inference) {
      setInferenceTransport('api');
      setBrowserProbe(null);
      return;
    }

    const sessions = ((inference.sessions as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    const defaultSessionName = typeof inference.default_session === 'string' ? inference.default_session : '';
    const defaultSession = defaultSessionName ? ((sessions[defaultSessionName] as Record<string, unknown> | undefined) ?? {}) : {};
    setInferenceTransport(defaultSession.transport === 'browser' ? 'browser' : 'api');
    setBrowserProbe(null);
  }, [sharedInferenceState, selectedRun?.id]);

  useEffect(() => {
    if (!changesetSchemaConfigOpen) return;
    if (stageChangesetSchemaText.trim()) return;
    void loadCanonicalChangesetSchema(false);
  }, [changesetSchemaConfigOpen, stageChangesetSchemaText]);

  useEffect(() => {
    if (!globalApplyChangesetOpen) return;
    const currentGlobalState = ((selectedRun?.context?.workflow_engine as Record<string, unknown> | undefined)?.global_state as Record<string, unknown> | undefined) ?? {};
    const currentCapabilities = (currentGlobalState.capabilities as Record<string, unknown> | undefined) ?? {};
    const currentGatewayChangeset = (currentCapabilities['gateway_model/changeset'] as Record<string, unknown> | undefined) ?? {};
    setGlobalApplyChangesetText(typeof currentGatewayChangeset.draft === 'string' ? currentGatewayChangeset.draft : '');
    setGlobalApplyChangesetResult(null);
    void refreshChangesetHistory();
  }, [globalApplyChangesetOpen, selectedRun?.id, selectedRun?.repo_ref, selectedRun?.context]);

  useEffect(() => {
    const step = selectedWorkflowStep;
    if (!step) return;

    const globalState = ((selectedRun?.context?.workflow_engine as Record<string, unknown> | undefined)?.global_state ?? {}) as Record<string, unknown>;
    const globalCapabilities = (globalState.capabilities ?? {}) as Record<string, unknown>;
    const inferenceConfig = (globalCapabilities.inference ?? {}) as Record<string, unknown>;
    const promptFragments = ((inferenceConfig.prompt_fragments ?? {}) as Record<string, unknown>);
    const promptFragmentEnabled = ((inferenceConfig.prompt_fragment_enabled ?? {}) as Record<string, unknown>);
    const repoContext = (globalCapabilities.context_export ?? {}) as Record<string, unknown>;
    const globalCompileConfig = (globalCapabilities.compile_commands ?? {}) as Record<string, unknown>;
    const selectedExecution = (selectedStageState?.execution ?? {}) as Record<string, unknown>;
    const selectedCompileConfig = (selectedExecution.compile_checks ?? {}) as Record<string, unknown>;
    const stepCompileConfig = (step.execution?.compile_checks ?? {}) as Record<string, unknown>;
    const compileConfig = Array.isArray(selectedCompileConfig.commands) || typeof selectedCompileConfig.commands_text === 'string'
      ? selectedCompileConfig
      : Array.isArray(stepCompileConfig.commands) || typeof stepCompileConfig.commands_text === 'string'
        ? stepCompileConfig
        : globalCompileConfig;
    const review = (selectedStageState?.review ?? {}) as Record<string, unknown>;
    const includeFiles = Array.isArray(repoContext.include_files)
      ? repoContext.include_files.filter((value): value is string => typeof value === 'string')
      : [];

    if (step.step_type === 'code' && typeof promptFragments.changeset_schema !== 'string') {
      void loadCanonicalChangesetSchema(false);
    }

    const globalChangesetSchema = (globalCapabilities.changeset_schema ?? {}) as Record<string, unknown>;
    const selectedPrompt = ((selectedStageState?.prompt ?? {}) as Record<string, unknown>);
    const selectedExecutionLogic = (selectedStageState?.execution_logic ?? step.execution_logic ?? {}) as Record<string, unknown>;
    const selectedAutomation = (selectedExecutionLogic.automation ?? {}) as Record<string, unknown>;
    const hydratedUserInput = getString(selectedPrompt.user_input) ?? getString(promptFragments.user_input) ?? '';
    const hydratedSchemaText = getString(globalChangesetSchema.schema) ?? '';
    const hydratedInjectContext = getBoolean(sharedInferenceState?.repo_context_armed) ?? Boolean(step.prompt?.include_repo_context);
    const hydratedInjectChangesetSchema = getBoolean(sharedInferenceState?.changeset_schema_armed) ?? (step.prompt?.include_changeset_schema ?? step.step_type === 'code');
    const repoContextSharedStatus = stageIncludeRepoContext ? 'ARMED' : 'OFF';
    const changesetSchemaSharedStatus = stageIncludeChangesetSchema ? 'ARMED' : 'OFF';
    const canToggleSharedRepoContext = step.step_type === 'design' || step.step_type === 'code';
    const canToggleSharedChangesetSchema = step.step_type === 'code';

    if (step.step_type === 'code' && !hydratedSchemaText.trim()) {
      void loadCanonicalChangesetSchema(false);
    }

    setStageUserInput(hydratedUserInput);
    setStageChangesetSchemaText(hydratedSchemaText);
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
        : typeof compileConfig.commands_text === 'string'
          ? compileConfig.commands_text
          : ''
    );
    setStageApproved(Boolean(review.approved));
    setStageRejected(Boolean(review.rejected));
    setStageIncludeRepoContext(hydratedInjectContext);
    setStageIncludeChangesetSchema(hydratedInjectChangesetSchema);
    setStageIncludeApplyError(
      typeof selectedAutomation.include_apply_error === 'boolean'
        ? Boolean(selectedAutomation.include_apply_error)
        : step.step_type === 'code'
    );
    setStageIncludeCompileError(
      typeof selectedAutomation.include_compile_error === 'boolean'
        ? Boolean(selectedAutomation.include_compile_error)
        : step.step_type === 'code'
    );
    setStageAutoApplyChangeset(
      typeof selectedAutomation.auto_apply_changeset === 'boolean'
        ? Boolean(selectedAutomation.auto_apply_changeset)
        : Boolean((step.execution?.changeset_apply as Record<string, unknown> | undefined)?.enabled ?? step.step_type === 'code')
    );
    const inferenceSessions = ((inferenceConfig.sessions as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    const inferenceSessionName = typeof inferenceConfig.default_session === 'string' ? inferenceConfig.default_session : '';
    const inferenceSession = inferenceSessionName ? ((inferenceSessions[inferenceSessionName] as Record<string, unknown> | undefined) ?? {}) : {};
    setInferenceTransport(inferenceSession.transport === 'browser' ? 'browser' : 'api');
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
    setStageRepoContextInlinePrompt(Boolean(repoContext.inline_repo_context_in_prompt));
  }, [selectedStageHydrationKey, selectedRun?.context, selectedStageState]);

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
      apply_error: stageApplyError,
      compile_error: stageCompileError
    };
    const promptFragmentEnabled: Record<string, unknown> = {
      apply_error: stageIncludeApplyError && Boolean(stageApplyError.trim()),
      compile_error: stageIncludeCompileError && Boolean(stageCompileError.trim())
    };
    const currentGlobalState = ((selectedRun?.context?.workflow_engine as Record<string, unknown> | undefined)?.global_state as Record<string, unknown> | undefined) ?? {};
    const currentResources = (currentGlobalState.resources as Record<string, unknown> | undefined) ?? {};
    const currentCapabilities = (currentGlobalState.capabilities as Record<string, unknown> | undefined) ?? {};
    const currentInference = (currentCapabilities.inference as Record<string, unknown> | undefined) ?? {};
    const currentInferenceBrowser = (currentInference.browser as Record<string, unknown> | undefined) ?? {};
    const currentContextExport = (currentCapabilities.context_export as Record<string, unknown> | undefined) ?? {};
    const currentCompileCommands = (currentCapabilities.compile_commands as Record<string, unknown> | undefined) ?? {};
    const currentChangesetSchema = (currentCapabilities.changeset_schema as Record<string, unknown> | undefined) ?? {};
    const currentGatewayChangeset = (currentCapabilities['gateway_model/changeset'] as Record<string, unknown> | undefined) ?? {};
    const currentPlanner = (currentCapabilities.planner as Record<string, unknown> | undefined) ?? {};

    return {
      ...currentGlobalState,
      resources: {
        ...currentResources,
        repo: {
          ...((currentResources.repo as Record<string, unknown> | undefined) ?? {}),
          git_ref: stageRepoContextGitRef || 'WORKTREE'
        }
      },
      capabilities: {
        ...currentCapabilities,
        inference: {
          ...currentInference,
          transport: inferenceTransport,
          prompt_fragments: {
            ...((currentInference.prompt_fragments as Record<string, unknown> | undefined) ?? {}),
            ...promptFragments
          },
          prompt_fragment_enabled: {
            ...((currentInference.prompt_fragment_enabled as Record<string, unknown> | undefined) ?? {}),
            ...promptFragmentEnabled
          },
          browser: {
            ...currentInferenceBrowser,
            ...(browserCdpUrl.trim() ? { cdp_url: browserCdpUrl.trim() } : {}),
            target_url: browserTargetUrl,
            session_id: browserSessionId.trim() || null
          }
        },
        planner: {
          fragment_armed: Boolean(currentPlanner.fragment_armed),
          schema_armed: Boolean(currentPlanner.schema_armed),
          auto_apply_armed: Boolean(currentPlanner.auto_apply_armed),
          selected_feature_id: currentPlanner.selected_feature_id ?? null,
          supervisor_run_id: currentPlanner.supervisor_run_id ?? null,
          schema_id: 'supervisor_feature_plan_item_v1',
          preserve_rough_definition: true
        },
        context_export: {
          ...currentContextExport,
          enabled: stageIncludeRepoContext,
          git_ref: stageRepoContextGitRef || 'WORKTREE',
          include_files: includeFiles,
          exclude_regex: excludeRegex,
          save_path: stageRepoContextSavePath || '/tmp/repo_context.txt',
          skip_binary: stageRepoContextSkipBinary,
          skip_gitignore: stageRepoContextSkipGitignore,
          include_staged_diff: stageRepoContextIncludeStagedDiff,
          include_unstaged_diff: stageRepoContextIncludeUnstagedDiff,
          inline_repo_context_in_prompt: stageRepoContextInlinePrompt
        },
        changeset_schema: {
          ...currentChangesetSchema,
          enabled: stageIncludeChangesetSchema,
          schema: stageChangesetSchemaText
        },
        'gateway_model/changeset': {
          ...currentGatewayChangeset
        },

        compile_commands: {
          ...currentCompileCommands,
          commands: compileCommands
        }
      }
    } as Record<string, unknown>;
  }




  const repoTreeScopeKey = useMemo(() => [
    view,
    view === 'builder' ? repoRef.trim() : selectedRun?.id ?? '',
    stageRepoContextGitRef.trim() || 'WORKTREE',
    String(stageRepoContextSkipBinary),
    String(stageRepoContextSkipGitignore)
  ].join('|'), [
    view,
    repoRef,
    selectedRun?.id,
    stageRepoContextGitRef,
    stageRepoContextSkipBinary,
    stageRepoContextSkipGitignore
  ]);

  useEffect(() => {
    if (!repoContextConfigOpen) return;
    setTreeRootData(null);
    setTreeChildrenByParent({});
    setSelectedRepoDirs(new Set());
    void loadRepoTreeForActiveRef('', true);
  }, [repoContextConfigOpen, repoTreeScopeKey]);



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
    const payload = incoming.payload as Record<string, unknown>;
    const snapshotContext = payload.final_context ?? payload.run_context ?? payload.prepared_context;
    if (snapshotContext && typeof snapshotContext === 'object' && !Array.isArray(snapshotContext)) {
      const snapshotStatus = typeof payload.prepared_status === 'string'
        ? payload.prepared_status as WorkflowRunStatus
        : typeof payload.status === 'string'
          ? payload.status as WorkflowRunStatus
          : undefined;
      const snapshotStepId = typeof payload.current_step_id === 'string'
        ? payload.current_step_id
        : incoming.step_id;
      setRuns((prev) => prev.map((run) => run.id === runId ? {
        ...run,
        ...(snapshotStatus ? { status: snapshotStatus } : {}),
        current_step_id: snapshotStepId ?? run.current_step_id,
        context: snapshotContext as Record<string, unknown>
      } : run));
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

  async function refreshRunDetailsOnOpen(runId: string) {
    const [run, runEvents] = await Promise.all([openWorkflowRun(runId), listRunEvents(runId)]);
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
    setSelectedRunId(runId);
    setView('monitor');
    setMonitorView('workflow_detail');
    void refreshRunDetailsOnOpen(runId);
    void refreshLiveMonitor(runId);
  }

  function backToWorkflowList() {
    setMonitorView('workflow_list');
  }

  async function openBuilder() {
    try {
      setBusy(true);
      setError(null);
      await refreshRunsAndTemplates();
      setView('builder');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleSaveTemplate() {
    try {
      setBusy(true);
      setError(null);
      const parsed = builderMode === 'json'
        ? (JSON.parse(jsonDraft) as WorkflowTemplateDefinition)
        : applyBuilderGlobalsToDefinition(compiledBuilderDefinition, builderGlobals);
      if (!parsed) {
        throw new Error('Builder has not produced a compiled workflow definition yet.');
      }
      const template = await createTemplate({ name: workflowName, description: workflowDescription, repo_ref: repoRef, definition: parsed });
      await refreshRunsAndTemplates();
      setSelectedTemplateId(template.id);
      setTemplateModalOpen(false);
      if (createRunAfterSave) {
        const run = await createRun({
          template_id: template.id,
          title: workflowName,
          repo_ref: repoRef,
          definition: parsed,
          context: {
            workflow_engine: {}
          }
        });
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
      const template = templates.find((item) => item.id === templateId);
      const run = await createRun({
        template_id: templateId,
        title: workflowName,
        repo_ref: repoRef,
        definition: template?.definition,
        context: {
          workflow_engine: {}
        }
      });
      await refreshRunsAndTemplates(run.id);
      setView('monitor');
      setMonitorView('workflow_detail');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleCreateWorkflow() {
    try {
      setBusy(true);
      setError(null);
      const parsed = builderMode === 'json'
        ? (JSON.parse(jsonDraft) as WorkflowTemplateDefinition)
        : applyBuilderGlobalsToDefinition(compiledBuilderDefinition, builderGlobals);
      if (!parsed) {
        throw new Error('Builder has not produced a compiled workflow definition yet.');
      }
      const template = await createTemplate({ name: workflowName, description: workflowDescription, repo_ref: repoRef, definition: parsed });
      const run = await createRun({
        template_id: template.id,
        title: workflowName,
        repo_ref: repoRef,
        definition: parsed,
        context: {
          workflow_engine: {}
        }
      });
      await refreshRunsAndTemplates(run.id);
      setSelectedTemplateId(template.id);
      setTemplateModalOpen(false);
      setView('monitor');
      setMonitorView('workflow_detail');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }


  function handleLoadTemplateMetadata(templateId?: string | null) {
    if (!templateId) {
      setError('Select a template first.');
      return;
    }
    const template = templates.find((item) => item.id === templateId);
    if (!template) {
      setError('Selected template was not found.');
      return;
    }
    setError(null);
    setSelectedTemplateId(template.id);
    setWorkflowName(template.name);
    setWorkflowDescription(template.description);
    setRepoRef(template.repo_ref);
    setCompiledBuilderDefinition(template.definition);
    setLoadedTemplateDefinition(template.definition);
    setBuilderLoadRevision((prev) => prev + 1);
    setBuilderGlobals(normalizeBuilderGlobals(template.definition?.globals ?? null));
    setJsonDraft(JSON.stringify(template.definition, null, 2));
    setBuilderMode('builder');
    setLoadTemplateOpen(false);
  }

  async function handleDeleteTemplate(templateId: string) {
    try {
      setBusy(true);
      setError(null);
      await deleteTemplate(templateId);
      const nextTemplateId = selectedTemplateId === templateId ? null : selectedTemplateId;
      await refreshRunsAndTemplates();
      setSelectedTemplateId(nextTemplateId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleStartRun() {
    if (!selectedRunId) return;
    const runId = selectedRunId;
    try {
      setBusy(true);
      setError(null);
      const prepared = await prepareWorkflowStage(runId);
      const preparedRun = prepared.run;
      if (preparedRun) {
        setRuns((prev) => [
          preparedRun,
          ...prev.filter((item) => item.id !== preparedRun.id)
        ]);
        setSelectedRunId(preparedRun.id);
      } else {
        await refreshRunDetails(runId);
      }
      await startWorkflowRun(runId);
      await refreshRunDetails(runId);
      await refreshLiveMonitor(runId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  const currentAutonomousStep = selectedRunDefinition?.steps.find((step) => step.id === selectedRun?.current_step_id)
    ?? selectedRunDefinition?.steps[0]
    ?? null;
  const canRunCurrentStageAutomatically = Boolean(
    currentAutonomousStep
      && ((((currentAutonomousStep.advancement as Record<string, unknown> | undefined)?.auto_run_on_enter) === true)
        || currentAutonomousStep.automation_mode === 'automatic')
  );

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
    const runId = selectedRunId;
    try {
      setPauseRequestBusy(true);
      setError(null);
      await pauseWorkflowRun(runId);
      await refreshRunDetails(runId);
      await refreshLiveMonitor(runId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setPauseRequestBusy(false);
    }
  }

  async function handleForceWaitRun() {
    if (!selectedRunId) return;
    try {
      setBusy(true);
      setError(null);
      await forceWaitWorkflowRun(selectedRunId);
      await refreshRunDetails(selectedRunId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function patchGlobalCapabilityState(fragment: Record<string, unknown>) {
    if (!selectedRun?.id) return;
    const currentGlobalState = ((selectedRun.context?.workflow_engine as Record<string, unknown> | undefined)?.global_state as Record<string, unknown> | undefined) ?? {};
    await patchWorkflowGlobalState(selectedRun.id, {
      ...currentGlobalState,
      ...fragment
    });
    await refreshRunDetails(selectedRun.id);
  }

  async function patchPlannerCapabilityState(patch: Record<string, unknown>) {
    if (!selectedRun?.id) return;
    const currentPlanner = (sharedPlannerFragmentState ?? {}) as Record<string, unknown>;
    const nextSelectedFeatureId = Object.prototype.hasOwnProperty.call(patch, 'selected_feature_id')
      ? patch.selected_feature_id
      : currentPlanner.selected_feature_id ?? selectedPlannerFeatureId ?? null;
    const normalizedSelectedFeatureId = typeof nextSelectedFeatureId === 'string' && nextSelectedFeatureId.trim()
      ? nextSelectedFeatureId
      : null;

    await patchGlobalCapabilityState({
      capabilities: {
        planner: {
          ...currentPlanner,
          ...patch,
          fragment_armed: Boolean((Object.prototype.hasOwnProperty.call(patch, 'fragment_armed') ? patch.fragment_armed : currentPlanner.fragment_armed) && normalizedSelectedFeatureId),
          selected_feature_id: normalizedSelectedFeatureId,
          supervisor_run_id: patch.supervisor_run_id ?? currentPlanner.supervisor_run_id ?? plannerSupervisorRunId,
          schema_id: 'supervisor_feature_plan_item_v1',
          preserve_rough_definition: true
        }
      }
    });
  }

  async function openRepoSupervisorPlanner() {
    const rootRepoPath = (selectedRun?.repo_ref ?? repoRef ?? '').trim();
    if (!rootRepoPath) {
      setError('Repo path is required before opening the planner.');
      return;
    }

    try {
      setError(null);
      const rootParts = rootRepoPath.replace(/\\/g, '/').split('/').filter(Boolean);
      const repoName = rootParts[rootParts.length - 1] ?? 'Repo';
      const response = await ensureSupervisorPlannerRun({
        root_repo_path: rootRepoPath,
        title: `${repoName} Planner`
      });
      setSupervisorPlannerRun(response.supervisor_run);
      setSupervisorPlannerOpen(true);
      if (selectedRun?.id) {
        await patchPlannerCapabilityState({
          supervisor_run_id: response.supervisor_run.id
        });
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  function loadBuilderRepoContextConfig() {
    const globals = ((compiledBuilderDefinition?.globals as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    const capabilities = ((globals.capabilities as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    const contextExport = ((capabilities.context_export as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;

    const includeFiles = Array.isArray(contextExport.include_files)
      ? contextExport.include_files.filter((value): value is string => typeof value === 'string')
      : [];
    const excludeRegex = Array.isArray(contextExport.exclude_regex)
      ? contextExport.exclude_regex.filter((value): value is string => typeof value === 'string')
      : [];

    setStageRepoContextGitRef(
      typeof contextExport.git_ref === 'string' && contextExport.git_ref.trim()
        ? contextExport.git_ref
        : 'WORKTREE'
    );
    syncRepoSelectionState(includeFiles);
    setStageRepoContextExcludeRegexText(excludeRegex.join('\n'));
    setStageRepoContextSavePath(
      typeof contextExport.save_path === 'string' && contextExport.save_path.trim()
        ? contextExport.save_path
        : '/tmp/repo_context.txt'
    );
    setStageRepoContextSkipBinary(typeof contextExport.skip_binary === 'boolean' ? contextExport.skip_binary : true);
    setStageRepoContextSkipGitignore(typeof contextExport.skip_gitignore === 'boolean' ? contextExport.skip_gitignore : true);
    setStageRepoContextIncludeStagedDiff(Boolean(contextExport.include_staged_diff));
    setStageRepoContextIncludeUnstagedDiff(Boolean(contextExport.include_unstaged_diff));
    setStageRepoContextInlinePrompt(Boolean(contextExport.inline_repo_context_in_prompt));
  }

  function loadBuilderChangesetSchemaConfig() {
    const globals = ((compiledBuilderDefinition?.globals as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    const capabilities = ((globals.capabilities as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    const changesetSchema = ((capabilities.changeset_schema as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    setStageChangesetSchemaText(typeof changesetSchema.schema === 'string' ? changesetSchema.schema : '');
  }

  function loadBuilderApplyChangesetConfig() {
    const globals = ((compiledBuilderDefinition?.globals as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    const capabilities = ((globals.capabilities as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    const gatewayChangeset = ((capabilities['gateway_model/changeset'] as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    setGlobalApplyChangesetText(typeof gatewayChangeset.draft === 'string' ? gatewayChangeset.draft : '');
  }

  function loadBuilderGitPatchPayloadConfig() {
    const globals = ((compiledBuilderDefinition?.globals as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    const capabilities = ((globals.capabilities as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    const gitPatchPayload = ((capabilities.git_patch_payload as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    setGitPatchPayloadMode(gitPatchPayload.mode === 'apply' ? 'apply' : 'generate');
    setGitPatchPayloadScope(gitPatchPayload.scope === 'staged' || gitPatchPayload.scope === 'unstaged' ? gitPatchPayload.scope : 'both');
    setGitPatchPayloadText(typeof gitPatchPayload.payload_text === 'string' ? gitPatchPayload.payload_text : '');
    setGitPatchPayloadReverse(Boolean(gitPatchPayload.reverse));
  }

  async function handleSaveGlobalChangesetSchema() {
    if (view === 'builder') {
      saveBuilderCapability('changeset_schema', {
        schema: stageChangesetSchemaText
      });
      setChangesetSchemaConfigOpen(false);
      return;
    }

    const currentGlobalState = ((selectedRun?.context?.workflow_engine as Record<string, unknown> | undefined)?.global_state as Record<string, unknown> | undefined) ?? {};
    const currentCapabilities = (currentGlobalState.capabilities as Record<string, unknown> | undefined) ?? {};
    const currentChangesetSchema = (currentCapabilities.changeset_schema as Record<string, unknown> | undefined) ?? {};
    await patchGlobalCapabilityState({
      capabilities: {
        ...currentCapabilities,
        changeset_schema: {
          ...currentChangesetSchema,
          schema: stageChangesetSchemaText
        }
      }
    });
    setChangesetSchemaConfigOpen(false);
  }

  async function handleSaveGlobalApplyChangeset() {
    if (view === 'builder') {
      saveBuilderCapability('gateway_model/changeset', {
        draft: globalApplyChangesetText
      });
      setGlobalApplyChangesetOpen(false);
      return;
    }

    const currentGlobalState = ((selectedRun?.context?.workflow_engine as Record<string, unknown> | undefined)?.global_state as Record<string, unknown> | undefined) ?? {};
    const currentCapabilities = (currentGlobalState.capabilities as Record<string, unknown> | undefined) ?? {};
    const currentGatewayChangeset = (currentCapabilities['gateway_model/changeset'] as Record<string, unknown> | undefined) ?? {};
    await patchGlobalCapabilityState({
      capabilities: {
        ...currentCapabilities,
        'gateway_model/changeset': {
          ...currentGatewayChangeset,
          draft: globalApplyChangesetText
        }
      }
    });
    setGlobalApplyChangesetOpen(false);
  }

  async function handleRunGitPatchPayload() {
    if (!selectedRun?.id) {
      setGitPatchPayloadStatus('Select or create a workflow run before using git patch payload.');
      return;
    }

    try {
      setGitPatchPayloadBusy(true);
      setGitPatchPayloadStatus(null);
      const input = gitPatchPayloadMode === 'apply'
        ? {
            mode: 'apply',
            scope: gitPatchPayloadScope,
            payload_text: gitPatchPayloadText,
            reverse: gitPatchPayloadReverse,
          }
        : {
            mode: 'generate',
            scope: gitPatchPayloadScope,
          };
      const json = await executeWorkflowCapability(selectedRun.id, 'git_patch_payload', input);
      const results = Array.isArray(json.results) ? json.results : [];
      const first = results[0] as Record<string, unknown> | undefined;
      const payload = first?.payload as Record<string, unknown> | undefined;

      if (gitPatchPayloadMode === 'generate') {
        const payloadText = typeof payload?.payload_text === 'string' ? payload.payload_text : '';
        if (!payloadText) {
          throw new Error('Git patch payload response did not include payload_text.');
        }
        setGitPatchPayloadText(payloadText);
        setGitPatchPayloadStatus('Git patch payload generated.');
      } else {
        setGitPatchPayloadStatus('Git patch payload applied.');
        await refreshRunDetails(selectedRun.id);
      }
    } catch (err) {
      setGitPatchPayloadStatus(err instanceof Error ? err.message : String(err));
    } finally {
      setGitPatchPayloadBusy(false);
    }
  }

  async function refreshChangesetHistory() {
    if (!selectedRun?.id) {
      setGlobalApplyChangesetHistory([]);
      return;
    }
    try {
      setGlobalApplyChangesetHistoryBusy(true);
      const rows = await listWorkflowChangesets(selectedRun.workflow_key || selectedRun.id, 50);
      setGlobalApplyChangesetHistory(rows);
    } catch {
      setGlobalApplyChangesetHistory([]);
    } finally {
      setGlobalApplyChangesetHistoryBusy(false);
    }
  }

  async function handleLoadGlobalChangesetAttempt(item: ChangesetAttemptSummary, mode: 'input' | 'output') {
    const workflowKey = selectedRun?.workflow_key || selectedRun?.id;
    if (!workflowKey) return;
    try {
      setGlobalApplyChangesetHistoryBusy(true);
      const detail = await getWorkflowChangeset(workflowKey, item.id);
      setGlobalApplyChangesetText(detail.normalized_payload_json || detail.payload_text || '');
      setGlobalApplyChangesetResult(changesetOutputWithoutPayload(detail.result_json));
      setManualCapabilityResponse('');
      setGlobalApplyChangesetPanelMode(mode);
      setManualCapabilityStatus(mode === 'input' ? 'Loaded changeset input.' : 'Loaded changeset output.');
    } catch (err) {
      setManualCapabilityStatus(`Error loading changeset: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setGlobalApplyChangesetHistoryBusy(false);
    }
  }

  async function handleApplyGlobalChangeset() {
    if (!selectedRun?.id) {
      setManualCapabilityStatus('Select or create a workflow run before applying a changeset.');
      return;
    }
    await runManualCapability(async () => {
      const json = await applyWorkflowChangeset(selectedRun.workflow_key || selectedRun.id, {
        git_ref: 'WORKTREE',
        payload_text: globalApplyChangesetText,
      });
      const cleanJson = changesetOutputWithoutPayload(json);
      setGlobalApplyChangesetResult(cleanJson);
      setGlobalApplyChangesetPanelMode('output');
      await refreshChangesetHistory();
      return cleanJson;
    }, 'Changeset applied.');
  }

  function globalApplyResultText() {
    if (!globalApplyChangesetResult) return '';
    return JSON.stringify(changesetOutputWithoutPayload(globalApplyChangesetResult), null, 2);
  }

  function changesetOutputWithoutPayload(value: unknown): ApplyChangesetResponse {
    const noisyKeys = new Set([
      'normalized_payload_json',
      'normalized_payload',
      'payload_text',
      'payload',
      'input',
      'changeset_payload'
    ]);
    const strip = (input: unknown): unknown => {
      if (Array.isArray(input)) return input.map(strip);
      if (!input || typeof input !== 'object') return input;
      return Object.fromEntries(
        Object.entries(input as Record<string, unknown>)
          .filter(([key]) => !noisyKeys.has(key))
          .map(([key, next]) => [key, strip(next)])
      );
    };
    const stripped = strip(value);
    return stripped && typeof stripped === 'object'
      ? stripped as ApplyChangesetResponse
      : { result: stripped } as ApplyChangesetResponse;
  }

  function compactRepoLabel(repoRef: string) {
    const normalized = (repoRef || '').replace(/\\/g, '/').replace(/\/+$/g, '');
    return normalized.split('/').filter(Boolean).pop() || repoRef || 'repo';
  }

  function changesetFileActionSummary(item: ChangesetAttemptSummary) {
    if (item.file_action_summaries?.length) {
      return item.file_action_summaries.map((file) => ({
        path: file.path,
        applied: file.applied || 0,
        failed: file.failed || 0,
        total: file.total || 0,
      }));
    }

    const successFiles = item.successful_files || [];
    const failedFiles = item.failed_files || [];
    const paths = Array.from(new Set([...successFiles, ...failedFiles]));
    const totalActions = Math.max(0, item.total_actions || 0);
    const appliedActions = Math.max(0, item.applied_actions || 0);
    const failedActions = Math.max(0, item.failed_actions || Math.max(0, totalActions - appliedActions));
    if (!paths.length) return [];
    if (paths.length === 1) return [{ path: paths[0], applied: appliedActions, failed: failedActions, total: totalActions }];
    return paths.map((path, index) => {
      const successful = successFiles.includes(path);
      const failed = failedFiles.includes(path);
      const baseTotal = Math.floor(totalActions / paths.length);
      const totalRemainder = totalActions % paths.length;
      const total = baseTotal + (index < totalRemainder ? 1 : 0);
      const applied = successful ? Math.max(1, Math.floor(appliedActions / Math.max(1, successFiles.length))) : 0;
      const failedCount = failed ? Math.max(1, Math.floor(failedActions / Math.max(1, failedFiles.length))) : Math.max(0, total - applied);
      return { path, applied: Math.min(applied, total), failed: Math.min(failedCount, total), total };
    });
  }

  async function copyTextToClipboard(text: string, label: string) {
    if (!text) return;
    await navigator.clipboard.writeText(text);
    setManualCapabilityStatus(`${label} copied.`);
  }

  function newGlobalChangeset() {
    setGlobalApplyChangesetText('');
    setGlobalApplyChangesetResult(null);
    setGlobalApplyChangesetPanelMode('input');
    setManualCapabilityStatus(null);
    setManualCapabilityResponse('');
  }

  function visibleGlobalChangesetPanelText() {
    return globalApplyChangesetPanelMode === 'output'
      ? (manualCapabilityResponse || globalApplyResultText())
      : globalApplyChangesetText;
  }

  function changesetStatusColor(status: string) {
    switch (status) {
      case 'applied':
        return 'green';
      case 'partial':
        return 'yellow';
      case 'failed':
        return 'red';
      default:
        return 'gray';
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

  function resolveActiveRepoRef(): string {
    return view === 'builder' ? repoRef.trim() : '';
  }

  async function listRepoTreeForCurrentScope(basePath: string): Promise<RepoTreeResponse> {
    const gitRef = stageRepoContextGitRef.trim() || 'WORKTREE';
    const options = {
      basePath,
      skipBinary: stageRepoContextSkipBinary,
      skipGitignore: stageRepoContextSkipGitignore
    };

    if (view !== 'builder' && selectedRun?.id) {
      return listWorkflowRepoTree(selectedRun.id, gitRef, options);
    }

    const activeRepoRef = repoRef.trim();
    if (!activeRepoRef) {
      throw new Error('Set a repo path to browse files.');
    }

    return listRepoTree(activeRepoRef, gitRef, options);
  }

  async function loadRepoTreeForActiveRef(basePath: string, replaceRoot = false) {
    if (view !== 'builder' && !selectedRun?.id) {
      setTreeError('Select a workflow run to browse files.');
      return;
    }
    if (view === 'builder' && !repoRef.trim()) {
      setTreeError('Set a repo path to browse files.');
      return;
    }

    if (loadingTreeDirs.has(basePath)) return;

    setTreeError(null);
    if (replaceRoot) setTreeBusy(true);
    setLoadingTreeDirs((prev) => {
      const next = new Set(prev);
      next.add(basePath);
      return next;
    });

    try {
      const data = await listRepoTreeForCurrentScope(basePath);

      if (replaceRoot) {
        setTreeRootData(data);
        setTreeChildrenByParent({ '': data.entries });
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

  async function loadTreeSubtree(basePath: string): Promise<{ children: Record<string, RepoTreeEntry[]>; files: string[] }> {
    const data = await listRepoTreeForCurrentScope(basePath);

    const children: Record<string, RepoTreeEntry[]> = {
      [basePath]: data.entries
    };
    const files: string[] = [];

    for (const entry of data.entries) {
      if (entry.kind === 'file') {
        files.push(entry.path);
      } else if (entry.has_children) {
        const nested = await loadTreeSubtree(entry.path);
        Object.assign(children, nested.children);
        files.push(...nested.files);
      }
    }

    return { children, files };
  }

  async function toggleDirectory(entry: RepoTreeEntry, checked: boolean) {
    if (view !== 'builder' && !selectedRun?.id) return;
    if (view === 'builder' && !repoRef.trim()) return;

    if (checked) {
      const nested = await loadTreeSubtree(entry.path);
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
  }  const composedInferencePrompt = useMemo(() => {
    if (selectedWorkflowStep?.id === 'compile') {
      return stageCompileCommandsText.trim()
        ? `### COMPILE COMMANDS\n${stageCompileCommandsText.trim()}`
        : '';
    }

    const parts: string[] = [];
    if (stageIncludeRepoContext) parts.push('### REPO CONTEXT\nAttached repo context from backend export');
    if (selectedWorkflowStep?.step_type === 'code' && stageIncludeChangesetSchema) {
      parts.push(`### CHANGESET SCHEMA\n${stageChangesetSchemaText.trim() || 'Use ChangeSet JSON version 1. Return only the JSON payload.'}`);
    }
    if (selectedWorkflowStep?.step_type === 'code' && stageIncludeApplyError && stageApplyError.trim()) parts.push(`### APPLY ERROR\n${stageApplyError.trim()}`);
    if (selectedWorkflowStep?.step_type === 'code' && stageIncludeCompileError && stageCompileError.trim()) parts.push(`### COMPILE ERROR\n${stageCompileError.trim()}`);
    if (selectedWorkflowStep?.step_type === 'review' && stageReviewNotes.trim()) parts.push(`### REVIEW NOTES\n${stageReviewNotes.trim()}`);
    if (stageUserInput.trim()) parts.push(`### USER INPUT\n${stageUserInput.trim()}`);
    return parts.join('\n\n');
  }, [selectedWorkflowStep?.step_type, stageCompileCommandsText, stageIncludeRepoContext, stageIncludeChangesetSchema, stageChangesetSchemaText, stageIncludeApplyError, stageApplyError, stageIncludeCompileError, stageCompileError, stageReviewNotes, stageUserInput]);

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

  function getBoolean(value: unknown): boolean | null {
  return typeof value === 'boolean' ? value : null;
}

function getString(value: unknown): string | null {
  return typeof value === 'string' ? value : null;
}

type MarkdownSegment =
  | { kind: 'heading'; level: number; text: string }
  | { kind: 'code'; language: string; text: string }
  | { kind: 'paragraph'; text: string };

function parseLightMarkdown(input: string): MarkdownSegment[] {
  const lines = input.split(/\r?\n/);
  const segments: MarkdownSegment[] = [];

  let paragraph: string[] = [];
  let inCode = false;
  let codeLanguage = '';
  let codeLines: string[] = [];

  function flushParagraph() {
    const text = paragraph.join('\n').trim();
    if (text) {
      segments.push({ kind: 'paragraph', text });
    }
    paragraph = [];
  }

  for (const line of lines) {
    const fence = line.match(/^```([A-Za-z0-9_-]*)\s*$/);

    if (fence) {
      if (inCode) {
        segments.push({
          kind: 'code',
          language: codeLanguage,
          text: codeLines.join('\n'),
        });
        inCode = false;
        codeLanguage = '';
        codeLines = [];
      } else {
        flushParagraph();
        inCode = true;
        codeLanguage = fence[1] ?? '';
        codeLines = [];
      }
      continue;
    }

    if (inCode) {
      codeLines.push(line);
      continue;
    }

    const heading = line.match(/^(#{1,6})\s+(.+)$/);
    if (heading) {
      flushParagraph();
      segments.push({
        kind: 'heading',
        level: heading[1].length,
        text: heading[2],
      });
      continue;
    }

    if (!line.trim()) {
      flushParagraph();
      continue;
    }

    paragraph.push(line);
  }

  if (inCode) {
    segments.push({
      kind: 'code',
      language: codeLanguage,
      text: codeLines.join('\n'),
    });
  }

  flushParagraph();
  return segments;
}

function looksLikeChangesetPayload(text: string): boolean {
  const trimmed = text.trim();
  if (!trimmed) return false;

  const normalized = trimmed
    .replace(/^```(?:json|changeset)?\s*/i, '')
    .replace(/```$/i, '')
    .replace(/\\"/g, '"');

  return (
    normalized.includes('"version"') &&
    normalized.includes('"operations"') &&
    normalized.includes('"op"')
  );
}

function compactPreviewTitle(heading: string | null, text: string): string | null {
  const normalizedHeading = heading?.trim().toLowerCase() ?? '';

  if (normalizedHeading === 'changeset schema') {
    return 'Changeset schema';
  }

  if (normalizedHeading === 'output' && looksLikeChangesetPayload(text)) {
    return 'Changeset output';
  }

  if (!normalizedHeading && looksLikeChangesetPayload(text)) {
    return 'Changeset payload';
  }

  return null;
}

function normalizeCompactPreviewContent(content: string): string {
  const trimmed = content.trim();
  const withoutLanguagePrefix = trimmed.replace(/^(JSON|json|CHANGESET|changeset)\s*(?=\{)/, '');

  if (withoutLanguagePrefix.includes('\\n') && !withoutLanguagePrefix.includes('\n')) {
    return withoutLanguagePrefix
      .replace(/\\r\\n/g, '\n')
      .replace(/\\n/g, '\n')
      .replace(/\\t/g, '  ')
      .replace(/\\"/g, '"');
  }

  return withoutLanguagePrefix.replace(/\\"/g, '"');
}

function CompactPreviewBlock(props: { title: string; content: string; language?: string }) {
  const [opened, setOpened] = useState(false);
  const displayContent = normalizeCompactPreviewContent(props.content);
  const lineCount = displayContent ? displayContent.split(/\r?\n/).length : 0;

  return (
    <>
      <Box
        p="sm"
        style={{
          border: '1px solid var(--mantine-color-dark-4)',
          borderRadius: 12,
          background: 'rgba(255,255,255,0.03)',
        }}
      >
        <Group justify="space-between" align="center" gap="sm" wrap="nowrap">
          <Stack gap={2} style={{ minWidth: 0 }}>
            <Text fw={600} size="sm">{props.title}</Text>
            <Text size="xs" c="dimmed">
              Hidden by default · {lineCount.toLocaleString()} lines · {displayContent.length.toLocaleString()} chars
            </Text>
          </Stack>
          <Button size="xs" variant="light" onClick={() => setOpened(true)}>
            Open
          </Button>
        </Group>
      </Box>

      <Modal
        opened={opened}
        onClose={() => setOpened(false)}
        title={props.title}
        size="90vw"
        centered
        scrollAreaComponent={ScrollArea.Autosize}
        styles={{
          content: { height: '88vh' },
          body: { height: 'calc(88vh - 72px)' },
        }}
      >
        <Stack gap="xs" h="100%">
          <Group justify="space-between" align="center">
            <Text size="xs" c="dimmed">
              {lineCount.toLocaleString()} lines · {displayContent.length.toLocaleString()} chars
            </Text>
            <Button size="xs" variant="light" onClick={() => setOpened(false)}>
              Close
            </Button>
          </Group>

          <Box
            component="pre"
            p="md"
            style={{
              flex: 1,
              minHeight: 0,
              margin: 0,
              border: '1px solid var(--mantine-color-dark-4)',
              borderRadius: 12,
              overflow: 'auto',
              whiteSpace: 'pre-wrap',
              overflowWrap: 'anywhere',
              wordBreak: 'break-word',
              fontFamily:
                'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, Liberation Mono, monospace',
              fontSize: 13,
              lineHeight: 1.55,
              background: 'rgba(0,0,0,0.22)',
            }}
          >
            {props.language ? (
              <Text component="div" size="xs" c="dimmed" mb="xs">
                {props.language}
              </Text>
            ) : null}
            <code>{displayContent}</code>
          </Box>
        </Stack>
      </Modal>
    </>
  );
}

function MarkdownPreviewContent(props: { content: string; emptyText: string }) {
  const text = props.content || props.emptyText;
  const segments = parseLightMarkdown(text);
  let activeHeading: string | null = null;
  const hiddenIndexes = new Set<number>();

  return (
    <Stack gap="sm">
      {segments.map((segment, index) => {
        if (hiddenIndexes.has(index)) {
          return null;
        }

        if (segment.kind === 'heading') {
          activeHeading = segment.text;
          const next = segments[index + 1];
          const nextText = next?.kind === 'paragraph' || next?.kind === 'code' ? next.text : '';
          const compactTitle = compactPreviewTitle(segment.text, nextText);

          if (compactTitle && next) {
            hiddenIndexes.add(index + 1);
            return (
              <CompactPreviewBlock
                key={index}
                title={compactTitle}
                content={nextText}
                language={next.kind === 'code' ? next.language : undefined}
              />
            );
          }

          return (
            <Title
              key={index}
              order={Math.min(Math.max(segment.level + 2, 4), 6) as 4 | 5 | 6}
              mt={index === 0 ? 0 : 'sm'}
            >
              {segment.text}
            </Title>
          );
        }

        if (segment.kind === 'code') {
          const compactTitle = compactPreviewTitle(activeHeading, segment.text);
          if (compactTitle) {
            return (
              <CompactPreviewBlock
                key={index}
                title={compactTitle}
                content={segment.text}
                language={segment.language}
              />
            );
          }

          return (
            <Box
              key={index}
              component="pre"
              p="md"
              style={{
                margin: 0,
                border: '1px solid var(--mantine-color-dark-4)',
                borderRadius: 12,
                overflowX: 'auto',
                whiteSpace: 'pre',
                fontFamily:
                  'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, Liberation Mono, monospace',
                fontSize: 13,
                lineHeight: 1.55,
                background: 'rgba(0,0,0,0.22)',
              }}
            >
              {segment.language ? (
                <Text component="div" size="xs" c="dimmed" mb="xs">
                  {segment.language}
                </Text>
              ) : null}
              <code>{segment.text}</code>
            </Box>
          );
        }

        const compactTitle = compactPreviewTitle(activeHeading, segment.text);
        if (compactTitle) {
          return (
            <CompactPreviewBlock
              key={index}
              title={compactTitle}
              content={segment.text}
            />
          );
        }

        return (
          <Text
            key={index}
            size="sm"
            style={{
              whiteSpace: 'pre-wrap',
              overflowWrap: 'anywhere',
              wordBreak: 'break-word',
              lineHeight: 1.7,
            }}
          >
            {segment.text}
          </Text>
        );
      })}
    </Stack>
  );
}

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
        <Box p="md" h="100%" style={{ flex: 1, border: '1px solid var(--mantine-color-dark-4)', borderRadius: 12, minHeight: 220, overflow: 'auto', background: 'linear-gradient(180deg, rgba(255,255,255,0.02), rgba(255,255,255,0.01))' }}>
          <MarkdownPreviewContent content={content} emptyText={emptyText} />
        </Box>
      </Stack>
    );
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

  function renderStageStreamPanel(emptyText: string) {
    if (selectedWorkflowStep?.step_type === 'sap_import') {
      return (
        <SapImportObjectBrowserPanel
          objects={sapImportObjects}
          visibleObjects={sapImportVisibleObjects}
          groupedObjects={sapImportGroupedObjects}
          checkedUris={sapImportCheckedUris}
          objectFilter={sapImportObjectFilter}
          onObjectFilterChange={setSapImportObjectFilter}
          onClearFilter={() => setSapImportObjectFilter('')}
          onToggleUri={toggleSapImportUri}
          onToggleGroup={toggleSapImportGroup}
        />
      );
    }
    if (selectedWorkflowStep?.step_type === 'review') {
      return (
        <Suspense fallback={
          <Stack gap="sm" p="md">
            <Group gap="xs">
              <Loader size="sm" />
              <Text size="sm" c="dimmed">Loading diff viewer…</Text>
            </Group>
          </Stack>
        }>
          <ReviewDiffViewerPanel
            repoRef={resolveRepoRefForRun(selectedRun)}
            state={reviewSourceControlState}
            onPersistState={persistReviewSourceControlState}
          />
        </Suspense>
      );
    }
    if (selectedWorkflowStep?.step_type === 'sap_export') {
      return <></>;
    }
    return renderPreviewPanel('Stage stream', stageStreamContent, emptyText, 'stream');
  }

  function buildInteractiveStagePayload() {
    const step = selectedWorkflowStep;
    const stepType = step?.step_type ?? null;

    if (stepType === 'compile') {
      return {
        execution_logic: {
          kind: 'compile_stage_policy'
        }
      } as Record<string, unknown>;
    }

    if (stepType === 'review') {
      return {
        review: {
          approved: stageApproved,
          rejected: stageRejected,
          notes: stageReviewNotes,
          source_control: reviewSourceControlState
        }
      } as Record<string, unknown>;
    }

    const payload: Record<string, unknown> = {
      prompt: {
        user_input: stageUserInput
      }
    };

    if (stepType === 'design') {
      const designMode = readStringValue(step, 'config.design_mode', 'v1');
      payload.config = {
        design_mode: designMode
      };
      payload.execution_logic = {
        kind: 'design_stage_policy',
        mode: designMode,
        connection_bundles: ['design_code_inference_default'],
        connections: {
          inference: {
            repo_context: {}
          }
        },
        automation: {}
      };
    }

    if (stepType === 'code') {
      payload.execution_logic = {
        kind: 'code_stage_policy',
        connection_bundles: ['design_code_inference_default'],
        connections: {
          inference: {
            repo_context: {},
            changeset_schema: {}
          }
        },
        automation: {
          include_apply_error: stageIncludeApplyError,
          include_compile_error: stageIncludeCompileError,
          auto_apply_changeset: stageAutoApplyChangeset
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
      await refreshRunDetails(selectedRun.id);
      return json as Record<string, unknown>;
    }, `Selected stage ${stepId}.`);
    setSelectedStepId(stepId);
  }

  function handleStageCardClick(stepId: string) {
    if (!selectedRun || isBackendRunLocked) {
      return;
    }
    if (!isInteractiveMode || selectedRun.current_step_id === stepId) {
      return;
    }
    setPendingStageSelectionId(stepId);
  }

  async function confirmStageSelection() {
    if (!pendingStageSelectionId) return;
    const stepId = pendingStageSelectionId;
    await handleManualSelectStep(stepId);
    setPendingStageSelectionId(null);
  }

  async function syncInteractiveGlobalState() {
    if (!selectedRun) return;
    const globalPayload = buildInteractiveGlobalStatePayload();
    await patchWorkflowGlobalState(selectedRun.id, globalPayload);
    await refreshRunDetails(selectedRun.id);
  }

  async function patchInteractiveGlobalStateWithoutRefresh(runId: string) {
    const globalPayload = buildInteractiveGlobalStatePayload();
    await patchWorkflowGlobalState(runId, globalPayload);
  }

  async function onToggleSharedRepoContext() {
    if (!selectedRun?.id) return;
    const nextEnabled = !Boolean(sharedInferenceState?.repo_context_armed);
    await patchGlobalCapabilityState({
      capabilities: {
        inference: {
          repo_context_armed: nextEnabled
        }
      }
    });
  }

  async function onToggleSharedChangesetSchema() {
    if (!selectedRun?.id) return;
    const nextEnabled = !Boolean(sharedInferenceState?.changeset_schema_armed);
    await patchGlobalCapabilityState({
      capabilities: {
        inference: {
          changeset_schema_armed: nextEnabled
        }
      }
    });
  }

  async function onTogglePlanningFragment() {
    if (!selectedRun?.id) return;
    const nextEnabled = !Boolean(sharedPlannerFragmentState?.fragment_armed && selectedPlannerFeatureId);
    if (nextEnabled && !selectedPlannerFeatureId) {
      setPlannerFragmentConfigOpen(true);
      return;
    }
    await patchPlannerCapabilityState({
      fragment_armed: nextEnabled,
      selected_feature_id: selectedPlannerFeatureId ?? null,
      selected_feature: nextEnabled ? selectedPlannerFeature : null,
      supervisor_run_id: plannerSupervisorRunId
    });
  }

  async function savePlannerFragmentSelection(featureId: string | null) {
    if (!selectedRun?.id) return;
    setPlannerSelectedFeatureIdDraft(featureId);
    const selectedFeature = featureId
      ? plannerFeatureItems.find((item) => item.id === featureId) ?? null
      : null;
    await patchPlannerCapabilityState({
      fragment_armed: Boolean(featureId),
      selected_feature_id: featureId,
      selected_feature: selectedFeature,
      supervisor_run_id: plannerSupervisorRunId
    });
    setPlannerFragmentConfigOpen(false);
  }

  async function persistReviewSourceControlState(next: ReviewSourceControlState) {
    setLocalReviewSourceControlState(next);
    if (!selectedRun || !selectedWorkflowStep) return;
    const review = (selectedStageState?.review ?? {}) as Record<string, unknown>;
    await patchWorkflowStageState(selectedRun.id, selectedWorkflowStep.id, {
      review: {
        approved: Boolean(review.approved),
        rejected: Boolean(review.rejected),
        notes: typeof review.notes === 'string' ? review.notes : '',
        source_control: next
      }
    });
    await refreshSelectedRunArtifacts();
  }

  async function handleDispositionReview(disposition: string) {
    if (!selectedRun) return;
    const runId = selectedRun.id;
    await runManualCapability(async () => {
      const json = await resolveWorkflowDispositionReview(runId, disposition) as Record<string, unknown>;
      await refreshSelectedRunArtifacts();

      if (disposition === 'move_next') {
        const nextStepId = typeof json.current_step_id === 'string' ? json.current_step_id : null;
        if (nextStepId) {
          setSelectedStepId(nextStepId);
        }
      }

      return json;
    }, `Disposition selected: ${disposition}.`);
  }

  async function handleManualPatchStageState() {
    if (!selectedRun || !selectedRunStepId) return;
    const stepId = selectedRunStepId;
    await runManualCapability(async () => {
      const payload = buildInteractiveStagePayload();
      const json = await patchWorkflowStageState(selectedRun.id, stepId, payload);
      return json as Record<string, unknown>;
    }, 'Patched stage state.');
  }

  async function patchCurrentStageStateBeforeRun() {
    if (!selectedRun || !selectedRunStepId) return;
    const payload = buildInteractiveStagePayload();
    await patchWorkflowStageState(selectedRun.id, selectedRunStepId, payload);
    await refreshRunDetails(selectedRun.id);
  }

  async function handleManualRunWithPatchedState() {
    if (!selectedRun || !selectedRunStepId || isBackendRunLocked) return;

    const runId = selectedRun.id;
    const stepId = selectedRunStepId;
    const payload = buildInteractiveStagePayload();

    await runManualCapability(async () => {
      const json = await runCurrentWorkflowStep(runId, stepId, payload);
      await refreshSelectedRunArtifacts();
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

  function openGlobalInferenceConfig() {
    setInferenceStatus(null);
    setGlobalInferenceConfigOpen(true);
  }

  function currentInferencePanelGlobals(): Record<string, unknown> | null {
    if (view === 'builder') {
      return (builderGlobals ?? compiledBuilderDefinition?.globals ?? loadedTemplateDefinition?.globals ?? null) as Record<string, unknown> | null;
    }
    return (((selectedRun?.context?.workflow_engine as Record<string, unknown> | undefined)?.global_state as Record<string, unknown> | undefined) ?? null) as Record<string, unknown> | null;
  }

  function currentInferencePanelDefinition(): WorkflowTemplateDefinition | null {
    if (view === 'builder') {
      return compiledBuilderDefinition ?? loadedTemplateDefinition ?? null;
    }
    return selectedRunDefinition;
  }

  function normalizeBuilderDefinition(definition?: WorkflowTemplateDefinition | null): WorkflowTemplateDefinition | null {
    if (!definition) {
      return null;
    }
    const globals = (definition.globals ?? {}) as Record<string, unknown>;
    const existingResources = ((globals.resources as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    const existingCapabilities = ((globals.capabilities as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    const existingAutomation = ((globals.automation as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>;
    return {
      ...definition,
      globals: {
        resources: existingResources,
        capabilities: existingCapabilities,
        automation: existingAutomation,
      },
    };
  }

  function normalizeBuilderGlobals(globals?: WorkflowTemplateDefinition['globals'] | null): WorkflowTemplateDefinition['globals'] {
    const fallback = defaultGlobals();
    if (!globals) {
      return fallback;
    }

    const value = globals as Record<string, unknown>;
    return {
      resources: {
        ...(fallback.resources ?? {}),
        ...(((value.resources as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>),
      },
      capabilities: {
        ...(fallback.capabilities ?? {}),
        ...(((value.capabilities as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>),
      },
      automation: {
        ...(fallback.automation ?? {}),
        ...(((value.automation as Record<string, unknown> | undefined) ?? {}) as Record<string, unknown>),
      },
    } as WorkflowTemplateDefinition['globals'];
  }

  function isPlainObject(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
  }

  function deepMergeRecords(base: Record<string, unknown>, patch: Record<string, unknown>): Record<string, unknown> {
    const next: Record<string, unknown> = { ...base };
    for (const [key, value] of Object.entries(patch)) {
      const current = next[key];
      if (isPlainObject(current) && isPlainObject(value)) {
        next[key] = deepMergeRecords(current, value);
      } else {
        next[key] = value;
      }
    }
    return next;
  }

  function applyBuilderGlobalsToDefinition(
    definition: WorkflowTemplateDefinition | null | undefined,
    globals: WorkflowTemplateDefinition['globals'] | null | undefined
  ): WorkflowTemplateDefinition | null {
    const base = normalizeBuilderDefinition(definition);
    if (!base) {
      return null;
    }
    return {
      ...base,
      globals: normalizeBuilderGlobals(globals ?? base.globals ?? null),
    };
  }

  function patchBuilderGlobals(patch: Record<string, unknown>) {
    setBuilderGlobals((prev) => {
      const base = normalizeBuilderGlobals(prev ?? compiledBuilderDefinition?.globals ?? loadedTemplateDefinition?.globals ?? null);
      const next = deepMergeRecords(base as Record<string, unknown>, patch) as WorkflowTemplateDefinition['globals'];

      setCompiledBuilderDefinition((current) => applyBuilderGlobalsToDefinition(current, next));
      setLoadedTemplateDefinition((current) => applyBuilderGlobalsToDefinition(current, next));
      setJsonDraft((currentDraft) => {
        try {
          if (!currentDraft.trim()) {
            return currentDraft;
          }
          const parsed = JSON.parse(currentDraft) as WorkflowTemplateDefinition;
          const withGlobals = applyBuilderGlobalsToDefinition(parsed, next);
          return withGlobals ? JSON.stringify(withGlobals, null, 2) : currentDraft;
        } catch {
          return currentDraft;
        }
      });

      return next;
    });
  }

  function patchBuilderCapability(capabilityKey: string, patch: Record<string, unknown>) {
    patchBuilderGlobals({
      capabilities: {
        [capabilityKey]: patch,
      },
    });
  }

  function syncBuilderRepoResource() {
    const trimmed = repoRef.trim();
    if (!trimmed) {
      return '';
    }
    patchBuilderGlobals({
      resources: {
        repo: {
          repo_ref: trimmed,
          git_ref: 'WORKTREE',
        },
      },
    });
    return trimmed;
  }

  function saveBuilderCapability(capabilityKey: string, patch: Record<string, unknown>) {
    syncBuilderRepoResource();
    patchBuilderCapability(capabilityKey, patch);
  }

  async function handleSaveBuilderRepoContext() {
    const includeFiles = stageRepoContextIncludeFilesText
      .split('\n')
      .map((item) => item.trim())
      .filter(Boolean);
    const excludeRegex = stageRepoContextExcludeRegexText
      .split('\n')
      .map((item) => item.trim())
      .filter(Boolean);

    if (view === 'builder') {
      saveBuilderCapability('context_export', {
        git_ref: stageRepoContextGitRef.trim() || 'WORKTREE',
        include_files: includeFiles,
        exclude_regex: excludeRegex,
        save_path: stageRepoContextSavePath.trim() || '/tmp/repo_context.txt',
        skip_binary: stageRepoContextSkipBinary,
        skip_gitignore: stageRepoContextSkipGitignore,
        include_staged_diff: stageRepoContextIncludeStagedDiff,
        include_unstaged_diff: stageRepoContextIncludeUnstagedDiff,
        inline_repo_context_in_prompt: stageRepoContextInlinePrompt,
      });
      syncRepoSelectionState(includeFiles);
      setRepoContextConfigOpen(false);
      return;
    }

    if (!selectedRun) return;
    await syncInteractiveGlobalState();
    syncRepoSelectionState(includeFiles);
    setRepoContextConfigOpen(false);
  }

  async function handleSaveInferenceSessionsPanel(inferencePatch: Record<string, unknown>) {
    try {
      setInferenceBusy(true);
      setInferenceStatus(null);
      if (view === 'builder') {
        saveBuilderCapability('inference', inferencePatch);
        setInferenceStatus('Inference sessions saved.');
        setGlobalInferenceConfigOpen(false);
        return;
      }
      if (!selectedRun) return;
      const currentGlobalState = ((selectedRun.context?.workflow_engine as Record<string, unknown> | undefined)?.global_state as Record<string, unknown> | undefined) ?? {};
      const currentCapabilities = (currentGlobalState.capabilities as Record<string, unknown> | undefined) ?? {};
      await patchWorkflowGlobalState(selectedRun.id, {
        capabilities: {
          ...currentCapabilities,
          inference: inferencePatch,
        },
      });
      setInferenceStatus('Inference sessions saved.');
      setGlobalInferenceConfigOpen(false);
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

  useEffect(() => {
    if (view === 'builder' || monitorView !== 'workflow_detail') {
      return;
    }

    const hasRepoRef = Boolean((selectedRun?.repo_ref ?? repoRef ?? '').trim());

    const handler = (event: KeyboardEvent) => {
      if (!event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) {
        return;
      }

      const key = event.key;
      if (!/^[1-9]$/.test(key)) {
        return;
      }

      event.preventDefault();

      if (key === '1') {
        setActiveWorkspaceTab('workflows');
        return;
      }

      if (key === '2') {
        if (hasRepoRef) {
          setActiveWorkspaceTab('diff');
        }
        return;
      }

      if (key === '3') {
        if (hasRepoRef) {
          setActiveWorkspaceTab('commits');
        }
        return;
      }

      if (key === '4') {
        if (hasRepoRef) {
          setActiveWorkspaceTab('files');
        }
        return;
      }

      if (key === '5') {
        setActiveWorkspaceTab('capabilities');
      }
    };

    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [view, monitorView, selectedRun?.repo_ref, repoRef]);


  return (
    <AppShell padding="md">
      <AppShell.Main>
        <Stack>
          {error ? <Alert color="red">{error}</Alert> : null}

          {supervisorPlannerRun ? (
            <SupervisorPlannerModal
              opened={supervisorPlannerOpen}
              run={supervisorPlannerRun}
              templates={templates}
              onClose={() => setSupervisorPlannerOpen(false)}
              onSaved={async () => {
                const refreshed = await getSupervisorRun(supervisorPlannerRun.id);
                setSupervisorPlannerRun(refreshed);
                if (selectedRun?.id) await refreshRunDetails(selectedRun.id);
              }}
              selectionMode
              selectedFeatureId={selectedPlannerFeatureId}
              onSelectFeature={async (feature) => {
                setPlannerSelectedFeatureIdDraft(feature.id);
                await patchPlannerCapabilityState({
                  fragment_armed: true,
                  selected_feature_id: feature.id,
                  selected_feature: feature,
                  supervisor_run_id: supervisorPlannerRun.id
                });
                setSupervisorPlannerOpen(false);
              }}
              onWorkflowRunCreated={(workflowRunId) => void openWorkflow(workflowRunId)}
            />
          ) : null}

          {view !== 'builder' && monitorView === 'workflow_detail' ? (
            <Tabs value={activeWorkspaceTab} onChange={(value) => setActiveWorkspaceTab((value as WorkspaceTabKey) ?? 'workflows')}>
              <Tabs.List>
                <Tabs.Tab value="workflows">Workflow (Alt+1)</Tabs.Tab>
                <Tabs.Tab value="diff" disabled={!((selectedRun?.repo_ref ?? repoRef ?? '').trim())}>Changes (Alt+2)</Tabs.Tab>
                <Tabs.Tab value="commits" disabled={!((selectedRun?.repo_ref ?? repoRef ?? '').trim())}>Commits (Alt+3)</Tabs.Tab>
                <Tabs.Tab value="files" disabled={!((selectedRun?.repo_ref ?? repoRef ?? '').trim())}>Repository (Alt+4)</Tabs.Tab>
                <Tabs.Tab value="capabilities">Capabilities (Alt+5)</Tabs.Tab>
              </Tabs.List>
            </Tabs>
          ) : null}

          {view === 'builder' ? (
            <Modal
              opened={view === 'builder'}
              onClose={() => setView('monitor')}
              title="Workflow Builder"
              size="calc(100vw - 32px)"
              centered
              fullScreen
              padding="md"
              zIndex={200}
              styles={{
                body: { paddingTop: 0, height: 'calc(100vh - 72px)' },
                content: { background: 'var(--mantine-color-body)' }
              }}
            >
              <Stack h="100%" gap="sm">
                <Card withBorder p="sm">
                  <Stack gap="sm">
                    <Group justify="space-between" align="flex-start" wrap="wrap">
                      <Stack gap={2}>
                        <Title order={3}>Create workflow</Title>
                        <Text c="dimmed" size="sm">Build the workflow on the canvas, then load or save templates from this panel.</Text>
                      </Stack>
                      <Group>
                        <Button variant="default" onClick={() => setLoadTemplateOpen(true)} disabled={templates.length === 0}>Load template</Button>
                        <Button variant="light" onClick={() => setTemplateModalOpen(true)}>Save template</Button>
                        <Button variant="default" onClick={() => setView('monitor')}>Close</Button>
                      </Group>
                    </Group>

                    <Grid gutter="sm" align="end">
                      <Grid.Col span={{ base: 12, md: 3 }}>
                        <TextInput label="Workflow name" value={workflowName} onChange={(e) => setWorkflowName(e.currentTarget.value)} />
                      </Grid.Col>
                      <Grid.Col span={{ base: 12, md: 3 }}>
                        <TextInput label="Repo path" placeholder="C:/repo or /home/user/repo" value={repoRef} onChange={(e) => setRepoRef(e.currentTarget.value)} />
                      </Grid.Col>
                      <Grid.Col span={{ base: 12, md: 4 }}>
                        <TextInput label="Description" value={workflowDescription} onChange={(e) => setWorkflowDescription(e.currentTarget.value)} />
                      </Grid.Col>
                      <Grid.Col span={{ base: 12, md: 2 }}>
                        <Button fullWidth onClick={() => void handleCreateWorkflow()} loading={busy}>
                          Create workflow
                        </Button>
                      </Grid.Col>
                    </Grid>
                  </Stack>
                </Card>

                <Card withBorder p={0} style={{ overflow: 'hidden', flex: 1, minHeight: 0 }}>
                  <WorkflowBuilderEditor
                    key={`builder-load-${builderLoadRevision}`}
                    initialDefinition={loadedTemplateDefinition}
                    builderGlobals={builderGlobals}
                    onCompiledDefinitionChange={(next) => {
                      const withGlobals = applyBuilderGlobalsToDefinition(next, builderGlobals);
                      if (!withGlobals) {
                        return;
                      }
                      setCompiledBuilderDefinition(withGlobals);
                      setJsonDraft(JSON.stringify(withGlobals, null, 2));
                    }}
                    onError={setError}
                    onOpenCapabilityConfig={(capabilityKey) => {
                      openBuilderCapabilityConfig(capabilityKey, {
                        openRepo: () => {
                          setError(null);
                          syncBuilderRepoResource();
                          loadBuilderRepoContextConfig();
                          setRepoContextConfigOpen(true);
                        },
                        openInference: () => {
                          setError(null);
                          syncBuilderRepoResource();
                          openGlobalInferenceConfig();
                        },
                        openSchema: () => {
                          setError(null);
                          syncBuilderRepoResource();
                          loadBuilderChangesetSchemaConfig();
                          setChangesetSchemaConfigOpen(true);
                        },
                        openApplyChangeset: () => {
                          setError(null);
                          syncBuilderRepoResource();
                          loadBuilderApplyChangesetConfig();
                          setGlobalApplyChangesetOpen(true);
                        },
                        openGitPatchPayload: () => {
                          setError(null);
                          syncBuilderRepoResource();
                          loadBuilderGitPatchPayloadConfig();
                          setGitPatchPayloadOpen(true);
                        },
                      });
                    }}
                  />
                </Card>
              </Stack>
            </Modal>
          ) : activeWorkspaceTab === 'diff' ? (
            <Suspense fallback={<Card withBorder p="lg"><Group gap="xs"><Loader size="sm" /><Text size="sm" c="dimmed">Loading changes view…</Text></Group></Card>}>
              <ReviewDiffViewerPanel
                repoRef={(selectedRun?.repo_ref ?? repoRef ?? '').trim()}
                state={reviewSourceControlState}
                onPersistState={persistReviewSourceControlState}
                forceViewerOpen
              />
            </Suspense>
          ) : activeWorkspaceTab === 'commits' ? (
            <Suspense fallback={<Card withBorder p="lg"><Group gap="xs"><Loader size="sm" /><Text size="sm" c="dimmed">Loading commit summary…</Text></Group></Card>}>
              <CommitSummaryPanel repoRef={(selectedRun?.repo_ref ?? repoRef ?? '').trim()} />
            </Suspense>
          ) : activeWorkspaceTab === 'files' ? (
            <Suspense fallback={<Card withBorder p="lg"><Group gap="xs"><Loader size="sm" /><Text size="sm" c="dimmed">Loading repository view…</Text></Group></Card>}>
              <RepoMonacoFileEditorPanel repoRef={(selectedRun?.repo_ref ?? repoRef ?? '').trim()} />
            </Suspense>
          ) : activeWorkspaceTab === 'capabilities' ? (
            <Card withBorder>
              <GlobalCapabilitiesPanel
                repoContextArmed={!!sharedInferenceState?.repo_context_armed}
                changesetSchemaArmed={!!sharedInferenceState?.changeset_schema_armed}
                plannerArmed={!!sharedPlannerFragmentState?.supervisor_run_id}
                onOpenInference={() => {
                  openGlobalInferenceConfig();
                }}
                onOpenRepoFragment={() => {
                  setRepoContextConfigOpen(true);
                }}
                onOpenChangesetSchema={() => {
                  setChangesetSchemaConfigOpen(true);
                }}
                onOpenPlanner={() => {
                  void openRepoSupervisorPlanner();
                }}
                onOpenApplyChangeset={() => {
                  setGlobalApplyChangesetOpen(true);
                }}
                onOpenGitPatchPayload={() => {
                  setGitPatchPayloadOpen(true);
                }}
              />
            </Card>
          ) : monitorView === 'workflow_list' ? (
            <Stack>
              <Card withBorder>
                <Stack gap="sm">
                  <Group justify="space-between" align="center" wrap="wrap">
                    <Stack gap={2}>
                      <Title order={4}>Workspace monitor</Title>
                      <Text size="sm" c="dimmed">Switch between single workflow runs and repo-level supervisor orchestration.</Text>
                    </Stack>
                    <Group>
                      {monitorHomeView === 'workflows' ? (
                        <Button size="xs" onClick={() => void openBuilder()} loading={busy}>
                          New workflow
                        </Button>
                      ) : null}
                      <Button
                        size="xs"
                        variant="default"
                        leftSection={<IconRefresh size={16} />}
                        onClick={() => void refreshRunsAndTemplates()}
                      >
                        Refresh
                      </Button>
                    </Group>
                  </Group>
                  <Tabs value={monitorHomeView} onChange={(value) => setMonitorHomeView((value as MonitorHomeView) ?? 'workflows')}>
                    <Tabs.List>
                      <Tabs.Tab value="workflows">Workflows</Tabs.Tab>
                      <Tabs.Tab value="supervisors">Supervisors</Tabs.Tab>
                    </Tabs.List>
                  </Tabs>
                </Stack>
              </Card>

              {monitorHomeView === 'supervisors' ? (
                <SupervisorPanel onOpenWorkflowRun={(workflowRunId) => openWorkflow(workflowRunId)} />
              ) : (
                <>
                  <Card withBorder>
                    <Stack>
                      <Group justify="space-between" align="center" wrap="wrap">
                        <Title order={4}>Workflow list</Title>
                        <Group>
                          <Button size="xs" onClick={() => void openBuilder()} loading={busy}>
                            New workflow
                          </Button>
                          <Button
                            size="xs"
                            variant="default"
                            leftSection={<IconRefresh size={16} />}
                            onClick={() => void refreshRunsAndTemplates()}
                          >
                            Refresh
                          </Button>
                        </Group>
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
                </>
              )}
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
                          <Group justify="space-between" align="flex-start" wrap="wrap">
                            <Group>
                              <Button leftSection={<IconPlayerPlay size={16} />} onClick={() => void handleStartRun()} loading={busy} disabled={!selectedRunId || (!canRunCurrentStageAutomatically && selectedRun?.status !== 'success') || isBackendRunLocked}>Run autonomously</Button>
                              <Button variant="default" leftSection={<IconPlayerPause size={16} />} onClick={() => void handlePauseRun()} loading={pauseRequestBusy} disabled={!canRequestRunPause}>Pause after stage</Button>
                              <Button variant="default" leftSection={<IconRefresh size={16} />} onClick={() => selectedRunId && void refreshRunDetails(selectedRunId)}>Refresh run</Button>
                              <Button variant="default" onClick={() => void handleForceWaitRun()} disabled={!selectedRunId || selectedRun?.status !== 'running'}>Force unlock</Button>
                            </Group>
                            <Stack gap={2} align="flex-end">
                              <Text size="xs" c="dimmed">Created: {formatTimestamp(selectedRun.created_at)}</Text>
                              <Text size="xs" c="dimmed">Updated: {formatTimestamp(selectedRun.updated_at)}</Text>
                            </Stack>
                          </Group>
                          <Card withBorder>
                            <Stack gap="md">
                              <Group justify="space-between" align="center">
                                <Title order={6}>Workflow controls</Title>
                              </Group>
                              <Group>
                                <Button variant="default" onClick={() => void handleManualPatchStageState()} disabled={!isInteractiveMode || !selectedRunStepId || isBackendRunLocked || hasPendingDispositionReview}>Save stage inputs</Button>
                                <Button onClick={() => void handleManualRunWithPatchedState()} disabled={!isInteractiveMode || !selectedRunStepId || isBackendRunLocked || hasPendingDispositionReview} loading={manualCapabilityBusy}>Run stage</Button>
                                <Button variant="light" onClick={() => setRunContextOpen(true)} disabled={!selectedRun}>View run context</Button>
                              </Group>
                            </Stack>
                          </Card>
                        </Stack>
                        <Card withBorder>
                          <Stack gap="md">
                            <Text fw={600}>Workflow progress</Text>
                            {selectedRunDefinition ? (
                              <Group gap="sm" wrap="wrap" align="stretch">
                                  {selectedRunDefinition.steps.map((step, index) => {
                                    const isCurrent = selectedRun?.current_step_id === step.id;
                                    const currentIndex = selectedRunDefinition.steps.findIndex((item) => item.id === selectedRun?.current_step_id);
                                    const isCompleted = currentIndex >= 0 && index < currentIndex;
                                    const isUnknownCurrentStep = Boolean(selectedRun?.current_step_id) && currentIndex < 0;
                                    const color = isCurrent ? 'blue' : isCompleted ? 'green' : 'gray';
                                    return (
                                      <Group key={step.id} gap="sm" wrap="nowrap" align="center">
                                        <Box
                                          p="md"
                                          onClick={() => handleStageCardClick(step.id)}
                                          style={{
                                            minWidth: 180,
                                            borderRadius: 12,
                                            border: `1px solid var(--mantine-color-${color}-6)`,
                                            background: isCurrent
                                              ? 'rgba(34, 139, 230, 0.14)'
                                              : isCompleted
                                                ? 'rgba(64, 192, 87, 0.12)'
                                                : 'rgba(255,255,255,0.02)',
                                            cursor: 'pointer'
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
                                        {index < selectedRunDefinition.steps.length - 1 ? <Text c="dimmed" fw={700}>→</Text> : null}
                                      </Group>
                                    );
                                  })}
                                  {Boolean(selectedRun?.current_step_id) && selectedRunDefinition.steps.findIndex((item) => item.id === selectedRun.current_step_id) < 0 ? (
                                    <Alert color="yellow" title="Current stage not in displayed definition">
                                      Current step id: {selectedRun.current_step_id}
                                    </Alert>
                                  ) : null}
                                </Group>
                            ) : (
                              <Text c="dimmed">The selected run is not linked to a loaded template.</Text>
                            )}
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
                            {!inferenceRequiredForSelectedStep || !inferenceRequiresConnection || inferenceReady ? (
                              <>
                                {pendingDispositionReview ? (
                                  <Card withBorder>
                                    <Stack gap="sm">
                                      <Alert color="yellow" title="Stage outcome needs a disposition">
                                        <Stack gap={4}>
                                          <Text size="sm">{pendingDispositionReview.message || 'Review the completed stage output and choose how the workflow should continue.'}</Text>
                                          <Text size="xs" c="dimmed">Stage: {pendingDispositionReview.stageId}</Text>
                                          {pendingDispositionReview.nextStepId ? <Text size="xs" c="dimmed">Next: {pendingDispositionReview.nextStepId}</Text> : null}
                                          {pendingDispositionReview.recommendedDisposition ? <Text size="xs" c="dimmed">Recommended: {pendingDispositionReview.recommendedDisposition}</Text> : null}
                                        </Stack>
                                      </Alert>
                                      <Group gap="xs" wrap="wrap">
                                        {pendingDispositionReview.availableDispositions.map((disposition) => {
                                          const label = disposition === 'move_next' ? 'Continue' : 'Pause';
                                          const color = disposition === 'pause' ? 'yellow' : undefined;
                                          return (
                                            <Button
                                              key={disposition}
                                              variant={disposition === 'move_next' ? 'filled' : 'light'}
                                              color={color}
                                              loading={manualCapabilityBusy}
                                              disabled={busy || manualCapabilityBusy}
                                              onClick={() => void handleDispositionReview(disposition)}
                                            >
                                              {label}
                                            </Button>
                                          );
                                        })}
                                      </Group>
                                    </Stack>
                                  </Card>
                                ) : null}
                                {selectedWorkflowStep?.step_type === 'sap_import' ? (
                                  <SapImportStageControlsPanel
                                    status={sapImportStatus}
                                    packageName={sapImportPackageName}
                                    includeSubpackages={sapImportIncludeSubpackages}
                                    includeXmlArtifacts={sapImportIncludeXmlArtifacts}
                                    searchBusy={sapImportSearchBusy || sapImportApplyBusy}
                                    checkedCount={sapImportCheckedUris.size}
                                    onLoad={() => void handleSapImportSearch()}
                                    onApplySelection={() => void applySapImportSelection()}
                                    onPackageNameChange={(value) => {
                                      setSapImportPackageName(value);
                                      patchSelectedStepDescriptorField('config.sap_import.package_name', value);
                                    }}
                                    onIncludeSubpackagesChange={(value) => {
                                      setSapImportIncludeSubpackages(value);
                                      patchSelectedStepDescriptorField('config.sap_import.include_subpackages', value);
                                    }}
                                    onIncludeXmlArtifactsChange={(value) => {
                                      setSapImportIncludeXmlArtifacts(value);
                                      patchSelectedStepDescriptorField('config.sap_import.include_xml_artifacts', value);
                                    }}
                                  />
                                ) : selectedWorkflowStep?.step_type === 'sap_export' ? (
                                  <SapExportStageInputsPanel
                                    selectedWorkflowStep={selectedWorkflowStep ?? null}
                                    repoRef={selectedRun?.repo_ref ?? ''}
                                    onPatchSelectedStepConfig={patchSelectedStepDescriptorField}
                                  />
                                ) : (
                                  <BackendDrivenStageInputsPanel
                                    descriptor={selectedStageDescriptor}
                                    selectedWorkflowStep={selectedWorkflowStep ?? null}
                                    repoFragmentSummary={repoFragmentSummary}
                                    stageApplyError={stageApplyError}
                                    stageCompileError={stageCompileError}
                                    stageCompileCommandsText={stageCompileCommandsText}
                                    stageUserInput={stageUserInput}
                                    inferenceConnectionStatus={inferenceConnectionStatus}
                                    inferenceTransport={inferenceTransport}
                                    sharedInferenceState={sharedInferenceState}
                                    sharedPlannerFragmentState={sharedPlannerFragmentState}
                  plannerAvailableForRepo={repoPlannerAvailable || Boolean(plannerSupervisorRunId)}
                  activePlannerFeatureTitle={selectedPlannerFeature
                    ? (typeof selectedPlannerFeature.title === 'string' && selectedPlannerFeature.title.trim()
                        ? selectedPlannerFeature.title.trim()
                        : typeof selectedPlannerFeature.summary === 'string' && selectedPlannerFeature.summary.trim()
                          ? selectedPlannerFeature.summary.trim()
                          : null)
                    : null}
                                    stageIncludeRepoContext={stageIncludeRepoContext}
                                    stageIncludeChangesetSchema={stageIncludeChangesetSchema}
                                    disabled={isBackendRunLocked}
                                    onToggleSharedRepoContext={onToggleSharedRepoContext}
                                    onToggleSharedChangesetSchema={onToggleSharedChangesetSchema}
                                    onTogglePlanningFragment={onTogglePlanningFragment}
                                    onOpenPlanner={openRepoSupervisorPlanner}
                                    onPatchSelectedStepConfig={patchSelectedStepDescriptorField}
                                    onOpenInferenceConfig={openGlobalInferenceConfig}
                                    onOpenRepoConfig={() => setRepoContextConfigOpen(true)}
                                    onOpenSchemaConfig={() => setChangesetSchemaConfigOpen(true)}
                                    onOpenApplyErrorConfig={() => setApplyErrorConfigOpen(true)}
                                    onOpenCompileErrorConfig={() => setCompileErrorConfigOpen(true)}
                                    onOpenChanges={() => setActiveWorkspaceTab('diff')}
                                  />
                                )}

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

        <Modal
          opened={repoContextConfigOpen}
          onClose={() => setRepoContextConfigOpen(false)}
          title="Repo fragment"
          size="min(1600px, calc(100vw - 64px))"
          centered
          padding="md"
          zIndex={300}
          withCloseButton
          styles={{
            body: { paddingTop: 0, height: 'calc(100vh - 140px)' },
            content: { background: 'var(--mantine-color-body)', maxHeight: 'calc(100vh - 32px)' }
          }}
        >
          <Stack h="100%" gap="md">
            <TextInput label="Git ref" value={stageRepoContextGitRef} onChange={(e) => setStageRepoContextGitRef(e.currentTarget.value)} placeholder="WORKTREE" />
            <TextInput label="Save path" value={stageRepoContextSavePath} onChange={(e) => setStageRepoContextSavePath(e.currentTarget.value)} placeholder="/tmp/repo_context.txt" />
            <SimpleGrid cols={{ base: 1, md: 2 }}>
              <Switch label="Skip binary" checked={stageRepoContextSkipBinary} onChange={(e) => setStageRepoContextSkipBinary(e.currentTarget.checked)} />
              <Switch label="Skip .gitignore" checked={stageRepoContextSkipGitignore} onChange={(e) => setStageRepoContextSkipGitignore(e.currentTarget.checked)} />
              <Switch label="Include staged diff" checked={stageRepoContextIncludeStagedDiff} onChange={(e) => setStageRepoContextIncludeStagedDiff(e.currentTarget.checked)} />
              <Switch label="Include unstaged diff" checked={stageRepoContextIncludeUnstagedDiff} onChange={(e) => setStageRepoContextIncludeUnstagedDiff(e.currentTarget.checked)} />
              <Switch label="Inline repo context in prompt instead of uploading attachment" checked={stageRepoContextInlinePrompt} onChange={(e) => setStageRepoContextInlinePrompt(e.currentTarget.checked)} />
            </SimpleGrid>
            <Group justify="space-between">
              <Group>
                <Button
                  size="xs"
                  variant="light"
                  onClick={() => {
                    const activeRepoRef = (view === 'builder' ? repoRef : (selectedRun?.repo_ref ?? repoRef)).trim();
                    if (activeRepoRef) {
                      void loadRepoTreeForActiveRef('', true);
                    }
                  }}
                  disabled={!(view === 'builder' ? repoRef : (selectedRun?.repo_ref ?? repoRef)).trim()}
                >
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
                  const activeRepoRef = (view === 'builder' ? repoRef : (selectedRun?.repo_ref ?? repoRef)).trim();
                  if (activeRepoRef) {
                    void loadRepoTreeForActiveRef(path, false);
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
            <Group justify="flex-end">
              <Button size="xs" variant="default" onClick={() => setRepoContextConfigOpen(false)}>Cancel</Button>
              <Button size="xs" onClick={handleSaveBuilderRepoContext}>Save</Button>
            </Group>
          </Stack>
        </Modal>

        <Modal
          opened={plannerFragmentConfigOpen}
          onClose={() => setPlannerFragmentConfigOpen(false)}
          title="Planner Fragment"
          size="calc(100vw - 96px)"
          centered
          zIndex={300}
        >
          <Stack gap="md">
            <TextInput
              label="Search planner features"
              placeholder="Search by feature name, summary, or status"
              value={plannerFeatureSearch}
              onChange={(event) => setPlannerFeatureSearch(event.currentTarget.value)}
            />
            {plannerFeatureItems.length === 0 ? (
              <Text c="dimmed" size="sm">No planner features available.</Text>
            ) : filteredPlannerFeatureItems.length === 0 ? (
              <Text c="dimmed" size="sm">No planner features match the current search.</Text>
            ) : (
              <ScrollArea h="calc(100vh - 330px)" type="auto">
                <Table striped highlightOnHover withTableBorder>
                  <Table.Thead>
                    <Table.Tr>
                      <Table.Th>Feature</Table.Th>
                      <Table.Th>Status</Table.Th>
                      <Table.Th>Last modified</Table.Th>
                      <Table.Th>Actions</Table.Th>
                    </Table.Tr>
                  </Table.Thead>
                  <Table.Tbody>
                    {filteredPlannerFeatureItems.map((item) => {
                      const id = typeof item.id === 'string' ? item.id : '';
                      const title = typeof item.title === 'string' && item.title.trim()
                        ? item.title.trim()
                        : typeof item.summary === 'string' && item.summary.trim()
                          ? item.summary.trim()
                          : id;
                      const summary = typeof item.summary === 'string' ? item.summary.trim() : '';
                      const status = typeof item.status === 'string' && item.status.trim() ? item.status.trim() : 'available';
                      const modified = typeof item.updated_at === 'string' && item.updated_at.trim()
                        ? item.updated_at.trim()
                        : typeof item.updatedAt === 'string' && item.updatedAt.trim()
                          ? item.updatedAt.trim()
                          : typeof item.last_modified === 'string' && item.last_modified.trim()
                            ? item.last_modified.trim()
                            : typeof item.modified_at === 'string' && item.modified_at.trim()
                              ? item.modified_at.trim()
                              : '';
                      const isSelected = selectedPlannerFeatureIds.includes(id);

                      return (
                        <Table.Tr key={id || title}>
                          <Table.Td>
                            <Stack gap={2}>
                              <Group gap="xs" wrap="nowrap">
                                <Text fw={600} size="sm">{title}</Text>
                                {isSelected ? <Badge size="xs" color="green" variant="light">Selected</Badge> : null}
                              </Group>
                              {summary ? <Text size="xs" c="dimmed" lineClamp={2}>{summary}</Text> : null}
                            </Stack>
                          </Table.Td>
                          <Table.Td><Badge variant="light">{status}</Badge></Table.Td>
                          <Table.Td><Text size="sm" c={modified ? undefined : 'dimmed'}>{modified || '—'}</Text></Table.Td>
                          <Table.Td>
                            <Group gap="xs" wrap="nowrap">
                              <Button size="xs" variant="light" onClick={() => setPlannerFeatureViewItem(item)}>View</Button>
                              <Button size="xs" onClick={() => void savePlannerFragmentSelection(id)} disabled={!id || isSelected}>Select</Button>
                            </Group>
                          </Table.Td>
                        </Table.Tr>
                      );
                    })}
                  </Table.Tbody>
                </Table>
              </ScrollArea>
            )}
            <Group justify="space-between">
              <Button size="xs" variant="light" color="red" onClick={() => void savePlannerFragmentSelection(null)} disabled={selectedPlannerFeatureIds.length === 0}>Clear selection</Button>
              <Button size="xs" variant="default" onClick={() => setPlannerFragmentConfigOpen(false)}>Close</Button>
            </Group>
          </Stack>
        </Modal>

        <Modal
          opened={plannerFeatureViewItem !== null}
          onClose={() => setPlannerFeatureViewItem(null)}
          title="Planner feature"
          size="calc(100vw - 96px)"
          centered
          zIndex={310}
        >
          {plannerFeatureViewItem ? (
            <Stack gap="md">
              <Group justify="space-between" align="flex-start">
                <Stack gap={4} style={{ minWidth: 0, flex: 1 }}>
                  <Text fw={700} size="lg">{String(plannerFeatureViewItem.title ?? plannerFeatureViewItem.summary ?? plannerFeatureViewItem.id ?? '')}</Text>
                  {typeof plannerFeatureViewItem.status === 'string' ? <Badge variant="light">{plannerFeatureViewItem.status}</Badge> : null}
                </Stack>
                <Button
                  size="xs"
                  onClick={() => {
                    const id = typeof plannerFeatureViewItem.id === 'string' ? plannerFeatureViewItem.id : null;
                    if (id) void savePlannerFragmentSelection(id);
                  }}
                  disabled={typeof plannerFeatureViewItem.id !== 'string' || selectedPlannerFeatureIds.includes(plannerFeatureViewItem.id)}
                >
                  Select
                </Button>
              </Group>
              {typeof plannerFeatureViewItem.rough_summary === 'string' && plannerFeatureViewItem.rough_summary.trim() ? (
                <Stack gap="xs">
                  <Text fw={600}>Original rough feature prompt</Text>
                  <Text size="sm">{plannerFeatureViewItem.rough_summary}</Text>
                </Stack>
              ) : null}
              {typeof plannerFeatureViewItem.summary === 'string' && plannerFeatureViewItem.summary.trim() ? (
                <Stack gap="xs">
                  <Text fw={600}>Refined feature summary</Text>
                  <Text size="sm">{plannerFeatureViewItem.summary}</Text>
                </Stack>
              ) : null}
              {Array.isArray(plannerFeatureViewItem.requirements) && plannerFeatureViewItem.requirements.length > 0 ? (
                <Stack gap="xs">
                  <Text fw={600}>Detailed requirements</Text>
                  <Stack gap={4}>{plannerFeatureViewItem.requirements.filter((item): item is string => typeof item === 'string').map((item) => <Text key={item} size="sm">• {item}</Text>)}</Stack>
                </Stack>
              ) : null}
              {Array.isArray(plannerFeatureViewItem.acceptance_criteria) && plannerFeatureViewItem.acceptance_criteria.length > 0 ? (
                <Stack gap="xs">
                  <Text fw={600}>Acceptance criteria</Text>
                  <Stack gap={4}>{plannerFeatureViewItem.acceptance_criteria.filter((item): item is string => typeof item === 'string').map((item) => <Text key={item} size="sm">• {item}</Text>)}</Stack>
                </Stack>
              ) : null}
              {Array.isArray(plannerFeatureViewItem.implementation_notes) && plannerFeatureViewItem.implementation_notes.length > 0 ? (
                <Stack gap="xs">
                  <Text fw={600}>Implementation notes</Text>
                  <Stack gap={4}>{plannerFeatureViewItem.implementation_notes.filter((item): item is string => typeof item === 'string').map((item) => <Text key={item} size="sm">• {item}</Text>)}</Stack>
                </Stack>
              ) : null}
              {Array.isArray(plannerFeatureViewItem.review_expectations) && plannerFeatureViewItem.review_expectations.length > 0 ? (
                <Stack gap="xs">
                  <Text fw={600}>Review expectations</Text>
                  <Stack gap={4}>{plannerFeatureViewItem.review_expectations.filter((item): item is string => typeof item === 'string').map((item) => <Text key={item} size="sm">• {item}</Text>)}</Stack>
                </Stack>
              ) : null}
              {Array.isArray(plannerFeatureViewItem.dependencies) && plannerFeatureViewItem.dependencies.length > 0 ? (
                <Stack gap="xs">
                  <Text fw={600}>Dependencies</Text>
                  <Stack gap={4}>{plannerFeatureViewItem.dependencies.filter((item): item is string => typeof item === 'string').map((item) => <Text key={item} size="sm">• {item}</Text>)}</Stack>
                </Stack>
              ) : null}
              {Array.isArray(plannerFeatureViewItem.target_files_or_areas) && plannerFeatureViewItem.target_files_or_areas.length > 0 ? (
                <Stack gap="xs">
                  <Text fw={600}>Target files or areas</Text>
                  <Stack gap={4}>{plannerFeatureViewItem.target_files_or_areas.filter((item): item is string => typeof item === 'string').map((item) => <Text key={item} size="sm">• {item}</Text>)}</Stack>
                </Stack>
              ) : null}
              <Divider />
              <SimpleGrid cols={{ base: 1, md: 2 }}>
                {typeof plannerFeatureViewItem.id === 'string' ? (
                  <Text size="xs" c="dimmed">Feature id: {plannerFeatureViewItem.id}</Text>
                ) : null}
                {typeof plannerFeatureViewItem.refinement_workflow_run_id === 'string' ? (
                  <Text size="xs" c="dimmed">Refinement workflow run: {plannerFeatureViewItem.refinement_workflow_run_id}</Text>
                ) : null}
              </SimpleGrid>
            </Stack>
          ) : null}
        </Modal>

        <Modal
          opened={globalInferenceConfigOpen}
          onClose={() => setGlobalInferenceConfigOpen(false)}
          title="Inference connector"
          size="min(1600px, calc(100vw - 64px))"
          centered
          padding="md"
          zIndex={300}
          withCloseButton
          styles={{
            body: { paddingTop: 0, height: 'calc(100vh - 140px)' },
            content: { background: 'var(--mantine-color-body)', maxHeight: 'calc(100vh - 32px)' }
          }}
        >
          <InferenceSessionsPanel
            opened={globalInferenceConfigOpen}
            globals={currentInferencePanelGlobals()}
            definition={currentInferencePanelDefinition()}
            busy={inferenceBusy}
            status={inferenceStatus}
            onCancel={() => setGlobalInferenceConfigOpen(false)}
            onSave={handleSaveInferenceSessionsPanel}
          />
        </Modal>

        <Modal
          opened={changesetSchemaConfigOpen}
          onClose={() => setChangesetSchemaConfigOpen(false)}
          title="Workflow changeset schema"
          size="min(1600px, calc(100vw - 64px))"
          centered
          padding="md"
          zIndex={300}
          withCloseButton
          styles={{
            body: { paddingTop: 0, height: 'calc(100vh - 140px)' },
            content: { background: 'var(--mantine-color-body)', maxHeight: 'calc(100vh - 32px)' }
          }}
        >
          <Stack h="100%" gap="md">
            <Group justify="space-between">
              <Text size="sm" c="dimmed">Patch the workflow-global changeset schema capability state.</Text>
              <Button size="xs" variant="light" loading={changesetSchemaBusy} onClick={() => void loadCanonicalChangesetSchema(true)}>
                Reload from API
              </Button>
            </Group>
            <Textarea
              label="Changeset schema guidance"
              value={stageChangesetSchemaText}
              onChange={(e) => setStageChangesetSchemaText(e.currentTarget.value)}
              placeholder="Canonical backend changeset schema will populate here by default; you can override it."
              autosize={false}
              styles={{ root: { flex: 1 }, wrapper: { flex: 1 }, input: { height: '100%', minHeight: 'calc(100vh - 280px)' } }}
            />
            <Group justify="flex-end">
              <Button size="xs" variant="default" onClick={() => setChangesetSchemaConfigOpen(false)}>Cancel</Button>
              <Button size="xs" onClick={() => void handleSaveGlobalChangesetSchema()} loading={busy}>Save</Button>
            </Group>
          </Stack>
        </Modal>

        <Modal
          opened={gitPatchPayloadOpen}
          onClose={() => setGitPatchPayloadOpen(false)}
          title="Generate/apply git patch payload"
          size="min(1600px, calc(100vw - 64px))"
          centered
          padding="md"
          zIndex={300}
          withCloseButton
          styles={{
            body: { paddingTop: 0, height: 'calc(100vh - 140px)' },
            content: { background: 'var(--mantine-color-body)', maxHeight: 'calc(100vh - 32px)' }
          }}
        >
          <Stack h="100%" gap="md">
            <Text size="sm" c="dimmed">Generate a portable git patch payload from this repo, or apply one generated from another repo.</Text>
            <Select
              label="Mode"
              value={gitPatchPayloadMode}
              onChange={(value) => {
                setGitPatchPayloadMode(value === 'apply' ? 'apply' : 'generate');
                setGitPatchPayloadStatus(null);
              }}
              data={[
                { label: 'Generate payload', value: 'generate' },
                { label: 'Apply payload', value: 'apply' }
              ]}
            />
            <Select
              label="Scope"
              value={gitPatchPayloadScope}
              onChange={(value) => setGitPatchPayloadScope(value === 'staged' || value === 'unstaged' ? value : 'both')}
              data={[
                { label: 'Staged', value: 'staged' },
                { label: 'Unstaged', value: 'unstaged' },
                { label: 'Both', value: 'both' }
              ]}
            />
            <Box style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
              {gitPatchPayloadMode === 'generate' ? (
                <Textarea
                  label="Generated payload"
                  value={gitPatchPayloadText}
                  onChange={(event) => setGitPatchPayloadText(event.currentTarget.value)}
                  autosize={false}
                  styles={{ root: { flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }, wrapper: { flex: 1, minHeight: 0, display: 'flex' }, input: { height: '100%', minHeight: 0, fontFamily: 'monospace', overflowY: 'auto', resize: 'none' } }}
                />
              ) : (
                <Stack h="100%" gap="xs" style={{ minHeight: 0 }}>
                  <Textarea
                    label="Payload to apply"
                    value={gitPatchPayloadText}
                    onChange={(event) => setGitPatchPayloadText(event.currentTarget.value)}
                    autosize={false}
                    styles={{ root: { flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }, wrapper: { flex: 1, minHeight: 0, display: 'flex' }, input: { height: '100%', minHeight: 0, fontFamily: 'monospace', overflowY: 'auto', resize: 'none' } }}
                  />
                  <Checkbox
                    label="Reverse apply"
                    checked={gitPatchPayloadReverse}
                    onChange={(event) => setGitPatchPayloadReverse(event.currentTarget.checked)}
                  />
                </Stack>
              )}
            </Box>
            {gitPatchPayloadStatus ? <Alert color={gitPatchPayloadStatus.toLowerCase().includes('error') ? 'red' : 'blue'}>{gitPatchPayloadStatus}</Alert> : null}
            <Group justify="space-between">
              <Group gap="xs">
                {gitPatchPayloadText.trim() ? (
                  <Button size="xs" variant="default" onClick={() => void navigator.clipboard.writeText(gitPatchPayloadText)}>Copy payload</Button>
                ) : null}
              </Group>
              <Group gap="xs">
                <Button size="xs" variant="default" onClick={() => setGitPatchPayloadOpen(false)}>Close</Button>
                <Button size="xs" onClick={() => void handleRunGitPatchPayload()} loading={gitPatchPayloadBusy}>
                  {gitPatchPayloadMode === 'apply' ? 'Apply payload' : 'Generate payload'}
                </Button>
              </Group>
            </Group>
          </Stack>
        </Modal>

        <Modal
          opened={globalApplyChangesetOpen}
          onClose={() => setGlobalApplyChangesetOpen(false)}
          title="Manually apply changeset"
          size="min(1800px, calc(100vw - 32px))"
          centered
          padding="md"
          zIndex={300}
          withCloseButton
          styles={{
            body: { paddingTop: 0, height: 'calc(100vh - 120px)' },
            content: { background: 'var(--mantine-color-body)', maxHeight: 'calc(100vh - 16px)' }
          }}
        >
          <Stack h="100%" gap="md">
            <Group justify="space-between" align="center">
              <Text size="sm" c="dimmed">Paste a changeset, apply it, then use the same box to review the result/error.</Text>
              <Group gap="xs">
                <Button size="xs" variant="default" onClick={() => setGlobalApplyChangesetPanelMode(globalApplyChangesetPanelMode === 'input' ? 'output' : 'input')} disabled={!globalApplyChangesetResult && globalApplyChangesetPanelMode === 'input'}>
                  {globalApplyChangesetPanelMode === 'input' ? 'Show output' : 'Show input'}
                </Button>
                <Button size="xs" variant="default" onClick={newGlobalChangeset}>Clear / new</Button>
                <Button size="xs" variant="light" onClick={() => void copyTextToClipboard(visibleGlobalChangesetPanelText(), 'Visible text')} disabled={!visibleGlobalChangesetPanelText().trim()}>Copy visible</Button>
                <Button size="xs" variant="light" onClick={() => void copyTextToClipboard(globalApplyChangesetText, 'Last changeset')} disabled={!globalApplyChangesetText.trim()}>Copy changeset</Button>
                <Button size="xs" variant="light" onClick={() => void refreshChangesetHistory()} loading={globalApplyChangesetHistoryBusy}>Refresh log</Button>
              </Group>
            </Group>

            {manualCapabilityStatus ? <Alert color={manualCapabilityStatus.toLowerCase().includes('error') || manualCapabilityStatus.toLowerCase().includes('failed') ? 'red' : 'blue'} variant="light">{manualCapabilityStatus}</Alert> : null}

            <Grid gutter="md" style={{ flex: 1, minHeight: 0, height: 'calc(100vh - 230px)', overflow: 'hidden' }}>
              <Grid.Col span={8} style={{ minHeight: 0, height: '100%', display: 'flex', flexDirection: 'column' }}>
                <Stack gap="xs" style={{ flex: 1, minHeight: 0, height: '100%', display: 'flex', flexDirection: 'column' }}>
                  <Group justify="space-between" align="center">
                    <Text fw={600} size="sm">{globalApplyChangesetPanelMode === 'input' ? 'Changeset payload' : 'Apply result'}</Text>
                    <Badge variant="light">{globalApplyChangesetPanelMode === 'input' ? 'input' : 'output'}</Badge>
                  </Group>

                  {globalApplyChangesetPanelMode === 'input' ? (
                    <Box style={{ flex: 1, minHeight: 0, height: '100%', display: 'flex', flexDirection: 'column' }}>
                      <Textarea
                        value={globalApplyChangesetText}
                        onChange={(e) => setGlobalApplyChangesetText(e.currentTarget.value)}
                        placeholder="Paste a version 1 changeset JSON payload."
                        autosize={false}
                        style={{ flex: 1, minHeight: 0, height: '100%', display: 'flex', flexDirection: 'column' }}
                        styles={{ root: { flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }, wrapper: { flex: 1, minHeight: 0, display: 'flex' }, input: { height: 'calc(100vh - 360px)', minHeight: 360, fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, Liberation Mono, monospace', overflowY: 'auto', resize: 'none' } }}
                      />
                    </Box>
                  ) : (
                    <Box
                      component="pre"
                      p="sm"
                      style={{
                        flex: 1,
                        minHeight: 0,
                        height: '100%',
                        margin: 0,
                        overflow: 'auto',
                        border: '1px solid var(--mantine-color-dark-4)',
                        borderRadius: 12,
                        whiteSpace: 'pre-wrap',
                        overflowWrap: 'anywhere',
                        fontSize: 12,
                        lineHeight: 1.45,
                        background: 'rgba(0,0,0,0.20)'
                      }}
                    >
                      {manualCapabilityResponse || globalApplyResultText() || 'Apply output and errors will appear here.'}
                    </Box>
                  )}

                  <Group justify="flex-end" style={{ flex: '0 0 auto' }}>
                    <Button size="xs" variant="default" onClick={() => setGlobalApplyChangesetOpen(false)}>Close</Button>
                    <Button size="xs" variant="default" onClick={() => setGlobalApplyChangesetPanelMode(globalApplyChangesetPanelMode === 'input' ? 'output' : 'input')} disabled={!globalApplyChangesetResult && globalApplyChangesetPanelMode === 'input'}>
                      {globalApplyChangesetPanelMode === 'input' ? 'Output' : 'Input'}
                    </Button>
                    <Button size="xs" variant="light" onClick={() => void handleSaveGlobalApplyChangeset()} loading={busy} disabled={globalApplyChangesetPanelMode !== 'input'}>Save draft</Button>
                    <Button size="xs" onClick={() => void handleApplyGlobalChangeset()} loading={manualCapabilityBusy} disabled={!globalApplyChangesetText.trim() || globalApplyChangesetPanelMode !== 'input'}>Apply changeset</Button>
                  </Group>
                </Stack>
              </Grid.Col>

              <Grid.Col span={4} style={{ minHeight: 0, height: '100%', display: 'flex', flexDirection: 'column' }}>
                <Stack gap="xs" style={{ flex: 1, minHeight: 0, height: '100%', display: 'flex', flexDirection: 'column' }}>
                  <Group justify="space-between" align="center">
                    <Text fw={600} size="sm">Changeset log</Text>
                    <Badge variant="light">{globalApplyChangesetHistory.length}</Badge>
                  </Group>
                  <ScrollArea h="100%" type="auto" style={{ border: '1px solid var(--mantine-color-dark-4)', borderRadius: 12 }}>
                    <Stack gap="xs" p="xs">
                      {globalApplyChangesetHistory.length === 0 ? (
                        <Text size="sm" c="dimmed">No logged changesets yet.</Text>
                      ) : globalApplyChangesetHistory.map((item) => {
                        const fileSummaries = changesetFileActionSummary(item);
                        return (
                          <Card key={item.id} withBorder p="xs">
                            <Stack gap={6}>
                              <Group justify="space-between" gap="xs" wrap="nowrap">
                                <Group gap={6} wrap="nowrap">
                                  <Badge color={changesetStatusColor(item.status)} variant="light">{item.status}</Badge>
                                  <Badge variant="outline">{compactRepoLabel(item.repo_ref)}</Badge>
                                </Group>
                                <Text size="xs" c="dimmed">{new Date(item.created_at).toLocaleString()}</Text>
                              </Group>
                              <Text size="xs" lineClamp={2}>{item.display_summary || item.error_summary || 'No summary'}</Text>
                              {fileSummaries.length ? (
                                <details>
                                  <summary>{fileSummaries.length} modified files</summary>
                                  <Stack gap={4} mt={6}>
                                    {fileSummaries.map((file) => (
                                      <Group key={file.path} justify="space-between" gap="xs" wrap="nowrap">
                                        <Text size="xs" truncate>{file.path}</Text>
                                        <Text size="xs" c="dimmed" style={{ whiteSpace: 'nowrap' }}>{file.applied}/{file.total} applied{file.failed ? `, ${file.failed} failed` : ''}</Text>
                                      </Group>
                                    ))}
                                  </Stack>
                                </details>
                              ) : null}
                              <Group gap="xs">
                                <Button size="xs" variant="light" onClick={() => void handleLoadGlobalChangesetAttempt(item, 'input')}>View input</Button>
                                <Button size="xs" variant="light" onClick={() => void handleLoadGlobalChangesetAttempt(item, 'output')}>View output</Button>
                              </Group>
                            </Stack>
                          </Card>
                        );
                      })}
                    </Stack>
                  </ScrollArea>
                </Stack>
              </Grid.Col>
            </Grid>
          </Stack>
        </Modal>

        <Modal
          opened={applyErrorConfigOpen}
          onClose={() => setApplyErrorConfigOpen(false)}
          title="Apply error fragment"
          size="calc(100vw - 32px)"
          centered
          fullScreen
          padding="md"
          zIndex={300}
          styles={{
            body: { paddingTop: 0, height: 'calc(100vh - 72px)' },
            content: { background: 'var(--mantine-color-body)' }
          }}
        >
          <Stack h="100%" gap="md">
            <Textarea label="Apply error" minRows={12} value={stageApplyError} onChange={(e) => setStageApplyError(e.currentTarget.value)} placeholder="Paste apply failures for the next retry prompt" />
            <Group justify="flex-end"><Button size="xs" onClick={() => setApplyErrorConfigOpen(false)}>Done</Button></Group>
          </Stack>
        </Modal>

        <Modal
          opened={compileErrorConfigOpen}
          onClose={() => setCompileErrorConfigOpen(false)}
          title="Compile error fragment"
          size="calc(100vw - 32px)"
          centered
          fullScreen
          padding="md"
          zIndex={300}
          styles={{
            body: { paddingTop: 0, height: 'calc(100vh - 72px)' },
            content: { background: 'var(--mantine-color-body)' }
          }}
        >
          <Stack h="100%" gap="md">
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
                  <MarkdownPreviewContent
                    content={previewViewerMode === 'stream' ? stageStreamContent : previewViewerMode === 'prompt' ? composedInferencePrompt : inferenceResponse}
                    emptyText={previewViewerMode === 'stream' ? 'No stage stream yet.' : previewViewerMode === 'prompt' ? 'No prompt fragments enabled yet.' : 'No inference response yet.'}
                  />
                </Box>
              </ScrollArea>
            </Box>
          </Stack>
        </Modal>

        <Modal opened={templateModalOpen} onClose={() => setTemplateModalOpen(false)} title="Save template" centered zIndex={300}>
          <Stack>
            <TextInput label="Template name" value={workflowName} onChange={(e) => setWorkflowName(e.currentTarget.value)} placeholder="My workflow template" />
            <Textarea label="Description" value={workflowDescription} onChange={(e) => setWorkflowDescription(e.currentTarget.value)} minRows={3} autosize />
            <Switch label="Create run after save" checked={createRunAfterSave} onChange={(e) => setCreateRunAfterSave(e.currentTarget.checked)} />
            <Group justify="flex-end">
              <Button variant="default" onClick={() => setTemplateModalOpen(false)}>Cancel</Button>
              <Button onClick={() => void handleSaveTemplate()} loading={busy}>Save template</Button>
            </Group>
          </Stack>
        </Modal>

        <Modal opened={loadTemplateOpen} onClose={() => setLoadTemplateOpen(false)} title="Load template" size="lg" centered zIndex={300}>
          <Stack>
            {templates.length === 0 ? (
              <Text c="dimmed" size="sm">No saved templates yet.</Text>
            ) : (
              <ScrollArea.Autosize mah={420} offsetScrollbars>
                <Stack gap="sm" pr="xs">
                  {templates.map((template) => {
                    const selected = template.id === selectedTemplateId;
                    return (
                      <Card
                        key={template.id}
                        withBorder
                        padding="sm"
                        radius="md"
                        style={{
                          cursor: 'pointer',
                          borderColor: selected ? 'var(--mantine-color-blue-6)' : undefined,
                          background: selected ? 'rgba(34, 139, 230, 0.12)' : undefined,
                          boxShadow: selected ? '0 0 0 1px var(--mantine-color-blue-6) inset' : undefined
                        }}
                        onClick={() => setSelectedTemplateId(template.id)}
                      >
                        <Box
                          style={{
                            display: 'grid',
                            gridTemplateColumns: 'minmax(0, 1fr) auto',
                            gap: 12,
                            alignItems: 'start'
                          }}
                        >
                          <Stack gap={4} style={{ minWidth: 0 }}>
                            <Text fw={600} c={selected ? 'blue.3' : undefined}>{template.name}</Text>
                            <Text size="sm" c="dimmed">{template.description || 'No description provided.'}</Text>
                          </Stack>
                          <ActionIcon
                            color="red"
                            variant="subtle"
                            aria-label={`Delete ${template.name}`}
                            style={{ flexShrink: 0 }}
                            onClick={(event) => {
                              event.stopPropagation();
                              void handleDeleteTemplate(template.id);
                            }}
                          >
                            <IconTrash size={16} />
                          </ActionIcon>
                        </Box>
                      </Card>
                    );
                  })}
                </Stack>
              </ScrollArea.Autosize>
            )}
            {templates.length > 0 && !selectedTemplateId ? (
              <Text c="dimmed" size="sm">Select a template to load it into the builder.</Text>
            ) : null}
            <Group justify="flex-end">
              <Button variant="default" onClick={() => setLoadTemplateOpen(false)}>Close</Button>
              <Button variant="default" onClick={() => handleLoadTemplateMetadata(selectedTemplateId)} disabled={!selectedTemplateId}>Load template</Button>
            </Group>
          </Stack>
        </Modal>

        <Modal opened={!!pendingStageSelectionId} onClose={() => setPendingStageSelectionId(null)} title="Move to stage" centered>
          <Stack gap="md">
            <Text>
              Move this run to {pendingStageSelection?.name ?? pendingStageSelectionId ?? 'the selected stage'}?
            </Text>
            <Group justify="flex-end">
              <Button variant="default" onClick={() => setPendingStageSelectionId(null)}>Cancel</Button>
              <Button onClick={() => void confirmStageSelection()} loading={manualCapabilityBusy} disabled={isBackendRunLocked}>Confirm</Button>
            </Group>
          </Stack>
        </Modal>


        <Modal opened={runContextOpen} onClose={() => setRunContextOpen(false)} title="Run context" size="min(1100px, 96vw)" centered>
          <Stack>
            <JsonInput value={JSON.stringify(selectedRun?.context ?? {}, null, 2)} readOnly autosize minRows={20} />
          </Stack>
        </Modal>

      </AppShell.Main>
    </AppShell>
  );
}