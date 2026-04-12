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
import { FileDiff, PatchDiff, Virtualizer } from '@pierre/diffs/react';
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
  forceViewerOpen?: boolean;
};

const MIN_SIDEBAR_WIDTH = 280;
const MAX_SIDEBAR_WIDTH = 620;
const DEFAULT_SIDEBAR_WIDTH = 360;
const LARGE_SINGLE_FILE_RENDER_LINE_LIMIT = 8000;

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
  const { repoRef, state, onPersistState, forceViewerOpen = false } = props;
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
  const resizeFrame = useRef<number | null>(null);
  const refreshDiffRequestIdRef = useRef(0);
  const [diffManifest, setDiffManifest] = useState<ReviewDiffManifestResponse | null>(null);
  const [filePatchByPath, setFilePatchByPath] = useState<Record<string, string>>({});
  const [filePatchBusyByPath, setFilePatchBusyByPath] = useState<Record<string, boolean>>({});
  const [collapsedByPath, setCollapsedByPath] = useState<Record<string, boolean>>({});

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

  const selectedFilePatch = useMemo(() => {
    if (!state.selected_path || !diff?.patch?.trim()) {
      return '';
    }
    return diff.patch;
  }, [state.selected_path, diff?.patch]);

  const selectedFilePayloadInfo = useMemo(() => {
    if (!state.selected_path || !diff?.patch?.trim()) {
      return {
        fileCount: 0,
        containsSelectedFile: false,
        isExactSelectedFilePayload: false,
        selectedUnifiedLineCount: 0,
      };
    }

    try {
      const files = parsePatchFiles(diff.patch).flatMap((patch) => patch.files ?? []);
      const selected = files.find((file) => file.name === state.selected_path) ?? null;
      return {
        fileCount: files.length,
        containsSelectedFile: files.some((file) => file.name === state.selected_path),
        isExactSelectedFilePayload: files.length === 1 && selected?.name === state.selected_path,
        selectedUnifiedLineCount: selected?.unifiedLineCount ?? 0,
      };
    } catch {
      return {
        fileCount: 0,
        containsSelectedFile: false,
        isExactSelectedFilePayload: false,
        selectedUnifiedLineCount: 0,
      };
    }
  }, [state.selected_path, diff?.patch]);

  const parsedFileDiffByPath = useMemo<Record<string, FileDiffMetadata | null>>(() => {
    const next: Record<string, FileDiffMetadata | null> = {};
    for (const file of diffManifest?.files ?? []) {
      const patch = filePatchByPath[file.path];
      if (!patch || !patch.trim()) {
        next[file.path] = null;
        continue;
      }
      try {
        const parsed = parsePatchFiles(patch).flatMap((item) => item.files ?? []);
        next[file.path] = parsed.find((entry) => entry.name === file.path) ?? parsed[0] ?? null;
      } catch {
        next[file.path] = null;
      }
    }
    return next;
  }, [diffManifest, filePatchByPath]);

  const scopeDiffRows = useMemo(() => {
    return (diffManifest?.files ?? []).map((file) => ({
      file,
      status: selectedScopeCountsByPath[file.path] ?? null,
      parsed: parsedFileDiffByPath[file.path] ?? null,
    }));
  }, [diffManifest, parsedFileDiffByPath, selectedScopeCountsByPath]);

  const hasScopeDiffRows = scopeDiffRows.length > 0;
  const allScopeRowsCollapsed = hasScopeDiffRows && scopeDiffRows.every(({ file }) => collapsedByPath[file.path] !== false);

  function setAllScopeRowsCollapsed(collapsed: boolean) {
    setCollapsedByPath(
      Object.fromEntries(scopeDiffRows.map(({ file }) => [file.path, collapsed]))
    );
  }

  function toggleScopeRowCollapsed(path: string) {
    setCollapsedByPath((current) => ({
      ...current,
      [path]: !(current[path] ?? false),
    }));
  }

  const singleFileRenderedLineCount = useMemo(() => {
    return selectedFilePayloadInfo.selectedUnifiedLineCount;
  }, [selectedFilePayloadInfo]);

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

  async function refreshScopeFilePatches(
    nextState: ReviewSourceControlState,
    files: ReviewDiffManifestFileEntry[],
    requestId: number
  ) {
    if (!repoRef.trim() || nextState.selected_path || files.length === 0) {
      if (refreshDiffRequestIdRef.current !== requestId) {
        return;
      }
      setFilePatchByPath({});
      setFilePatchBusyByPath({});
      setCollapsedByPath({});
      return;
    }

    const busy: Record<string, boolean> = {};
    const collapsed: Record<string, boolean> = {};
    for (const file of files) {
      busy[file.path] = true;
      collapsed[file.path] = false;
    }
    if (refreshDiffRequestIdRef.current !== requestId) {
      return;
    }
    setFilePatchByPath({});
    setFilePatchBusyByPath(busy);
    setCollapsedByPath(collapsed);

    const patchEntries = await Promise.all(
      files.map(async (file) => {
        try {
          const json = await getReviewFilePatch({
            repo_ref: repoRef,
            scope: nextState.selected_scope,
            path: file.path,
            context_lines: nextState.whole_file ? 1000 : clampContextLines(nextState.context_lines),
            whole_file: nextState.whole_file,
          });
          return [file.path, json.patch] as const;
        } catch {
          return [file.path, ''] as const;
        }
      })
    );

    if (refreshDiffRequestIdRef.current !== requestId) {
      return;
    }

    const nextPatchByPath: Record<string, string> = {};
    for (const [path, patch] of patchEntries) {
      nextPatchByPath[path] = patch;
    }
    setFilePatchByPath(nextPatchByPath);
    setFilePatchBusyByPath({});
  }

  async function refreshDiff(nextState: ReviewSourceControlState) {
    if (!repoRef.trim() || !viewerOpen) return;

    const requestId = ++refreshDiffRequestIdRef.current;

    try {
      setDiffBusy(true);
      setDiffError(null);

      const manifest = await getReviewDiffManifest({
        repo_ref: repoRef,
        scope: nextState.selected_scope,
      });
      if (refreshDiffRequestIdRef.current !== requestId) {
        return;
      }
      setDiffManifest(manifest);

      if (nextState.selected_path) {
        const json = await getReviewDiff({
          repo_ref: repoRef,
          scope: nextState.selected_scope,
          path: nextState.selected_path,
          context_lines: nextState.whole_file ? 1000 : clampContextLines(nextState.context_lines),
          whole_file: nextState.whole_file,
        });
        if (refreshDiffRequestIdRef.current !== requestId) {
          return;
        }
        setDiff(json);
        setFilePatchByPath({});
        setFilePatchBusyByPath({});
        return;
      }

      setDiff(null);
      if (refreshDiffRequestIdRef.current === requestId) {
        setDiffBusy(false);
      }
      await refreshScopeFilePatches(nextState, manifest.files, requestId);
      return;
    } catch (err) {
      if (refreshDiffRequestIdRef.current !== requestId) {
        return;
      }
      setDiffError(err instanceof Error ? err.message : String(err));
    } finally {
      if (refreshDiffRequestIdRef.current === requestId) {
        setDiffBusy(false);
      }
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


  useEffect(() => {
    void refreshStatus();
  }, [repoRef]);

  useEffect(() => {
    if (viewerOpen) {
      void refreshDiff(state);
    }
  }, [repoRef, viewerOpen, state.selected_scope, state.selected_path, state.context_lines, state.whole_file]);


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

  useEffect(() => {
    if (forceViewerOpen && !viewerOpen) {
      setViewerOpen(true);
    }
  }, [forceViewerOpen, viewerOpen]);

  const viewerContent = (
    <Box style={{ height: '100%', display: 'grid', gridTemplateColumns: showSidebar ? `1fr 8px ${effectiveSidebarWidth}px` : '1fr' }}>
      <Box p="sm" style={{ minHeight: 0, overflow: 'hidden', display: 'flex', flexDirection: 'column' }}>
        <Card withBorder p="sm" style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
          <Group justify="space-between" align="center" mb="sm">
            <Group>
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
              {!state.selected_path && scopeDiffRows.length > 0 ? (
                <Text size="xs" c="dimmed">Virtualized multi-file patch diff</Text>
              ) : null}
            </Group>
            <Group gap="xs">
              {!state.selected_path && scopeDiffRows.length > 0 ? (
                <>
                  <Button
                    size="xs"
                    variant="default"
                    onClick={() => setAllScopeRowsCollapsed(false)}
                  >
                    Expand all
                  </Button>
                  <Button
                    size="xs"
                    variant="default"
                    onClick={() => setAllScopeRowsCollapsed(true)}
                  >
                    Collapse all
                  </Button>
                </>
              ) : null}
              {state.selected_path ? (
                diff ? <Badge variant="light">{diff.from_ref} → {diff.to_ref}</Badge> : null
              ) : diffManifest ? (
                <Badge variant="light">{diffManifest.from_ref} → {diffManifest.to_ref}</Badge>
              ) : null}
            </Group>
          </Group>
          <Box style={{ flex: 1, minHeight: 0 }}>
            {diffBusy ? (
              <Group justify="center" py="xl"><Loader /></Group>
            ) : diffError ? (
              <Alert color="red">{diffError}</Alert>
            ) : state.selected_path ? (
              !selectedFilePayloadInfo.containsSelectedFile ? (
                <Alert color="yellow" title="Selected file diff unavailable">
                  <Stack gap="sm">
                    <Text size="sm">
                      The selected file was not found in the current diff payload.
                    </Text>
                    <Group>
                      <Button
                        size="xs"
                        variant="filled"
                        onClick={() => void patchState({ selected_path: null })}
                      >
                        Open virtualized scope diff
                      </Button>
                    </Group>
                  </Stack>
                </Alert>
              ) : !selectedFilePayloadInfo.isExactSelectedFilePayload ? (
                <Alert color="yellow" title="Selected file diff is stale">
                  <Stack gap="sm">
                    <Text size="sm">
                      The current payload contains {selectedFilePayloadInfo.fileCount} file diffs, so it is not safe to render in selected-file mode.
                    </Text>
                    <Group>
                      <Button
                        size="xs"
                        variant="filled"
                        onClick={() => void patchState({ selected_path: null })}
                      >
                        Open virtualized scope diff
                      </Button>
                    </Group>
                  </Stack>
                </Alert>
              ) : singleFileRenderedLineCount > LARGE_SINGLE_FILE_RENDER_LINE_LIMIT ? (
                <Alert color="yellow" title="Large single-file diff">
                  <Stack gap="sm">
                    <Text size="sm">
                      This patch is too large for the non-virtualized single-file renderer.
                    </Text>
                    <Group>
                      <Button
                        size="xs"
                        variant="filled"
                        onClick={() => void patchState({ selected_path: null })}
                      >
                        Open virtualized scope diff
                      </Button>
                      <Button
                        size="xs"
                        variant="default"
                        onClick={() => void patchState({ whole_file: false })}
                      >
                        Reduce context
                      </Button>
                    </Group>
                  </Stack>
                </Alert>
              ) : selectedFilePatch ? (
                <ScrollArea h="100%" type="auto">
                  <Box p={0} style={{ overflow: 'hidden' }}>
                    <PatchDiff
                      patch={selectedFilePatch}
                      options={{
                        theme: {
                          dark: 'pierre-dark',
                          light: 'pierre-light'
                        },
                        diffStyle: state.diff_style
                      }}
                    />
                  </Box>
                </ScrollArea>
              ) : (
                <Text c="dimmed" size="sm">No diff available.</Text>
              )
            ) : scopeDiffRows.length > 0 ? (
              <ScrollArea h="100%" type="auto">
                <Box p="xs" style={{ minHeight: '100%' }}>
                  <Virtualizer contentStyle={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
                    {scopeDiffRows.map(({ file, status, parsed }) => {
                      const collapsed = collapsedByPath[file.path] ?? false;
                      return (
                        <Card key={file.path} withBorder p={0} style={{ overflow: 'hidden' }}>
                          <Box
                            px="sm"
                            py="xs"
                            style={{
                              position: 'sticky',
                              top: 0,
                              zIndex: 2,
                              background: 'var(--mantine-color-body)',
                              borderBottom: '1px solid rgba(255,255,255,0.08)'
                            }}
                          >
                            <Group justify="space-between" wrap="nowrap" gap="xs">
                              <Group gap="xs" wrap="nowrap" style={{ minWidth: 0, flex: 1 }}>
                                <Badge variant="outline">{status ? statusCode(status) : 'M'}</Badge>
                                <Button
                                  size="compact-xs"
                                  variant="subtle"
                                  onClick={() => toggleScopeRowCollapsed(file.path)}
                                >
                                  {collapsed ? 'Expand' : 'Collapse'}
                                </Button>
                                <Text size="sm" fw={600} style={{ wordBreak: 'break-word' }}>{file.path}</Text>
                              </Group>
                              <Group gap="xs" wrap="nowrap">
                                <Badge color="green" variant="light">+{status?.additions ?? file.additions}</Badge>
                                <Badge color="red" variant="light">-{status?.deletions ?? file.deletions}</Badge>
                              </Group>
                            </Group>
                          </Box>
                          {!collapsed ? (
                            parsed ? (
                              <Box p={0} style={{ overflow: 'hidden' }}>
                                <FileDiff
                                  fileDiff={parsed}
                                  options={{
                                    theme: {
                                      dark: 'pierre-dark',
                                      light: 'pierre-light'
                                    },
                                    diffStyle: state.diff_style
                                  }}
                                />
                              </Box>
                            ) : filePatchBusyByPath[file.path] ? (
                              <Box p="md">
                                <Group justify="center" py="lg">
                                  <Loader size="sm" />
                                </Group>
                              </Box>
                            ) : (
                              <Box p="md">
                                <Text c="dimmed" size="sm">No diff available.</Text>
                              </Box>
                            )
                          ) : null}
                        </Card>
                      );
                    })}
                  </Virtualizer>
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
  );

  return (
    <>
      {statusError ? <Alert color="red">{statusError}</Alert> : null}

      {forceViewerOpen ? (
        <Box style={{ height: 'calc(100vh - 180px)', minHeight: 520 }}>
          {viewerContent}
        </Box>
      ) : (
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
          {viewerContent}
        </Modal>
      )}
    </>
  );
}
