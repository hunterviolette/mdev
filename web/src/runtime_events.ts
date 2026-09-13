import {
  getRuntimeSnapshot,
  openRuntimeEventStream,
  type EventChainSummaryResponse,
  type RuntimeEdge,
  type RuntimeEventEnvelope,
  type RuntimeNode,
  type RuntimeSnapshotResponse,
  type StageExecutionEvent
} from './api';

export type RuntimeEventStore = {
  nodesByKey: Record<string, RuntimeNode>;
  childrenByNodeKey: Record<string, RuntimeEdge[]>;
  parentsByNodeKey: Record<string, RuntimeEdge[]>;
  workflowEventsByRunId: Record<string, StageExecutionEvent[]>;
  latestSequenceNo: number;
  serverTime: string | null;
  connected: boolean;
};

export const emptyRuntimeEventStore: RuntimeEventStore = {
  nodesByKey: {},
  childrenByNodeKey: {},
  parentsByNodeKey: {},
  workflowEventsByRunId: {},
  latestSequenceNo: 0,
  serverTime: null,
  connected: false
};

export type ExecutionSemanticStatus =
  | 'running'
  | 'user_input'
  | 'paused'
  | 'waiting'
  | 'completed'
  | 'failed'
  | 'future'
  | 'unknown';

export type ExecutionPresentation = {
  status: ExecutionSemanticStatus;
  tone: 'blue' | 'yellow' | 'green' | 'red' | 'gray';
  label: string;
};

export function normalizeExecutionStatus(value: string | null | undefined): ExecutionSemanticStatus {
  const normalized = (value ?? '')
    .trim()
    .toLowerCase()
    .replace(/[\s-]+/g, '_');

  if (['running', 'active', 'integrating', 'ready_for_integration', 'patch_ready'].includes(normalized)) {
    return 'running';
  }
  if (['user_input', 'awaiting_user_input', 'waiting_user', 'input_required'].includes(normalized)) {
    return 'user_input';
  }
  if (normalized === 'paused' || normalized === 'pause_error') {
    return 'paused';
  }
  if (normalized === 'waiting') {
    return 'waiting';
  }
  if (['success', 'complete', 'completed', 'done', 'applied', 'integrated', 'succeeded'].includes(normalized)) {
    return 'completed';
  }
  if (['failed', 'blocked', 'error', 'cancelled', 'deleted'].includes(normalized) || normalized.startsWith('error_code:')) {
    return 'failed';
  }
  if (['future', 'up_next', 'draft'].includes(normalized)) {
    return 'future';
  }
  return 'unknown';
}

export function executionPresentation(value: string | null | undefined): ExecutionPresentation {
  const status = normalizeExecutionStatus(value);
  switch (status) {
    case 'running':
      return { status, tone: 'blue', label: 'RUNNING' };
    case 'user_input':
      return { status, tone: 'blue', label: 'USER INPUT' };
    case 'paused':
      return { status, tone: 'yellow', label: 'PAUSED' };
    case 'waiting':
      return { status, tone: 'yellow', label: 'WAITING' };
    case 'completed':
      return { status, tone: 'green', label: 'COMPLETE' };
    case 'failed':
      return { status, tone: 'red', label: 'FAILED' };
    case 'future':
      return { status, tone: 'gray', label: 'UP NEXT' };
    default:
      return { status, tone: 'gray', label: 'UNKNOWN' };
  }
}

export function executionTone(value: string | null | undefined) {
  return executionPresentation(value).tone;
}

export function executionStatusFromPayload(
  payload: unknown,
  fallback?: string | null
): ExecutionSemanticStatus {
  const record = payload && typeof payload === 'object' && !Array.isArray(payload)
    ? payload as Record<string, unknown>
    : {};
  const result = record.result && typeof record.result === 'object' && !Array.isArray(record.result)
    ? record.result as Record<string, unknown>
    : {};
  const executionState = typeof result.execution_state === 'string'
    ? result.execution_state
    : typeof record.execution_state === 'string'
      ? record.execution_state
      : '';
  if (executionState === 'awaiting_user_input') return 'user_input';

  const disposition = typeof result.disposition === 'string'
    ? result.disposition
    : typeof record.disposition === 'string'
      ? record.disposition
      : '';
  if (disposition === 'pause_error') return 'paused';

  const statusValue = typeof result.status === 'string'
    ? result.status
    : typeof record.status === 'string'
      ? record.status
      : '';
  const status = normalizeExecutionStatus(statusValue);
  return status === 'unknown' ? normalizeExecutionStatus(fallback) : status;
}

export function runtimeEventExecutionStatus(
  event: StageExecutionEvent | null | undefined
): ExecutionSemanticStatus {
  if (!event) return 'unknown';

  const payloadStatus = executionStatusFromPayload(event.payload);
  if (payloadStatus !== 'unknown') return payloadStatus;
  if (event.level === 'error' || event.kind.endsWith('_failed')) return 'failed';
  if (event.kind.endsWith('_completed')) return 'completed';
  if (event.kind.endsWith('_started') || event.kind.endsWith('_running')) return 'running';
  if (event.kind.includes('waiting_for_operator_checkpoint') || event.kind.includes('input_required')) return 'user_input';
  if (event.kind.includes('waiting')) return 'waiting';
  return 'unknown';
}

export function workflowNodeKey(runId: string) {
  return `workflow_run:${runId}`;
}

export function supervisorNodeKey(supervisorRunId: string) {
  return `supervisor_run:${supervisorRunId}`;
}

export function isTerminalRuntimeStatus(status: string | null | undefined) {
  return status === 'success'
    || status === 'error'
    || status === 'cancelled'
    || status === 'completed'
    || status === 'failed'
    || status === 'applied';
}

export function runtimeEventStatusForKind(kind: string, ok: boolean | null | undefined) {
  if (kind.endsWith('_waiting') || kind === 'stage_execution_waiting_for_operator_checkpoint') return 'waiting';
  if (ok === false) return 'error';
  if (ok === true) return 'success';
  return 'running';
}

export function reduceRuntimeSnapshot(previous: RuntimeEventStore, snapshot: RuntimeSnapshotResponse): RuntimeEventStore {
  const nodesByKey: Record<string, RuntimeNode> = {};
  const childrenByNodeKey: Record<string, RuntimeEdge[]> = {};
  const parentsByNodeKey: Record<string, RuntimeEdge[]> = {};

  for (const node of snapshot.nodes) {
    nodesByKey[node.key] = node;
  }

  for (const edge of snapshot.edges) {
    childrenByNodeKey[edge.parent_key] = [...(childrenByNodeKey[edge.parent_key] ?? []), edge];
    parentsByNodeKey[edge.child_key] = [...(parentsByNodeKey[edge.child_key] ?? []), edge];
  }

  for (const key of Object.keys(childrenByNodeKey)) {
    childrenByNodeKey[key] = childrenByNodeKey[key].slice().sort((a, b) => a.sort_order - b.sort_order);
  }

  return {
    ...previous,
    nodesByKey,
    childrenByNodeKey,
    parentsByNodeKey,
    latestSequenceNo: Math.max(previous.latestSequenceNo, snapshot.latest_sequence_no),
    serverTime: snapshot.server_time
  };
}

const RUNTIME_GLOBAL_CURSOR_KEY = 'mdev-runtime-global-cursor-v1';

function readRuntimeGlobalCursor(): number {
  try {
    const value = Number(window.sessionStorage.getItem(RUNTIME_GLOBAL_CURSOR_KEY));
    return Number.isFinite(value) && value > 0 ? value : 0;
  } catch {
    return 0;
  }
}

function writeRuntimeGlobalCursor(cursor: number) {
  try {
    window.sessionStorage.setItem(RUNTIME_GLOBAL_CURSOR_KEY, String(cursor));
  } catch {
  }
}

export function reduceRuntimeEvent(previous: RuntimeEventStore, envelope: RuntimeEventEnvelope): RuntimeEventStore {
  const event = envelope.event;
  const current = previous.workflowEventsByRunId[event.run_id] ?? [];
  const exists = current.some((item) => item.id === event.id);
  const nextEvents = exists
    ? current.map((item) => item.id === event.id ? event : item)
    : [...current, event];

  nextEvents.sort((a, b) => a.global_sequence_no - b.global_sequence_no);
  const latestSequenceNo = Math.max(previous.latestSequenceNo, event.global_sequence_no);
  writeRuntimeGlobalCursor(latestSequenceNo);

  return {
    ...previous,
    workflowEventsByRunId: {
      ...previous.workflowEventsByRunId,
      [event.run_id]: nextEvents
    },
    latestSequenceNo
  };
}

const RUNTIME_EVENT_BUS_CHANNEL = 'mdev-runtime-event-bus-v1';
const RUNTIME_EVENT_BUS_LEADER_KEY = 'mdev-runtime-event-bus-leader-v1';
const RUNTIME_EVENT_BUS_LEADER_HEARTBEAT_MS = 2000;
const RUNTIME_EVENT_BUS_LEADER_STALE_MS = 7000;

type RuntimeEventBusBroadcastMessage = {
  sourceId: string;
  type: 'connected' | 'disconnected' | 'runtime_snapshot' | 'runtime_projection' | 'runtime_event';
  payload?: unknown;
};

type RuntimeEventBusLeaderRecord = {
  tabId: string;
  expiresAt: number;
};

function createRuntimeEventBusTabId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return `${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function readRuntimeEventBusLeader(): RuntimeEventBusLeaderRecord | null {
  try {
    const raw = window.localStorage.getItem(RUNTIME_EVENT_BUS_LEADER_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as RuntimeEventBusLeaderRecord;
    if (!parsed?.tabId || typeof parsed.expiresAt !== 'number') return null;
    return parsed;
  } catch {
    return null;
  }
}

function writeRuntimeEventBusLeader(tabId: string) {
  try {
    window.localStorage.setItem(RUNTIME_EVENT_BUS_LEADER_KEY, JSON.stringify({
      tabId,
      expiresAt: Date.now() + RUNTIME_EVENT_BUS_LEADER_STALE_MS
    }));
  } catch {
  }
}

function clearRuntimeEventBusLeaderIfOwned(tabId: string) {
  try {
    const leader = readRuntimeEventBusLeader();
    if (leader?.tabId === tabId) {
      window.localStorage.removeItem(RUNTIME_EVENT_BUS_LEADER_KEY);
    }
  } catch {
  }
}

type RuntimeEventBusHandlers = {
  onOpen?: () => void;
  onClose?: () => void;
  onSnapshot?: (snapshot: RuntimeSnapshotResponse) => void;
  onProjection?: (projection: EventChainSummaryResponse) => void;
  onEvent?: (event: RuntimeEventEnvelope) => void;
  onError?: () => void;
};

function startRuntimeEventBus(handlers: RuntimeEventBusHandlers) {
  let disposed = false;
  let source: EventSource | null = null;
  let electionTimer: number | null = null;
  let heartbeatTimer: number | null = null;
  let isLeader = false;
  const tabId = createRuntimeEventBusTabId();
  const channel = typeof BroadcastChannel !== 'undefined'
    ? new BroadcastChannel(RUNTIME_EVENT_BUS_CHANNEL)
    : null;

  function applyBroadcastMessage(message: RuntimeEventBusBroadcastMessage) {
    switch (message.type) {
      case 'connected':
        handlers.onOpen?.();
        return;
      case 'disconnected':
        handlers.onClose?.();
        return;
      case 'runtime_snapshot':
        handlers.onSnapshot?.(message.payload as RuntimeSnapshotResponse);
        return;
      case 'runtime_projection':
        handlers.onProjection?.(message.payload as EventChainSummaryResponse);
        return;
      case 'runtime_event':
        handlers.onEvent?.(message.payload as RuntimeEventEnvelope);
        return;
    }
  }

  function broadcast(type: RuntimeEventBusBroadcastMessage['type'], payload?: unknown, applyLocal = true) {
    const message: RuntimeEventBusBroadcastMessage = { sourceId: tabId, type, payload };
    if (applyLocal) applyBroadcastMessage(message);
    channel?.postMessage(message);
  }

  function clearElectionTimer() {
    if (electionTimer !== null) {
      window.clearInterval(electionTimer);
      electionTimer = null;
    }
  }

  function clearHeartbeatTimer() {
    if (heartbeatTimer !== null) {
      window.clearInterval(heartbeatTimer);
      heartbeatTimer = null;
    }
  }

  function closeSource() {
    if (source) {
      source.close();
      source = null;
    }
  }

  function connect() {
    if (disposed || !isLeader) return;

    closeSource();
    const nextSource = openRuntimeEventStream({
      scope: 'all'
    });
    source = nextSource;

    nextSource.onopen = () => {
      if (disposed || !isLeader) return;
      broadcast('connected');
    };

    nextSource.addEventListener('runtime_snapshot', (raw) => {
      if (disposed || !isLeader) return;
      try {
        const snapshot = JSON.parse((raw as MessageEvent<string>).data) as RuntimeSnapshotResponse;
        writeRuntimeGlobalCursor(snapshot.latest_sequence_no);
        broadcast('runtime_snapshot', snapshot);
      } catch {
      }
    });

    nextSource.addEventListener('runtime_projection', (raw) => {
      if (disposed || !isLeader) return;
      try {
        const projection = JSON.parse((raw as MessageEvent<string>).data) as EventChainSummaryResponse;
        broadcast('runtime_projection', projection);
      } catch {
      }
    });

    nextSource.addEventListener('runtime_event', (raw) => {
      if (disposed || !isLeader) return;
      try {
        const event = JSON.parse((raw as MessageEvent<string>).data) as RuntimeEventEnvelope;
        const cursor = event.event.global_sequence_no;
        if (Number.isFinite(cursor) && cursor > 0) {
          writeRuntimeGlobalCursor(cursor);
        }
        broadcast('runtime_event', event);
      } catch {
      }
    });

    nextSource.onerror = () => {
      if (disposed || !isLeader) return;
      handlers.onError?.();
      broadcast('disconnected');
    };
  }

  function stopLeading() {
    if (!isLeader) return;
    isLeader = false;
    clearHeartbeatTimer();
    closeSource();
    clearRuntimeEventBusLeaderIfOwned(tabId);
    broadcast('disconnected');
  }

  function startLeading() {
    if (disposed || isLeader) return;
    isLeader = true;
    writeRuntimeEventBusLeader(tabId);
    heartbeatTimer = window.setInterval(() => {
      if (disposed || !isLeader) return;
      writeRuntimeEventBusLeader(tabId);
    }, RUNTIME_EVENT_BUS_LEADER_HEARTBEAT_MS);
    connect();
  }

  function electLeader() {
    if (disposed || isLeader) return;
    const leader = readRuntimeEventBusLeader();
    if (leader && leader.expiresAt > Date.now() && leader.tabId !== tabId) return;

    writeRuntimeEventBusLeader(tabId);
    const claimed = readRuntimeEventBusLeader();
    if (claimed?.tabId === tabId) {
      startLeading();
    }
  }

  channel?.addEventListener('message', (event) => {
    if (disposed) return;
    const message = event.data as RuntimeEventBusBroadcastMessage;
    if (!message || message.sourceId === tabId) return;
    applyBroadcastMessage(message);
  });

  window.addEventListener('storage', (event) => {
    if (disposed || event.key !== RUNTIME_EVENT_BUS_LEADER_KEY) return;
    const leader = readRuntimeEventBusLeader();
    if (isLeader && leader?.tabId && leader.tabId !== tabId && leader.expiresAt > Date.now()) {
      stopLeading();
    }
  });

  electLeader();
  electionTimer = window.setInterval(electLeader, RUNTIME_EVENT_BUS_LEADER_HEARTBEAT_MS);

  return () => {
    if (disposed) return;
    disposed = true;
    clearElectionTimer();
    stopLeading();
    channel?.close();
    handlers.onClose?.();
  };
}

const runtimeEventBusSubscribers = new Set<RuntimeEventBusHandlers>();
let stopSharedRuntimeEventBus: (() => void) | null = null;

function dispatchRuntimeEventBus<K extends keyof RuntimeEventBusHandlers>(
  key: K,
  value?: Parameters<NonNullable<RuntimeEventBusHandlers[K]>>[0]
) {
  for (const subscriber of runtimeEventBusSubscribers) {
    const handler = subscriber[key] as ((payload?: unknown) => void) | undefined;
    handler?.(value);
  }
}

function ensureSharedRuntimeEventBus() {
  if (stopSharedRuntimeEventBus) return;

  stopSharedRuntimeEventBus = startRuntimeEventBus({
    onOpen: () => dispatchRuntimeEventBus('onOpen'),
    onClose: () => dispatchRuntimeEventBus('onClose'),
    onSnapshot: (snapshot) => dispatchRuntimeEventBus('onSnapshot', snapshot),
    onProjection: (projection) => dispatchRuntimeEventBus('onProjection', projection),
    onEvent: (event) => dispatchRuntimeEventBus('onEvent', event),
    onError: () => dispatchRuntimeEventBus('onError')
  });
}

export function subscribeRuntimeEventBus(handlers: RuntimeEventBusHandlers) {
  runtimeEventBusSubscribers.add(handlers);
  ensureSharedRuntimeEventBus();

  return () => {
    runtimeEventBusSubscribers.delete(handlers);

    if (runtimeEventBusSubscribers.size === 0 && stopSharedRuntimeEventBus) {
      const stop = stopSharedRuntimeEventBus;
      stopSharedRuntimeEventBus = null;
      stop();
    }
  };
}
