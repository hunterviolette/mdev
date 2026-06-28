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

export function reduceRuntimeEvent(previous: RuntimeEventStore, envelope: RuntimeEventEnvelope): RuntimeEventStore {
  const event = envelope.event;
  const current = previous.workflowEventsByRunId[event.run_id] ?? [];
  const exists = current.some((item) => item.id === event.id);
  const nextEvents = exists
    ? current.map((item) => item.id === event.id ? event : item)
    : [...current, event];

  nextEvents.sort((a, b) => a.sequence_no - b.sequence_no);

  return {
    ...previous,
    workflowEventsByRunId: {
      ...previous.workflowEventsByRunId,
      [event.run_id]: nextEvents
    },
    latestSequenceNo: Math.max(previous.latestSequenceNo, event.sequence_no)
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

export function subscribeRuntimeEventBus(handlers: RuntimeEventBusHandlers) {
  let disposed = false;
  let source: EventSource | null = null;
  let reconnectTimer: number | null = null;
  let electionTimer: number | null = null;
  let heartbeatTimer: number | null = null;
  let reconnectAttempt = 0;
  let lastEventSequenceNo = 0;
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

  function clearReconnectTimer() {
    if (reconnectTimer !== null) {
      window.clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
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

  async function refreshSnapshotFromBackend() {
    try {
      const snapshot = await getRuntimeSnapshot({ scope: 'all' });
      if (disposed || !isLeader) return;
      broadcast('runtime_snapshot', snapshot);
    } catch {
    }
  }

  function scheduleReconnect() {
    if (disposed || !isLeader || reconnectTimer !== null) return;

    closeSource();
    broadcast('disconnected');

    const delayMs = Math.min(1000 * 2 ** reconnectAttempt, 10000);
    reconnectAttempt += 1;

    reconnectTimer = window.setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, delayMs);
  }

  function connect() {
    if (disposed || !isLeader) return;

    closeSource();
    const nextSource = openRuntimeEventStream({
      scope: 'all',
      after_sequence: lastEventSequenceNo
    });
    source = nextSource;

    nextSource.onopen = () => {
      if (disposed || !isLeader) return;
      reconnectAttempt = 0;
      broadcast('connected');
      void refreshSnapshotFromBackend();
    };

    nextSource.addEventListener('runtime_snapshot', (raw) => {
      if (disposed || !isLeader) return;
      try {
        const snapshot = JSON.parse((raw as MessageEvent<string>).data) as RuntimeSnapshotResponse;
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
        lastEventSequenceNo = Math.max(lastEventSequenceNo, event.event.sequence_no ?? 0);
        broadcast('runtime_event', event);
      } catch {
      }
    });

    nextSource.onerror = () => {
      if (disposed || !isLeader) return;
      handlers.onError?.();
      scheduleReconnect();
    };
  }

  function stopLeading() {
    if (!isLeader) return;
    isLeader = false;
    clearReconnectTimer();
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
