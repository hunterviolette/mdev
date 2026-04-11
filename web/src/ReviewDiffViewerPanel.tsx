import { useEffect, useMemo, useRef, useState } from 'react';
import {
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  Divider,
  Group,
  Loader,
  Modal,
  NumberInput,
  ScrollArea,
  SegmentedControl,
  Stack,
  Text,
  Tooltip,
} from '@mantine/core';
import { parsePatchFiles, type FileDiffMetadata } from '@pierre/diffs';
import { FileDiff } from '@pierre/diffs/react';
import {
  getReviewDiff,
  getReviewDiffManifest,
  getReviewFilePatch,
  getReviewStatus,
  stageReviewDiff,
  unstageReviewDiff,
  type ReviewDiffManifestFileEntry,
  type ReviewDiffManifestResponse,
  type ReviewDiffResponse,
  type ReviewDiffScope,
  type ReviewFilePatchResponse,
  type ReviewStatusFileEntry,
} from './api';

export type ReviewSourceControlState = {
  selected_scope: ReviewDiffScope;
  selected_path: string | null;
  diff_style: 'unified' | 'split';
  only_changes: boolean;
  context_lines: number;
  whole_file: boolean;
};

type ReviewDiffViewerPanelProps = {
  repoRef: string;
  state: ReviewSourceControlState;
  onPersistState: (next: ReviewSourceControlState) => Promise<void>;
};

const MIN_SIDEBAR_WIDTH = 280;
const MAX_SIDEBAR_WIDTH = 620;
const DEFAULT_SIDEBAR_WIDTH = 360;
const MAX_CONCURRENT_FILE_PATCHES = 1;
const VIRTUAL_OVERSCAN_ROWS = 6;

function clampContextLines(value: number | string | null | undefined): number {
  const numeric = typeof value === 'number' ? value : Number(value ?? 10);
  if (!Number.isFinite(numeric)) {
    return 10;
  }
  return Math.max(0, Math.min(1000, Math.round(numeric)));
}

function clampSidebarWidth(value: number | null | undefined): number {
  const numeric = typeof value === 'number' ? value : Number(value ?? DEFAULT_SIDEBAR_WIDTH);
  if (!Number.isFinite(numeric)) {
    return DEFAULT_SIDEBAR_WIDTH;
  }
  return Math.max(MIN_SIDEBAR_WIDTH, Math.min(MAX_SIDEBAR_WIDTH, Math.round(numeric)));
}

function sumCounts(files: ReviewStatusFileEntry[]) {
  return files.reduce(
    (acc, file) => {
      acc.additions += file.additions;
      acc.deletions += file.deletions;
      return acc;
    },
    { additions: 0, deletions: 0 }
  );
}

function statusCode(file: ReviewStatusFileEntry): string {
  if (file.untracked) {
    return 'U';
  }
  const code = `${file.index_status}${file.worktree_status}`.replace(/\./g, '').trim();
  return code || 'M';
}

function ScopeHeader(props: {
  title: string;
  active: boolean;
  fileCount: number;
  additions: number;
  deletions: number;
  compactCounts: boolean;
  buttonLabel: string;
  actionBusy: boolean;
  onSelect: () => void;
  onAction: () => Promise<void>;
}) {
  const { title, active, fileCount, additions, deletions, compactCounts, buttonLabel, actionBusy, onSelect, onAction } = props;

  return (
    <Group justify="space-between" align="center" wrap="nowrap">
      <Button
        variant={active ? 'filled' : 'default'}
        onClick={onSelect}
        style={{ flex: 1, justifyContent: 'space-between' }}
      >
        <Group gap="xs" wrap="nowrap">
          <Text fw={700} size="sm">{title}</Text>
          <Badge variant="light">{fileCount}</Badge>
          {!compactCounts ? <Badge color="green" variant="light">+{additions}</Badge> : null}
          {!compactCounts ? <Badge color="red" variant="light">-{deletions}</Badge> : null}
        </Group>
      </Button>
      <Tooltip label={`${fileCount} files · +${additions} / -${deletions}`} withArrow>
        <Button size="xs" variant="light" loading={actionBusy} onClick={() => void onAction()}>
          {buttonLabel}
        </Button>
      </Tooltip>
    </Group>
  );
}

function FileRow(props: {
  scope: ReviewDiffScope;
  file: ReviewStatusFileEntry;
  active: boolean;
  actionBusy: boolean;
  onSelect: () => void;
  onStage: () => Promise<void>;
  onUnstage: () => Promise<void>;
}) {
  const { scope, file, active, actionBusy, onSelect, onStage, onUnstage } = props;
  return (
    <Box
      onClick={onSelect}
      style={{
        cursor: 'pointer',
        padding: '8px 10px',
        borderRadius: 8,
        background: active ? 'rgba(34, 139, 230, 0.16)' : 'rgba(255,255,255,0.02)',
        border: active ? '1px solid rgba(34, 139, 230, 0.4)' : '1px solid rgba(255,255,255,0.05)'
      }}
    >
      <Group justify="space-between" align="center" wrap="nowrap">
        <Group gap="xs" wrap="nowrap" style={{ minWidth: 0, flex: 1 }}>
          <Badge variant="outline">{statusCode(file)}</Badge>
          <Text size="sm" fw={active ? 700 : 500} style={{ wordBreak: 'break-word' }}>
            {file.path}
          </Text>
        </Group>
        <Group gap={6} wrap="nowrap">
          <Badge color="green" variant="light">+{file.additions}</Badge>
          <Badge color="red" variant="light">-{file.deletions}</Badge>
          {scope === 'unstaged' ? (
            <Button
              size="compact-xs"
              variant="subtle"
              loading={actionBusy}
              onClick={(event) => {
                event.stopPropagation();
                void onStage();
              }}
            >
              Stage
            </Button>
          ) : (
            <Button
              size="compact-xs"
              variant="subtle"
              loading={actionBusy}
              onClick={(event) => {
                event.stopPropagation();
                void onUnstage();
              }}
            >
              Unstage
            </Button>
          )}
        </Group>
      </Group>
    </Box>
  );
}

export function ReviewDiffViewerPanel(props: ReviewDiffViewerPanelProps) {
  const { repoRef, state, onPersistState } = props;
  const [statusBusy, setStatusBusy] = useState(false);
  const [diffBusy, setDiffBusy] = useState(false);
  const [actionBusy, setActionBusy] = useState(false);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [diffError, setDiffError] = useState<string | null>(null);
  const [stagedFiles, setStagedFiles] = useState<ReviewStatusFileEntry[]>([]);
  const [unstagedFiles, setUnstagedFiles] = useState<ReviewStatusFileEntry[]>([]);
  const [branchSummary, setBranchSummary] = useState<string>('');
  const [diff, setDiff] = useState<ReviewDiffResponse | null>(null);
  const [viewerOpen, setViewerOpen] = useState(false);
  const [resizing, setResizing] = useState(false);
  const [sidebarHidden, setSidebarHidden] = useState(false);
  const [sidebarWidth, setSidebarWidth] = useState(DEFAULT_SIDEBAR_WIDTH);
  const [sidebarWidthDraft, setSidebarWidthDraft] = useState<number | null>(null);
  const [collapsedDiffFiles, setCollapsedDiffFiles] = useState<Record<string, boolean>>({});
  const resizeFrame = useRef<number | null>(null);
  const [currentStickyFileKey, setCurrentStickyFileKey] = useState<string>('');
  const [showStickyFileHeader, setShowStickyFileHeader] = useState(false);
  const diffViewportRef = useRef<HTMLDivElement | null>(null);
  const [hydratedDiffFiles, setHydratedDiffFiles] = useState<Record<string, boolean>>({});
  const [diffScrollTop, setDiffScrollTop] = useState(0);
  const [diffViewportHeight, setDiffViewportHeight] = useState(0);
  const [diffManifest, setDiffManifest] = useState<ReviewDiffManifestResponse | null>(null);
  const [filePatchByPath, setFilePatchByPath] = useState<Record<string, ReviewFilePatchResponse>>({});
  const [filePatchBusyByPath, setFilePatchBusyByPath] = useState<Record<string, boolean>>({});
  const [parsedFileDiffByPath, setParsedFileDiffByPath] = useState<Record<string, FileDiffMetadata | null>>({});
  const [measuredRowHeights, setMeasuredRowHeights] = useState<Record<string, number>>({});
  const filePatchQueueRef = useRef<string[]>([]);
  const filePatchInFlightRef = useRef<Set<string>>(new Set());

  const selectedTitle = state.selected_scope === 'staged' ? 'Staged diff' : 'Unstaged diff';
  const stagedTotals = useMemo(() => sumCounts(stagedFiles), [stagedFiles]);
  const unstagedTotals = useMemo(() => sumCounts(unstagedFiles), [unstagedFiles]);
  const allTotals = useMemo(() => sumCounts([...stagedFiles, ...unstagedFiles]), [stagedFiles, unstagedFiles]);
  const selectedTotals = state.selected_scope === 'staged' ? stagedTotals : unstagedTotals;
  const selectedScopeFiles = useMemo(
    () => diffManifest?.files ?? (state.selected_scope === 'staged' ? stagedFiles : unstagedFiles),
    [diffManifest, state.selected_scope, stagedFiles, unstagedFiles]
  );
  const renderedDiffFileKeys = useMemo(
    () => selectedScopeFiles.map((file) => file.path),
    [selectedScopeFiles]
  );
  const selectedScopeCountsByPath = useMemo(
    () => Object.fromEntries(selectedScopeFiles.map((file) => [file.path, file])),
    [selectedScopeFiles]
  );

  const virtualRowHeights = useMemo(
    () => selectedScopeFiles.map((file) => {
      const measuredHeight = measuredRowHeights[file.path];
      if (measuredHeight && measuredHeight > 0) {
        return measuredHeight;
      }

      const isCollapsed = Boolean(collapsedDiffFiles[file.path]);
      const hasParsedDiff = Boolean(parsedFileDiffByPath[file.path]);
      if (isCollapsed) {
        return 56;
      }
      return hasParsedDiff ? 160 : 72;
    }),
    [selectedScopeFiles, collapsedDiffFiles, parsedFileDiffByPath, measuredRowHeights]
  );

  const virtualRowOffsets = useMemo(() => {
    const offsets: number[] = [];
    let running = 0;
    for (const height of virtualRowHeights) {
      offsets.push(running);
      running += height + 16;
    }
    return offsets;
  }, [virtualRowHeights]);

  const virtualTotalHeight = useMemo(() => {
    if (virtualRowHeights.length === 0) {
      return 0;
    }
    return virtualRowOffsets[virtualRowOffsets.length - 1] + virtualRowHeights[virtualRowHeights.length - 1];
  }, [virtualRowHeights, virtualRowOffsets]);

  const visibleRowRange = useMemo(() => {
    if (selectedScopeFiles.length === 0) {
      return { start: 0, end: 0 };
    }

    const viewportTop = diffScrollTop;
    const viewportBottom = diffScrollTop + Math.max(diffViewportHeight, 1);

    let start = 0;
    while (
      start < selectedScopeFiles.length &&
      virtualRowOffsets[start] + virtualRowHeights[start] < viewportTop
    ) {
      start += 1;
    }

    let end = start;
    while (
      end < selectedScopeFiles.length &&
      virtualRowOffsets[end] < viewportBottom
    ) {
      end += 1;
    }

    start = Math.max(0, start - VIRTUAL_OVERSCAN_ROWS);
    end = Math.min(selectedScopeFiles.length, end + VIRTUAL_OVERSCAN_ROWS);

    return { start, end };
  }, [selectedScopeFiles.length, diffScrollTop, diffViewportHeight, virtualRowHeights, virtualRowOffsets]);

  function toggleDiffFileCollapsed(fileKey: string) {
    setCollapsedDiffFiles((current) => ({
      ...current,
      [fileKey]: !current[fileKey],
    }));
  }

  function setAllDiffFilesCollapsed(nextCollapsed: boolean) {
    setCollapsedDiffFiles(
      Object.fromEntries(renderedDiffFileKeys.map((key) => [key, nextCollapsed]))
    );
  }

  async function refreshStatus() {
    if (!repoRef.trim()) return;
    try {
      setStatusBusy(true);
      setStatusError(null);
      const json = await getReviewStatus(repoRef);
      setStagedFiles(json.staged);
      setUnstagedFiles(json.unstaged);
      const pieces = [json.branch ?? 'HEAD'];
      if (json.upstream) {
        pieces.push(`↥${json.ahead} ↧${json.behind} · ${json.upstream}`);
      }
      setBranchSummary(pieces.join(' · '));
    } catch (err) {
      setStatusError(err instanceof Error ? err.message : String(err));
    } finally {
      setStatusBusy(false);
    }
  }

  async function refreshDiff(nextState: ReviewSourceControlState) {
    if (!repoRef.trim() || !viewerOpen) return;
    try {
      setDiffBusy(true);
      setDiffError(null);
      setFilePatchByPath({});
      setFilePatchBusyByPath({});
      setParsedFileDiffByPath({});
      filePatchQueueRef.current = [];
      filePatchInFlightRef.current.clear();

      if (nextState.selected_path) {
        const json = await getReviewDiff({
          repo_ref: repoRef,
          scope: nextState.selected_scope,
          path: nextState.selected_path,
          context_lines: nextState.whole_file ? 1000 : clampContextLines(nextState.context_lines),
          whole_file: nextState.whole_file,
        });
        setDiff(json);
        setDiffManifest(null);
        return;
      }

      const manifest = await getReviewDiffManifest({
        repo_ref: repoRef,
        scope: nextState.selected_scope,
      });
      setDiff(null);
      setDiffManifest(manifest);
    } catch (err) {
      setDiffError(err instanceof Error ? err.message : String(err));
    } finally {
      setDiffBusy(false);
    }
  }

  async function patchState(patch: Partial<ReviewSourceControlState>) {
    const next = {
      ...state,
      ...patch,
      context_lines: patch.context_lines === undefined ? state.context_lines : clampContextLines(patch.context_lines),
    };
    await onPersistState(next);
    if (viewerOpen) {
      await refreshDiff(next);
    }
  }

  async function runStageAction(kind: 'stage' | 'unstage', scope: ReviewDiffScope, path: string | null) {
    if (!repoRef.trim()) return;
    try {
      setActionBusy(true);
      setStatusError(null);
      if (kind === 'stage') {
        await stageReviewDiff({ repo_ref: repoRef, scope, path });
      } else {
        await unstageReviewDiff({ repo_ref: repoRef, scope, path });
      }
      const nextScope: ReviewDiffScope = kind === 'stage' && path ? 'staged' : kind === 'unstage' && path ? 'unstaged' : state.selected_scope;
      const nextState = {
        ...state,
        selected_scope: nextScope,
        selected_path: path,
      };
      await onPersistState(nextState);
      await refreshStatus();
      await refreshDiff(nextState);
    } catch (err) {
      setStatusError(err instanceof Error ? err.message : String(err));
    } finally {
      setActionBusy(false);
    }
  }

  function pumpFilePatchQueue() {
    while (
      filePatchInFlightRef.current.size < MAX_CONCURRENT_FILE_PATCHES &&
      filePatchQueueRef.current.length > 0
    ) {
      const path = filePatchQueueRef.current.shift();
      if (!path) {
        break;
      }
      if (filePatchByPath[path] || filePatchInFlightRef.current.has(path)) {
        continue;
      }

      filePatchInFlightRef.current.add(path);
      setFilePatchBusyByPath((current) => ({ ...current, [path]: true }));

      void getReviewFilePatch({
        repo_ref: repoRef,
        scope: state.selected_scope,
        path,
        context_lines: state.whole_file ? 1000 : clampContextLines(state.context_lines),
        whole_file: state.whole_file,
      })
        .then((json) => {
          setFilePatchByPath((current) => ({ ...current, [path]: json }));
          try {
            const parsed = json.patch.trim()
              ? parsePatchFiles(json.patch).flatMap((patch) => patch.files ?? [])[0] ?? null
              : null;
            setParsedFileDiffByPath((current) => ({ ...current, [path]: parsed }));
          } catch {
            setParsedFileDiffByPath((current) => ({ ...current, [path]: null }));
          }
        })
        .catch((err) => {
          setDiffError(err instanceof Error ? err.message : String(err));
        })
        .finally(() => {
          filePatchInFlightRef.current.delete(path);
          setFilePatchBusyByPath((current) => {
            const next = { ...current };
            delete next[path];
            return next;
          });
          pumpFilePatchQueue();
        });
    }
  }

  function ensureFilePatch(path: string) {
    if (!repoRef.trim() || state.selected_path || filePatchByPath[path] || filePatchInFlightRef.current.has(path)) {
      return;
    }
    if (!filePatchQueueRef.current.includes(path)) {
      filePatchQueueRef.current.push(path);
    }
    pumpFilePatchQueue();
  }

  function ensureFilePatchesInOrder(paths: string[]) {
    if (!repoRef.trim() || state.selected_path || paths.length === 0) {
      return;
    }

    const seen = new Set<string>();
    const prioritized = paths.filter((path) => {
      if (seen.has(path) || filePatchByPath[path] || filePatchInFlightRef.current.has(path)) {
        return false;
      }
      seen.add(path);
      return true;
    });

    if (prioritized.length === 0) {
      return;
    }

    const remainder = filePatchQueueRef.current.filter((path) => !seen.has(path));
    filePatchQueueRef.current = [...prioritized, ...remainder];
    pumpFilePatchQueue();
  }

  useEffect(() => {
    void refreshStatus();
  }, [repoRef]);

  useEffect(() => {
    if (viewerOpen) {
      void refreshDiff(state);
    }
  }, [repoRef, viewerOpen, state.selected_scope, state.selected_path, state.context_lines, state.whole_file]);


  useEffect(() => {
    setCollapsedDiffFiles({});
  }, [diff?.patch, diffManifest?.scope, state.selected_scope, state.selected_path]);

  useEffect(() => {
    setMeasuredRowHeights({});
  }, [diff?.patch, diffManifest?.scope, state.selected_scope, state.selected_path]);

  useEffect(() => {
    setHydratedDiffFiles(
      Object.fromEntries(renderedDiffFileKeys.slice(0, 4).map((key) => [key, true]))
    );
  }, [renderedDiffFileKeys, diff?.patch, diffManifest?.scope]);

  useEffect(() => {
    const nextFileKey = renderedDiffFileKeys[0] ?? '';
    setCurrentStickyFileKey(nextFileKey);
    setShowStickyFileHeader(false);
  }, [renderedDiffFileKeys, diff?.patch, diffManifest?.scope]);

  useEffect(() => {
    if (!viewerOpen || state.selected_path || renderedDiffFileKeys.length === 0 || selectedScopeFiles.length === 0) {
      setShowStickyFileHeader(false);
      return;
    }

    const HEADER_HEIGHT = 44;
    const STICKY_ON_OFFSET = 10;
    const STICKY_OFF_OFFSET = -6;
    let activeIndex = 0;

    for (let i = 0; i < selectedScopeFiles.length; i += 1) {
      const rowTop = virtualRowOffsets[i] ?? 0;
      if (rowTop <= diffScrollTop + 1) {
        activeIndex = i;
      } else {
        break;
      }
    }

    const activeFile = selectedScopeFiles[activeIndex];
    const activeRowTop = virtualRowOffsets[activeIndex] ?? 0;
    const nextFileKey = activeFile?.path ?? renderedDiffFileKeys[0] ?? '';
    const stickyBoundary = activeRowTop + HEADER_HEIGHT;

    setCurrentStickyFileKey((current) => (current === nextFileKey ? current : nextFileKey));
    setShowStickyFileHeader((current) => {
      const shouldShow = current
        ? diffScrollTop > stickyBoundary + STICKY_OFF_OFFSET
        : diffScrollTop > stickyBoundary + STICKY_ON_OFFSET;
      return current === shouldShow ? current : shouldShow;
    });
  }, [viewerOpen, state.selected_path, renderedDiffFileKeys, selectedScopeFiles, diffScrollTop, virtualRowOffsets]);

  useEffect(() => {
    if (!viewerOpen || state.selected_path || selectedScopeFiles.length === 0) {
      return;
    }

    const nextKeys = selectedScopeFiles
      .slice(visibleRowRange.start, visibleRowRange.end)
      .map((file) => file.path);

    if (nextKeys.length === 0) {
      return;
    }

    setHydratedDiffFiles((current) => {
      const next = { ...current };
      let changed = false;
      for (const key of nextKeys) {
        if (!next[key]) {
          next[key] = true;
          changed = true;
        }
      }
      return changed ? next : current;
    });

    ensureFilePatchesInOrder(nextKeys);
  }, [viewerOpen, state.selected_path, selectedScopeFiles, visibleRowRange, repoRef, state.selected_scope, state.context_lines, state.whole_file]);

  useEffect(() => {
    if (!viewerOpen) {
      return;
    }
    setSidebarHidden(false);
    setSidebarWidth(DEFAULT_SIDEBAR_WIDTH);
    setSidebarWidthDraft(null);
    setResizing(false);
  }, [viewerOpen]);

  useEffect(() => {
    const viewportElement = diffViewportRef.current;
    if (!viewerOpen || !viewportElement) {
      return;
    }

    function updateViewportMetrics() {
      const viewport = diffViewportRef.current;
      if (!viewport) {
        return;
      }
      setDiffScrollTop(viewport.scrollTop);
      setDiffViewportHeight(viewport.clientHeight);
    }

    updateViewportMetrics();
    viewportElement.addEventListener('scroll', updateViewportMetrics, { passive: true });
    window.addEventListener('resize', updateViewportMetrics);
    return () => {
      viewportElement.removeEventListener('scroll', updateViewportMetrics);
      window.removeEventListener('resize', updateViewportMetrics);
    };
  }, [viewerOpen, state.selected_path, renderedDiffFileKeys]);

  useEffect(() => {
    const viewport = diffViewportRef.current;
    if (!viewerOpen || state.selected_path || !viewport) {
      return;
    }

    const elements = Array.from(
      viewport.querySelectorAll<HTMLElement>('[data-virtual-file-row="true"]')
    );

    if (elements.length === 0) {
      return;
    }

    setMeasuredRowHeights((current) => {
      const next = { ...current };
      let changed = false;

      for (const element of elements) {
        const fileKey = element.dataset.fileKey;
        if (!fileKey) {
          continue;
        }
        const measured = Math.max(56, Math.ceil(element.getBoundingClientRect().height));
        if (!Number.isFinite(measured)) {
          continue;
        }
        if (Math.abs((next[fileKey] ?? 0) - measured) > 2) {
          next[fileKey] = measured;
          changed = true;
        }
      }

      return changed ? next : current;
    });
  }, [viewerOpen, state.selected_path, visibleRowRange, parsedFileDiffByPath, collapsedDiffFiles, state.diff_style]);

  useEffect(() => {
    if (!resizing) {
      return;
    }

    function handleMove(event: MouseEvent) {
      if (resizeFrame.current) {
        cancelAnimationFrame(resizeFrame.current);
      }
      resizeFrame.current = requestAnimationFrame(() => {
        const width = window.innerWidth - event.clientX;
        setSidebarWidthDraft(clampSidebarWidth(width));
      });
    }

    function handleUp() {
      setResizing(false);
      if (resizeFrame.current) {
        cancelAnimationFrame(resizeFrame.current);
        resizeFrame.current = null;
      }

      const committedWidth = clampSidebarWidth(sidebarWidthDraft ?? sidebarWidth);
      setSidebarWidthDraft(null);
      setSidebarWidth(committedWidth);
    }

    window.addEventListener('mousemove', handleMove);
    window.addEventListener('mouseup', handleUp);
    return () => {
      window.removeEventListener('mousemove', handleMove);
      window.removeEventListener('mouseup', handleUp);
    };
  }, [resizing, sidebarWidthDraft, sidebarWidth]);

  const effectiveSidebarWidth = clampSidebarWidth(sidebarWidthDraft ?? sidebarWidth);
  const showSidebar = !sidebarHidden;
  const compactCounts = effectiveSidebarWidth < 340;

  return (
    <Stack gap="md">
      <Group justify="space-between" align="center">
        <Group>
          <Button variant={viewerOpen ? 'filled' : 'light'} onClick={() => setViewerOpen((value) => !value)}>
            {viewerOpen ? 'Close diff viewer' : 'Open diff viewer'}
          </Button>
          <Button variant="default" onClick={() => void refreshStatus()} loading={statusBusy}>Refresh</Button>
        </Group>
        <Group gap="xs">
          {branchSummary ? <Badge variant="light">{branchSummary}</Badge> : null}
          <Badge color="green" variant="light">+{allTotals.additions}</Badge>
          <Badge color="red" variant="light">-{allTotals.deletions}</Badge>
        </Group>
      </Group>

      {statusError ? <Alert color="red">{statusError}</Alert> : null}

      <Modal
        opened={viewerOpen}
        onClose={() => setViewerOpen(false)}
        withCloseButton={false}
        fullScreen
        padding={0}
        radius={0}
        styles={{
          content: {
            inset: 0,
            width: '100vw',
            maxWidth: '100vw',
            height: '100vh',
            maxHeight: '100vh',
            margin: 0,
            display: 'flex',
            flexDirection: 'column'
          },
          body: {
            flex: 1,
            padding: 0,
            minHeight: 0
          }
        }}
      >
        <Box style={{ height: '100%', display: 'grid', gridTemplateColumns: showSidebar ? `1fr 8px ${effectiveSidebarWidth}px` : '1fr' }}>
          <Box p="sm" style={{ minHeight: 0, overflow: 'hidden', display: 'flex', flexDirection: 'column' }}>
            <Card withBorder p="sm" style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
              <Group justify="space-between" align="center" mb="sm">
                <Group>
                  <Button variant="filled" onClick={() => setViewerOpen(false)}>Close diff viewer</Button>
                  <Button variant="default" onClick={() => void refreshStatus()} loading={statusBusy}>Refresh</Button>
                  <Button variant="default" onClick={() => setSidebarHidden((value) => !value)}>
                    {showSidebar ? 'Hide source control' : 'Show source control'}
                  </Button>
                </Group>
                <Group gap="xs">
                  {branchSummary ? <Badge variant="light">{branchSummary}</Badge> : null}
                  <Badge variant="light">{state.selected_scope === 'staged' ? 'Staged' : 'Unstaged'}</Badge>
                  <Badge color="green" variant="light">+{selectedTotals.additions}</Badge>
                  <Badge color="red" variant="light">-{selectedTotals.deletions}</Badge>
                </Group>
              </Group>
              <Divider mb="sm" />
              <Group justify="space-between" mb="sm">
                <Group gap="sm">
                  <Text fw={600}>{selectedTitle}</Text>
                  {!state.selected_path && renderedDiffFileKeys.length > 0 ? (
                    <>
                      <Button size="xs" variant="default" onClick={() => setAllDiffFilesCollapsed(false)}>
                        Expand all
                      </Button>
                      <Button size="xs" variant="default" onClick={() => setAllDiffFilesCollapsed(true)}>
                        Collapse all
                      </Button>
                    </>
                  ) : null}
                </Group>
                {state.selected_path ? (
                  diff ? <Badge variant="light">{diff.from_ref} → {diff.to_ref}</Badge> : null
                ) : diffManifest ? (
                  <Badge variant="light">{diffManifest.from_ref} → {diffManifest.to_ref}</Badge>
                ) : null}
              </Group>
              <Box style={{ flex: 1, minHeight: 0 }}>
                {diffBusy ? (
                  <Group justify="center" py="xl"><Loader /></Group>
                ) : diffError ? (
                  <Alert color="red">{diffError}</Alert>
                ) : state.selected_path ? (
                  !diff?.patch ? (
                    <Text c="dimmed" size="sm">No diff available.</Text>
                  ) : (() => {
                    const singleFileDiff = parsePatchFiles(diff.patch).flatMap((patch) => patch.files ?? [])[0];
                    return singleFileDiff ? (
                      <ScrollArea h="100%" type="auto">
                        <Box p={0} style={{ overflow: 'hidden' }}>
                          <Box style={{ marginTop: -36 }}>
                            <FileDiff
                              fileDiff={singleFileDiff}
                              options={{
                                theme: {
                                  dark: 'pierre-dark',
                                  light: 'pierre-light'
                                },
                                diffStyle: state.diff_style
                              }}
                            />
                          </Box>
                        </Box>
                      </ScrollArea>
                    ) : (
                      <Text c="dimmed" size="sm">No diff available.</Text>
                    );
                  })()
                ) : renderedDiffFileKeys.length > 0 ? (
                  <ScrollArea h="100%" type="auto" viewportRef={diffViewportRef}>
                    <Box style={{ paddingBottom: '1rem' }}>
                      {showStickyFileHeader && currentStickyFileKey ? (
                        <Box
                          px="sm"
                          py="xs"
                          style={{
                            position: 'sticky',
                            top: 0,
                            zIndex: 3,
                            marginBottom: '0.75rem',
                            background: 'var(--mantine-color-body)',
                            border: '1px solid rgba(255,255,255,0.08)',
                            borderRadius: 8
                          }}
                        >
                          <Group gap="xs" wrap="nowrap" style={{ minWidth: 0 }}>
                            <Badge variant="outline">
                              {selectedScopeCountsByPath[currentStickyFileKey]
                                ? statusCode(selectedScopeCountsByPath[currentStickyFileKey] as ReviewStatusFileEntry)
                                : 'M'}
                            </Badge>
                            <Text size="sm" fw={600} style={{ wordBreak: 'break-word' }}>
                              {currentStickyFileKey}
                            </Text>
                          </Group>
                        </Box>
                      ) : null}

                      <Box style={{ height: visibleRowRange.start > 0 ? virtualRowOffsets[visibleRowRange.start] : 0 }} />

                      <Stack gap="md">
                        {selectedScopeFiles.slice(visibleRowRange.start, visibleRowRange.end).map((file, localIndex) => {
                          const absoluteIndex = visibleRowRange.start + localIndex;
                          const fileKey = file.path;
                          const isCollapsed = Boolean(collapsedDiffFiles[fileKey]);
                          const fileCounts = selectedScopeCountsByPath[fileKey];
                          const shouldRenderDiffBody = Boolean(hydratedDiffFiles[fileKey]);
                          const parsedFileDiff: FileDiffMetadata | null = parsedFileDiffByPath[fileKey] ?? null;

                          if (!isCollapsed && !parsedFileDiff && shouldRenderDiffBody) {
                            ensureFilePatch(fileKey);
                          }

                          return (
                            <Box
                              key={`${state.selected_scope}-${fileKey}`}
                              data-virtual-file-row="true"
                              data-file-key={fileKey}
                              style={{ minHeight: virtualRowHeights[absoluteIndex] ?? 110 }}
                            >
                              <Card
                                withBorder
                                p={0}
                                style={{
                                  overflow: 'hidden',
                                  contentVisibility: 'auto',
                                  containIntrinsicSize: isCollapsed ? '56px' : '240px'
                                }}
                              >
                                <Box
                                  px="sm"
                                  py="xs"
                                  style={{
                                    background: 'var(--mantine-color-body)',
                                    borderBottom: isCollapsed ? 'none' : '1px solid rgba(255,255,255,0.08)'
                                  }}
                                >
                                  <Group justify="space-between" align="center" wrap="nowrap">
                                    <Group gap="xs" wrap="nowrap" style={{ minWidth: 0, flex: 1 }}>
                                      <Badge variant="outline">{fileCounts ? statusCode(fileCounts as ReviewStatusFileEntry) : 'M'}</Badge>
                                      <Text size="sm" fw={600} style={{ wordBreak: 'break-word' }}>
                                        {fileKey}
                                      </Text>
                                    </Group>
                                    <Group gap="xs" align="center" wrap="nowrap">
                                      <Badge color="green" variant="light">+{fileCounts?.additions ?? 0}</Badge>
                                      <Badge color="red" variant="light">-{fileCounts?.deletions ?? 0}</Badge>
                                      <Button
                                        size="compact-xs"
                                        variant="subtle"
                                        onClick={() => toggleDiffFileCollapsed(fileKey)}
                                      >
                                        {isCollapsed ? 'Expand' : 'Collapse'}
                                      </Button>
                                    </Group>
                                  </Group>
                                </Box>
                                {!isCollapsed ? (
                                  parsedFileDiff ? (
                                    <Box p={0} style={{ overflow: 'hidden' }}>
                                      <Box style={{ marginTop: -36 }}>
                                        <FileDiff
                                          fileDiff={parsedFileDiff}
                                          options={{
                                            theme: {
                                              dark: 'pierre-dark',
                                              light: 'pierre-light'
                                            },
                                            diffStyle: state.diff_style
                                          }}
                                        />
                                      </Box>
                                    </Box>
                                  ) : shouldRenderDiffBody || filePatchBusyByPath[fileKey] || filePatchInFlightRef.current.has(fileKey) ? (
                                    <Box p="md">
                                      <Group justify="center" py="lg">
                                        <Loader size="sm" />
                                      </Group>
                                    </Box>
                                  ) : null
                                ) : null}
                              </Card>
                            </Box>
                          );
                        })}
                      </Stack>

                      <Box
                        style={{
                          height: Math.max(
                            0,
                            virtualTotalHeight - (visibleRowRange.end > 0 ? virtualRowOffsets[visibleRowRange.end - 1] + virtualRowHeights[visibleRowRange.end - 1] : 0)
                          )
                        }}
                      />
                    </Box>
                  </ScrollArea>
                ) : (
                  <Text c="dimmed" size="sm">No diff available.</Text>
                )}
              </Box>
            </Card>
          </Box>

          {showSidebar ? (
            <Box
              onMouseDown={() => {
                setSidebarWidthDraft(clampSidebarWidth(sidebarWidth));
                setResizing(true);
              }}
              style={{
                cursor: 'col-resize',
                background: resizing ? 'rgba(34, 139, 230, 0.35)' : 'rgba(255,255,255,0.06)',
                transition: resizing ? 'none' : 'background 120ms ease'
              }}
            />
          ) : null}

          {showSidebar ? (
            <Box
              p="sm"
              style={{
                borderLeft: '1px solid rgba(255,255,255,0.08)',
                minHeight: 0,
                overflow: 'hidden',
                display: 'flex',
                flexDirection: 'column'
              }}
            >
              <Card withBorder p="sm" mb="md">
                <Stack gap="sm">
                  <Group justify="space-between" align="center">
                    <Text fw={600}>Diff browser</Text>
                    <Button size="xs" variant="subtle" onClick={() => setSidebarHidden(true)}>Hide</Button>
                  </Group>
                  <SegmentedControl
                    value={state.diff_style}
                    onChange={(value) => void patchState({ diff_style: value as 'unified' | 'split' })}
                    data={[
                      { label: 'Unified', value: 'unified' },
                      { label: 'Split', value: 'split' }
                    ]}
                  />
                  <Group>
                    <Checkbox label="Only show changes" checked={state.only_changes} onChange={(event) => void patchState({ only_changes: event.currentTarget.checked })} />
                    <Checkbox label="Whole file" checked={state.whole_file} onChange={(event) => void patchState({ whole_file: event.currentTarget.checked })} />
                  </Group>
                  <NumberInput
                    label="Context lines"
                    min={0}
                    max={1000}
                    step={1}
                    value={state.context_lines}
                    disabled={state.whole_file}
                    onChange={(value) => void patchState({ context_lines: clampContextLines(value) })}
                  />
                </Stack>
              </Card>

              <Box style={{ flex: 1, minHeight: 0 }}>
                <ScrollArea h="100%" type="auto">
                  <Stack gap="md">
                    <ScopeHeader
                      title="Staged"
                      active={state.selected_scope === 'staged' && state.selected_path === null}
                      fileCount={stagedFiles.length}
                      additions={stagedTotals.additions}
                      deletions={stagedTotals.deletions}
                      compactCounts={compactCounts}
                      buttonLabel="Unstage all"
                      actionBusy={actionBusy}
                      onSelect={() => void patchState({ selected_scope: 'staged', selected_path: null })}
                      onAction={() => runStageAction('unstage', 'staged', null)}
                    />
                    {stagedFiles.map((file) => (
                      <FileRow
                        key={`staged:${file.path}`}
                        scope="staged"
                        file={file}
                        active={state.selected_scope === 'staged' && state.selected_path === file.path}
                        actionBusy={actionBusy}
                        onSelect={() => void patchState({ selected_scope: 'staged', selected_path: file.path })}
                        onStage={() => runStageAction('stage', 'staged', file.path)}
                        onUnstage={() => runStageAction('unstage', 'staged', file.path)}
                      />
                    ))}
                    <Divider />
                    <ScopeHeader
                      title="Unstaged"
                      active={state.selected_scope === 'unstaged' && state.selected_path === null}
                      fileCount={unstagedFiles.length}
                      additions={unstagedTotals.additions}
                      deletions={unstagedTotals.deletions}
                      compactCounts={compactCounts}
                      buttonLabel="Stage all"
                      actionBusy={actionBusy}
                      onSelect={() => void patchState({ selected_scope: 'unstaged', selected_path: null })}
                      onAction={() => runStageAction('stage', 'unstaged', null)}
                    />
                    {unstagedFiles.map((file) => (
                      <FileRow
                        key={`unstaged:${file.path}`}
                        scope="unstaged"
                        file={file}
                        active={state.selected_scope === 'unstaged' && state.selected_path === file.path}
                        actionBusy={actionBusy}
                        onSelect={() => void patchState({ selected_scope: 'unstaged', selected_path: file.path })}
                        onStage={() => runStageAction('stage', 'unstaged', file.path)}
                        onUnstage={() => runStageAction('unstage', 'unstaged', file.path)}
                      />
                    ))}
                  </Stack>
                </ScrollArea>
              </Box>
            </Box>
          ) : null}
        </Box>
      </Modal>
    </Stack>
  );
}
