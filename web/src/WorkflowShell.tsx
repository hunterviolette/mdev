import { Suspense, lazy, memo, useEffect, useMemo, useRef, useState } from 'react';
import {
  ActionIcon,
  Alert,
  Anchor,
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
  getWorkflowChangeset,
  getChangesetSchema,
  getRun,
  getRuntimeProjection,
  getRuntimeSnapshot,
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
  type InferenceTransport,
  type EventChainSummaryResponse,
  type RuntimeEventEnvelope,
  type RuntimeProjectionResponse,
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
import type { DiffPanelState } from './DiffPanel';
import { ensureSupervisorPlannerRun, getSupervisorRun, listSupervisorRuns, type FeaturePlanItem, type SupervisorRun } from './supervisor_api';
import { WorkflowBuilderEditor } from './WorkflowBuilderEditor';
import { SupervisorPanel } from './SupervisorPanel';
import { SupervisorPlannerModal } from './SupervisorPlannerModal';
import { defaultGlobals, descriptorMap, flattenStageFields } from './workflow_builder';
import {
  emptyRuntimeEventStore,
  reduceRuntimeEvent,
  reduceRuntimeSnapshot,
  subscribeRuntimeEventBus,
  type RuntimeEventStore
} from './runtime_events';

const DiffPanel = lazy(async () => {
  const mod = await import('./DiffPanel');
  return { default: mod.DiffPanel };
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


type ModelIoDirection = 'input' | 'output' | 'error';

type ModelIoContentBlock = {
  index: number;
  label: string;
  capabilityKey: string;
  contentFormat: string;
  content: string;
  role: string;
  source: string;
  enabled: boolean;
  defaultCollapsed: boolean;
  charCount: number;
};

type ModelIoTurn = {
  id: string;
  sequenceNo: number;
  createdAt: string;
  direction: ModelIoDirection;
  role: string;
  content: string;
  provider: string;
  model: string;
  transport: string;
  source: string;
  stepId: string;
  stageType: string;
  blockLabel: string;
  blocks: ModelIoContentBlock[];
};

type ModelIoSourceEvent = {
  id: string;
  kind: string;
  message: string;
  payload: Record<string, unknown>;
  created_at: string;
  sequence_no?: number;
  step_id?: string | null;
};

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function stringFrom(value: unknown): string {
  return typeof value === 'string' ? value : '';
}

function formatModelIoContent(content: string): string {
  const trimmed = content.trim();
  if (!trimmed) return '';
  if (/^```[\s\S]*```$/m.test(trimmed)) return trimmed;
  return trimmed;
}

function findStructuredPayloadStart(content: string): number {
  const lines = content.split('\n');
  let offset = 0;

  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed === '{' || trimmed === '[' || trimmed.startsWith('{"') || trimmed.startsWith('[{"')) {
      return offset;
    }
    offset += line.length + 1;
  }

  return -1;
}

function tryPrettyJson(content: string): string {
  try {
    return JSON.stringify(JSON.parse(content), null, 2);
  } catch {
    return content.trim();
  }
}

function summarizeStructuredPayload(content: string): string {
  try {
    const parsed = JSON.parse(content);
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      const record = parsed as Record<string, unknown>;
      const feature = asRecord(record.feature);
      const title = stringFrom(feature?.title) || stringFrom(record.title) || stringFrom(record.name);
      const keys = Object.keys(record).slice(0, 8).join(', ');
      if (title && keys) return `${title} · keys: ${keys}`;
      if (title) return title;
      if (keys) return `keys: ${keys}`;
    }
    if (Array.isArray(parsed)) return `${parsed.length} items`;
  } catch {
  }

  const firstLine = content.trim().split('\n').find((line) => line.trim().length > 0) ?? '';
  return firstLine.length > 140 ? `${firstLine.slice(0, 140)}…` : firstLine;
}

function formatCollapsibleStructuredPayload(content: string, label: string): string {
  const summary = summarizeStructuredPayload(content);
  const pretty = tryPrettyJson(content);
  return `${label}${summary ? ` — ${summary}` : ''}\n\n\`\`\`json\n${pretty}\n\`\`\``;
}

function formatReadableModelIoContent(content: string, direction: ModelIoDirection): string {
  const trimmed = formatModelIoContent(content);
  if (!trimmed) return '';

  const structuredStart = findStructuredPayloadStart(trimmed);
  if (structuredStart > 0) {
    return trimmed;
  }

  if (structuredStart === 0 && trimmed.length > 1200) {
    return trimmed;
  }

  if (trimmed.length > 6000) {
    return `Large ${direction} payload — ${trimmed.length.toLocaleString()} chars\n\n\`\`\`text\n${trimmed}\n\`\`\``;
  }

  return trimmed;
}

function readModelIoContentBlocks(meta: Record<string, unknown>, direction: ModelIoDirection): ModelIoContentBlock[] {
  const candidates = [
    meta.blocks,
    direction === 'input' ? meta.input_blocks : meta.output_blocks,
    direction === 'input' ? meta.prompt_blocks : meta.response_blocks,
    meta.content_blocks
  ];

  const rawBlocks = candidates.find((candidate) => Array.isArray(candidate)) as Array<Record<string, unknown>> | undefined;
  if (!rawBlocks) return [];

  return rawBlocks.map((block, index) => {
    const capabilityKey = stringFrom(block.capability_key) || stringFrom(block.capability) || stringFrom(block.key);
    const label = stringFrom(block.label) || stringFrom(block.title) || labelFromCapabilityKey(capabilityKey) || `Block ${index + 1}`;
    const role = stringFrom(block.role);
    const charCount = typeof block.char_count === 'number' ? block.char_count : stringFrom(block.content).length;
    return {
      index,
      label,
      capabilityKey,
      contentFormat: stringFrom(block.content_format) || stringFrom(block.format),
      content: stringFrom(block.content),
      role,
      source: stringFrom(block.source),
      enabled: block.enabled !== false,
      defaultCollapsed: typeof block.default_collapsed === 'boolean' ? block.default_collapsed : role !== 'user',
      charCount
    };
  });
}

function blockLabelForCodeFence(turnBlocks: ModelIoContentBlock[], fenceIndex: number, fallback: string): string {
  const block = turnBlocks[fenceIndex];
  if (!block) return fallback;
  return block.label;
}

function labelFromCapabilityKey(key: string): string {
  const normalized = key.trim();
  if (!normalized) return '';
  return normalized
    .replace(/[\/_-]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
    .replace(/\b\w/g, (char) => char.toUpperCase());
}

function modelIoBlockLabelFromMeta(meta: Record<string, unknown>, direction: ModelIoDirection, fallbackLanguage = ''): string {
  const explicitLabel = stringFrom(meta.block_label) || stringFrom(meta.label) || stringFrom(meta.title);
  if (explicitLabel) return explicitLabel;

  const capabilityKey = stringFrom(meta.capability_key) || stringFrom(meta.capability) || stringFrom(meta.source_capability) || stringFrom(meta.key);
  const capabilityLabel = labelFromCapabilityKey(capabilityKey);
  if (capabilityLabel) return `${capabilityLabel} ${direction === 'input' ? 'input' : direction === 'error' ? 'error' : 'output'}`;

  const source = asRecord(meta.source);
  const sourceCapability = source ? labelFromCapabilityKey(stringFrom(source.capability) || stringFrom(source.key)) : '';
  if (sourceCapability) return `${sourceCapability} ${direction === 'input' ? 'input' : direction === 'error' ? 'error' : 'output'}`;

  const language = fallbackLanguage.trim();
  if (language) return `${language} block`;

  return direction === 'input' ? 'Model input block' : direction === 'error' ? 'Model error block' : 'Model output block';
}

function pushModelIoTurn(
  turns: ModelIoTurn[],
  seen: Set<string>,
  event: ModelIoSourceEvent,
  direction: ModelIoDirection,
  role: string,
  content: string,
  meta: Record<string, unknown>,
  ordinal: number
) {
  const normalizedContent = formatReadableModelIoContent(content, direction);
  if (!normalizedContent) return;

  const key = [
    direction,
    role,
    event.created_at,
    stringFrom(meta.provider),
    stringFrom(meta.model),
    stringFrom(meta.transport),
    stringFrom(meta.step_id) || event.step_id || '',
    normalizedContent.slice(0, 512)
  ].join('|');

  if (seen.has(key)) return;
  seen.add(key);

  turns.push({
    id: `${event.id}:${direction}:${ordinal}`,
    sequenceNo: event.sequence_no ?? ordinal,
    createdAt: event.created_at,
    direction,
    role,
    content: normalizedContent,
    provider: stringFrom(meta.provider),
    model: stringFrom(meta.model),
    transport: stringFrom(meta.transport),
    source: event.kind || event.message || 'model',
    stepId: stringFrom(meta.step_id) || event.step_id || '',
    stageType: stringFrom(meta.stage_type),
    blockLabel: modelIoBlockLabelFromMeta(meta, direction),
    blocks: readModelIoContentBlocks(meta, direction)
  });
}

function pushInferencePayloadTurns(
  turns: ModelIoTurn[],
  seen: Set<string>,
  event: ModelIoSourceEvent,
  inferencePayload: Record<string, unknown>,
  ordinalBase: number
) {
  const modelIo = asRecord(inferencePayload.model_io);
  if (modelIo) {
    pushModelIoTurn(turns, seen, event, 'input', 'user', stringFrom(modelIo.input), modelIo, ordinalBase);
    pushModelIoTurn(turns, seen, event, stringFrom(modelIo.status) === 'failed' ? 'error' : 'output', 'assistant', stringFrom(modelIo.output), modelIo, ordinalBase + 1);
    return;
  }

  const result = asRecord(inferencePayload.result);
  const prompt = stringFrom(inferencePayload.prompt);
  const output = stringFrom(result?.text);
  const meta = {
    provider: stringFrom(inferencePayload.provider),
    model: stringFrom(inferencePayload.model),
    transport: stringFrom(result?.transport),
    capability_key: 'inference'
  };

  pushModelIoTurn(turns, seen, event, 'input', 'user', prompt, meta, ordinalBase);
  pushModelIoTurn(turns, seen, event, stringFrom(result?.message) ? 'error' : 'output', 'assistant', output, meta, ordinalBase + 1);
}

function collectModelIoTurns(events: ModelIoSourceEvent[]): ModelIoTurn[] {
  const turns: ModelIoTurn[] = [];
  const seen = new Set<string>();

  events.forEach((event, eventIndex) => {
    const payload = asRecord(event.payload) ?? {};
    const directModelIo = asRecord(payload.model_io);

    if (directModelIo && stringFrom(directModelIo.content)) {
      const direction = stringFrom(directModelIo.direction) as ModelIoDirection;
      pushModelIoTurn(
        turns,
        seen,
        event,
        direction === 'input' || direction === 'error' ? direction : 'output',
        stringFrom(directModelIo.role) || (direction === 'input' ? 'user' : 'assistant'),
        stringFrom(directModelIo.content),
        directModelIo,
        eventIndex * 10
      );
    }

    if (payload.capability === 'inference') {
      const resultPayload = asRecord(payload.result);
      if (resultPayload) {
        pushInferencePayloadTurns(turns, seen, event, resultPayload, eventIndex * 10 + 1);
      }
    }

    const capabilityResults = Array.isArray(payload.capability_results)
      ? payload.capability_results as Array<Record<string, unknown>>
      : [];

    capabilityResults.forEach((entry, entryIndex) => {
      if (stringFrom(entry.key) !== 'inference') return;
      const resultPayload = asRecord(entry.result);
      if (resultPayload) {
        pushInferencePayloadTurns(turns, seen, event, resultPayload, eventIndex * 10 + entryIndex + 1);
      }
    });
  });

  return turns
    .filter((turn, index, allTurns) => {
      const key = [
        turn.direction,
        turn.role,
        turn.stageType,
        turn.stepId,
        turn.provider,
        turn.model,
        turn.transport,
        normalizeModelHistoryContentForDedupe(turn.content)
      ].join('|');

      return allTurns.findIndex((candidate) => [
        candidate.direction,
        candidate.role,
        candidate.stageType,
        candidate.stepId,
        candidate.provider,
        candidate.model,
        candidate.transport,
        normalizeModelHistoryContentForDedupe(candidate.content)
      ].join('|') === key) === index;
    })
    .sort((a, b) => a.sequenceNo - b.sequenceNo || a.id.localeCompare(b.id));
}

function formatModelIoTranscript(turns: ModelIoTurn[], fallbackInput: string, fallbackOutput: string): string {
  if (turns.length > 0) return `${turns.length.toLocaleString()} model history turns`;

  const fallbackCount = [fallbackInput, fallbackOutput].filter((item) => item.trim()).length;
  if (fallbackCount > 0) return `${fallbackCount.toLocaleString()} fallback model history turns`;

  return '';
}

type ModelIoExchange = {
  id: string;
  sequenceNo: number;
  createdAt: string;
  stageType: string;
  stepId: string;
  provider: string;
  model: string;
  transport: string;
  input?: ModelIoTurn;
  output?: ModelIoTurn;
  error?: ModelIoTurn;
};

function normalizeModelHistoryContentForDedupe(value: string | undefined): string {
  return (value ?? '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, 2000);
}

function groupModelIoExchanges(turns: ModelIoTurn[]): ModelIoExchange[] {
  const exchanges: ModelIoExchange[] = [];
  let current: ModelIoExchange | null = null;
  const seenExchanges = new Set<string>();

  turns.forEach((turn) => {
    if (turn.direction === 'input' || !current) {
      current = {
        id: turn.id,
        sequenceNo: turn.sequenceNo,
        createdAt: turn.createdAt,
        stageType: turn.stageType,
        stepId: turn.stepId,
        provider: turn.provider,
        model: turn.model,
        transport: turn.transport,
        input: turn.direction === 'input' ? turn : undefined,
        output: turn.direction === 'output' ? turn : undefined,
        error: turn.direction === 'error' ? turn : undefined
      };
      exchanges.push(current);
      return;
    }

    if (turn.direction === 'output') {
      current.output = turn;
      return;
    }

    if (turn.direction === 'error') {
      current.error = turn;
    }
  });

  return exchanges.filter((exchange) => {
    const key = [
      exchange.stageType,
      exchange.stepId,
      exchange.provider,
      exchange.model,
      exchange.transport,
      normalizeModelHistoryContentForDedupe(exchange.input?.content),
      normalizeModelHistoryContentForDedupe(exchange.output?.content),
      normalizeModelHistoryContentForDedupe(exchange.error?.content)
    ].join('|');

    if (seenExchanges.has(key)) return false;
    seenExchanges.add(key);
    return true;
  });
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function formatCodeBlockContent(language: string, code: string): string {
  const normalizedLanguage = language.trim().toLowerCase();
  const trimmedCode = code.trim();

  if (normalizedLanguage === 'json') {
    try {
      return JSON.stringify(JSON.parse(trimmedCode), null, 2);
    } catch {
      return trimmedCode;
    }
  }

  return trimmedCode;
}

function summarizeModelHistoryCodeBlock(label: string, language: string, code: string): string {
  const normalizedLanguage = language.trim().toLowerCase() || 'text';
  const trimmed = code.trim();
  const blockKind = label || `${normalizedLanguage} block`;

  if (!trimmed) return blockKind;

  if (normalizedLanguage === 'json') {
    try {
      const parsed = JSON.parse(trimmed);
      if (Array.isArray(parsed)) return `${blockKind} · ${parsed.length.toLocaleString()} items`;
      if (parsed && typeof parsed === 'object') {
        const keys = Object.keys(parsed as Record<string, unknown>).slice(0, 6).join(', ');
        if (keys) return `${blockKind} · keys: ${keys}`;
      }
    } catch {
    }
  }

  const lineCount = trimmed.split('\n').length;
  return `${blockKind} · ${lineCount.toLocaleString()} line${lineCount === 1 ? '' : 's'} · ${trimmed.length.toLocaleString()} chars`;
}

function shouldCollapseModelHistoryCodeBlock(language: string, code: string): boolean {
  const normalizedLanguage = language.trim().toLowerCase() || 'text';
  const trimmed = code.trim();
  if (!trimmed) return false;
  if (trimmed.length > 500) return true;
  if (trimmed.split('\n').length > 12) return true;
  return normalizedLanguage === 'json' || normalizedLanguage === 'rust' || normalizedLanguage === 'rs' || normalizedLanguage === 'typescript' || normalizedLanguage === 'ts' || normalizedLanguage === 'javascript' || normalizedLanguage === 'js' || normalizedLanguage === 'text';
}

function capabilityBlockLanguage(block: ModelIoContentBlock): string {
  const format = block.contentFormat.trim().toLowerCase();
  if (format === 'json' || format === 'application/json') return 'json';
  if (format === 'rust' || format === 'rs') return 'rust';
  if (format === 'typescript' || format === 'ts') return 'typescript';
  if (format === 'javascript' || format === 'js') return 'javascript';
  if (format === 'markdown' || format === 'md') return 'markdown';
  return 'text';
}

function CapabilityContentBlock(props: { block: ModelIoContentBlock }) {
  const language = capabilityBlockLanguage(props.block);
  const code = formatCodeBlockContent(language, props.block.content);
  const summary = summarizeModelHistoryCodeBlock(props.block.label, language, code);

  if (props.block.role === 'user') {
    return (
      <Box p="sm">
        <Code
          block
          style={{
            whiteSpace: 'pre-wrap',
            overflowWrap: 'anywhere',
            wordBreak: 'break-word',
            fontSize: 12,
            lineHeight: 1.55,
          }}
        >
          {code}
        </Code>
      </Box>
    );
  }

  return (
    <Box p="sm">
      <details open={!props.block.defaultCollapsed} style={{ border: '1px solid rgba(139,148,158,0.24)', borderRadius: 8, background: 'rgba(0,0,0,0.16)', padding: 10 }}>
        <summary style={{ cursor: 'pointer' }}>
          <Group component="span" gap="xs" wrap="wrap" align="center">
            <Text component="span" size="xs" fw={700} tt="uppercase" style={{ letterSpacing: '0.06em' }}>
              {props.block.label}
            </Text>
            {props.block.role ? <Badge size="xs" variant="outline">{props.block.role}</Badge> : null}
            {props.block.source ? <Badge size="xs" variant="outline">{props.block.source}</Badge> : null}
            <Badge size="xs" variant="outline">{(props.block.charCount || code.length).toLocaleString()} chars</Badge>
            <Text component="span" size="xs" c="dimmed" style={{ minWidth: 160, flex: '1 1 280px' }}>
              {summary}
            </Text>
          </Group>
        </summary>
        <Box mt="xs">
          <Code
            block
            style={{
              whiteSpace: 'pre-wrap',
              overflowWrap: 'anywhere',
              wordBreak: 'break-word',
              fontSize: 12,
              lineHeight: 1.55,
            }}
          >
            {code}
          </Code>
        </Box>
      </details>
    </Box>
  );
}

function renderCapabilityBlocks(blocks: ModelIoContentBlock[]): JSX.Element[] {
  return blocks
    .filter((block) => block.content.trim())
    .map((block) => <CapabilityContentBlock key={`${block.index}:${block.capabilityKey}:${block.label}`} block={block} />);
}

function describeJsonValue(value: unknown): string {
  if (Array.isArray(value)) return `${value.length.toLocaleString()} item${value.length === 1 ? '' : 's'}`;
  if (value && typeof value === 'object') {
    const keys = Object.keys(value as Record<string, unknown>);
    return `${keys.length.toLocaleString()} key${keys.length === 1 ? '' : 's'}${keys.length > 0 ? `: ${keys.slice(0, 6).join(', ')}` : ''}`;
  }
  if (typeof value === 'string') return `${value.length.toLocaleString()} chars`;
  if (value === null) return 'null';
  return typeof value;
}

function jsonBlockTitle(key: string, value: unknown, fallback: string): string {
  const normalizedKey = key.trim();
  if (normalizedKey) return labelFromCapabilityKey(normalizedKey);
  if (Array.isArray(value)) return `${fallback} array`;
  if (value && typeof value === 'object') return `${fallback} object`;
  return fallback;
}

function renderJsonModelHistoryContent(content: string, fallbackLabel: string): JSX.Element[] | null {
  const trimmed = content.trim();
  if (!trimmed || !(trimmed.startsWith('{') || trimmed.startsWith('['))) return null;

  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    return null;
  }

  const entries = parsed && typeof parsed === 'object' && !Array.isArray(parsed)
    ? Object.entries(parsed as Record<string, unknown>)
    : [['', parsed]] as Array<[string, unknown]>;

  return entries.map(([key, value], index) => {
    const pretty = JSON.stringify(value, null, 2);
    const title = jsonBlockTitle(key, value, fallbackLabel || 'JSON response');
    const summary = describeJsonValue(value);

    return (
      <Box key={`json-response-${index}-${key || 'root'}`} p="sm">
        <details style={{ border: '1px solid rgba(139,148,158,0.24)', borderRadius: 8, background: 'rgba(0,0,0,0.16)', padding: 10 }}>
          <summary style={{ cursor: 'pointer' }}>
            <Group component="span" gap="xs" wrap="wrap" align="center">
              <Text component="span" size="xs" fw={700} tt="uppercase" style={{ letterSpacing: '0.06em' }}>
                {title}
              </Text>
              <Badge size="xs" variant="outline">json</Badge>
              <Badge size="xs" variant="outline">{pretty.length.toLocaleString()} chars</Badge>
              <Text component="span" size="xs" c="dimmed" style={{ minWidth: 160, flex: '1 1 280px' }}>
                {summary}
              </Text>
            </Group>
          </summary>
          <Box mt="xs">
            <Code
              block
              style={{
                whiteSpace: 'pre-wrap',
                overflowWrap: 'anywhere',
                wordBreak: 'break-word',
                fontSize: 12,
                lineHeight: 1.55,
              }}
            >
              {pretty}
            </Code>
          </Box>
        </details>
      </Box>
    );
  });
}

function ModelHistoryMarkdownContent(props: { content: string; direction: ModelIoDirection; blockLabel: string; blocks: ModelIoContentBlock[] }) {
  const nodes: JSX.Element[] = [];
  const explicitCapabilityBlocks = renderCapabilityBlocks(props.blocks);
  const structuredJsonNodes = explicitCapabilityBlocks.length === 0
    ? renderJsonModelHistoryContent(props.content, props.blockLabel || modelIoBlockLabelFromMeta({}, props.direction, 'json'))
    : null;
  if (structuredJsonNodes) return <Stack gap={0}>{structuredJsonNodes}</Stack>;

  const fencePattern = /```([^\n`]*)\n([\s\S]*?)```/g;
  let cursor = 0;
  let match: RegExpExecArray | null;
  let index = 0;

  while ((match = fencePattern.exec(props.content)) !== null) {
    const before = props.content.slice(cursor, match.index);
    const language = (match[1] || 'text').trim() || 'text';
    const code = formatCodeBlockContent(language, match[2] || '');

    if (before.trim()) {
      nodes.push(
        <Text
          key={`text-${index}`}
          component="div"
          size="sm"
          p="sm"
          style={{
            whiteSpace: 'pre-wrap',
            lineHeight: 1.65,
            overflowWrap: 'anywhere',
            wordBreak: 'break-word',
          }}
        >
          {before.trim()}
        </Text>
      );
    }

    const blockLabel = blockLabelForCodeFence(props.blocks, index, modelIoBlockLabelFromMeta({}, props.direction, language));
    const collapseCode = shouldCollapseModelHistoryCodeBlock(language, code);
    const codeSummary = summarizeModelHistoryCodeBlock(blockLabel, language, code);

    nodes.push(
      <Box key={`code-${index}`} p="sm">
        {collapseCode ? (
          <details style={{ border: '1px solid rgba(139,148,158,0.24)', borderRadius: 8, background: 'rgba(0,0,0,0.16)', padding: 10 }}>
            <summary style={{ cursor: 'pointer' }}>
              <Group component="span" gap="xs" wrap="nowrap">
                <Badge size="xs" variant="light">{blockLabel}</Badge>
                <Text component="span" size="xs" c="dimmed" truncate>
                  {codeSummary}
                </Text>
              </Group>
            </summary>
            <Box mt="xs">
              <Code
                block
                style={{
                  whiteSpace: 'pre-wrap',
                  overflowWrap: 'anywhere',
                  wordBreak: 'break-word',
                  fontSize: 12,
                  lineHeight: 1.55,
                }}
              >
                {code}
              </Code>
            </Box>
          </details>
        ) : (
          <>
            <Group justify="space-between" mb={6}>
              <Badge size="xs" variant="light">{blockLabel}</Badge>
              <Badge size="xs" variant="outline">{code.length.toLocaleString()} chars</Badge>
            </Group>
            <Code
              block
              style={{
                whiteSpace: 'pre-wrap',
                overflowWrap: 'anywhere',
                wordBreak: 'break-word',
                fontSize: 12,
                lineHeight: 1.55,
              }}
            >
              {code}
            </Code>
          </>
        )}
      </Box>
    );

    cursor = match.index + match[0].length;
    index += 1;
  }

  const after = props.content.slice(cursor);
  if (after.trim()) {
    nodes.push(
      <Text
        key={`text-${index}`}
        component="div"
        size="sm"
        p="sm"
        style={{
          whiteSpace: 'pre-wrap',
          lineHeight: 1.65,
          overflowWrap: 'anywhere',
          wordBreak: 'break-word',
        }}
      >
        {after.trim()}
      </Text>
    );
  }

  return <Stack gap={0}>{explicitCapabilityBlocks.length > 0 ? explicitCapabilityBlocks : nodes}</Stack>;
}

function ModelTurnCard(props: { label: string; turn?: ModelIoTurn; tone: 'input' | 'output' | 'error' }) {
  if (!props.turn) return null;

  const toneStyles = props.tone === 'input'
    ? { border: 'rgba(88,166,255,0.45)', background: 'rgba(56,139,253,0.08)', badge: 'blue' }
    : props.tone === 'error'
      ? { border: 'rgba(248,81,73,0.55)', background: 'rgba(248,81,73,0.08)', badge: 'red' }
      : { border: 'rgba(63,185,80,0.45)', background: 'rgba(46,160,67,0.08)', badge: 'green' };

  return (
    <Box
      p="md"
      mt="sm"
      style={{
        border: `1px solid ${toneStyles.border}`,
        background: toneStyles.background,
        borderRadius: 12,
        minWidth: 0,
      }}
    >
      <Group justify="space-between" align="center" mb="xs">
        <Group gap="xs">
          <Text size="xs" fw={800} tt="uppercase" style={{ letterSpacing: '0.08em' }}>
            {props.label}
          </Text>
          <Badge size="xs" variant="outline">
            {props.turn.content.length.toLocaleString()} chars
          </Badge>
        </Group>
        <Badge size="xs" color={toneStyles.badge} variant="light">
          {props.turn.role}
        </Badge>
      </Group>
      <Box
        style={{
          border: '1px solid rgba(139,148,158,0.18)',
          borderRadius: 8,
          background: 'rgba(0,0,0,0.14)',
          overflow: 'hidden',
        }}
      >
        <ModelHistoryMarkdownContent content={props.turn.content} direction={props.turn.direction} blockLabel={props.turn.blockLabel} blocks={props.turn.blocks} />
      </Box>
    </Box>
  );
}

function modelHistoryCopyText(turns: ModelIoTurn[]): string {
  const exchanges = groupModelIoExchanges(turns);
  return exchanges
    .map((exchange, index) => {
      const lines = [
        `Exchange ${index + 1}`,
        [
          exchange.createdAt ? `Time: ${formatTimestamp(exchange.createdAt)}` : '',
          exchange.stageType ? `Stage: ${exchange.stageType}` : '',
          exchange.stepId ? `Step: ${exchange.stepId}` : '',
          exchange.model ? `Model: ${exchange.model}` : '',
          exchange.provider ? `Provider: ${exchange.provider}` : '',
          exchange.transport ? `Transport: ${exchange.transport}` : ''
        ].filter(Boolean).join(' · '),
        exchange.input ? `\nPROMPT SENT TO MODEL\n${exchange.input.content}` : '',
        exchange.output ? `\nMODEL RESPONSE\n${exchange.output.content}` : '',
        exchange.error ? `\nMODEL ERROR\n${exchange.error.content}` : ''
      ];
      return lines.filter(Boolean).join('\n');
    })
    .join('\n\n---\n\n');
}

function ModelHistoryContent(props: { turns: ModelIoTurn[]; fallbackInput: string; fallbackOutput: string; emptyText: string }) {
  let turns = props.turns;

  if (turns.length === 0) {
    const fallbackTurns: ModelIoTurn[] = [];
    if (props.fallbackInput.trim()) {
      fallbackTurns.push({
        id: 'fallback:input',
        sequenceNo: 0,
        createdAt: '',
        direction: 'input',
        role: 'user',
        content: formatReadableModelIoContent(props.fallbackInput, 'input'),
        provider: '',
        model: '',
        transport: '',
        source: 'fallback',
        stepId: '',
        stageType: '',
        blockLabel: 'Fallback model input',
        blocks: []
      });
    }
    if (props.fallbackOutput.trim()) {
      fallbackTurns.push({
        id: 'fallback:output',
        sequenceNo: 1,
        createdAt: '',
        direction: 'output',
        role: 'assistant',
        content: formatReadableModelIoContent(props.fallbackOutput, 'output'),
        provider: '',
        model: '',
        transport: '',
        source: 'fallback',
        stepId: '',
        stageType: '',
        blockLabel: 'Fallback model output',
        blocks: []
      });
    }
    turns = fallbackTurns;
  }

  const exchanges = groupModelIoExchanges(turns);
  const [selectedExchangeIndex, setSelectedExchangeIndex] = useState<number | null>(null);
  const [showTimeline, setShowTimeline] = useState(false);

  useEffect(() => {
    setSelectedExchangeIndex((previous) => {
      if (previous === null) return null;
      return Math.min(previous, Math.max(0, exchanges.length - 1));
    });
  }, [exchanges.length]);

  if (exchanges.length === 0) {
    return <Text size="sm" c="dimmed">{props.emptyText}</Text>;
  }

  const activeExchangeIndex = selectedExchangeIndex ?? exchanges.length - 1;
  const exchange = exchanges[activeExchangeIndex] ?? exchanges[exchanges.length - 1];
  const status = exchange.error ? 'failed' : exchange.output ? 'completed' : 'pending';
  const statusColor = exchange.error ? 'red' : exchange.output ? 'green' : 'yellow';
  const meta = [
    exchange.createdAt ? formatTimestamp(exchange.createdAt) : '',
    exchange.stageType ? `Stage: ${exchange.stageType}` : '',
    exchange.stepId ? `Step: ${exchange.stepId}` : '',
    exchange.model ? `Model: ${exchange.model}` : '',
    exchange.provider ? `Provider: ${exchange.provider}` : '',
    exchange.transport ? `Transport: ${exchange.transport}` : ''
  ].filter(Boolean);

  return (
    <Stack gap="md">
      <Card
        withBorder
        radius="md"
        p="sm"
        style={{
          position: 'sticky',
          top: 0,
          zIndex: 3,
          background: 'rgba(31,31,31,0.96)',
          borderColor: 'rgba(139,148,158,0.32)',
          backdropFilter: 'blur(8px)'
        }}
      >
        <Stack gap="xs">
          <Group justify="space-between" align="center">
            <Group gap="xs">
              <Badge variant="light">{exchanges.length.toLocaleString()} exchanges</Badge>
              <Badge color={statusColor} variant="light">Viewing {activeExchangeIndex + 1}</Badge>
              <Text size="xs" c="dimmed">
                {exchange.createdAt ? formatTimestamp(exchange.createdAt) : 'Latest exchange'}
              </Text>
            </Group>
            <Group gap="xs">
              <Button size="compact-xs" variant="subtle" onClick={() => setSelectedExchangeIndex(0)} disabled={activeExchangeIndex === 0}>
                First
              </Button>
              <Button size="compact-xs" variant="subtle" onClick={() => setSelectedExchangeIndex(Math.max(0, activeExchangeIndex - 1))} disabled={activeExchangeIndex === 0}>
                Previous
              </Button>
              <Button size="compact-xs" variant="subtle" onClick={() => setSelectedExchangeIndex(Math.min(exchanges.length - 1, activeExchangeIndex + 1))} disabled={activeExchangeIndex >= exchanges.length - 1}>
                Next
              </Button>
              <Button size="compact-xs" variant="subtle" onClick={() => setSelectedExchangeIndex(null)} disabled={activeExchangeIndex >= exchanges.length - 1 && selectedExchangeIndex === null}>
                Latest
              </Button>
              <Button size="compact-xs" variant="light" onClick={() => setShowTimeline((value) => !value)}>
                {showTimeline ? 'Hide history' : 'Show history'}
              </Button>
            </Group>
          </Group>
          {showTimeline ? (
            <Group gap={6} wrap="wrap">
              {exchanges.map((item, index) => (
                <Button
                  key={`jump-${item.id}`}
                  size="compact-xs"
                  variant={index === activeExchangeIndex ? 'filled' : 'light'}
                  color={item.error ? 'red' : item.output ? 'green' : 'yellow'}
                  onClick={() => setSelectedExchangeIndex(index)}
                >
                  {index + 1}{item.createdAt ? ` · ${formatTimestamp(item.createdAt)}` : ''}
                </Button>
              ))}
            </Group>
          ) : null}
        </Stack>
      </Card>

      <Card
        id={`model-exchange-${activeExchangeIndex + 1}`}
        key={exchange.id}
        withBorder
        radius="lg"
        p="md"
        style={{
          scrollMarginTop: 96,
          background: 'linear-gradient(180deg, rgba(255,255,255,0.055), rgba(255,255,255,0.025))',
          borderColor: 'rgba(139,148,158,0.32)',
          minWidth: 0,
          maxHeight: 'calc(100vh - 280px)',
          overflow: 'hidden',
        }}
      >
        <Stack gap="sm">
          <Group justify="space-between" align="flex-start" gap="md">
            <Stack gap={4} style={{ minWidth: 0 }}>
              <Text fw={800}>Exchange {activeExchangeIndex + 1}</Text>
              <Text size="xs" c="dimmed" style={{ lineHeight: 1.45 }}>
                {meta.join(' · ')}
              </Text>
            </Stack>
            <Badge color={statusColor} variant="light">
              {status}
            </Badge>
          </Group>
          <Divider />
          <ScrollArea.Autosize
            mah="calc(100vh - 420px)"
            type="auto"
            offsetScrollbars
            style={{ minHeight: 0 }}
          >
            <Stack gap="sm" pr="xs">
              <ModelTurnCard label="Prompt sent to model" turn={exchange.input} tone="input" />
              <ModelTurnCard label="Model response" turn={exchange.output} tone="output" />
              <ModelTurnCard label="Model error" turn={exchange.error} tone="error" />
            </Stack>
          </ScrollArea.Autosize>
        </Stack>
      </Card>
    </Stack>
  );
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

export function WorkflowShell(props: {
  route?: {
    path: string;
    workflowRunId: string | null;
    workflowView?: 'workflow' | 'changes' | 'commits' | 'repository' | 'capabilities' | null;
    supervisorRunId: string | null;
    supervisorView?: 'planner' | 'sprint' | null;
  };
  navigate?: (path: string) => void;
}) {
  const [view, setView] = useState<ShellView>('monitor');
  const [builderMode, setBuilderMode] = useState<BuilderMode>('builder');
  const [monitorView, setMonitorView] = useState<MonitorView>('workflow_list');
  const [monitorHomeView, setMonitorHomeView] = useState<MonitorHomeView>('workflows');
  const [supervisorCreateRequestToken, setSupervisorCreateRequestToken] = useState(0);
  const [supervisorRefreshRequestToken, setSupervisorRefreshRequestToken] = useState(0);
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
  const [runtimeEvents, setRuntimeEvents] = useState<RuntimeEventStore>(emptyRuntimeEventStore);
  const [runtimeProjectionsByRunId, setRuntimeProjectionsByRunId] = useState<Record<string, EventChainSummaryResponse>>({});

  const selectedRunIdRef = useRef<string | null>(null);
  const allWorkflowEventsRef = useRef<Record<string, WorkflowEvent[]>>({});
  const hydratedWorkflowEventRunsRef = useRef<Set<string>>(new Set());
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

  useEffect(() => {
    if (monitorView !== 'workflow_detail') return;
    if (!selectedRun) return;
    const runTitle = selectedRun.title?.trim() || 'Untitled';
    const tabTitle = workflowTabTitle(activeWorkspaceTab);
    document.title = tabTitle === 'workflow'
      ? `Workflow · ${runTitle}`
      : `Workflow · ${tabTitle} · ${runTitle}`;
  }, [monitorView, selectedRun, activeWorkspaceTab]);
  const isInteractiveMode = selectedRun?.status === 'paused'
    || selectedRun?.status === 'waiting'
    || selectedRun?.status === 'draft'
    || selectedRun?.status === 'success'
    || selectedRun?.status === 'error'
    || selectedRun?.status === 'cancelled';
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

  function normalizeCheckpointDisposition(disposition: string) {
    if (disposition === 'continue_auto' || disposition === 'auto' || disposition === 'autonomous') return 'continue_auto';
    if (disposition === 'select_stage' || disposition === 'select' || disposition === 'continue_manual' || disposition === 'manual') return 'select_stage';
    if (disposition === 'continue_auto' || disposition === 'auto' || disposition === 'autonomous' || disposition === 'move_next' || disposition === 'continue') return 'continue_auto';
    if (disposition === 'pause_error' || disposition === 'pause' || disposition === 'paused') return 'pause_error';
    return disposition;
  }

  function checkpointDispositionLabel(disposition: string) {
    switch (normalizeCheckpointDisposition(disposition)) {
      case 'continue_auto':
        return 'Continue';
      case 'select_stage':
        return 'Select';
      case 'pause_error':
        return 'Pause';
      default:
        return disposition.replace(/_/g, ' ');
    }
  }

  function checkpointDispositionColor(disposition: string) {
    switch (normalizeCheckpointDisposition(disposition)) {
      case 'continue_auto':
        return 'green';
      case 'select_stage':
        return 'blue';
      case 'pause_error':
        return 'yellow';
      default:
        return undefined;
    }
  }

  const pendingDispositionReview = useMemo(() => {
    const workflowEngine = ((selectedRun?.context as Record<string, unknown> | undefined)?.workflow_engine ?? undefined) as Record<string, unknown> | undefined;
    const runState = (workflowEngine?.run_state ?? {}) as Record<string, unknown>;
    const blockedOn = (runState.blocked_on ?? null) as Record<string, unknown> | null;
    if (!blockedOn || (blockedOn.kind !== 'operator_checkpoint' && blockedOn.kind !== 'disposition_review')) return null;

    return {
      stageId: typeof blockedOn.stage_id === 'string' ? blockedOn.stage_id : selectedRun?.current_step_id ?? '',
      stageType: typeof blockedOn.stage_type === 'string' ? blockedOn.stage_type : '',
      recommendedDisposition: typeof blockedOn.recommended_disposition === 'string' ? blockedOn.recommended_disposition : '',
      nextStepId: typeof blockedOn.next_step_id === 'string' ? blockedOn.next_step_id : '',
      message: typeof blockedOn.message === 'string' ? blockedOn.message : '',
      availableDispositions: ['continue_auto', 'pause_error', 'select_stage']
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

  const persistedDiffPanelState = useMemo<DiffPanelState>(() => {
    const review = (selectedStageState?.review ?? {}) as Record<string, unknown>;
    const sourceControl = (review.source_control ?? {}) as Record<string, unknown>;
    return {
      selected_scope: sourceControl.selected_scope === 'staged' ? 'staged' : 'unstaged',
      selected_path: typeof sourceControl.selected_path === 'string' && sourceControl.selected_path.trim()
        ? sourceControl.selected_path
        : null,
      diff_style: sourceControl.diff_style === 'split' ? 'split' : 'unified',
      only_changes: Boolean(sourceControl.whole_file) ? false : sourceControl.only_changes !== false,
      context_lines: typeof sourceControl.context_lines === 'number' ? sourceControl.context_lines : 4,
      whole_file: Boolean(sourceControl.whole_file)
    };
  }, [selectedStageState]);
  const [localReviewSourceControlState, setLocalReviewSourceControlState] = useState<DiffPanelState>({
    selected_scope: 'unstaged',
    selected_path: null,
    diff_style: 'unified',
    only_changes: true,
    context_lines: 4,
    whole_file: false
  });
  useEffect(() => {
    if (selectedWorkflowStep?.step_type === 'review') {
      setLocalReviewSourceControlState(persistedDiffPanelState);
    }
  }, [persistedDiffPanelState, selectedWorkflowStep?.step_type]);
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
    const runId = selectedRunIdRef.current;
    if (!runId) return;

    const events = runtimeEvents.workflowEventsByRunId[runId] ?? [];
    const latest = events[events.length - 1];
    if (!latest) return;

    const shouldHydrateSelectedRun = latest.kind === 'workflow_waiting_for_operator_checkpoint'
      || latest.kind === 'stage_execution_waiting_for_operator_checkpoint'
      || latest.kind === 'stage_execution_waiting_for_disposition_review'
      || latest.kind === 'operator_checkpoint_resolved'
      || latest.kind === 'stage_execution_completed'
      || latest.kind === 'supervisor.workflow_terminal'
      || latest.kind === 'run_started'
      || latest.kind === 'run_status_changed';

    if (!shouldHydrateSelectedRun) return;

    let cancelled = false;
    void getRun(runId)
      .then((run) => {
        if (cancelled) return;
        setRuns((prev) => [run, ...prev.filter((item) => item.id !== run.id)]);
      })
      .catch(() => undefined);

    return () => {
      cancelled = true;
    };
  }, [selectedRunId, runtimeEvents.workflowEventsByRunId]);

  useEffect(() => {
    const routedRunId = props.route?.workflowRunId ?? null;
    const routedSupervisorRunId = props.route?.supervisorRunId ?? null;
    const routedPath = props.route?.path ?? window.location.pathname;

    if (routedRunId) {
      const routedWorkflowTab = workspaceTabFromRouteView(props.route?.workflowView ?? null);
      setView((value) => value === 'monitor' ? value : 'monitor');
      setMonitorView((value) => value === 'workflow_detail' ? value : 'workflow_detail');
      setActiveWorkspaceTab((value) => value === routedWorkflowTab ? value : routedWorkflowTab);
      if (routedRunId !== selectedRunIdRef.current) {
        setSelectedRunId(routedRunId);
        void refreshRunDetailsOnOpen(routedRunId);
      }
      return;
    }

    if (routedSupervisorRunId || routedPath === '/supervisors') {
      setView((value) => value === 'monitor' ? value : 'monitor');
      setMonitorView((value) => value === 'workflow_list' ? value : 'workflow_list');
      setMonitorHomeView((value) => value === 'supervisors' ? value : 'supervisors');
      setActiveWorkspaceTab((value) => value === 'workflows' ? value : 'workflows');
      return;
    }

    if (routedPath === '/workflows' || routedPath === '/') {
      setView((value) => value === 'monitor' ? value : 'monitor');
      setMonitorView((value) => value === 'workflow_list' ? value : 'workflow_list');
      setMonitorHomeView((value) => value === 'workflows' ? value : 'workflows');
      setActiveWorkspaceTab((value) => value === 'workflows' ? value : 'workflows');
    }
  }, [props.route?.path, props.route?.workflowRunId, props.route?.workflowView, props.route?.supervisorRunId]);

  useEffect(() => {
    if (!selectedRunId) return;
    if (monitorView !== 'workflow_detail') return;
    if (props.route?.supervisorRunId) return;
    const nextPath = workflowTabRoute(selectedRunId, activeWorkspaceTab);
    if ((props.route?.path ?? window.location.pathname) === nextPath) return;
    props.navigate?.(nextPath);
  }, [selectedRunId, monitorView, activeWorkspaceTab, props.route?.workflowRunId, props.route?.workflowView, props.route?.supervisorRunId, props.navigate]);



  useEffect(() => {
    allWorkflowEventsRef.current = allWorkflowEvents;
  }, [allWorkflowEvents]);

  useEffect(() => {
    return () => {
      for (const timer of Object.values(runRefreshTimersRef.current)) {
        window.clearTimeout(timer);
      }
      runRefreshTimersRef.current = {};
    };
  }, []);

  useEffect(() => {
    let cancelled = false;

    async function loadInitialGraph() {
      try {
        const snapshot = await getRuntimeSnapshot({ scope: 'all' });
        if (cancelled) return;
        setRuntimeEvents((prev) => reduceRuntimeSnapshot(prev, snapshot));
        hydrateRunsFromRuntimeSnapshot(snapshot.nodes);
      } catch {
      }
    }

    void loadInitialGraph();

    const unsubscribe = subscribeRuntimeEventBus({
      onOpen: () => {
        if (cancelled) return;
        setRuntimeEvents((prev) => ({ ...prev, connected: true }));
      },
      onClose: () => {
        if (cancelled) return;
        setRuntimeEvents((prev) => ({ ...prev, connected: false }));
      },
      onError: () => {
        if (cancelled) return;
        setRuntimeEvents((prev) => ({ ...prev, connected: false }));
        const runId = selectedRunIdRef.current;
        if (runId) {
          void hydrateWorkflowEventsFromHistory(runId);
          void hydrateRuntimeProjection(runId);
        }
      },
      onSnapshot: (snapshot) => {
        if (cancelled) return;
        setRuntimeEvents((prev) => reduceRuntimeSnapshot(prev, snapshot));
        hydrateRunsFromRuntimeSnapshot(snapshot.nodes ?? []);
      },
      onProjection: (projection) => {
        if (cancelled) return;
        setRuntimeProjectionsByRunId((prev) => ({
          ...prev,
          [projection.run_id]: projection
        }));
      },
      onEvent: (incoming) => {
        if (cancelled) return;
        setRuntimeEvents((prev) => reduceRuntimeEvent(prev, incoming));
        applyIncomingWorkflowEvent(incoming.event.run_id, incoming.event);

        if (incoming.event.run_id === selectedRunIdRef.current) {
          void hydrateRuntimeProjection(incoming.event.run_id);
        }
      }
    });

    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  useEffect(() => {
    void refreshRunsAndTemplates(props.route?.workflowRunId ?? undefined);
  }, []);

  useEffect(() => {
    if (!selectedRunId) {
      setEvents([]);
      setLiveExecutionTrails([]);
      return;
    }

    const storedEvents = runtimeEvents.workflowEventsByRunId[selectedRunId] ?? [];
    const workflowEvents = storedEvents as WorkflowEvent[];
    const projection = runtimeProjectionsByRunId[selectedRunId] ?? null;
    setEvents(workflowEvents);
    setAllWorkflowEvents((prev) => prev[selectedRunId] === workflowEvents ? prev : {
      ...prev,
      [selectedRunId]: workflowEvents
    });
    setLiveExecutionTrails(projection ? mapLiveExecutionTrailsFromProjection(projection) : []);

    if (!hydratedWorkflowEventRunsRef.current.has(selectedRunId)) {
      hydratedWorkflowEventRunsRef.current.add(selectedRunId);
      void hydrateWorkflowEventsFromHistory(selectedRunId);
      void hydrateRuntimeProjection(selectedRunId);
    } else if (!projection) {
      void hydrateRuntimeProjection(selectedRunId);
    }
  }, [selectedRunId, runtimeEvents.workflowEventsByRunId, runtimeProjectionsByRunId]);

  useEffect(() => {
    setEventStreamConnected(runtimeEvents.connected);
    setEventStreamStatusText(runtimeEvents.connected ? 'Runtime stream connected' : 'Runtime stream disconnected');
  }, [runtimeEvents.connected]);

  useEffect(() => {
    if (!selectedRunId) return;
    if (monitorView !== 'workflow_detail') return;

    const selectedStatus = selectedRun?.status ?? '';
    const shouldPollBackend = selectedStatus === 'queued'
      || selectedStatus === 'running'
      || selectedStatus === 'waiting'
      || busy
      || manualCapabilityBusy
      || pauseRequestBusy;

    if (!shouldPollBackend) return;

    let cancelled = false;
    const runId = selectedRunId;

    async function refreshSelectedRunFromBackend() {
      if (cancelled) return;
      try {
        await Promise.all([
          refreshRunDetails(runId),
          hydrateRuntimeProjection(runId)
        ]);
      } catch {
      }
    }

    void refreshSelectedRunFromBackend();
    const timer = window.setInterval(() => {
      void refreshSelectedRunFromBackend();
    }, 3500);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [selectedRunId, selectedRun?.status, monitorView, busy, manualCapabilityBusy, pauseRequestBusy]);


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
            ...currentInferenceBrowser
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
    const explicitSelection = arguments.length > 0;
    const [runsRes, templatesRes] = await Promise.all([listRuns(), listTemplates()]);
    setRuns(runsRes);
    setTemplates(templatesRes);

    const routedRunId = props.route?.workflowRunId ?? null;
    const currentSelectedRunId = selectedRunIdRef.current;
    const resolvedRunId = explicitSelection
      ? nextSelectedRunId ?? null
      : routedRunId
        ?? currentSelectedRunId
        ?? (monitorView === 'workflow_detail' ? null : runsRes[0]?.id ?? null);

    if (resolvedRunId !== selectedRunIdRef.current) {
      setSelectedRunId(resolvedRunId);
    }
    if (!selectedTemplateId && templatesRes[0]) setSelectedTemplateId(templatesRes[0].id);
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
      start_event_payload: capability.inputPayload ?? null,
      end_event_payload: capability.outputPayload ?? null,
      latest_payload: capability.latestPayload ?? null,
      input_payload: capability.inputPayload ?? null,
      output_payload: capability.outputPayload ?? null,
      output: capability.outputPayload ?? capability.latestPayload ?? null
    };
  }

  function payloadIndicatesUserWait(payload: unknown): boolean {
    const record = asRecord(payload) ?? {};
    const result = asRecord(record.result) ?? {};
    const nestedResult = asRecord(result.result) ?? {};

    return record.waiting_for_user === true
      || record.needs_user_response === true
      || result.waiting_for_user === true
      || result.needs_user_response === true
      || nestedResult.waiting_for_user === true
      || nestedResult.needs_user_response === true;
  }

  function eventIndicatesOperatorCheckpointWait(event: StageExecutionEvent | null): boolean {
    if (!event) return false;
    const payload = asRecord(event.payload) ?? {};
    const capability = typeof payload.capability === 'string' ? payload.capability : '';
    return event.kind === 'operator_checkpoint_waiting'
      || event.kind === 'stage_execution_waiting_for_operator_checkpoint'
      || event.kind === 'workflow_waiting_for_operator_checkpoint'
      || (capability === 'operator_checkpoint' && payloadIndicatesUserWait(event.payload));
  }

  function deriveCapabilityStatusLabel(event: StageExecutionEvent | null, fallback: string): string {
    if (!event) return fallback;
    if (eventIndicatesOperatorCheckpointWait(event)) return 'USER INPUT';
    if (event.level === 'error' || event.kind.endsWith('_failed')) return 'FAILED';
    if (event.kind.endsWith('_completed')) return 'COMPLETE';
    if (event.kind.endsWith('_started')) return 'RUNNING';
    return fallback;
  }

  function deriveCapabilityStatusColor(event: StageExecutionEvent | null, fallback: string): string {
    if (!event) return fallback;
    if (eventIndicatesOperatorCheckpointWait(event)) return 'yellow';
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

  function capabilityDisplayMessageFromPayload(payload: unknown, fallback?: string | null): string {
    const record = asRecord(payload);
    if (!record) return fallback ?? '';

    const result = asRecord(record.result);
    const nestedPayload = asRecord(record.payload);
    const nestedOutput = asRecord(record.output);
    const nestedResponse = asRecord(record.response);
    const nestedError = asRecord(record.error);

    const candidates = [
      record.error_message,
      record.error,
      nestedError?.message,
      nestedError?.summary,
      result?.error_message,
      result?.error,
      result?.message,
      result?.summary,
      result?.status,
      nestedOutput?.error_message,
      nestedOutput?.error,
      nestedOutput?.message,
      nestedOutput?.summary,
      nestedOutput?.status,
      nestedResponse?.error_message,
      nestedResponse?.error,
      nestedResponse?.message,
      nestedResponse?.summary,
      nestedResponse?.status,
      nestedPayload?.error_message,
      nestedPayload?.error,
      nestedPayload?.message,
      nestedPayload?.summary,
      nestedPayload?.status,
      record.message,
      record.summary,
      record.status
    ];

    for (const candidate of candidates) {
      const value = stringFrom(candidate);
      if (value) return value;
    }

    const lines = record.lines ?? result?.lines ?? nestedPayload?.lines ?? nestedOutput?.lines ?? nestedResponse?.lines;
    if (Array.isArray(lines)) {
      const value = lines
        .filter((item): item is string => typeof item === 'string' && item.trim().length > 0)
        .slice(0, 3)
        .join('\n')
        .trim();
      if (value) return value;
    }

    return fallback ?? '';
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

  function stripRuntimeContextFromCapabilityPayload(payload: unknown): unknown {
    const record = asRecord(payload);
    if (!record) return payload ?? null;
    const runtimeKeys = new Set([
      'run_context',
      'final_context',
      'prepared_context',
      'workflow_engine',
      'global_state',
      'local_state',
      'stage_state',
      'capability_results',
      'available_transitions',
      'blocked_on',
      'next_step_id',
      'current_step_id'
    ]);
    const entries = Object.entries(record).filter(([key]) => !runtimeKeys.has(key));
    if (entries.length === 0) return null;
    return Object.fromEntries(entries);
  }

  function capabilitySpecificPayload(event: StageExecutionEvent | null | undefined): unknown {
    if (!event) return null;
    if (!event.capability_invocation_id && isStageResultEvent(event)) return null;
    return stripRuntimeContextFromCapabilityPayload(event.payload);
  }

  function capabilityResultSpecificPayload(result: Record<string, unknown>): unknown {
    return stripRuntimeContextFromCapabilityPayload(result.result ?? result);
  }

  function normalizeCapabilityOrderKey(value: string): string {
    return value.trim().toLowerCase().replace(/[\s\/_-]+/g, '');
  }

  function capabilityDefinitionOrderForStep(stepId: string): Map<string, number> {
    const step = selectedRunDefinition?.steps.find((item) => item.id === stepId);
    const order = new Map<string, number>();
    let index = 0;
    for (const node of step?.execution_plan ?? []) {
      if (node.kind !== 'capability' || node.enabled === false) continue;
      const rawKey = stringFrom((node as unknown as Record<string, unknown>).key);
      if (!rawKey) continue;
      order.set(normalizeCapabilityOrderKey(rawKey), index);
      order.set(normalizeCapabilityOrderKey(formatCapabilityLabel(rawKey)), index);
      index += 1;
    }
    return order;
  }

  function buildLiveCapabilitiesFromEvents(trail: LiveStageTrail, rawEvents: StageExecutionEvent[]): LiveCapabilityTrail[] {
    const eventsAsc = rawEvents.slice().sort((a, b) => a.sequence_no - b.sequence_no || a.created_at.localeCompare(b.created_at));
    const grouped = new Map<string, StageExecutionEvent[]>();

    for (const event of eventsAsc) {
      const eventPayload = asRecord(event.payload) ?? {};
      const capabilityKey = stringFrom(eventPayload.capability)
        || stringFrom(eventPayload.capability_key)
        || stringFrom(eventPayload.key)
        || stringFrom(eventPayload.name);
      const capabilityId = event.capability_invocation_id
        ?? (capabilityKey ? `${trail.stageExecutionId}:capability:${capabilityKey}` : null);
      if (!capabilityId) continue;
      const bucket = grouped.get(capabilityId) ?? [];
      bucket.push(event);
      grouped.set(capabilityId, bucket);
    }

    const mapped: LiveCapabilityTrail[] = [];

    const firstSequenceByCapabilityId = new Map<string, number>();
    for (const [capabilityId, capabilityEvents] of grouped.entries()) {
      const firstEvent = capabilityEvents[0] ?? null;
      if (firstEvent) {
        firstSequenceByCapabilityId.set(capabilityId, firstEvent.sequence_no);
      }
    }

    for (const [capabilityId, capabilityEvents] of grouped.entries()) {
      const firstEvent = capabilityEvents[0] ?? null;
      const lastEvent = capabilityEvents[capabilityEvents.length - 1] ?? null;
      const startedEvent = capabilityEvents.find((event) => event.kind.endsWith('_started')) ?? firstEvent;
      const completedEvent = capabilityEvents.slice().reverse().find((event) => event.kind.endsWith('_completed') || event.kind.endsWith('_failed')) ?? null;
      const statusEvent = completedEvent ?? lastEvent;
      const startedPayload = asRecord(startedEvent?.payload) ?? {};
      const lastPayload = asRecord(lastEvent?.payload) ?? {};
      const capabilityName = formatCapabilityLabel(
        stringFrom(startedPayload.capability)
        || stringFrom(startedPayload.capability_key)
        || stringFrom(startedPayload.key)
        || stringFrom(startedPayload.name)
        || stringFrom(lastPayload.capability)
        || stringFrom(lastPayload.capability_key)
        || stringFrom(lastPayload.key)
        || stringFrom(lastPayload.name)
        || capabilityNameFromKind(startedEvent?.kind ?? lastEvent?.kind ?? capabilityId)
      );
      mapped.push({
        key: capabilityId,
        capabilityId,
        name: capabilityName,
        statusColor: deriveCapabilityStatusColor(statusEvent, 'gray'),
        statusLabel: deriveCapabilityStatusLabel(statusEvent, 'INFO'),
        message: capabilityDisplayMessageFromPayload(
          capabilitySpecificPayload(completedEvent) ?? capabilitySpecificPayload(statusEvent),
          statusEvent?.message ?? capabilityName
        ),
        startedAtText: startedEvent ? formatTimestamp(startedEvent.created_at) : '—',
        startedAtRaw: startedEvent?.created_at ?? null,
        durationText: formatDuration(startedEvent?.created_at ?? null, completedEvent?.created_at ?? null),
        durationMs: null,
        latestCreatedAt: statusEvent?.created_at ?? firstEvent?.created_at ?? '',
        isActive: completedEvent === null,
        isNew: capabilityEvents.some((event) => recentEventIds.has(event.id)),
        eventCount: capabilityEvents.length,
        latestLevel: statusEvent?.level ?? 'info',
        latestKind: statusEvent?.kind ?? '',
        latestPayload: capabilitySpecificPayload(statusEvent),
        inputPayload: capabilitySpecificPayload(startedEvent),
        outputPayload: capabilitySpecificPayload(completedEvent)
      });
    }

    const resultStageEvent = eventsAsc.slice().reverse().find((event) => isStageResultEvent(event)) ?? null;
    const resultStagePayload = asRecord(resultStageEvent?.payload) ?? {};
    const capabilityResults = Array.isArray(resultStagePayload.capability_results)
      ? resultStagePayload.capability_results as Array<Record<string, unknown>>
      : [];

    const resultOrderByCapabilityName = new Map<string, number>();
    capabilityResults.forEach((result, index) => {
      const resultKey = stringFrom(result.key) || stringFrom(result.capability) || stringFrom(result.name);
      if (resultKey) {
        resultOrderByCapabilityName.set(normalizeCapabilityOrderKey(resultKey), index);
        resultOrderByCapabilityName.set(normalizeCapabilityOrderKey(formatCapabilityLabel(resultKey)), index);
      }
    });

    for (const result of capabilityResults) {
      const resultKey = stringFrom(result.key) || stringFrom(result.capability) || stringFrom(result.name);
      if (!resultKey) continue;
      const resultLabel = formatCapabilityLabel(resultKey);
      const existing = mapped.find((capability) => capability.name.toLowerCase() === resultLabel.toLowerCase());
      const ok = result.ok !== false;
      const resultPayload = capabilityResultSpecificPayload(result);
      const resultRecord = asRecord(resultPayload) ?? {};
      const resultWaitingForUser = resultKey === 'operator_checkpoint' && payloadIndicatesUserWait(resultPayload);
      if (existing) {
        const resultClosesCapability = existing.outputPayload == null && !resultWaitingForUser;
        existing.statusColor = resultWaitingForUser ? 'yellow' : ok ? 'green' : 'red';
        existing.statusLabel = resultWaitingForUser ? 'USER INPUT' : ok ? 'SUCCESS' : 'ERROR';
        existing.isActive = resultWaitingForUser;
        existing.outputPayload = resultWaitingForUser ? existing.outputPayload : existing.outputPayload ?? resultPayload;
        existing.latestPayload = resultPayload ?? existing.latestPayload;
        if (resultClosesCapability && resultStageEvent) {
          existing.latestCreatedAt = resultStageEvent.created_at;
          existing.latestKind = resultStageEvent.kind;
          existing.durationText = formatDuration(existing.startedAtRaw, resultStageEvent.created_at);
        }
        existing.latestLevel = ok ? 'info' : 'error';
        existing.message = capabilityDisplayMessageFromPayload(resultPayload, existing.message);
        continue;
      }

      const capabilityId = `${trail.stageExecutionId}:result:${resultKey}`;
      mapped.push({
        key: capabilityId,
        capabilityId,
        name: resultLabel,
        statusColor: resultWaitingForUser ? 'yellow' : ok ? 'green' : 'red',
        statusLabel: resultWaitingForUser ? 'USER INPUT' : ok ? 'SUCCESS' : 'ERROR',
        message: capabilityDisplayMessageFromPayload(resultPayload, resultLabel),
        startedAtText: resultStageEvent ? formatTimestamp(resultStageEvent.created_at) : '—',
        startedAtRaw: resultStageEvent?.created_at ?? null,
        durationText: 'elapsed —',
        durationMs: null,
        latestCreatedAt: resultStageEvent?.created_at ?? '',
        isActive: resultWaitingForUser,
        isNew: resultStageEvent ? recentEventIds.has(resultStageEvent.id) : false,
        eventCount: 1,
        latestLevel: ok ? 'info' : 'error',
        latestKind: resultStageEvent?.kind ?? 'capability_result',
        latestPayload: resultPayload,
        inputPayload: null,
        outputPayload: resultWaitingForUser ? null : resultPayload
      });
    }

    if (resultStageEvent && !eventIndicatesOperatorCheckpointWait(resultStageEvent)) {
      for (const capability of mapped) {
        if (!capability.isActive) continue;
        capability.isActive = false;
        capability.statusColor = capability.statusColor === 'red' ? 'red' : 'green';
        capability.statusLabel = capability.statusLabel === 'FAILED' || capability.statusLabel === 'ERROR' ? capability.statusLabel : 'SUCCESS';
        capability.latestCreatedAt = resultStageEvent.created_at;
        capability.latestKind = resultStageEvent.kind;
        capability.latestLevel = capability.statusColor === 'red' ? 'error' : 'info';
        capability.durationText = formatDuration(capability.startedAtRaw, resultStageEvent.created_at);
      }
    }

    return mapped.sort((a, b) => {
      if (a.isActive !== b.isActive) return a.isActive ? -1 : 1;

      const latestCreatedAtOrder = b.latestCreatedAt.localeCompare(a.latestCreatedAt);
      if (latestCreatedAtOrder !== 0) return latestCreatedAtOrder;

      const aFirstSequence = firstSequenceByCapabilityId.get(a.capabilityId) ?? Number.MAX_SAFE_INTEGER;
      const bFirstSequence = firstSequenceByCapabilityId.get(b.capabilityId) ?? Number.MAX_SAFE_INTEGER;
      if (aFirstSequence !== bFirstSequence) return bFirstSequence - aFirstSequence;

      const aNameKey = normalizeCapabilityOrderKey(a.name);
      const bNameKey = normalizeCapabilityOrderKey(b.name);
      const aResultOrder = resultOrderByCapabilityName.get(aNameKey) ?? Number.MAX_SAFE_INTEGER;
      const bResultOrder = resultOrderByCapabilityName.get(bNameKey) ?? Number.MAX_SAFE_INTEGER;
      if (aResultOrder !== bResultOrder) return bResultOrder - aResultOrder;

      return a.name.localeCompare(b.name);
    });
  }

  function isTerminalStageEvent(event: StageExecutionEvent): boolean {
    if (event.capability_invocation_id) return false;
    return event.kind === 'stage_execution_completed'
      || event.kind === 'stage_executed'
      || event.kind === 'capability_stage_completed'
      || event.kind === 'capability_stage_failed';
  }

  function isStageResultEvent(event: StageExecutionEvent): boolean {
    if (event.capability_invocation_id) return false;
    const payload = asRecord(event.payload) ?? {};
    const capabilityResults = payload.capability_results;
    return isTerminalStageEvent(event)
      || event.kind === 'stage_execution_waiting_for_operator_checkpoint'
      || event.kind === 'stage_execution_waiting_for_disposition_review'
      || (Array.isArray(capabilityResults) && capabilityResults.length > 0);
  }

  function mapLiveExecutionTrailsFromProjection(projection: EventChainSummaryResponse): LiveStageTrail[] {
    return projection.stages.map((stage) => ({
      key: stage.key,
      stepId: stage.step_id,
      label: stage.label,
      stageExecutionId: stage.stage_execution_id,
      latestCreatedAt: stage.latest_created_at,
      durationMs: stage.duration_ms,
      isActive: stage.is_active,
      isCurrent: stage.is_current,
      capabilities: stage.capabilities.map((capability) => ({
        key: capability.key,
        capabilityId: capability.capability_id,
        name: capability.name,
        statusColor: capability.status_color,
        statusLabel: capability.status_label,
        message: capability.message,
        startedAtText: capability.started_at ? formatTimestamp(capability.started_at) : '—',
        startedAtRaw: capability.started_at ?? null,
        durationText: formatDurationMs(capability.duration_ms ?? null, capability.started_at ?? null, capability.completed_at ?? null),
        durationMs: capability.duration_ms ?? null,
        latestCreatedAt: capability.latest_created_at,
        isActive: capability.is_active,
        isNew: Boolean(capability.start_event_id && recentEventIds.has(capability.start_event_id))
          || Boolean(capability.end_event_id && recentEventIds.has(capability.end_event_id)),
        eventCount: capability.event_count,
        latestLevel: capability.latest_level ?? capability.status_label.toLowerCase(),
        latestKind: capability.latest_kind ?? '',
        latestPayload: capability.latest_payload ?? capability.end_payload ?? capability.start_payload ?? null,
        inputPayload: capability.start_payload ?? null,
        outputPayload: capability.end_payload ?? null
      }))
    }));
  }

  function mergeStageExecutionEvents(existing: StageExecutionEvent[], incoming: StageExecutionEvent[]): StageExecutionEvent[] {
    const byId = new Map<string, StageExecutionEvent>();
    for (const event of existing) {
      byId.set(event.id, event);
    }
    for (const event of incoming) {
      byId.set(event.id, event);
    }
    return Array.from(byId.values()).sort((a, b) => a.sequence_no - b.sequence_no || a.created_at.localeCompare(b.created_at));
  }

  function mergeWorkflowEventsIntoRuntimeStore(runId: string, incoming: Array<WorkflowEvent | StageExecutionEvent>) {
    const stageEvents = incoming.map((event) => event as StageExecutionEvent);
    setRuntimeEvents((prev) => {
      const merged = mergeStageExecutionEvents(prev.workflowEventsByRunId[runId] ?? [], stageEvents);
      const latestSequenceNo = merged.reduce((latest, event) => Math.max(latest, event.sequence_no), prev.latestSequenceNo);
      return {
        ...prev,
        workflowEventsByRunId: {
          ...prev.workflowEventsByRunId,
          [runId]: merged
        },
        latestSequenceNo
      };
    });
  }

  async function hydrateWorkflowEventsFromHistory(runId: string) {
    const runEvents = await listRunEvents(runId);
    mergeWorkflowEventsIntoRuntimeStore(runId, runEvents);
  }

  async function hydrateRuntimeProjection(runId: string) {
    try {
      const response: RuntimeProjectionResponse = await getRuntimeProjection({ run_id: runId });
      const projection = response.runs.find((item) => item.run_id === runId) ?? response.runs[0] ?? null;
      if (!projection) return;
      setRuntimeProjectionsByRunId((prev) => ({
        ...prev,
        [projection.run_id]: projection
      }));
    } catch {
    }
  }

  function mergeWorkflowEvents(existing: WorkflowEvent[], incoming: WorkflowEvent & { sequence_no?: number }): WorkflowEvent[] {
    const deduped = existing.filter((item) => item.id !== incoming.id);
    return [...deduped, incoming].sort((a, b) => a.created_at.localeCompare(b.created_at));
  }

  function mergeWorkflowEngineRunState(context: Record<string, unknown>, runStatePatch: Record<string, unknown>): Record<string, unknown> {
    const workflowEngine = asRecord(context.workflow_engine) ?? {};
    const runState = asRecord(workflowEngine.run_state) ?? {};
    return {
      ...context,
      workflow_engine: {
        ...workflowEngine,
        run_state: {
          ...runState,
          ...runStatePatch
        }
      }
    };
  }

  function projectRunStateFromRuntimeEvent(run: WorkflowRun, incoming: StageExecutionEvent): WorkflowRun {
    const payload = asRecord(incoming.payload) ?? {};
    if (incoming.kind === 'stage_execution_waiting_for_operator_checkpoint' || incoming.kind === 'stage_execution_waiting_for_disposition_review') {
      const stepId = stringFrom(payload.step_id) || incoming.step_id || run.current_step_id || '';
      const stepType = stringFrom(payload.step_type);
      const disposition = stringFrom(payload.disposition) || 'move_next';
      const message = stringFrom(payload.message) || incoming.message;
      return {
        ...run,
        status: 'waiting',
        current_step_id: stepId || run.current_step_id,
        updated_at: incoming.created_at,
        context: mergeWorkflowEngineRunState(run.context as Record<string, unknown>, {
          blocked_on: {
            kind: 'operator_checkpoint',
            stage_id: stepId,
            stage_type: stepType,
            recommended_disposition: disposition === 'paused' || disposition === 'pause' || disposition === 'pause_error' ? 'pause_error' : 'continue_manual',
            next_step_id: '',
            message,
            available_dispositions: ['continue_auto', 'continue_manual', 'pause_error']
          }
        })
      };
    }

    if (isTerminalStageEvent(incoming)) {
      const workflowEngine = asRecord((run.context as Record<string, unknown>).workflow_engine) ?? {};
      const runState = asRecord(workflowEngine.run_state) ?? {};
      const blockedOn = asRecord(runState.blocked_on);
      if (blockedOn?.kind === 'operator_checkpoint' || blockedOn?.kind === 'disposition_review') return run;
      return {
        ...run,
        updated_at: incoming.created_at,
        context: mergeWorkflowEngineRunState(run.context as Record<string, unknown>, {
          blocked_on: null
        })
      };
    }

    return run;
  }

  function applyIncomingWorkflowEvent(runId: string, incoming: StageExecutionEvent) {
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

    setRuns((prev) => prev.map((run) => run.id === runId ? projectRunStateFromRuntimeEvent(run, incoming) : run));

    const payload = incoming.payload as Record<string, unknown>;
    const snapshotContext = payload.final_context ?? payload.run_context ?? payload.prepared_context;
    if (incoming.kind !== 'stage_execution_waiting_for_operator_checkpoint' && incoming.kind !== 'stage_execution_waiting_for_disposition_review' && snapshotContext && typeof snapshotContext === 'object' && !Array.isArray(snapshotContext)) {
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
        context: snapshotContext as Record<string, unknown>,
        updated_at: incoming.created_at
      } : run));
    }
  }

  function hydrateRunsFromRuntimeSnapshot(nodes: Array<{ key: string; node_type: string; id: string; status: string; title: string; repo_ref: string; workflow_key?: string | null; current_step_id?: string | null; updated_at: string }>) {
    const workflowNodes = nodes.filter((node) => node.node_type === 'workflow_run');
    if (workflowNodes.length === 0) return;

    setRuns((prev) => {
      const existingById = new Map(prev.map((run) => [run.id, run]));
      for (const node of workflowNodes) {
        const existing = existingById.get(node.id);
        if (!existing) continue;
        existingById.set(node.id, {
          ...existing,
          status: node.status as WorkflowRunStatus,
          title: node.title,
          repo_ref: node.repo_ref,
          workflow_key: node.workflow_key ?? existing.workflow_key,
          current_step_id: node.current_step_id ?? existing.current_step_id,
          updated_at: node.updated_at
        });
      }
      return Array.from(existingById.values()).sort((a, b) => b.updated_at.localeCompare(a.updated_at));
    });
  }

  async function refreshRunDetails(runId: string) {
    const [run, runEvents] = await Promise.all([getRun(runId), listRunEvents(runId)]);
    setRuns((prev) => [run, ...prev.filter((item) => item.id !== run.id)]);
    mergeWorkflowEventsIntoRuntimeStore(run.id, runEvents);
    hydratedWorkflowEventRunsRef.current.add(run.id);
    await hydrateRuntimeProjection(run.id);
    if (selectedRunIdRef.current === run.id) {
      setSelectedRunId(run.id);
    }
  }

  async function refreshRunDetailsOnOpen(runId: string) {
    const [run, runEvents] = await Promise.all([openWorkflowRun(runId), listRunEvents(runId)]);
    setRuns((prev) => [run, ...prev.filter((item) => item.id !== run.id)]);
    mergeWorkflowEventsIntoRuntimeStore(run.id, runEvents);
    hydratedWorkflowEventRunsRef.current.add(run.id);
    await hydrateRuntimeProjection(run.id);
    setSelectedRunId(run.id);
  }


  function workflowRoute(runId: string) {
    return `/workflows/${encodeURIComponent(runId)}`;
  }

  function workflowTabRoute(runId: string, tab: WorkspaceTabKey) {
    const base = workflowRoute(runId);
    switch (tab) {
      case 'diff':
        return `${base}/changes`;
      case 'commits':
        return `${base}/commits`;
      case 'files':
        return `${base}/repository`;
      case 'capabilities':
        return `${base}/capabilities`;
      default:
        return base;
    }
  }

  function workspaceTabFromRouteView(view: 'workflow' | 'changes' | 'commits' | 'repository' | 'capabilities' | null | undefined): WorkspaceTabKey {
    switch (view) {
      case 'changes':
        return 'diff';
      case 'commits':
        return 'commits';
      case 'repository':
        return 'files';
      case 'capabilities':
        return 'capabilities';
      default:
        return 'workflows';
    }
  }

  function workflowTabTitle(tab: WorkspaceTabKey): string {
    switch (tab) {
      case 'diff':
        return 'changes';
      case 'commits':
        return 'commits';
      case 'files':
        return 'repository';
      case 'capabilities':
        return 'capabilities';
      default:
        return 'workflow';
    }
  }

  function shouldUseBrowserNavigation(event: { defaultPrevented: boolean; button: number; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean }) {
    return event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey;
  }

  function handleWorkflowLinkClick(event: { defaultPrevented: boolean; button: number; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; preventDefault: () => void }, runId: string) {
    if (shouldUseBrowserNavigation(event)) return;
    event.preventDefault();
    void openWorkflow(runId);
  }

  async function openWorkflow(runId: string) {
    setSelectedRunId(runId);
    setView('monitor');
    setMonitorView('workflow_detail');
    setActiveWorkspaceTab('workflows');
    props.navigate?.(workflowTabRoute(runId, 'workflows'));
    void refreshRunDetailsOnOpen(runId);
  }

  function backToWorkflowList() {
    setMonitorView('workflow_list');
    setMonitorHomeView('workflows');
    props.navigate?.('/workflows');
  }

  function handleWorkflowListLinkClick(event: { defaultPrevented: boolean; button: number; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; preventDefault: () => void }) {
    if (shouldUseBrowserNavigation(event)) return;
    event.preventDefault();
    backToWorkflowList();
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
    if (hasPendingDispositionReview) {
      await handleDispositionReview('pause');
      return;
    }
    const runId = selectedRunId;
    try {
      setPauseRequestBusy(true);
      setError(null);
      await pauseWorkflowRun(runId);
      await refreshRunDetails(runId);
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
      setManualCapabilityStatus('Workflow execution cancelled and returned to operator control.');
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
  }

  const composedInferencePrompt = useMemo(() => {
    if (selectedWorkflowStep?.step_type === 'compile') {
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
    const executionItems = selectedLiveExecutionState?.chain?.items ?? [];
    const stageEvents = selectedStepId ? events.filter((event) => event.step_id === selectedStepId) : events;

    if (selectedWorkflowStep?.step_type === 'compile') {
      const parts: string[] = [];
      if (composedInferencePrompt.trim()) parts.push(`### INPUT\n${composedInferencePrompt}`);

      let compileResults: Array<Record<string, unknown>> = [];

      for (let i = executionItems.length - 1; i >= 0; i -= 1) {
        const rows = extractCompileResultsFromPayload(executionItems[i].payload);
        if (rows.length > 0) {
          compileResults = rows;
          break;
        }
      }

      if (compileResults.length === 0) {
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

    const sourceEvents = events.length > 0 ? events : executionItems;
    const turns = collectModelIoTurns(sourceEvents);

    if (turns.length > 0) return `${turns.length.toLocaleString()} model history turns`;
    if (selectedLiveExecutionState?.loading) return '### MODEL I/O HISTORY\nLoading workflow model history…';
    if (selectedLiveExecutionState?.error) return `### MODEL I/O HISTORY\nUnable to load workflow model history: ${selectedLiveExecutionState.error}`;
    return '';
  }, [composedInferencePrompt, events, inferenceResponse, selectedLiveExecutionState, selectedStepId, selectedWorkflowStep?.step_type]);

  const modelHistoryTurns = useMemo(() => {
    const executionItems = selectedLiveExecutionState?.chain?.items ?? [];
    const sourceEvents = events.length > 0 ? events : executionItems;
    return collectModelIoTurns(sourceEvents);
  }, [events, selectedLiveExecutionState]);

  const modelHistoryText = useMemo(() => modelHistoryCopyText(modelHistoryTurns), [modelHistoryTurns]);

  const previewViewerContent = previewViewerMode === 'stream'
    ? (modelHistoryText || stageStreamContent)
    : previewViewerMode === 'prompt'
      ? composedInferencePrompt
      : inferenceResponse;

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

function renderPreviewPanel(title: string, content: string, emptyText: string, mode: 'prompt' | 'response' | 'stream', body?: React.ReactNode) {
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
          {body ?? <MarkdownPreviewContent content={content} emptyText={emptyText} />}
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
        <Tabs defaultValue="model_io" h="100%" style={{ display: 'flex', flexDirection: 'column' }}>
          <Tabs.List>
            <Tabs.Tab value="model_io">Model history</Tabs.Tab>
            <Tabs.Tab value="diff">Diff</Tabs.Tab>
          </Tabs.List>
          <Tabs.Panel value="model_io" pt="sm" style={{ flex: 1, minHeight: 0 }}>
            {renderPreviewPanel(
              'Model history',
              stageStreamContent,
              emptyText,
              'stream',
              <ModelHistoryContent
                turns={modelHistoryTurns}
                fallbackInput={composedInferencePrompt}
                fallbackOutput={inferenceResponse}
                emptyText={emptyText}
              />
            )}
          </Tabs.Panel>
          <Tabs.Panel value="diff" pt="sm" style={{ flex: 1, minHeight: 0 }}>
            <Suspense fallback={
              <Stack gap="sm" p="md">
                <Group gap="xs">
                  <Loader size="sm" />
                  <Text size="sm" c="dimmed">Loading diff viewer…</Text>
                </Group>
              </Stack>
            }>
              <DiffPanel
                runId={selectedRunId}
                repoRef={resolveRepoRefForRun(selectedRun)}
                state={reviewSourceControlState}
                onPersistState={persistReviewSourceControlState}
              />
            </Suspense>
          </Tabs.Panel>
        </Tabs>
      );
    }
    if (selectedWorkflowStep?.step_type === 'sap_export') {
      return <></>;
    }
    return renderPreviewPanel(
      'Model history',
      stageStreamContent,
      emptyText,
      'stream',
      <ModelHistoryContent
        turns={modelHistoryTurns}
        fallbackInput={composedInferencePrompt}
        fallbackOutput={inferenceResponse}
        emptyText={emptyText}
      />
    );
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
    const runId = selectedRun.id;
    try {
      setManualCapabilityBusy(true);
      setManualCapabilityStatus(null);
      const json = await selectWorkflowStep(runId, stepId);
      setManualCapabilityResponse(JSON.stringify(json, null, 2));
      setManualCapabilityStatus(`Selected stage ${stepId}.`);
      setSelectedStepId(stepId);
      setRuns((prev) => prev.map((run) => run.id === runId
        ? {
            ...run,
            current_step_id: stepId,
            status: 'waiting',
            updated_at: new Date().toISOString()
          }
        : run
      ));
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setManualCapabilityStatus(message);
      setManualCapabilityResponse('');
    } finally {
      setManualCapabilityBusy(false);
    }
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

  async function persistReviewSourceControlState(next: DiffPanelState) {
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

  async function handleDispositionReview(disposition: string, selectedStepId?: string | null) {
    if (!selectedRun) return;
    const runId = selectedRun.id;
    const normalizedDisposition = normalizeCheckpointDisposition(disposition);
    await runManualCapability(async () => {
      const json = await resolveWorkflowDispositionReview(runId, normalizedDisposition, selectedStepId) as Record<string, unknown>;
      await refreshSelectedRunArtifacts();

      if (normalizedDisposition === 'continue_auto' || normalizedDisposition === 'select_stage') {
        const nextStepId = typeof json.current_step_id === 'string' ? json.current_step_id : selectedStepId ?? null;
        if (nextStepId) {
          setSelectedStepId(nextStepId);
        }
      }

      return json;
    }, `Checkpoint selected: ${checkpointDispositionLabel(normalizedDisposition)}.`);
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
              <DiffPanel
                runId={selectedRunId}
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
                    </Stack>
                    <Group>
                      <Button
                        size="xs"
                        onClick={() => {
                          if (monitorHomeView === 'workflows') {
                            void openBuilder();
                          } else {
                            setSupervisorCreateRequestToken((value) => value + 1);
                          }
                        }}
                        loading={monitorHomeView === 'workflows' ? busy : false}
                      >
                        {monitorHomeView === 'workflows' ? 'New workflow' : 'New supervisor'}
                      </Button>
                      <Button
                        size="xs"
                        variant="default"
                        leftSection={<IconRefresh size={16} />}
                        onClick={() => {
                          if (monitorHomeView === 'workflows') {
                            void refreshRunsAndTemplates();
                          } else {
                            setSupervisorRefreshRequestToken((value) => value + 1);
                          }
                        }}
                      >
                        Refresh
                      </Button>
                    </Group>
                  </Group>
                  <Tabs
                    value={monitorHomeView}
                    onChange={(value) => {
                      const next = (value as MonitorHomeView) ?? 'workflows';
                      setMonitorHomeView((current) => current === next ? current : next);
                      props.navigate?.(next === 'supervisors' ? '/supervisors' : '/workflows');
                    }}
                  >
                    <Tabs.List>
                      <Tabs.Tab value="workflows">Workflows</Tabs.Tab>
                      <Tabs.Tab value="supervisors">Supervisors</Tabs.Tab>
                    </Tabs.List>
                  </Tabs>
                </Stack>
              </Card>

              {monitorHomeView === 'supervisors' ? (
                <SupervisorPanel
                  supervisorRunId={props.route?.supervisorRunId ?? null}
                  supervisorView={props.route?.supervisorView ?? null}
                  navigate={props.navigate}
                  createRequestedToken={supervisorCreateRequestToken}
                  refreshRequestedToken={supervisorRefreshRequestToken}
                  onOpenWorkflowRun={(workflowRunId) => openWorkflow(workflowRunId)}
                />
              ) : (
                <>
                  <Card withBorder>
                    <Stack>
                      <Group justify="space-between" align="center" wrap="wrap">
                        <Title order={4}>Workflow list</Title>
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
                            <Table.Tr key={run.id}>
                              <Table.Td>
                                <Anchor href={workflowRoute(run.id)} onClick={(event) => handleWorkflowLinkClick(event, run.id)}>
                                  {run.title}
                                </Anchor>
                              </Table.Td>
                              <Table.Td><Badge color={statusColor(run.status)}>{run.status}</Badge></Table.Td>
                              <Table.Td><Code>{run.current_step_id ?? '—'}</Code></Table.Td>
                              <Table.Td><Code>{run.repo_ref}</Code></Table.Td>
                              <Table.Td>{formatTimestamp(run.updated_at)}</Table.Td>
                              <Table.Td>
                                <Group gap="xs">
                                  <Anchor href={workflowRoute(run.id)} onClick={(event) => handleWorkflowLinkClick(event, run.id)} size="sm">
                                    Open
                                  </Anchor>
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
                            <Button variant="light" component="a"
              href="/workflows"
              onClick={handleWorkflowListLinkClick}>Back to workflows</Button>
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
                              <Button variant="default" leftSection={<IconPlayerPause size={16} />} onClick={() => void handlePauseRun()} loading={pauseRequestBusy} disabled={!canRequestRunPause}>{hasPendingDispositionReview ? 'Pause outcome' : 'Pause after stage'}</Button>
                              <Button variant="default" leftSection={<IconRefresh size={16} />} onClick={() => selectedRunId && void refreshRunDetails(selectedRunId)}>Refresh run</Button>
                              <Button variant="default" onClick={() => void handleForceWaitRun()} disabled={!selectedRunId || selectedRun?.status !== 'running'}>Cancel run</Button>
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
                                      <Alert color="yellow" title="User input required">
                                        <Text size="sm">Choose how the workflow should continue.</Text>
                                      </Alert>
                                      <Group gap="xs" wrap="wrap">
                                        {pendingDispositionReview.availableDispositions.map((disposition) => {
                                          const normalizedDisposition = normalizeCheckpointDisposition(disposition);
                                          if (normalizedDisposition === 'select_stage') {
                                            return (
                                              <Select
                                                key={normalizedDisposition}
                                                size="xs"
                                                placeholder="Select stage"
                                                data={(selectedRun?.definition?.steps ?? []).map((step, index) => ({
                                                  value: step.id,
                                                  label: `${index + 1}. ${step.name || step.id}`
                                                }))}
                                                disabled={busy || manualCapabilityBusy}
                                                onChange={(stepId) => {
                                                  if (stepId) void handleDispositionReview('select_stage', stepId);
                                                }}
                                                w={150}
                                              />
                                            );
                                          }
                                          return (
                                            <Button
                                              key={normalizedDisposition}
                                              variant={normalizedDisposition === 'continue_auto' ? 'filled' : 'light'}
                                              color={checkpointDispositionColor(normalizedDisposition)}
                                              loading={manualCapabilityBusy}
                                              disabled={busy || manualCapabilityBusy}
                                              onClick={() => void handleDispositionReview(normalizedDisposition)}
                                            >
                                              {checkpointDispositionLabel(normalizedDisposition)}
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
                        <Button variant="light" size="xs" onClick={() => selectedRunId && void Promise.all([hydrateWorkflowEventsFromHistory(selectedRunId), hydrateRuntimeProjection(selectedRunId)])}>Refresh events</Button>
                      </Group>
                      {liveExecutionTrails.length > 0 ? (
                        <Stack gap="xs">
                          {liveExecutionTrails.map((trail, index) => {
                            const trailExpanded = isLiveExecutionExpanded(trail);
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

                                    {(() => {
                                      const capabilityCards = trail.capabilities;
                                      if (capabilityCards.length === 0) {
                                        return <Text size="sm" c="dimmed">No capability events in runtime store.</Text>;
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
                <Badge variant="light">{previewViewerMode === 'stream' && modelHistoryTurns.length > 0 ? `${groupModelIoExchanges(modelHistoryTurns).length.toLocaleString()} exchanges` : previewViewerContent ? `${previewViewerContent.length.toLocaleString()} chars` : 'empty'}</Badge>
                <Text size="sm" c="dimmed">Wrapped and formatted for review</Text>
              </Group>
              <Button size="xs" variant="light" onClick={() => { void navigator.clipboard.writeText(previewViewerContent); }} disabled={!previewViewerContent.trim()}>
                {previewViewerMode === 'stream' ? 'Copy stream' : previewViewerMode === 'prompt' ? 'Copy prompt' : 'Copy response'}
              </Button>
            </Group>
            <Box p="lg" style={{ border: '1px solid var(--mantine-color-dark-4)', borderRadius: 12, background: 'linear-gradient(180deg, rgba(255,255,255,0.02), rgba(255,255,255,0.01))' }}>
              <ScrollArea h="82vh" offsetScrollbars>
                <Box maw={920} mx="auto">
                  {previewViewerMode === 'stream' ? (
                    <ModelHistoryContent
                      turns={modelHistoryTurns}
                      fallbackInput={composedInferencePrompt}
                      fallbackOutput={inferenceResponse}
                      emptyText="No stage stream yet."
                    />
                  ) : (
                    <MarkdownPreviewContent
                      content={previewViewerContent}
                      emptyText={previewViewerMode === 'prompt' ? 'No prompt fragments enabled yet.' : 'No inference response yet.'}
                    />
                  )}
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