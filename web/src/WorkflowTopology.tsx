import { useEffect, useRef, useState } from 'react';
import './WorkflowTopology.css';

export type WorkflowTopologyStage = {
  id: string;
  label: string;
  state: 'complete' | 'active' | 'waiting' | 'failed' | 'future' | 'up_next' | 'draft';
  message?: string;
};

export function normalizeWorkflowTopologyState(value: string | null | undefined): WorkflowTopologyStage['state'] {
  const state = (value ?? '').trim().toLowerCase();
  if (['complete', 'completed', 'success', 'succeeded', 'done'].includes(state)) return 'complete';
  if (['failed', 'failure', 'error', 'cancelled', 'canceled'].includes(state)) return 'failed';
  if (['waiting', 'waiting_user', 'paused', 'blocked'].includes(state)) return 'waiting';
  if (['running', 'active', 'executing', 'in_progress'].includes(state)) return 'active';
  if (state === 'up_next' || state === 'next') return 'up_next';
  if (state === 'draft') return 'draft';
  return 'future';
}

type Props = {
  stages: WorkflowTopologyStage[];
  selectedId?: string | null;
  onSelect?: (id: string) => void;
  compact?: boolean;
  rail?: boolean;
};

const CELL_WIDTH = 116;
const ROW_HEIGHT = 112;
const NODE_SIZE = 40;
const PADDING_X = 30;
const PADDING_Y = 70;

const palette = {
  complete: { fill: '#153d30', stroke: '#35c487', text: '#e1fff1' },
  active: { fill: '#173e63', stroke: '#62b7ff', text: '#e9f5ff' },
  waiting: { fill: '#493915', stroke: '#f3b94d', text: '#fff1cb' },
  failed: { fill: '#4a2027', stroke: '#ff6775', text: '#ffe6e8' },
  future: { fill: '#272e38', stroke: '#536070', text: '#a0acba' },
  up_next: { fill: '#272e38', stroke: '#536070', text: '#a0acba' },
  draft: { fill: '#272e38', stroke: '#536070', text: '#a0acba' }
};

function isComplete(state: WorkflowTopologyStage['state']) {
  return state === 'complete';
}

function isActive(state: WorkflowTopologyStage['state']) {
  return state === 'active' || state === 'waiting';
}

export function WorkflowTopology({ stages, selectedId, onSelect, compact = false, rail = false }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);

  useEffect(() => {
    const element = containerRef.current;
    if (!element) return;
    const update = () => setWidth(element.clientWidth);
    update();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const count = stages.length;
  const availableColumns = Math.max(1, Math.floor((Math.max(width, 160) - PADDING_X * 2) / CELL_WIDTH));
  const columns = Math.min(count || 1, availableColumns);
  const rows = Math.ceil(count / columns);
  const graphWidth = columns * CELL_WIDTH + PADDING_X * 2;
  const graphHeight = PADDING_Y + Math.max(0, rows - 1) * ROW_HEIGHT + 48;
  const completedCount = stages.filter((stage) => isComplete(stage.state)).length;
  const currentIndex = stages.findIndex((stage) => isActive(stage.state) || stage.state === 'failed');
  const focusIndex = currentIndex >= 0 ? currentIndex : Math.max(0, completedCount - 1);

  const position = (index: number) => {
    const row = Math.floor(index / columns);
    const offset = index % columns;
    const column = row % 2 === 0 ? offset : columns - offset - 1;
    return {
      x: PADDING_X + CELL_WIDTH * (column + 0.5),
      y: PADDING_Y + row * ROW_HEIGHT,
      row
    };
  };

  const connector = (index: number) => {
    const from = position(index);
    const to = position(index + 1);
    const direction = to.x > from.x ? 1 : -1;
    const halfNode = NODE_SIZE / 2;

    if (from.row === to.row) {
      return `M ${from.x + direction * halfNode} ${from.y} H ${to.x - direction * halfNode}`;
    }

    const side = from.row % 2 === 0 ? 1 : -1;
    const turnX = from.x + side * (CELL_WIDTH / 2);
    const radius = 14;
    const startX = from.x + side * halfNode;
    const endX = to.x + side * halfNode;

    return [
      `M ${startX} ${from.y}`,
      `H ${turnX - side * radius}`,
      `Q ${turnX} ${from.y} ${turnX} ${from.y + radius}`,
      `V ${to.y - radius}`,
      `Q ${turnX} ${to.y} ${turnX - side * radius} ${to.y}`,
      `H ${endX}`
    ].join(' ');
  };

  if (!count) return null;

  if (rail) {
    return (
      <div className="mdev-topology-rail" role="group" aria-label={`Workflow progression: ${completedCount} of ${count} stages complete`}>
        <div className="mdev-topology-rail__heading">
          <span>Stage progression</span>
          <span>{completedCount} / {count}</span>
        </div>
        <div className="mdev-topology-rail__track">
          {stages.map((stage, index) => {
            const selected = selectedId === stage.id;
            const colors = palette[stage.state] ?? palette.future;
            return (
              <button
                key={stage.id}
                type="button"
                className="mdev-topology-rail__stage"
                data-state={stage.state}
                data-selected={selected}
                disabled={!onSelect}
                onClick={() => onSelect?.(stage.id)}
                aria-label={`Stage ${index + 1}: ${stage.label}, ${stage.state}`}
                aria-current={selected ? 'step' : undefined}
                title={`${stage.label} — ${stage.state}${stage.message ? ` — ${stage.message}` : ''}`}
              >
                <span className="mdev-topology-rail__indicator" style={{ backgroundColor: colors.fill, borderColor: colors.stroke, color: colors.text }}>
                  {stage.state === 'complete' ? '✓' : index + 1}
                </span>
                <span className="mdev-topology-rail__label">{stage.label}</span>
              </button>
            );
          })}
        </div>
      </div>
    );
  }

  return (
    <div className={`mdev-topology ${compact ? 'mdev-topology--compact' : ''}`} ref={containerRef}>
      <div className="mdev-topology__header">
        <div className="mdev-topology__heading">
          <span className="mdev-topology__heading-mark" />
          <span>Workflow topology</span>
        </div>
        <span className="mdev-topology__counter">{completedCount} / {count} complete</span>
      </div>
      <div className="mdev-topology__graph">
        <svg viewBox={`0 0 ${graphWidth} ${graphHeight}`} width="100%" style={{ maxWidth: graphWidth }} role="group" aria-label={`Workflow: ${completedCount} of ${count} stages complete`}>
          {stages.slice(0, -1).map((stage, index) => (
            <path key={`connector-${stage.id}`} d={connector(index)} fill="none" stroke={isComplete(stage.state) ? '#31b77d' : '#414c5b'} strokeWidth="3" strokeLinecap="round" className={isComplete(stage.state) ? 'mdev-topology__connector--complete' : 'mdev-topology__connector'} />
          ))}
          {stages.map((stage, index) => {
            const point = position(index);
            const colors = palette[stage.state] ?? palette.future;
            const selected = selectedId === stage.id;
            const running = isActive(stage.state);
            const label = stage.label.length > 16 ? `${stage.label.slice(0, 15)}…` : stage.label;
            return (
              <g key={stage.id} className={`mdev-topology__node ${running ? 'mdev-topology__node--active' : ''} ${onSelect ? 'mdev-topology__node--interactive' : ''}`} role={onSelect ? 'button' : undefined} tabIndex={onSelect ? 0 : undefined} aria-label={`${stage.label}: ${stage.state}`} aria-current={selected ? 'step' : undefined} onClick={() => onSelect?.(stage.id)} onKeyDown={(event) => { if (onSelect && (event.key === 'Enter' || event.key === ' ')) { event.preventDefault(); onSelect(stage.id); } }}>
                <title>{`${stage.label} — ${stage.state}${stage.message ? ` — ${stage.message}` : ''}`}</title>
                {running && <rect x={point.x - 27} y={point.y - 27} width="54" height="54" rx="14" fill={colors.stroke} opacity="0.14" className="mdev-topology__halo" />}
                {selected && <rect x={point.x - 25} y={point.y - 25} width="50" height="50" rx="13" fill="none" stroke="#dceeff" strokeWidth="1" strokeDasharray="3 3" />}
                <rect x={point.x - NODE_SIZE / 2} y={point.y - NODE_SIZE / 2} width={NODE_SIZE} height={NODE_SIZE} rx="10" fill={colors.fill} stroke={colors.stroke} strokeWidth={running || selected ? 2.5 : 1.5} />
                <text x={point.x} y={point.y + 5} textAnchor="middle" fontSize="13" fontWeight="700" fill={colors.text}>{isComplete(stage.state) ? '✓' : String(index + 1).padStart(2, '0')}</text>
                <text x={point.x} y={point.y - 34} textAnchor="middle" fontSize="11" fontWeight={running || selected ? 700 : 600} fill={running ? '#86caff' : stage.state === 'failed' ? '#ff8895' : '#c5cfdb'}>{label}</text>
              </g>
            );
          })}
        </svg>
      </div>
      {count > 1 && <div className="mdev-topology__footer"><span>STAGE {focusIndex + 1} OF {count}</span><span>{stages[focusIndex]?.label}</span></div>}
    </div>
  );
}
