import { useEffect, useMemo, useState } from 'react';
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
  Loader,
  Modal,
  ScrollArea,
  Select,
  Stack,
  Table,
  Tabs,
  Text,
  TextInput,
  Textarea,
  Title
} from '@mantine/core';
import { IconArrowLeft, IconBolt, IconPlus, IconRefresh } from '@tabler/icons-react';
import {
  createRun,
  createTemplate,
  deleteRun,
  invokeContextExport,
  invokeModelInference,
  listRepoTree,
  listRunEvents,
  listRuns,
  listTemplates,
  nextWorkflowStep,
  patchWorkflowStageState,
  previousWorkflowStep,
  runCurrentWorkflowStep,
  selectWorkflowStep,
  type RepoTreeResponse,
  type WorkflowEvent,
  type WorkflowRun,
  type WorkflowStepDefinition,
  type WorkflowTemplate,
  type WorkflowTemplateDefinition
} from './api';
import { RepoTree, type RepoTreeEntry } from './RepoTree';

type BuilderStepId = 'design' | 'code' | 'test' | 'review';
type AutomationMode = 'manual' | 'assisted' | 'automatic';
type CapabilityKey = 'context_export' | 'model_inference' | 'inject_user_context' | 'inject_changeset_schema';
type InferenceProvider = 'api' | 'browser';

type PausePolicy = {
  pauseOnEnter: boolean;
  pauseOnSuccess: boolean;
  pauseOnError: boolean;
  requireManualApproval: boolean;
  notifyOnPause: boolean;
};

type BuilderStep = {
  id: BuilderStepId;
  name: string;
  automationMode: AutomationMode;
  pause: PausePolicy;
  enabled: boolean;
  execution: {
    changesetApply: boolean;
    compileChecks: boolean;
    cargoCheck: boolean;
    npmBuild: boolean;
    sapActivation: boolean;
    retryOnApplyError: boolean;
    retryOnValidationError: boolean;
    maxConsecutiveApplyFailures: number;
  };
};

type PromptFragmentKey =
  | 'user_input'
  | 'repo_context'
  | 'changeset_schema'
  | 'apply_error'
  | 'compile_error';

type PromptFragmentState = Record<PromptFragmentKey, boolean>;
type PromptFragmentTextState = Record<PromptFragmentKey, string>;

function composeInferencePrompt(enabled: PromptFragmentState, values: PromptFragmentTextState) {
  const ordered: Array<[PromptFragmentKey, string]> = [
    ['repo_context', 'REPO CONTEXT'],
    ['changeset_schema', 'CHANGESET SCHEMA'],
    ['apply_error', 'APPLY ERROR'],
    ['compile_error', 'COMPILE ERROR'],
    ['user_input', 'USER INPUT']
  ];

  return ordered
    .filter(([key]) => enabled[key] && values[key].trim())
    .map(([key, label]) => `### ${label}\n${values[key].trim()}`)
    .join('\n\n');
}

const DEFAULT_STEPS: BuilderStep[] = [
  {
    id: 'design',
    name: 'Design',
    automationMode: 'manual',
    enabled: true,
    pause: {
      pauseOnEnter: false,
      pauseOnSuccess: true,
      pauseOnError: true,
      requireManualApproval: false,
      notifyOnPause: true
    },
    execution: {
      changesetApply: false,
      compileChecks: false,
      cargoCheck: false,
      npmBuild: false,
      sapActivation: false,
      retryOnApplyError: false,
      retryOnValidationError: false,
      maxConsecutiveApplyFailures: 5
    }
  },
  {
    id: 'code',
    name: 'Code',
    automationMode: 'automatic',
    enabled: true,
    pause: {
      pauseOnEnter: false,
      pauseOnSuccess: false,
      pauseOnError: true,
      requireManualApproval: false,
      notifyOnPause: true
    },
    execution: {
      changesetApply: true,
      compileChecks: true,
      cargoCheck: true,
      npmBuild: false,
      sapActivation: false,
      retryOnApplyError: true,
      retryOnValidationError: true,
      maxConsecutiveApplyFailures: 5
    }
  },
  {
    id: 'test',
    name: 'Test',
    automationMode: 'manual',
    enabled: true,
    pause: {
      pauseOnEnter: true,
      pauseOnSuccess: false,
      pauseOnError: true,
      requireManualApproval: false,
      notifyOnPause: true
    },
    execution: {
      changesetApply: false,
      compileChecks: false,
      cargoCheck: false,
      npmBuild: false,
      sapActivation: false,
      retryOnApplyError: false,
      retryOnValidationError: false,
      maxConsecutiveApplyFailures: 5
    }
  },
  {
    id: 'review',
    name: 'Review',
    automationMode: 'manual',
    enabled: true,
    pause: {
      pauseOnEnter: true,
      pauseOnSuccess: true,
      pauseOnError: true,
      requireManualApproval: true,
      notifyOnPause: true
    },
    execution: {
      changesetApply: false,
      compileChecks: false,
      cargoCheck: false,
      npmBuild: false,
      sapActivation: false,
      retryOnApplyError: false,
      retryOnValidationError: false,
      maxConsecutiveApplyFailures: 5
    }
  }
];

function toTemplateDefinition(
  steps: BuilderStep[],
  inferenceProvider: InferenceProvider,
  inferenceModel: string,
  browserTargetUrl: string,
  browserCdpUrl: string
): WorkflowTemplateDefinition {
  const enabledSteps = steps.filter((step) => step.enabled);
  return {
    version: 1,
    globals: {
      inference: {
        enabled: true,
        transport: inferenceProvider,
        model: inferenceModel,
        browser: {
          target_url: browserTargetUrl,
          cdp_url: browserCdpUrl
        }
      },
      prompt_fragments: {
        context_export: {
          enabled: true
        },
        inject_user_context: {
          enabled: true
        },
        inject_changeset_schema: {
          enabled: true
        }
      },
      capabilities: [
        {
          capability: 'model_inference',
          enabled: true,
          config: {
            transport: inferenceProvider,
            model: inferenceModel,
            browser: {
              target_url: browserTargetUrl,
              cdp_url: browserCdpUrl
            }
          },
          input_mapping: {},
          output_mapping: {}
        },
        {
          capability: 'context_export',
          enabled: true,
          config: {},
          input_mapping: {},
          output_mapping: {}
        },
        {
          capability: 'changeset_apply',
          enabled: true,
          config: {},
          input_mapping: {},
          output_mapping: {}
        }
      ]
    },

    steps: enabledSteps.map((step, index) => {
      const next = enabledSteps[index + 1];
      const commands: string[] = [];
      if (step.execution.cargoCheck) commands.push('cargo check');
      if (step.execution.npmBuild) commands.push('npm run build');
      if (step.execution.sapActivation) commands.push('sap_adt_activation');

      const transitions: WorkflowStepDefinition['transitions'] = [];
      if (next) {
        transitions.push({ when: { type: 'success' }, target_step_id: next.id });
      }
      if (step.pause.pauseOnError) {
        transitions.push({ when: { type: 'error' }, target_step_id: step.id });
      }

      return {
        id: step.id,
        name: step.name,
        step_type: step.id,
        automation_mode: step.automationMode,
        execution: {
          changeset_apply: {
            enabled: step.id === 'code' && step.execution.changesetApply,
            mode: step.id === 'code' && step.execution.changesetApply ? step.automationMode : 'manual',
            retry_on_apply_error: step.id === 'code' && step.execution.changesetApply && step.automationMode === 'automatic',
            retry_on_validation_error: step.id === 'code' && step.execution.changesetApply && step.automationMode === 'automatic',
            max_consecutive_apply_failures: step.execution.maxConsecutiveApplyFailures
          },
          compile_checks: {
            enabled: step.id === 'code' && step.execution.compileChecks,
            commands
          }
        },
        prompt: {
          include_repo_context: step.id === 'design' || step.id === 'code',
          include_changeset_schema: step.id === 'code',
          include_user_context: false
        },
        config: {
          pause_policy: {
            pause_on_enter: step.pause.pauseOnEnter,
            pause_on_success: step.pause.pauseOnSuccess,
            pause_on_error: step.pause.pauseOnError,
            require_manual_approval: step.pause.requireManualApproval,
            notify_on_pause: step.pause.notifyOnPause
          }
        },
        capabilities: [
          {
            capability: 'context_export',
            enabled: step.id === 'design' || step.id === 'code',
            config: {
              mode: 'artifact',
              attach_to_inference: true
            },
            input_mapping: {},
            output_mapping: {}
          },
          {
            capability: 'model_inference',
            enabled: step.id === 'design' || step.id === 'code' || step.id === 'review',
            config: {
              mode: 'send_prompt'
            },
            input_mapping: {},
            output_mapping: {}
          },
          {
            capability: 'changeset_apply',
            enabled: step.id === 'code' && step.execution.changesetApply,
            config: {
              mode: step.automationMode
            },
            input_mapping: {},
            output_mapping: {}
          },
          {
            capability: 'compile_checks',
            enabled: step.id === 'code' && step.execution.compileChecks,
            config: {
              commands
            },
            input_mapping: {},
            output_mapping: {}
          }
        ],
        execution_logic: step.id === 'code'
          ? {
              kind: 'code_stage_policy',
              retry_on_apply_error: step.execution.retryOnApplyError,
              retry_on_validation_error: step.execution.retryOnValidationError,
              max_consecutive_apply_failures: step.execution.maxConsecutiveApplyFailures
            }
          : step.id === 'review'
            ? {
                kind: 'review_stage_policy',
                require_manual_approval: step.pause.requireManualApproval
              }
            : null,
        execution_plan: [
          {
            kind: 'capability',
            key: 'context_export',
            enabled: step.id === 'design' || step.id === 'code',
            config: {
              mode: 'artifact',
              attach_to_inference: true
            },
            input_mapping: {},
            output_mapping: {},
            run_after: [],
            condition: null
          },
          {
            kind: 'capability',
            key: 'model_inference',
            enabled: step.id === 'design' || step.id === 'code' || step.id === 'review',
            config: {
              mode: 'send_prompt'
            },
            input_mapping: {},
            output_mapping: {},
            run_after: step.id === 'design' || step.id === 'code' ? ['context_export'] : [],
            condition: null
          },
          {
            kind: 'capability',
            key: 'changeset_apply',
            enabled: step.id === 'code' && step.execution.changesetApply,
            config: {
              mode: step.automationMode
            },
            input_mapping: {},
            output_mapping: {},
            run_after: ['model_inference'],
            condition: null
          },
          {
            kind: 'capability',
            key: 'compile_checks',
            enabled: step.id === 'code' && step.execution.compileChecks,
            config: {
              commands
            },
            input_mapping: {},
            output_mapping: {},
            run_after: ['changeset_apply'],
            condition: null
          },
          {
            kind: 'stage_logic',
            key: 'stage_policy',
            enabled: true,
            config: step.id === 'code'
              ? {
                  kind: 'code_stage_policy',
                  retry_on_apply_error: step.execution.retryOnApplyError,
                  retry_on_validation_error: step.execution.retryOnValidationError,
                  max_consecutive_apply_failures: step.execution.maxConsecutiveApplyFailures
                }
              : step.id === 'review'
                ? {
                    kind: 'review_stage_policy',
                    require_manual_approval: step.pause.requireManualApproval
                  }
                : {
                    kind: 'default_stage_policy'
                  },
            input_mapping: {},
            output_mapping: {},
            run_after: step.id === 'code'
              ? ['compile_checks']
              : step.id === 'design' || step.id === 'review'
                ? ['model_inference']
                : [],
            condition: null
          }
        ],
        transitions
      };
    })
  };
}

export function WorkflowShell() {
  const [runs, setRuns] = useState<WorkflowRun[]>([]);
  const [templates, setTemplates] = useState<WorkflowTemplate[]>([]);
  const [events, setEvents] = useState<WorkflowEvent[]>([]);
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);
  const [viewMode, setViewMode] = useState<'home' | 'workflow'>('home');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [createOpen, setCreateOpen] = useState(false);
  const [createMode, setCreateMode] = useState<'build' | 'copy' | 'save_template' | 'load'>('build');
  const [workflowName, setWorkflowName] = useState('Default workflow');
  const [workflowDescription, setWorkflowDescription] = useState('Design, Code, Test, Review workflow');
  const [runTitle, setRunTitle] = useState('New workflow run');
  const [repoRef, setRepoRef] = useState('');
  const [selectedTemplateId, setSelectedTemplateId] = useState<string | null>(null);
  const [builderSteps, setBuilderSteps] = useState<BuilderStep[]>(DEFAULT_STEPS);
  const [builderRepoIsGit, setBuilderRepoIsGit] = useState(false);

  const [capabilityOpen, setCapabilityOpen] = useState(false);
  const [activeCapability, setActiveCapability] = useState<CapabilityKey>('context_export');

  const [treeGitRef, setTreeGitRef] = useState('WORKTREE');
  const [treeRootData, setTreeRootData] = useState<RepoTreeResponse | null>(null);
  const [treeChildrenByParent, setTreeChildrenByParent] = useState<Record<string, RepoTreeEntry[]>>({});
  const [loadedTreeDirs, setLoadedTreeDirs] = useState<Set<string>>(new Set());
  const [loadingTreeDirs, setLoadingTreeDirs] = useState<Set<string>>(new Set());
  const [treeBusy, setTreeBusy] = useState(false);
  const [treeError, setTreeError] = useState<string | null>(null);
  const [selectedPaths, setSelectedPaths] = useState<string[]>([]);
  const [selectedDirPaths, setSelectedDirPaths] = useState<Set<string>>(new Set());

  const [contextMode, setContextMode] = useState<'entire_repo' | 'tree_select'>('tree_select');
  const [contextSavePath, setContextSavePath] = useState('/tmp/repo_context.txt');
  const [skipBinary, setSkipBinary] = useState(true);
  const [skipGitignore, setSkipGitignore] = useState(true);
  const [includeStagedDiff, setIncludeStagedDiff] = useState(false);
  const [includeUnstagedDiff, setIncludeUnstagedDiff] = useState(false);
  const [contextStatus, setContextStatus] = useState<string | null>(null);
  const [contextBusy, setContextBusy] = useState(false);

  const [inferenceProvider, setInferenceProvider] = useState<InferenceProvider>('api');
  const [inferenceModel, setInferenceModel] = useState('gpt-4.1');
  const [browserTargetUrl, setBrowserTargetUrl] = useState('https://chatgpt.com/');
  const [browserCdpUrl, setBrowserCdpUrl] = useState('http://127.0.0.1:9222');
  const [inferencePrompt, setInferencePrompt] = useState('');
  const [inferenceBusy, setInferenceBusy] = useState(false);
  const [inferenceStatus, setInferenceStatus] = useState<string | null>(null);
  const [inferenceResponse, setInferenceResponse] = useState('');
  const [browserProbe, setBrowserProbe] = useState<Record<string, unknown> | null>(null);
  const [responseViewerOpen, setResponseViewerOpen] = useState(false);
  const [previewViewerMode, setPreviewViewerMode] = useState<'response' | 'prompt'>('response');
  const [changesetSchemaConfigOpen, setChangesetSchemaConfigOpen] = useState(false);
  const [applyErrorConfigOpen, setApplyErrorConfigOpen] = useState(false);
  const [compileErrorConfigOpen, setCompileErrorConfigOpen] = useState(false);
  const [changesetSchemaBusy, setChangesetSchemaBusy] = useState(false);
  const [stepFragmentEnabled, setStepFragmentEnabled] = useState<PromptFragmentState>({
    user_input: true,
    repo_context: false,
    changeset_schema: false,
    apply_error: false,
    compile_error: false
  });
  const [stepFragmentValues, setStepFragmentValues] = useState<PromptFragmentTextState>({
    user_input: '',
    repo_context: '',
    changeset_schema: '',
    apply_error: '',
    compile_error: ''
  });
  const [expandedEventIds, setExpandedEventIds] = useState<Set<string>>(new Set());
  const [expandedStageHistoryIds, setExpandedStageHistoryIds] = useState<Set<string>>(new Set());
  const [expandedStageIds, setExpandedStageIds] = useState<Set<string>>(new Set());
  const [collapsedStageIds, setCollapsedStageIds] = useState<Set<string>>(new Set());

  const selectedRun = useMemo(() => runs.find((run) => run.id === selectedRunId) ?? null, [runs, selectedRunId]);
  const inferenceConnectionStatus = useMemo(() => {
    if (browserProbe?.ready) {
      return { color: 'green', label: 'URL ATTACHED' };
    }
    if (browserProbe?.session_id) {
      return { color: 'yellow', label: 'BRIDGE ATTACHED' };
    }
    return { color: 'red', label: 'NO BRIDGE ATTACHED' };
  }, [browserProbe]);
  const selectedTemplate = useMemo(() => templates.find((template) => template.id === selectedRun?.template_id) ?? null, [templates, selectedRun?.template_id]);
  const rootTreeEntries = useMemo(() => treeChildrenByParent[''] ?? [], [treeChildrenByParent]);
  const selectedSet = useMemo(() => new Set(selectedPaths), [selectedPaths]);
  const currentStepDefinition = useMemo(
    () => selectedTemplate?.definition.steps.find((step) => step.id === selectedRun?.current_step_id) ?? null,
    [selectedTemplate, selectedRun?.current_step_id]
  );
  const isDesignStep = currentStepDefinition?.step_type === 'design';
  const isCodeStep = currentStepDefinition?.step_type === 'code';
  const composedInferencePrompt = useMemo(
    () => composeInferencePrompt(stepFragmentEnabled, stepFragmentValues),
    [stepFragmentEnabled, stepFragmentValues]
  );

  function isStageWrapperEvent(event: WorkflowEvent): boolean {
    return event.kind === 'stage_executed' || event.kind === 'run_action_completed';
  }

  const eventsByStage = useMemo(() => {
    const order = selectedTemplate?.definition.steps.map((step) => step.id) ?? [];
    const grouped = new Map<string, WorkflowEvent[]>();

    for (const event of events) {
      const key = event.step_id ?? '__ungrouped__';
      const bucket = grouped.get(key);
      if (bucket) bucket.push(event);
      else grouped.set(key, [event]);
    }

    const orderedKeys = [
      ...order.filter((stepId) => grouped.has(stepId)),
      ...Array.from(grouped.keys()).filter((key) => key !== '__ungrouped__' && !order.includes(key)),
      ...(grouped.has('__ungrouped__') ? ['__ungrouped__'] : [])
    ];

    const stageGroups = orderedKeys.flatMap((stepId) => {
      const ascending = (grouped.get(stepId) ?? []).slice().sort((a, b) => {
        return new Date(a.created_at).getTime() - new Date(b.created_at).getTime();
      });

      if (stepId === '__ungrouped__') {
        const visibleEvents = ascending.filter((event) => !isStageWrapperEvent(event));
        const stageEvents = visibleEvents.slice().sort((a, b) => {
          return new Date(b.created_at).getTime() - new Date(a.created_at).getTime();
        });
        const latest = stageEvents[0] ?? null;
        return stageEvents.length === 0
          ? []
          : [{
              stepId,
              baseStepId: stepId,
              attempt: 1,
              label: 'Workflow events',
              isCurrent: false,
              events: stageEvents,
              latest
            }];
      }

      const attempts: WorkflowEvent[][] = [];
      let currentAttempt: WorkflowEvent[] = [];

      for (const event of ascending) {
        currentAttempt.push(event);
        if (event.kind === 'stage_executed') {
          attempts.push(currentAttempt);
          currentAttempt = [];
        }
      }

      const trailingVisibleEvents = currentAttempt.filter((event) => !isStageWrapperEvent(event));
      if (trailingVisibleEvents.length > 0) {
        attempts.push(currentAttempt);
      }

      const stepDef = selectedTemplate?.definition.steps.find((step) => step.id === stepId) ?? null;
      const latestAttemptIndex = attempts.length - 1;

      return attempts.flatMap((attemptEvents, index) => {
        const visibleEvents = attemptEvents.filter((event) => !isStageWrapperEvent(event));
        if (visibleEvents.length === 0) {
          return [];
        }

        const stageEvents = visibleEvents.slice().sort((a, b) => {
          return new Date(b.created_at).getTime() - new Date(a.created_at).getTime();
        });
        const latest = stageEvents[0] ?? null;
        const attempt = index + 1;
        return [{
          stepId: `${stepId}#${attempt}`,
          baseStepId: stepId,
          attempt,
          label: `${stepDef?.name ?? stepId} · Run ${attempt}`,
          isCurrent: selectedRun?.current_step_id === stepId && index === latestAttemptIndex,
          events: stageEvents,
          latest
        }];
      });
    });

    return stageGroups.reverse();
  }, [events, selectedTemplate, selectedRun?.current_step_id]);

  const currentStageGroup = useMemo(
    () => eventsByStage.find((stageGroup) => stageGroup.isCurrent) ?? null,
    [eventsByStage]
  );

  function toggleEventExpanded(eventId: string) {
    setExpandedEventIds((prev) => {
      const next = new Set(prev);
      if (next.has(eventId)) next.delete(eventId);
      else next.add(eventId);
      return next;
    });
  }

  function toggleStageHistoryExpanded(stepId: string) {
    setExpandedStageHistoryIds((prev) => {
      const next = new Set(prev);
      if (next.has(stepId)) next.delete(stepId);
      else next.add(stepId);
      return next;
    });
  }

  function toggleStageExpanded(stepId: string, isCurrent: boolean) {
    if (isCurrent) {
      setCollapsedStageIds((prev) => {
        const next = new Set(prev);
        if (next.has(stepId)) next.delete(stepId);
        else next.add(stepId);
        return next;
      });
      return;
    }

    setExpandedStageIds((prev) => {
      const next = new Set(prev);
      if (next.has(stepId)) next.delete(stepId);
      else next.add(stepId);
      return next;
    });
  }

  function eventTone(event: WorkflowEvent): { color: string; label: string } {
    const payload = (event.payload ?? {}) as Record<string, unknown>;
    const ok = payload.ok;

    if (event.level === 'error' || ok === false) {
      return { color: 'red', label: 'ERROR' };
    }

    if (event.level === 'warning') {
      return { color: 'yellow', label: 'WARNING' };
    }

    if (event.kind.includes('completed') || event.kind.includes('executed') || ok === true) {
      return { color: 'green', label: 'SUCCESS' };
    }

    return { color: 'blue', label: 'INFO' };
  }

  function summarizeEvent(event: WorkflowEvent): string {
    const payload = (event.payload ?? {}) as Record<string, unknown>;

    if (event.kind === 'terminal_completed' || event.kind === 'terminal_failed') {
      const outputs = Array.isArray(payload.outputs) ? payload.outputs as Array<Record<string, unknown>> : [];
      const commandCount = outputs.length;
      const failedCount = outputs.filter((row) => row.ok === false).length;
      if (failedCount > 0) {
        return `Ran ${commandCount} terminal command${commandCount === 1 ? '' : 's'} with ${failedCount} failure${failedCount === 1 ? '' : 's'}.`;
      }
      return `Ran ${commandCount} terminal command${commandCount === 1 ? '' : 's'} successfully.`;
    }

    if (event.kind === 'payload_gateway_completed' || event.kind === 'payload_gateway_failed') {
      const stats = payload.stats as Record<string, unknown> | undefined;
      const successCount = typeof stats?.successful_operations === 'number' ? stats.successful_operations : null;
      const failedCount = typeof stats?.failed_operations === 'number' ? stats.failed_operations : null;
      const totalCount = typeof stats?.total_operations === 'number' ? stats.total_operations : null;
      if (successCount !== null && failedCount !== null && totalCount !== null) {
        if (failedCount > 0) {
          return `Applied ${successCount}/${totalCount} operations successfully with ${failedCount} failure${failedCount === 1 ? '' : 's'}.`;
        }
        return `Applied ${successCount}/${totalCount} operations successfully.`;
      }
      const summary = typeof payload.summary === 'string' ? payload.summary : null;
      if (summary) return summary;
    }

    if (event.kind === 'context_export_completed') {
      const bytesWritten = payload.bytes_written;
      const outputPath = payload.output_path;
      if (typeof bytesWritten === 'number' && typeof outputPath === 'string') {
        return `Exported ${bytesWritten.toLocaleString()} bytes to ${outputPath}.`;
      }
    }

    if (event.kind === 'model_inference_completed') {
      const transport = payload.transport;
      const repoContextMode = payload.repo_context_mode;
      if (typeof transport === 'string' && typeof repoContextMode === 'string' && repoContextMode) {
        return `Inference completed via ${transport} with repo context ${repoContextMode}.`;
      }
      if (typeof transport === 'string') {
        return `Inference completed via ${transport}.`;
      }
    }

    if (event.kind === 'stage_executed' || event.kind === 'run_action_completed') {
      const result = payload.result as Record<string, unknown> | undefined;
      const status = (result?.status ?? payload.status) as string | undefined;
      const stepId = (result?.step_id ?? payload.step_id) as string | undefined;
      if (status || stepId) {
        return `Stage ${stepId ?? 'unknown'} resolved with status ${status ?? 'unknown'}.`;
      }
    }

    return event.message;
  }

  function capabilityLifecycleKey(kind: string): string | null {
    if (kind.endsWith('_started')) return kind.slice(0, -'_started'.length);
    if (kind.endsWith('_completed')) return kind.slice(0, -'_completed'.length);
    if (kind.endsWith('_failed')) return kind.slice(0, -'_failed'.length);
    return null;
  }

  function formatDurationMs(startedAt: string, endedAt: string): string {
    const delta = Math.max(new Date(endedAt).getTime() - new Date(startedAt).getTime(), 0);
    if (delta < 1000) return `${delta} ms`;
    const seconds = delta / 1000;
    if (seconds < 60) return `${seconds.toFixed(1)} s`;
    const minutes = Math.floor(seconds / 60);
    const rem = Math.round(seconds % 60);
    return `${minutes}m ${rem}s`;
  }

  type StageEventItem =
    | { type: 'single'; key: string; event: WorkflowEvent }
    | { type: 'capability'; key: string; capabilityKey: string; summaryEvent: WorkflowEvent; childEvents: WorkflowEvent[]; runtimeLabel: string | null };

  function buildStageEventItems(stageEvents: WorkflowEvent[]): StageEventItem[] {
    const ascending = stageEvents
      .filter((event) => !isStageWrapperEvent(event))
      .slice()
      .sort((a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime());
    const items: StageEventItem[] = [];
    const openByCapability = new Map<string, { index: number; startedAt: string }>();

    for (const event of ascending) {
      const capabilityKey = capabilityLifecycleKey(event.kind);
      if (!capabilityKey) {
        items.push({ type: 'single', key: event.id, event });
        continue;
      }

      if (event.kind.endsWith('_started')) {
        items.push({
          type: 'capability',
          key: `${capabilityKey}:${event.id}`,
          capabilityKey,
          summaryEvent: event,
          childEvents: [event],
          runtimeLabel: null
        });
        openByCapability.set(capabilityKey, { index: items.length - 1, startedAt: event.created_at });
        continue;
      }

      const open = openByCapability.get(capabilityKey);
      if (open && items[open.index]?.type === 'capability') {
        const item = items[open.index] as Extract<StageEventItem, { type: 'capability' }>;
        item.summaryEvent = event;
        item.childEvents.push(event);
        item.runtimeLabel = formatDurationMs(open.startedAt, event.created_at);
        openByCapability.delete(capabilityKey);
      } else {
        items.push({
          type: 'capability',
          key: `${capabilityKey}:${event.id}`,
          capabilityKey,
          summaryEvent: event,
          childEvents: [event],
          runtimeLabel: null
        });
      }
    }

    return items.reverse();
  }

  function renderPreviewPanel(
    title: string,
    content: string,
    emptyText: string,
    mode: 'response' | 'prompt'
  ) {
    const hasContent = Boolean(content.trim());

    return (
      <Stack gap="xs">
        <Group justify="space-between" align="center">
          <Text fw={600} size="sm">{title}</Text>
          <Group gap="xs">
            <Badge variant="light">{hasContent ? `${content.length.toLocaleString()} chars` : 'empty'}</Badge>
            <Button
              size="xs"
              variant="light"
              onClick={() => {
                setPreviewViewerMode(mode);
                setResponseViewerOpen(true);
              }}
              disabled={!hasContent}
            >
              Full screen
            </Button>
          </Group>
        </Group>

        <Box
          p="md"
          style={{
            border: '1px solid var(--mantine-color-dark-4)',
            borderRadius: 12,
            background: 'linear-gradient(180deg, rgba(255,255,255,0.02), rgba(255,255,255,0.01))',
            boxShadow: 'inset 0 1px 0 rgba(255,255,255,0.03)'
          }}
        >
          <ScrollArea h={220} offsetScrollbars>
            <Box maw={960} mx="auto">
              <Text
                size="sm"
                style={{
                  whiteSpace: 'pre-wrap',
                  overflowWrap: 'anywhere',
                  wordBreak: 'break-word',
                  lineHeight: 1.75,
                  letterSpacing: '0.01em',
                  fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, Liberation Mono, monospace'
                }}
              >
                {hasContent ? content : emptyText}
              </Text>
            </Box>
          </ScrollArea>
        </Box>
      </Stack>
    );
  }

  function renderInferenceResponsePanel(emptyText: string) {
    return renderPreviewPanel('Inference response', inferenceResponse, emptyText, 'response');
  }

  function renderComposedPromptPreviewPanel(emptyText: string) {
    return renderPreviewPanel('Composed prompt preview', composedInferencePrompt, emptyText, 'prompt');
  }

  async function refresh() {
    setError(null);
    try {
      const [templateData, runData] = await Promise.all([listTemplates(), listRuns()]);
      setTemplates(templateData);
      setRuns(runData);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function refreshEvents(runId: string) {
    setEvents(await listRunEvents(runId));
  }

  async function refreshTree(run: WorkflowRun) {
    await loadTreeDir(run, '', true);
  }

  async function loadTreeSubtree(run: WorkflowRun, basePath: string): Promise<{ children: Record<string, RepoTreeEntry[]>; files: string[]; loadedDirs: string[] }> {
    const data = await listRepoTree(run.repo_ref, treeGitRef, {
      basePath,
      skipBinary,
      skipGitignore
    });

    const children: Record<string, RepoTreeEntry[]> = {
      [basePath]: data.entries
    };
    const files = data.entries.filter((entry) => entry.kind === 'file').map((entry) => entry.path);
    const loadedDirs: string[] = [basePath];

    for (const entry of data.entries) {
      if (entry.kind === 'dir' && !entry.has_children) {
        children[entry.path] = [];
        loadedDirs.push(entry.path);
      }
    }

    const nestedResults = await Promise.all(
      data.entries
        .filter((entry) => entry.kind === 'dir' && entry.has_children)
        .map((entry) => loadTreeSubtree(run, entry.path))
    );

    for (const nested of nestedResults) {
      Object.assign(children, nested.children);
      files.push(...nested.files);
      loadedDirs.push(...nested.loadedDirs);
    }

    return { children, files, loadedDirs };
  }

  async function loadTreeDir(run: WorkflowRun, basePath: string, replaceRoot = false) {
    if (loadingTreeDirs.has(basePath)) {
      return;
    }

    setTreeError(null);
    if (replaceRoot) {
      setTreeBusy(true);
    }
    setLoadingTreeDirs((prev) => {
      const next = new Set(prev);
      next.add(basePath);
      return next;
    });

    try {
      const data = await listRepoTree(run.repo_ref, treeGitRef, {
        basePath,
        skipBinary,
        skipGitignore
      });

      if (replaceRoot) {
        setTreeRootData(data);
        setTreeChildrenByParent({ '': data.entries });
        setLoadedTreeDirs(new Set(['']));
      } else {
        setTreeChildrenByParent((prev) => ({
          ...prev,
          [basePath]: data.entries
        }));
        setLoadedTreeDirs((prev) => {
          const next = new Set(prev);
          next.add(basePath);
          return next;
        });
      }

      const nextChildren = replaceRoot
        ? { '': data.entries }
        : { ...treeChildrenByParent, [basePath]: data.entries };
      const visiblePaths = new Set<string>();
      for (const entries of Object.values(nextChildren)) {
        for (const entry of entries) {
          visiblePaths.add(entry.path);
        }
      }
      setSelectedPaths((prev) => prev.filter((path) => visiblePaths.has(path)));
    } catch (err) {
      setTreeError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoadingTreeDirs((prev) => {
        const next = new Set(prev);
        next.delete(basePath);
        return next;
      });
      if (replaceRoot) {
        setTreeBusy(false);
      }
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  useEffect(() => {
    let cancelled = false;

    async function probeBuilderRepo() {
      if (!createOpen || !repoRef.trim()) {
        if (!cancelled) {
          setBuilderRepoIsGit(false);
          setBuilderSteps((prev) => prev.map((step) => (
            step.id === 'code'
              ? {
                  ...step,
                  automationMode: 'manual',
                  execution: {
                    ...step.execution,
                    changesetApply: false
                  }
                }
              : step
          )));
        }
        return;
      }

      try {
        await listRepoTree(repoRef, 'WORKTREE');
        if (cancelled) return;
        setBuilderRepoIsGit(true);
        setBuilderSteps((prev) => prev.map((step) => (
          step.id === 'code'
            ? {
                ...step,
                automationMode: step.execution.changesetApply ? step.automationMode : 'automatic',
                execution: {
                  ...step.execution,
                  changesetApply: true,
                  maxConsecutiveApplyFailures: step.execution.maxConsecutiveApplyFailures || 5
                }
              }
            : step
        )));
      } catch {
        if (cancelled) return;
        setBuilderRepoIsGit(false);
        setBuilderSteps((prev) => prev.map((step) => (
          step.id === 'code'
            ? {
                ...step,
                automationMode: 'manual',
                execution: {
                  ...step.execution,
                  changesetApply: false
                }
              }
            : step
        )));
      }
    }

    void probeBuilderRepo();

    return () => {
      cancelled = true;
    };
  }, [createOpen, repoRef]);

  useEffect(() => {
    if (!currentStepDefinition) {
      return;
    }

    const currentStageType = currentStepDefinition.step_type;
    const isChangesetStage = currentStageType === 'code';

    setStepFragmentEnabled({
      user_input: true,
      repo_context: currentStepDefinition.prompt.include_repo_context,
      changeset_schema: isChangesetStage && currentStepDefinition.prompt.include_changeset_schema,
      apply_error: isChangesetStage,
      compile_error: isChangesetStage
    });
  }, [currentStepDefinition?.id, currentStepDefinition?.step_type]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      void refresh();
      if (selectedRunId) {
        void refreshEvents(selectedRunId);
      }
    }, 5000);
    return () => window.clearInterval(timer);
  }, [selectedRunId]);

  useEffect(() => {
    if (selectedRunId) {
      void refreshEvents(selectedRunId);
    } else {
      setEvents([]);
    }
  }, [selectedRunId]);

  useEffect(() => {
    if (!currentStageGroup) {
      return;
    }

    setExpandedStageIds(new Set());
    setExpandedStageHistoryIds(new Set());
    setCollapsedStageIds(new Set());
  }, [currentStageGroup?.stepId]);

  useEffect(() => {
    if (!selectedRun || !capabilityOpen || activeCapability !== 'context_export') {
      return;
    }
    void refreshTree(selectedRun);
  }, [selectedRun?.id, selectedRun?.repo_ref, capabilityOpen, activeCapability, treeGitRef, skipBinary, skipGitignore]);

  function openWorkflow(runId: string) {
    setSelectedRunId(runId);
    setViewMode('workflow');
  }

  function toggleFile(path: string) {
    setSelectedPaths((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return Array.from(next).sort();
    });
  }

  function setPaths(paths: string[], checked: boolean) {
    setSelectedPaths((prev) => {
      const next = new Set(prev);
      for (const path of paths) {
        if (checked) next.add(path);
        else next.delete(path);
      }
      return Array.from(next).sort();
    });
  }

  function setDirectorySelected(path: string, checked: boolean) {
    setSelectedDirPaths((prev) => {
      const next = new Set(prev);
      if (checked) next.add(path);
      else next.delete(path);
      return next;
    });
  }

  async function toggleDirectory(entry: RepoTreeEntry, checked: boolean) {
    if (!selectedRun) {
      return;
    }

    setDirectorySelected(entry.path, checked);

    const collectLoadedPaths = (dirPath: string): string[] => {
      const entries = treeChildrenByParent[dirPath] ?? [];
      const files: string[] = [];
      for (const child of entries) {
        if (child.kind === 'file') {
          files.push(child.path);
        } else {
          files.push(...collectLoadedPaths(child.path));
        }
      }
      return files;
    };

    const loadedFiles = collectLoadedPaths(entry.path);
    if (loadedFiles.length > 0) {
      setPaths(loadedFiles, checked);
    }

    setTreeError(null);
    setLoadingTreeDirs((prev) => {
      const next = new Set(prev);
      next.add(entry.path);
      return next;
    });

    try {
      const { children, files, loadedDirs } = await loadTreeSubtree(selectedRun, entry.path);

      setTreeChildrenByParent((prev) => ({
        ...prev,
        ...children
      }));

      setLoadedTreeDirs((prev) => {
        const next = new Set(prev);
        for (const dir of loadedDirs) {
          next.add(dir);
        }
        return next;
      });

      setSelectedPaths((prev) => {
        const next = new Set(prev);
        for (const path of files) {
          if (checked) next.add(path);
          else next.delete(path);
        }
        return Array.from(next).sort();
      });
    } catch (err) {
      setDirectorySelected(entry.path, !checked);
      setTreeError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoadingTreeDirs((prev) => {
        const next = new Set(prev);
        next.delete(entry.path);
        return next;
      });
    }
  }

  function patchStep(stepId: BuilderStepId, patch: Partial<BuilderStep>) {
    setBuilderSteps((prev) => prev.map((step) => (step.id === stepId ? { ...step, ...patch } : step)));
  }

  function patchStepExecution(stepId: BuilderStepId, patch: Partial<BuilderStep['execution']>) {
    setBuilderSteps((prev) => prev.map((step) => (
      step.id === stepId
        ? { ...step, execution: { ...step.execution, ...patch } }
        : step
    )));
  }

  function patchStepPause(stepId: BuilderStepId, patch: Partial<PausePolicy>) {
    setBuilderSteps((prev) => prev.map((step) => (
      step.id === stepId
        ? { ...step, pause: { ...step.pause, ...patch } }
        : step
    )));
  }

  function patchStepFragmentEnabled(key: PromptFragmentKey, enabled: boolean) {
    setStepFragmentEnabled((prev) => ({ ...prev, [key]: enabled }));
  }

  function patchStepFragmentValue(key: PromptFragmentKey, value: string) {
    setStepFragmentValues((prev) => ({ ...prev, [key]: value }));
  }

  function openContextExporter() {
    setActiveCapability('context_export');
    setCapabilityOpen(true);
  }

  function openChangesetSchemaConfigurator() {
    setChangesetSchemaConfigOpen(true);
    patchStepFragmentEnabled('changeset_schema', true);
  }

  function openApplyErrorConfigurator() {
    patchStepFragmentEnabled('apply_error', true);
    setApplyErrorConfigOpen(true);
  }

  function openCompileErrorConfigurator() {
    patchStepFragmentEnabled('compile_error', true);
    setCompileErrorConfigOpen(true);
  }

  function clearStageFragmentSelections() {
    setStepFragmentEnabled((prev) => ({
      ...prev,
      repo_context: false,
      changeset_schema: false,
      apply_error: false,
      compile_error: false
    }));
  }

  async function submitCreateWorkflow() {
    setBusy(true);
    setError(null);
    try {
      if (createMode === 'load') {
        const template = templates.find((t) => t.id === selectedTemplateId);
        if (!template) throw new Error('Choose a template to load.');
        setWorkflowName(template.name);
        setWorkflowDescription(template.description);
        setCreateMode('build');
        return;
      }

      let templateIdToUse = selectedTemplateId;

      if (createMode === 'build' || createMode === 'save_template') {
        const created = await createTemplate({
          name: workflowName,
          description: workflowDescription,
          definition: toTemplateDefinition(builderSteps, inferenceProvider, inferenceModel, browserTargetUrl, browserCdpUrl)
        });
        templateIdToUse = created.id;
      }

      if (createMode === 'copy') {
        const source = templates.find((t) => t.id === selectedTemplateId);
        if (!source) throw new Error('Choose a template to copy.');
        const copied = await createTemplate({
          name: workflowName,
          description: workflowDescription,
          definition: source.definition
        });
        templateIdToUse = copied.id;
      }

      const run = await createRun({
        template_id: templateIdToUse,
        title: runTitle,
        repo_ref: repoRef,
        context: {
          model_inference: {
            transport: inferenceProvider,
            model: inferenceModel,
            browser: {
              profile: 'auto',
              bridge_dir: 'bridge',
              cdp_url: browserCdpUrl,
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
          }
        }
      });

      await refresh();
      setSelectedRunId(run.id);
      setViewMode('workflow');
      setCreateOpen(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function runStage(stepId?: string) {
    if (!selectedRun) return;

    const effectiveStepId = stepId ?? selectedRun.current_step_id ?? undefined;
    if (!effectiveStepId) return;

    setInferenceBusy(true);
    setInferenceStatus(null);

    try {
      await patchWorkflowStageState(selectedRun.id, effectiveStepId, {
        prompt_fragments: stepFragmentValues,
        prompt_fragment_enabled: stepFragmentEnabled,
        composed_prompt: composedInferencePrompt,
        repo_context: {
          mode: contextMode,
          git_ref: treeGitRef,
          skip_binary: skipBinary,
          skip_gitignore: skipGitignore,
          include_staged_diff: includeStagedDiff,
          include_unstaged_diff: includeUnstagedDiff,
          include_files: contextMode === 'tree_select' ? selectedPaths : null,
          save_path: contextSavePath
        }
      });

      const result = await runCurrentWorkflowStep(selectedRun.id, effectiveStepId);
      const capabilityResults = (result.capability_results ?? []) as Array<Record<string, unknown>>;
      const inferenceResult = capabilityResults
        .find((row) => typeof row?.result === 'object' && row.result !== null)
        ?.result as Record<string, unknown> | undefined;

      if (inferenceResult?.text) {
        setInferenceResponse(String(inferenceResult.text));
      }

      if (stepFragmentEnabled.repo_context) {
        setSelectedPaths([]);
        setSelectedDirPaths(new Set());
      }

      setInferenceStatus(String(result.message ?? 'Stage executed through backend workflow engine.'));
      await refresh();
      await refreshEvents(selectedRun.id);
    } catch (err) {
      setInferenceStatus(err instanceof Error ? err.message : String(err));
    } finally {
      clearStageFragmentSelections();
      setInferenceBusy(false);
    }
  }

  async function moveToStage(stepId: string) {
    if (!selectedRun) return;
    await selectWorkflowStep(selectedRun.id, stepId);
    await refresh();
    await refreshEvents(selectedRun.id);
  }

  async function runContextExport() {
    if (!selectedRun) return;
    setContextBusy(true);
    setContextStatus(null);
    try {
      const response = await invokeContextExport(selectedRun.id, {
        step_id: 'context_export',
        payload: {
          repo_ref: selectedRun.repo_ref,
          git_ref: treeGitRef,
          exclude_regex: [],
          skip_binary: skipBinary,
          skip_gitignore: skipGitignore,
          include_staged_diff: includeStagedDiff,
          include_unstaged_diff: includeUnstagedDiff,
          include_files: contextMode === 'tree_select' ? selectedPaths : null,
          save_path: contextSavePath
        }
      });
      setContextStatus(`Context export completed: ${String(response.output_path ?? '')}`);
      await refreshEvents(selectedRun.id);
      await refresh();
    } catch (err) {
      setContextStatus(err instanceof Error ? err.message : String(err));
    } finally {
      setContextBusy(false);
    }
  }

  async function configureInference() {
    if (!selectedRun) return;
    setInferenceBusy(true);
    setInferenceStatus(null);
    try {
      await invokeModelInference(selectedRun.id, {
        step_id: selectedRun.current_step_id,
        action: 'configure',
        payload: {
          transport: inferenceProvider,
          model: inferenceModel,
          browser: {
            profile: 'auto',
            bridge_dir: 'bridge',
            cdp_url: browserCdpUrl,
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
        }
      });
      await refresh();
      await refreshEvents(selectedRun.id);
      setInferenceStatus('Inference configuration saved.');
    } catch (err) {
      setInferenceStatus(err instanceof Error ? err.message : String(err));
    } finally {
      setInferenceBusy(false);
    }
  }

  async function launchBrowserInference() {
    if (!selectedRun) return;
    setInferenceBusy(true);
    setInferenceStatus(null);
    try {
      const json = await invokeModelInference(selectedRun.id, {
        step_id: selectedRun.current_step_id,
        action: 'launch_browser',
        payload: {}
      });
      await refresh();
      await refreshEvents(selectedRun.id);
      setInferenceStatus(`Browser attached: ${String(json.session_id ?? '')}`);
    } catch (err) {
      setInferenceStatus(err instanceof Error ? err.message : String(err));
    } finally {
      setInferenceBusy(false);
    }
  }

  async function openBrowserInferenceUrl() {
    if (!selectedRun) return;
    setInferenceBusy(true);
    setInferenceStatus(null);
    try {
      await invokeModelInference(selectedRun.id, {
        step_id: selectedRun.current_step_id,
        action: 'open_url',
        payload: { url: browserTargetUrl }
      });
      await refresh();
      await refreshEvents(selectedRun.id);
      setInferenceStatus(`Opened URL: ${browserTargetUrl}`);
    } catch (err) {
      setInferenceStatus(err instanceof Error ? err.message : String(err));
    } finally {
      setInferenceBusy(false);
    }
  }

  async function probeBrowserInference() {
    if (!selectedRun) return;
    setInferenceBusy(true);
    setInferenceStatus(null);
    try {
      const json = await invokeModelInference(selectedRun.id, {
        step_id: selectedRun.current_step_id,
        action: 'probe_browser',
        payload: {}
      });
      setBrowserProbe((json.probe ?? null) as Record<string, unknown> | null);
      await refreshEvents(selectedRun.id);
      setInferenceStatus('Browser probe completed.');
    } catch (err) {
      setInferenceStatus(err instanceof Error ? err.message : String(err));
    } finally {
      setInferenceBusy(false);
    }
  }


  async function removeRun(runId: string) {
    await deleteRun(runId);
    if (selectedRunId === runId) {
      setSelectedRunId(null);
      setViewMode('home');
      setCapabilityOpen(false);
    }
    await refresh();
  }

  return (
    <AppShell padding="md">
      <AppShell.Main>
        <Stack>
          <Group justify="space-between">
            <Group>
              {viewMode === 'workflow' ? (
                <ActionIcon variant="light" onClick={() => setViewMode('home')}>
                  <IconArrowLeft size={16} />
                </ActionIcon>
              ) : null}
              <Title order={2}>{viewMode === 'home' ? 'Workflows' : selectedRun?.title ?? 'Workflow'}</Title>
            </Group>

            <Group>
              <Button variant="light" leftSection={<IconRefresh size={16} />} onClick={() => void refresh()}>
                Refresh
              </Button>
              {viewMode === 'home' ? (
                <Button leftSection={<IconPlus size={16} />} onClick={() => setCreateOpen(true)}>
                  Create workflow
                </Button>
              ) : (
                <Button leftSection={<IconBolt size={16} />} onClick={() => setCapabilityOpen(true)} disabled={!selectedRun}>
                  Global capability
                </Button>
              )}
            </Group>
          </Group>

          {error ? <Alert color="red">{error}</Alert> : null}

          {viewMode === 'home' ? (
            <Card withBorder>
              <Stack>
                <Group justify="space-between">
                  <Title order={4}>Workflow list</Title>
                  <Text c="dimmed" size="sm">Live status refreshes every 5 seconds</Text>
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
                      <Table.Tr key={run.id} onClick={() => openWorkflow(run.id)} style={{ cursor: 'pointer' }}>
                        <Table.Td>{run.title}</Table.Td>
                        <Table.Td><Badge>{run.status}</Badge></Table.Td>
                        <Table.Td>{run.current_step_id ?? '—'}</Table.Td>
                        <Table.Td><Code>{run.repo_ref}</Code></Table.Td>
                        <Table.Td>{new Date(run.updated_at).toLocaleString()}</Table.Td>
                        <Table.Td>
                          <Button
                            color="red"
                            variant="light"
                            size="xs"
                            onClick={(e) => {
                              e.stopPropagation();
                              void removeRun(run.id);
                            }}
                          >
                            Delete
                          </Button>
                        </Table.Td>
                      </Table.Tr>
                    ))}
                  </Table.Tbody>
                </Table>
              </Stack>
            </Card>
          ) : selectedRun ? (
            <Grid gutter="md" align="stretch">
              <Grid.Col span={{ base: 12, xl: 8 }}>
                <Card withBorder>
                <Stack>
                  <Group justify="space-between">
                    <Title order={4}>Workflow detail</Title>
                    <Badge>{selectedRun.status}</Badge>
                  </Group>
                  <Text fw={600}>{selectedRun.title}</Text>
                  <Code>{selectedRun.repo_ref}</Code>
                  <Text size="sm" c="dimmed">Current step: {selectedRun.current_step_id ?? '—'}</Text>

                  <Divider />
                  <Title order={5}>Workflow stages</Title>
                  {selectedTemplate && selectedTemplate.definition.steps.length ? (
                    <Group justify="space-between">
                      <Group>
                        <Button size="xs" variant="light" onClick={() => void previousWorkflowStep(selectedRun.id)}>
                          Previous stage
                        </Button>
                        <Button size="xs" onClick={() => void runStage(selectedRun.current_step_id ?? undefined)}>
                          Run stage
                        </Button>
                        <Button size="xs" variant="light" onClick={() => void nextWorkflowStep(selectedRun.id)}>
                          Next stage
                        </Button>
                      </Group>
                    </Group>
                  ) : null}
                  {selectedTemplate ? (
                    <Stack gap="xs">
                      {selectedTemplate.definition.steps.map((step, index) => {
                        const isCurrent = selectedRun.current_step_id === step.id;
                        return (
                          <Box
                            key={step.id}
                            p="sm"
                            style={{
                              border: isCurrent ? '1px solid var(--mantine-color-blue-4)' : '1px solid var(--mantine-color-dark-4)',
                              background: isCurrent ? 'rgba(34, 139, 230, 0.12)' : 'transparent',
                              borderRadius: 8
                            }}
                          >
                            <Group justify="space-between" align="start">
                              <div>
                                <Group gap="xs">
                                  <Badge variant={isCurrent ? 'filled' : 'light'} color={isCurrent ? 'blue' : 'gray'}>
                                    {index + 1}
                                  </Badge>
                                  <Text fw={700}>{step.name}</Text>
                                  <Badge variant="light">{step.automation_mode}</Badge>
                                  {isCurrent ? <Badge color="blue">CURRENT</Badge> : null}
                                </Group>
                                <Text size="sm" c="dimmed" mt={4}>{step.id}</Text>
                              </div>
                              <Group>
                                <Button size="xs" variant="light" onClick={() => void moveToStage(step.id)}>
                                  Select stage
                                </Button>
                              </Group>
                            </Group>
                            {isCurrent && currentStepDefinition ? (
                              <Stack gap="sm" mt="md">
                                <Textarea
                                  label="User input"
                                  minRows={4}
                                  value={stepFragmentValues.user_input}
                                  onChange={(e) => patchStepFragmentValue('user_input', e.currentTarget.value)}
                                  placeholder="Enter the message for the current step"
                                />

                                <Group align="end" gap="sm">
                                  <Checkbox
                                    checked={stepFragmentEnabled.repo_context}
                                    onChange={(e) => patchStepFragmentEnabled('repo_context', e.currentTarget.checked)}
                                    label="Include repo fragment"
                                  />
                                  <Button
                                    size="xs"
                                    variant="light"
                                    onClick={() => openContextExporter()}
                                    disabled={!selectedRun}
                                  >
                                    Context exporter
                                  </Button>
                                </Group>

                                {isCodeStep ? (
                                  <>
                                    <Group align="end" gap="sm">
                                      <Checkbox
                                        checked={stepFragmentEnabled.changeset_schema}
                                        onChange={(e) => patchStepFragmentEnabled('changeset_schema', e.currentTarget.checked)}
                                        label="Include changeset schema fragment"
                                      />
                                      <Button
                                        size="xs"
                                        variant="light"
                                        onClick={() => void openChangesetSchemaConfigurator()}
                                      >
                                        Configure schema
                                      </Button>
                                    </Group>
                                    {stepFragmentEnabled.changeset_schema ? (
                                      <Text size="sm" c="dimmed">
                                        {stepFragmentValues.changeset_schema.trim()
                                          ? `Schema configured (${stepFragmentValues.changeset_schema.length} chars)`
                                          : 'No schema configured yet.'}
                                      </Text>
                                    ) : null}

                                    <Group align="end" gap="sm">
                                      <Checkbox
                                        checked={stepFragmentEnabled.apply_error}
                                        onChange={(e) => patchStepFragmentEnabled('apply_error', e.currentTarget.checked)}
                                        label="Include apply error fragment"
                                      />
                                      <Button
                                        size="xs"
                                        variant="light"
                                        onClick={() => openApplyErrorConfigurator()}
                                      >
                                        Configure apply error
                                      </Button>
                                    </Group>
                                    {stepFragmentEnabled.apply_error ? (
                                      <Text size="sm" c="dimmed">
                                        {stepFragmentValues.apply_error.trim()
                                          ? `Apply error configured (${stepFragmentValues.apply_error.length} chars)`
                                          : 'No apply error configured yet.'}
                                      </Text>
                                    ) : null}

                                    <Group align="end" gap="sm">
                                      <Checkbox
                                        checked={stepFragmentEnabled.compile_error}
                                        onChange={(e) => patchStepFragmentEnabled('compile_error', e.currentTarget.checked)}
                                        label="Include compile error fragment"
                                      />
                                      <Button
                                        size="xs"
                                        variant="light"
                                        onClick={() => openCompileErrorConfigurator()}
                                      >
                                        Configure compile error
                                      </Button>
                                    </Group>
                                    {stepFragmentEnabled.compile_error ? (
                                      <Text size="sm" c="dimmed">
                                        {stepFragmentValues.compile_error.trim()
                                          ? `Compile error configured (${stepFragmentValues.compile_error.length} chars)`
                                          : 'No compile error configured yet.'}
                                      </Text>
                                    ) : null}
                                  </>
                                ) : null}

                                {renderComposedPromptPreviewPanel('No prompt fragments enabled yet.')}

                                {inferenceStatus ? <Alert color="blue">{inferenceStatus}</Alert> : null}
                                {renderInferenceResponsePanel('No inference response yet.')}
                              </Stack>
                            ) : null}
                          </Box>
                        );
                      })}
                    </Stack>
                  ) : (
                    <Alert color="gray">This run has no template definition attached, so stage structure cannot be shown.</Alert>
                  )}

                  <Divider />
                  <Title order={5}>Inference connection</Title>
                  <Stack gap="xs">
                    <Group>
                      {inferenceProvider === 'api' ? <Badge variant="light">Model: {inferenceModel}</Badge> : null}
                      <Badge color={inferenceConnectionStatus.color}>{inferenceConnectionStatus.label}</Badge>
                    </Group>
                    <TextInput label="Target URL" value={browserTargetUrl} onChange={(e) => setBrowserTargetUrl(e.currentTarget.value)} />
                    <TextInput label="CDP URL" value={browserCdpUrl} onChange={(e) => setBrowserCdpUrl(e.currentTarget.value)} />
                    <Group>
                      <Button size="xs" onClick={() => void configureInference()} loading={inferenceBusy}>Save config</Button>
                      <Button size="xs" onClick={() => void launchBrowserInference()} loading={inferenceBusy}>Launch + attach</Button>
                      <Button size="xs" variant="light" onClick={() => void openBrowserInferenceUrl()} loading={inferenceBusy}>Open URL</Button>
                      <Button size="xs" variant="light" onClick={() => void probeBrowserInference()} loading={inferenceBusy}>Probe</Button>
                    </Group>
                    {inferenceStatus ? <Alert color="blue">{inferenceStatus}</Alert> : null}
                    {browserProbe ? <Code block>{JSON.stringify(browserProbe, null, 2)}</Code> : null}
                  </Stack>

                  <Divider />
                  <Title order={5}>Global capabilities</Title>
                  <Group>
                    <Button variant="light" onClick={() => { setActiveCapability('context_export'); setCapabilityOpen(true); }}>Context exporter</Button>
                    <Button variant="light" onClick={() => { setActiveCapability('model_inference'); setCapabilityOpen(true); }}>Model inference</Button>
                    <Button variant="light" onClick={() => { setActiveCapability('inject_user_context'); setCapabilityOpen(true); }}>Inject user context</Button>
                    <Button variant="light" onClick={() => { setActiveCapability('inject_changeset_schema'); setCapabilityOpen(true); }}>Inject changeset schema</Button>
                  </Group>
                </Stack>
                </Card>
              </Grid.Col>

              <Grid.Col span={{ base: 12, xl: 4 }}>
                <Card withBorder style={{ height: 'calc(100vh - 140px)' }}>
                <Stack h="100%">
                  <Title order={4}>Live workflow events</Title>
                  <ScrollArea style={{ flex: 1 }} offsetScrollbars>
                    <Stack gap="xs">
                      {events.length === 0 ? (
                        <Text c="dimmed">No events yet.</Text>
                      ) : (
                        <Stack gap="sm">
                          {eventsByStage.map((stageGroup) => {
                            const stageKey = stageGroup.stepId;
                            const stageHistoryExpanded = stageGroup.isCurrent && expandedStageHistoryIds.has(stageKey);
                            const visibleStageEvents = stageGroup.isCurrent && !stageHistoryExpanded
                              ? stageGroup.events.slice(0, 4)
                              : stageGroup.events;
                            const hiddenEventCount = Math.max(stageGroup.events.length - visibleStageEvents.length, 0);
                            const stageExpanded = stageGroup.isCurrent
                              ? !collapsedStageIds.has(stageKey)
                              : expandedStageIds.has(stageKey);
                            const latestTone = stageGroup.latest ? eventTone(stageGroup.latest) : { color: 'gray', label: 'IDLE' };

                            return (
                              <Box
                                key={stageKey}
                                p="sm"
                                style={{
                                  border: stageGroup.isCurrent ? '1px solid var(--mantine-color-blue-4)' : '1px solid var(--mantine-color-dark-4)',
                                  borderRadius: 10,
                                  background: stageGroup.isCurrent ? 'rgba(34, 139, 230, 0.08)' : 'rgba(255,255,255,0.02)'
                                }}
                              >
                                <Group justify="space-between" align="flex-start" wrap="nowrap">
                                  <Stack gap={4} style={{ flex: 1 }}>
                                    <Group gap="xs" wrap="wrap">
                                      <Badge color={stageGroup.isCurrent ? 'blue' : latestTone.color}>
                                        {stageGroup.isCurrent ? 'CURRENT STAGE' : latestTone.label}
                                      </Badge>
                                      <Badge variant="light">{stageGroup.label}</Badge>
                                    </Group>
                                    <Text size="xs" c="dimmed">
                                      {visibleStageEvents.length} of {stageGroup.events.length} event{stageGroup.events.length === 1 ? '' : 's'} shown
                                    </Text>
                                  </Stack>

                                  <Group gap="xs">
                                    {stageGroup.isCurrent && stageGroup.events.length > 4 ? (
                                      <Button
                                        size="xs"
                                        variant="subtle"
                                        onClick={() => toggleStageHistoryExpanded(stageKey)}
                                      >
                                        {stageHistoryExpanded ? 'Show recent only' : `Show ${hiddenEventCount} earlier`}
                                      </Button>
                                    ) : null}
                                    <Button
                                      size="xs"
                                      variant="subtle"
                                      onClick={() => toggleStageExpanded(stageKey, stageGroup.isCurrent)}
                                    >
                                      {stageExpanded ? 'Collapse stage' : 'Expand stage'}
                                    </Button>
                                  </Group>
                                </Group>

                                {stageExpanded ? (
                                  <Stack gap="xs" mt="sm">
                                    {buildStageEventItems(visibleStageEvents).map((item) => {
                                      if (item.type === 'single') {
                                        const event = item.event;
                                        const tone = eventTone(event);
                                        const expanded = expandedEventIds.has(event.id);
                                        const summary = summarizeEvent(event);

                                        return (
                                          <Box
                                            key={item.key}
                                            p="sm"
                                            ml="md"
                                            style={{
                                              borderLeft: `3px solid var(--mantine-color-${tone.color}-6)`,
                                              borderRadius: 8,
                                              background: 'rgba(255,255,255,0.015)'
                                            }}
                                          >
                                            <Group justify="space-between" align="flex-start" wrap="nowrap">
                                              <Stack gap={4} style={{ flex: 1 }}>
                                                <Group gap="xs" wrap="wrap">
                                                  <Badge color={tone.color}>{tone.label}</Badge>
                                                  <Badge variant="light">{event.kind}</Badge>
                                                  <Text fw={600} size="sm">{summary}</Text>
                                                </Group>
                                                {summary !== event.message ? (
                                                  <Text size="xs" c="dimmed">{event.message}</Text>
                                                ) : null}
                                              </Stack>

                                              <Stack gap={6} align="flex-end">
                                                <Text size="xs" c="dimmed">{new Date(event.created_at).toLocaleString()}</Text>
                                                <Button
                                                  size="xs"
                                                  variant="subtle"
                                                  onClick={() => toggleEventExpanded(event.id)}
                                                >
                                                  {expanded ? 'Hide raw JSON' : 'Show raw JSON'}
                                                </Button>
                                              </Stack>
                                            </Group>

                                            {expanded ? (
                                              <Box mt="sm">
                                                <Code block>{JSON.stringify(event.payload, null, 2)}</Code>
                                              </Box>
                                            ) : null}
                                          </Box>
                                        );
                                      }

                                      const summaryEvent = item.summaryEvent;
                                      const tone = eventTone(summaryEvent);
                                      const expanded = expandedEventIds.has(item.key);
                                      const summary = summarizeEvent(summaryEvent);

                                      return (
                                        <Box
                                          key={item.key}
                                          p="sm"
                                          ml="md"
                                          style={{
                                            borderLeft: `3px solid var(--mantine-color-${tone.color}-6)`,
                                            borderRadius: 8,
                                            background: 'rgba(255,255,255,0.015)'
                                          }}
                                        >
                                          <Group justify="space-between" align="flex-start" wrap="nowrap">
                                            <Stack gap={4} style={{ flex: 1 }}>
                                              <Group gap="xs" wrap="wrap">
                                                <Badge color={tone.color}>{tone.label}</Badge>
                                                <Badge variant="light">{item.capabilityKey}</Badge>
                                                <Text fw={600} size="sm">{summary}</Text>
                                                {item.runtimeLabel ? <Badge variant="outline">{item.runtimeLabel}</Badge> : null}
                                              </Group>
                                              <Text size="xs" c="dimmed">
                                                {item.childEvents.length} lifecycle event{item.childEvents.length === 1 ? '' : 's'}
                                              </Text>
                                            </Stack>

                                            <Stack gap={6} align="flex-end">
                                              <Text size="xs" c="dimmed">{new Date(summaryEvent.created_at).toLocaleString()}</Text>
                                              <Button
                                                size="xs"
                                                variant="subtle"
                                                onClick={() => toggleEventExpanded(item.key)}
                                              >
                                                {expanded ? 'Hide capability logs' : 'Show capability logs'}
                                              </Button>
                                            </Stack>
                                          </Group>

                                          {expanded ? (
                                            <Stack gap="xs" mt="sm">
                                              {item.childEvents.slice().reverse().map((event) => (
                                                <Box key={event.id} p="xs" style={{ borderRadius: 8, background: 'rgba(255,255,255,0.02)' }}>
                                                  <Group justify="space-between" align="flex-start" wrap="nowrap">
                                                    <Stack gap={4} style={{ flex: 1 }}>
                                                      <Group gap="xs" wrap="wrap">
                                                        <Badge variant="light">{event.kind}</Badge>
                                                        <Text size="sm" fw={500}>{event.message}</Text>
                                                      </Group>
                                                    </Stack>
                                                    <Text size="xs" c="dimmed">{new Date(event.created_at).toLocaleString()}</Text>
                                                  </Group>
                                                  <Box mt="xs">
                                                    <Code block>{JSON.stringify(event.payload, null, 2)}</Code>
                                                  </Box>
                                                </Box>
                                              ))}
                                            </Stack>
                                          ) : null}
                                        </Box>
                                      );
                                    })}
                                  </Stack>
                                ) : null}
                              </Box>
                            );
                          })}
                        </Stack>
                      )}
                    </Stack>
                    </ScrollArea>
                  </Stack>
                </Card>
              </Grid.Col>
            </Grid>
          ) : (
            <Alert color="gray">Select a workflow run from the home page.</Alert>
          )}
        </Stack>

        <Modal
          opened={changesetSchemaConfigOpen}
          onClose={() => setChangesetSchemaConfigOpen(false)}
          title="Changeset schema fragment"
          size="80%"
          centered
        >
          <Stack>
            <Group justify="space-between">
              <Text size="sm" c="dimmed">Use the canonical payload-gateway schema example or paste custom guidance.</Text>
              <Button
                size="xs"
                variant="light"
                loading={changesetSchemaBusy}
                onClick={() => void openChangesetSchemaConfigurator()}
              >
                Reload from API
              </Button>
            </Group>
            <Textarea
              label="Changeset schema"
              minRows={18}
              value={stepFragmentValues.changeset_schema}
              onChange={(e) => patchStepFragmentValue('changeset_schema', e.currentTarget.value)}
              placeholder="Paste the changeset schema or guidance for code generation"
            />
            <Group justify="space-between">
              <Checkbox
                checked={stepFragmentEnabled.changeset_schema}
                onChange={(e) => patchStepFragmentEnabled('changeset_schema', e.currentTarget.checked)}
                label="Include changeset schema fragment"
              />
              <Button size="xs" onClick={() => setChangesetSchemaConfigOpen(false)}>Done</Button>
            </Group>
          </Stack>
        </Modal>

        <Modal
          opened={applyErrorConfigOpen}
          onClose={() => setApplyErrorConfigOpen(false)}
          title="Apply error fragment"
          size="70%"
          centered
        >
          <Stack>
            <Textarea
              label="Apply error"
              minRows={12}
              value={stepFragmentValues.apply_error}
              onChange={(e) => patchStepFragmentValue('apply_error', e.currentTarget.value)}
              placeholder="Paste apply failures for the next retry prompt"
            />
            <Group justify="space-between">
              <Checkbox
                checked={stepFragmentEnabled.apply_error}
                onChange={(e) => patchStepFragmentEnabled('apply_error', e.currentTarget.checked)}
                label="Include apply error fragment"
              />
              <Button size="xs" onClick={() => setApplyErrorConfigOpen(false)}>Done</Button>
            </Group>
          </Stack>
        </Modal>

        <Modal
          opened={compileErrorConfigOpen}
          onClose={() => setCompileErrorConfigOpen(false)}
          title="Compile error fragment"
          size="70%"
          centered
        >
          <Stack>
            <Textarea
              label="Compile error"
              minRows={12}
              value={stepFragmentValues.compile_error}
              onChange={(e) => patchStepFragmentValue('compile_error', e.currentTarget.value)}
              placeholder="Paste compile or test failures for the next retry prompt"
            />
            <Group justify="space-between">
              <Checkbox
                checked={stepFragmentEnabled.compile_error}
                onChange={(e) => patchStepFragmentEnabled('compile_error', e.currentTarget.checked)}
                label="Include compile error fragment"
              />
              <Button size="xs" onClick={() => setCompileErrorConfigOpen(false)}>Done</Button>
            </Group>
          </Stack>
        </Modal>

        <Modal
          opened={responseViewerOpen}
          onClose={() => setResponseViewerOpen(false)}
          title={previewViewerMode === 'prompt' ? 'Composed prompt preview' : 'Inference response'}
          size="min(1200px, 96vw)"
          centered
        >
          <Stack gap="md">
            <Group justify="space-between" align="center">
              <Group gap="xs">
                <Badge variant="light">
                  {(previewViewerMode === 'prompt' ? composedInferencePrompt : inferenceResponse)
                    ? `${(previewViewerMode === 'prompt' ? composedInferencePrompt : inferenceResponse).length.toLocaleString()} chars`
                    : 'empty'}
                </Badge>
                <Text size="sm" c="dimmed">Wrapped and formatted for review</Text>
              </Group>
              <Button
                size="xs"
                variant="light"
                onClick={() => {
                  void navigator.clipboard.writeText(previewViewerMode === 'prompt' ? composedInferencePrompt : inferenceResponse);
                }}
                disabled={!(previewViewerMode === 'prompt' ? composedInferencePrompt : inferenceResponse).trim()}
              >
                {previewViewerMode === 'prompt' ? 'Copy prompt' : 'Copy response'}
              </Button>
            </Group>

            <Box
              p="lg"
              style={{
                border: '1px solid var(--mantine-color-dark-4)',
                borderRadius: 12,
                background: 'linear-gradient(180deg, rgba(255,255,255,0.02), rgba(255,255,255,0.01))',
                boxShadow: 'inset 0 1px 0 rgba(255,255,255,0.03)'
              }}
            >
              <ScrollArea h="82vh" offsetScrollbars>
                <Box maw={920} mx="auto">
                  <Text
                    size="sm"
                    style={{
                      whiteSpace: 'pre-wrap',
                      overflowWrap: 'anywhere',
                      wordBreak: 'break-word',
                      lineHeight: 1.8,
                      letterSpacing: '0.01em',
                      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, Liberation Mono, monospace'
                    }}
                  >
                    {previewViewerMode === 'prompt'
                      ? (composedInferencePrompt || 'No prompt fragments enabled yet.')
                      : (inferenceResponse || 'No inference response yet.')}
                  </Text>
                </Box>
              </ScrollArea>
            </Box>
          </Stack>
        </Modal>

        <Modal opened={createOpen} onClose={() => setCreateOpen(false)} title="Create workflow" size="90%">
          <Stack>
            <Select
              label="Creation mode"
              value={createMode}
              onChange={(value) => setCreateMode((value as 'build' | 'copy' | 'save_template' | 'load') ?? 'build')}
              data={[
                { value: 'build', label: 'Build workflow' },
                { value: 'copy', label: 'Copy from template' },
                { value: 'save_template', label: 'Save template and create run' },
                { value: 'load', label: 'Load template metadata' }
              ]}
            />

            <TextInput label="Workflow name" value={workflowName} onChange={(e) => setWorkflowName(e.currentTarget.value)} />
            <TextInput label="Workflow description" value={workflowDescription} onChange={(e) => setWorkflowDescription(e.currentTarget.value)} />
            <TextInput label="Run title" value={runTitle} onChange={(e) => setRunTitle(e.currentTarget.value)} />
            <TextInput label="Repo path" value={repoRef} onChange={(e) => setRepoRef(e.currentTarget.value)} />

            {(createMode === 'copy' || createMode === 'load') ? (
              <Select
                label="Template"
                data={templates.map((t) => ({ value: t.id, label: t.name }))}
                value={selectedTemplateId}
                onChange={setSelectedTemplateId}
                searchable
                clearable
              />
            ) : null}

            <Divider label="Inference transport" labelPosition="center" />

            <Select
              label="Inference provider"
              value={inferenceProvider}
              onChange={(value) => setInferenceProvider((value as InferenceProvider) ?? 'api')}
              data={[
                { value: 'api', label: 'OpenAI API workflow' },
                { value: 'browser', label: 'Browser workflow' }
              ]}
            />

            <TextInput
              label="Model"
              value={inferenceModel}
              onChange={(e) => setInferenceModel(e.currentTarget.value)}
            />

            {inferenceProvider === 'browser' ? (
              <>
                <TextInput
                  label="Browser target URL"
                  value={browserTargetUrl}
                  onChange={(e) => setBrowserTargetUrl(e.currentTarget.value)}
                />
                <TextInput
                  label="CDP URL"
                  value={browserCdpUrl}
                  onChange={(e) => setBrowserCdpUrl(e.currentTarget.value)}
                />
              </>
            ) : null}

            <Divider label="Starter workflow builder" labelPosition="center" />

            <Stack>
              {builderSteps.map((step) => (
                <Card key={step.id} withBorder>
                  <Stack>
                    <Group justify="space-between">
                      <Group>
                        <Checkbox checked={step.enabled} onChange={(e) => patchStep(step.id, { enabled: e.currentTarget.checked })} />
                        <Title order={5}>{step.name}</Title>
                      </Group>
                      <Select
                        w={180}
                        label="Automation"
                        value={step.automationMode}
                        onChange={(value) => {
                          const nextMode = (value as AutomationMode) ?? 'manual';
                          if (step.id === 'code') {
                            patchStep(step.id, { automationMode: nextMode });
                            patchStepExecution(step.id, { changesetApply: nextMode !== 'manual' });
                            return;
                          }
                          patchStep(step.id, { automationMode: nextMode });
                        }}
                        data={step.id === 'design'
                          ? [{ value: 'manual', label: 'Manual' }]
                          : step.id === 'code'
                            ? step.execution.changesetApply && builderRepoIsGit
                              ? [
                                  { value: 'manual', label: 'Manual' },
                                  { value: 'assisted', label: 'Assisted' },
                                  { value: 'automatic', label: 'Automatic' }
                                ]
                              : [{ value: 'manual', label: 'Manual' }]
                            : [{ value: 'manual', label: 'Manual' }]}
                      />
                    </Group>

                    {step.id === 'code' ? (
                      <Box>
                        <Text fw={600} mb="xs">Capabilities</Text>
                        <Stack gap={6}>
                          <Checkbox
                            checked={step.execution.changesetApply}
                            disabled={!builderRepoIsGit}
                            onChange={(e) => {
                              const checked = e.currentTarget.checked;
                              patchStepExecution(step.id, { changesetApply: checked });
                              patchStep(step.id, { automationMode: checked ? 'automatic' : 'manual' });
                            }}
                            label="Apply changesets"
                          />
                          {!builderRepoIsGit ? (
                            <Text c="dimmed" size="sm">Apply Changesets is only available for git repositories.</Text>
                          ) : null}
                          {step.execution.changesetApply ? (
                            <Box pl="md">
                              <Text fw={600} size="sm" mb="xs">Apply changesets config</Text>
                              <TextInput
                                label="Max consecutive changeset failures before pause"
                                value={String(step.execution.maxConsecutiveApplyFailures)}
                                onChange={(e) => patchStepExecution(step.id, {
                                  maxConsecutiveApplyFailures: Math.max(1, Number.parseInt(e.currentTarget.value || '5', 10) || 5)
                                })}
                              />
                            </Box>
                          ) : null}
                        </Stack>
                      </Box>
                    ) : null}
                  </Stack>
                </Card>
              ))}
            </Stack>

            <Group justify="flex-end">
              <Button variant="light" onClick={() => setCreateOpen(false)}>Cancel</Button>
              <Button loading={busy} onClick={() => void submitCreateWorkflow()}>Create workflow</Button>
            </Group>
          </Stack>
        </Modal>

        <Modal opened={capabilityOpen} onClose={() => setCapabilityOpen(false)} title="Global capabilities" size="90%">
          <Tabs value={activeCapability} onChange={(value) => setActiveCapability((value as CapabilityKey) ?? 'context_export')}>
            <Tabs.List>
              <Tabs.Tab value="context_export">Context exporter</Tabs.Tab>
              <Tabs.Tab value="model_inference">Model inference</Tabs.Tab>
              <Tabs.Tab value="inject_user_context">Inject user context</Tabs.Tab>
              <Tabs.Tab value="inject_changeset_schema">Inject changeset schema</Tabs.Tab>
            </Tabs.List>

            <Tabs.Panel value="context_export" pt="md">
              {!selectedRun ? (
                <Alert color="gray">Select a workflow first.</Alert>
              ) : (
                <Stack>
                  <Group align="end">
                    <TextInput label="Repo" value={selectedRun.repo_ref} readOnly style={{ flex: 1 }} />
                    <TextInput label="Git ref" value={treeGitRef} onChange={(e) => setTreeGitRef(e.currentTarget.value)} />
                    <ActionIcon size="lg" variant="light" onClick={() => void refreshTree(selectedRun)}>
                      <IconRefresh size={18} />
                    </ActionIcon>
                  </Group>

                  <Group align="end">
                    <Select
                      label="Mode"
                      value={contextMode}
                      onChange={(value) => setContextMode((value as 'entire_repo' | 'tree_select') ?? 'tree_select')}
                      data={[
                        { value: 'entire_repo', label: 'ENTIRE REPO' },
                        { value: 'tree_select', label: 'TREE SELECT' }
                      ]}
                    />
                    <TextInput label="Save path" value={contextSavePath} onChange={(e) => setContextSavePath(e.currentTarget.value)} style={{ flex: 1 }} />
                    <Button loading={contextBusy} onClick={() => void runContextExport()} disabled={!contextSavePath || (contextMode === 'tree_select' && selectedPaths.length === 0)}>
                      Generate context file
                    </Button>
                  </Group>

                  <Group>
                    <Button color={skipBinary ? 'blue' : 'gray'} variant={skipBinary ? 'filled' : 'outline'} onClick={() => setSkipBinary((v) => !v)}>
                      {skipBinary ? 'Skip binary: ON' : 'Skip binary: OFF'}
                    </Button>
                    <Button color={skipGitignore ? 'blue' : 'gray'} variant={skipGitignore ? 'filled' : 'outline'} onClick={() => setSkipGitignore((v) => !v)}>
                      {skipGitignore ? 'Skip .gitignore: ON' : 'Skip .gitignore: OFF'}
                    </Button>
                    <Button color={includeStagedDiff ? 'blue' : 'gray'} variant={includeStagedDiff ? 'filled' : 'outline'} onClick={() => setIncludeStagedDiff((v) => !v)}>
                      {includeStagedDiff ? 'Staged diff: ON' : 'Staged diff: OFF'}
                    </Button>
                    <Button color={includeUnstagedDiff ? 'blue' : 'gray'} variant={includeUnstagedDiff ? 'filled' : 'outline'} onClick={() => setIncludeUnstagedDiff((v) => !v)}>
                      {includeUnstagedDiff ? 'Unstaged diff: ON' : 'Unstaged diff: OFF'}
                    </Button>
                  </Group>

                  {contextStatus ? <Alert color="blue">{contextStatus}</Alert> : null}
                  {treeError ? <Alert color="red">{treeError}</Alert> : null}

                  <Group justify="space-between">
                    <Text size="sm" c="dimmed">{treeRootData ? `Refreshed ${treeRootData.refreshed_at}` : 'No tree data loaded yet.'}</Text>
                    <Text size="sm">Selected files: <Code>{selectedPaths.length}</Code></Text>
                  </Group>

                  {treeBusy && !treeRootData ? (
                    <Group><Loader size="sm" /><Text size="sm">Scanning repository…</Text></Group>
                  ) : (
                    <RepoTree
                      rootEntries={rootTreeEntries}
                      childrenByParent={treeChildrenByParent}
                      loadingDirs={loadingTreeDirs}
                      selected={selectedSet}
                      selectedDirs={selectedDirPaths}
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
                      height={420}
                    />
                  )}
                </Stack>
              )}
            </Tabs.Panel>

            <Tabs.Panel value="model_inference" pt="md">
              {!selectedRun ? (
                <Alert color="gray">Select a workflow first.</Alert>
              ) : (
                <Stack>
                  <Group grow>
                    <Select
                      label="Transport"
                      value={inferenceProvider}
                      onChange={(value) => setInferenceProvider((value as InferenceProvider) ?? 'api')}
                      data={[
                        { value: 'api', label: 'OpenAI API' },
                        { value: 'browser', label: 'Browser workflow' }
                      ]}
                    />
                    {inferenceProvider === 'api' ? (
                      <TextInput
                        label="Model"
                        value={inferenceModel}
                        onChange={(e) => setInferenceModel(e.currentTarget.value)}
                      />
                    ) : null}
                    <Button loading={inferenceBusy} onClick={() => void configureInference()}>
                      Save config
                    </Button>
                  </Group>

                  {inferenceProvider === 'browser' ? (
                    <>
                      <Group grow>
                        <TextInput
                          label="CDP URL"
                          value={browserCdpUrl}
                          onChange={(e) => setBrowserCdpUrl(e.currentTarget.value)}
                        />
                        <TextInput
                          label="Chat URL"
                          value={browserTargetUrl}
                          onChange={(e) => setBrowserTargetUrl(e.currentTarget.value)}
                        />
                      </Group>
                      <Group>
                        <Button loading={inferenceBusy} onClick={() => void launchBrowserInference()}>
                          Launch + attach browser
                        </Button>
                        <Button variant="light" loading={inferenceBusy} onClick={() => void openBrowserInferenceUrl()}>
                          Open URL
                        </Button>
                        <Button variant="light" loading={inferenceBusy} onClick={() => void probeBrowserInference()}>
                          Probe
                        </Button>
                      </Group>
                      {browserProbe ? <Code block>{JSON.stringify(browserProbe, null, 2)}</Code> : null}
                    </>
                  ) : (
                    <Alert color="blue">This workflow is configured for API inference. Use the prompt box below to send API-backed turns.</Alert>
                  )}

                  <TextInput
                    label="Prompt"
                    value={inferencePrompt}
                    onChange={(e) => setInferencePrompt(e.currentTarget.value)}
                    placeholder="Send a design/code/test prompt"
                  />

                  <Group justify="flex-end">
                    <Button
                      loading={inferenceBusy}
                      onClick={() => void (async () => {
                        if (!selectedRun || !inferencePrompt.trim()) return;
                        setInferenceBusy(true);
                        setInferenceStatus(null);
                        try {
                          const json = await invokeModelInference(selectedRun.id, {
                            step_id: selectedRun.current_step_id,
                            action: 'send_prompt',
                            payload: {
                              prompt: inferencePrompt
                            }
                          });
                          const result = (json.result ?? {}) as Record<string, unknown>;
                          setInferenceResponse(String(result.text ?? ''));
                          setInferenceStatus(`Inference completed via ${String(result.transport ?? inferenceProvider)}.`);
                          await refresh();
                          await refreshEvents(selectedRun.id);
                        } catch (err) {
                          setInferenceStatus(err instanceof Error ? err.message : String(err));
                        } finally {
                          setInferenceBusy(false);
                        }
                      })()}
                      disabled={!inferencePrompt.trim()}
                    >
                      Send inference
                    </Button>
                  </Group>

                  {inferenceStatus ? <Alert color="blue">{inferenceStatus}</Alert> : null}
                  {renderInferenceResponsePanel('No inference response yet.')}
                </Stack>
              )}
            </Tabs.Panel>

            <Tabs.Panel value="inject_user_context" pt="md">
              <Alert color="blue">User context injection shell is present. Persist into run context next.</Alert>
            </Tabs.Panel>

            <Tabs.Panel value="inject_changeset_schema" pt="md">
              <Alert color="blue">Changeset schema injection shell is present. Feed it into prompt/context assembly next.</Alert>
            </Tabs.Panel>
          </Tabs>
        </Modal>
      </AppShell.Main>
    </AppShell>
  );
}
