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
  let reconnectAttempt = 0;
  let lastEventSequenceNo = 0;

  function clearReconnectTimer() {
    if (reconnectTimer !== null) {
      window.clearTimeout(reconnectTimer);
      reconnectTimer = null;
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
      if (disposed) return;
      handlers.onSnapshot?.(snapshot);
    } catch {
    }
  }

  function scheduleReconnect() {
    if (disposed || reconnectTimer !== null) return;

    closeSource();
    handlers.onClose?.();

    const delayMs = Math.min(1000 * 2 ** reconnectAttempt, 10000);
    reconnectAttempt += 1;

    reconnectTimer = window.setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, delayMs);
  }

  function connect() {
    if (disposed) return;

    closeSource();
    const nextSource = openRuntimeEventStream({
      scope: 'all',
      after_sequence: lastEventSequenceNo
    });
    source = nextSource;

    nextSource.onopen = () => {
      if (disposed) return;
      reconnectAttempt = 0;
      handlers.onOpen?.();
      void refreshSnapshotFromBackend();
    };

    nextSource.addEventListener('runtime_snapshot', (raw) => {
      if (disposed) return;
      try {
        const snapshot = JSON.parse((raw as MessageEvent<string>).data) as RuntimeSnapshotResponse;
        handlers.onSnapshot?.(snapshot);
      } catch {
      }
    });

    nextSource.addEventListener('runtime_projection', (raw) => {
      if (disposed) return;
      try {
        const projection = JSON.parse((raw as MessageEvent<string>).data) as EventChainSummaryResponse;
        handlers.onProjection?.(projection);
      } catch {
      }
    });

    nextSource.addEventListener('runtime_event', (raw) => {
      if (disposed) return;
      try {
        const event = JSON.parse((raw as MessageEvent<string>).data) as RuntimeEventEnvelope;
        lastEventSequenceNo = Math.max(lastEventSequenceNo, event.event.sequence_no ?? 0);
        handlers.onEvent?.(event);
      } catch {
      }
    });

    nextSource.onerror = () => {
      if (disposed) return;
      handlers.onError?.();
      scheduleReconnect();
    };
  }

  connect();

  return () => {
    if (disposed) return;
    disposed = true;
    clearReconnectTimer();
    closeSource();
    handlers.onClose?.();
  };
}
