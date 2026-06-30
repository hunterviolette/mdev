import { useEffect, useMemo, useRef, useState } from 'react';
import {
  ActionIcon,
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
import { DiffCode, inspectUnifiedPatch } from './DiffCode';
import { IconMinus, IconPlus, IconTrash } from '@tabler/icons-react';
import {
  getReviewDiff,
  getReviewDiffManifest,
  getReviewFilePatch,
  getReviewCommitDiff,
  getReviewCommitDiffManifest,
  discardWorkflowReviewDiff,
  getReviewStatus,
  stageReviewDiff,
  unstageReviewDiff,
  type ReviewDiffManifestFileEntry,
  type ReviewDiffManifestResponse,
  type ReviewDiffResponse,
  type ReviewCommitDiffManifestResponse,
  type ReviewCommitDiffResponse,
  type ReviewDiffScope,
  type ReviewStatusFileEntry,
} from './api';

export type DiffPanelState = {
  selected_scope: ReviewDiffScope;
  selected_path: string | null;
  diff_style: 'unified' | 'split';
  only_changes: boolean;
  context_lines: number;
  whole_file: boolean;
};

type DiffPanelProps = {
  runId: string | null;
  repoRef: string;
  state: DiffPanelState;
  onPersistState: (next: DiffPanelState) => Promise<void>;
  forceViewerOpen?: boolean;
  mode?: 'worktree' | 'commit';
  commitSha?: string | null;
  commitTitle?: string;
  commitSubtitle?: string;
  onClose?: () => void;
};

const MIN_SIDEBAR_WIDTH = 280;
const MAX_SIDEBAR_WIDTH = 620;
const DEFAULT_SIDEBAR_WIDTH = 360;
const LARGE_SINGLE_FILE_RENDER_LINE_LIMIT = 8000;

function clampContextLines(value: number | string | null | undefined): number {
  const numeric = typeof value === 'number' ? value : Number(value ?? 4);
  if (!Number.isFinite(numeric)) {
    return 4;
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

function sumCounts(files: Array<{ additions: number; deletions: number }>) {
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
  buttonLabel?: string;
  buttonTooltip?: string;
  actionBusy: boolean;
  extraAction?: JSX.Element | null;
  onSelect: () => void;
  onAction: () => Promise<void>;
}) {
  const { title, active, fileCount, additions, deletions, compactCounts, buttonLabel, buttonTooltip, actionBusy, extraAction, onSelect, onAction } = props;

  return (
    <Box
      style={{
        display: 'grid',
        gridTemplateColumns: 'minmax(0, 1fr) auto auto',
        gap: 6,
        alignItems: 'center',
        padding: 4,
        borderRadius: 7,
        background: active ? 'rgba(34, 139, 230, 0.28)' : 'rgba(255,255,255,0.025)',
        border: active ? '1px solid rgba(74, 171, 247, 0.88)' : '1px solid rgba(255,255,255,0.07)',
        boxShadow: active ? 'inset 3px 0 0 rgba(116, 192, 252, 0.95), 0 0 0 1px rgba(74, 171, 247, 0.18)' : undefined,
      }}
    >
      <Box
        onClick={onSelect}
        style={{
          minWidth: 0,
          height: 24,
          cursor: 'pointer',
          display: 'grid',
          gridTemplateColumns: compactCounts ? 'minmax(0, 1fr) auto' : 'minmax(0, 1fr) auto auto auto',
          gap: 6,
          alignItems: 'center',
          paddingInline: 6,
          borderRadius: 5,
        }}
      >
        <Text size="sm" fw={800} truncate>{title}</Text>
        <Badge size="xs" variant={active ? 'filled' : 'light'}>{fileCount}</Badge>
        {!compactCounts ? <Badge size="xs" color="green" variant="light">+{additions}</Badge> : null}
        {!compactCounts ? <Badge size="xs" color="red" variant="light">-{deletions}</Badge> : null}
      </Box>
      {buttonLabel && buttonTooltip ? (
        <Tooltip label={buttonTooltip} withArrow>
          <ActionIcon
            size="sm"
            variant="outline"
            color="blue"
            loading={actionBusy}
            aria-label={buttonTooltip}
            onClick={() => void onAction()}
            style={{ width: 28, height: 24, minWidth: 28 }}
          >
            <Text size="xs" fw={800}>{buttonLabel}</Text>
          </ActionIcon>
        </Tooltip>
      ) : null}
      {extraAction ?? null}
    </Box>
  );
}

type FileTreeNode =
  | {
      kind: 'dir';
      name: string;
      path: string;
      children: FileTreeNode[];
      additions: number;
      deletions: number;
      fileCount: number;
    }
  | {
      kind: 'file';
      name: string;
      path: string;
      file: ReviewStatusFileEntry;
      additions: number;
      deletions: number;
    };

function buildFileTree(files: ReviewStatusFileEntry[]): FileTreeNode[] {
  const root = new Map<string, FileTreeNode>();

  function ensureDir(parent: Map<string, FileTreeNode>, name: string, path: string): Extract<FileTreeNode, { kind: 'dir' }> {
    const existing = parent.get(name);
    if (existing?.kind === 'dir') return existing;

    const node: Extract<FileTreeNode, { kind: 'dir' }> = {
      kind: 'dir',
      name,
      path,
      children: [],
      additions: 0,
      deletions: 0,
      fileCount: 0,
    };
    parent.set(name, node);
    return node;
  }

  const childMaps = new WeakMap<Extract<FileTreeNode, { kind: 'dir' }>, Map<string, FileTreeNode>>();
  const childrenFor = (dir: Extract<FileTreeNode, { kind: 'dir' }>) => {
    let map = childMaps.get(dir);
    if (!map) {
      map = new Map(dir.children.map((child) => [child.name, child]));
      childMaps.set(dir, map);
    }
    return map;
  };

  for (const file of files) {
    const parts = file.path.split(/[\\/]+/).filter(Boolean);
    if (parts.length === 0) continue;

    let currentMap = root;
    let currentPath = '';
    const dirs: Extract<FileTreeNode, { kind: 'dir' }>[] = [];

    for (const part of parts.slice(0, -1)) {
      currentPath = currentPath ? `${currentPath}/${part}` : part;
      const dir = ensureDir(currentMap, part, currentPath);
      dirs.push(dir);
      currentMap = childrenFor(dir);
    }

    const name = parts[parts.length - 1];
    currentMap.set(name, {
      kind: 'file',
      name,
      path: file.path,
      file,
      additions: file.additions,
      deletions: file.deletions,
    });

    for (const dir of dirs) {
      dir.additions += file.additions;
      dir.deletions += file.deletions;
      dir.fileCount += 1;
    }
  }

  function finalize(nodes: FileTreeNode[]) {
    nodes.sort((a, b) => {
      if (a.kind !== b.kind) return a.kind === 'dir' ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
    for (const node of nodes) {
      if (node.kind === 'dir') {
        const map = childMaps.get(node);
        if (map) node.children = [...map.values()];
        finalize(node.children);
      }
    }
  }

  function compact(nodes: FileTreeNode[]): FileTreeNode[] {
    return nodes.map((node) => {
      if (node.kind === 'file') return node;

      let compacted: Extract<FileTreeNode, { kind: 'dir' }> = {
        ...node,
        children: compact(node.children),
      };

      while (
        compacted.children.length === 1 &&
        compacted.children[0].kind === 'dir'
      ) {
        const onlyChild = compacted.children[0];
        compacted = {
          ...onlyChild,
          name: `${compacted.name}/${onlyChild.name}`,
          path: onlyChild.path,
          additions: onlyChild.additions,
          deletions: onlyChild.deletions,
          fileCount: onlyChild.fileCount,
          children: onlyChild.children,
        };
      }

      return compacted;
    });
  }

  const nodes = [...root.values()];
  finalize(nodes);
  return compact(nodes);
}

function FileTreeRow(props: {
  node: FileTreeNode;
  depth: number;
  scope: ReviewDiffScope;
  scopeActive: boolean;
  selectedPath: string | null;
  selectedDirectoryPath: string | null;
  actionBusy: boolean;
  collapsedDirs: Record<string, boolean>;
  onToggleDir: (path: string) => void;
  onSelectDirectory: (path: string) => void;
  onSelectFile: (path: string) => void;
  onStage: (path: string) => Promise<void>;
  onUnstage: (path: string) => Promise<void>;
  onDiscard: (path: string) => void;
  readOnly?: boolean;
}) {
  const {
    node,
    depth,
    scope,
    scopeActive,
    selectedPath,
    selectedDirectoryPath,
    actionBusy,
    collapsedDirs,
    onToggleDir,
    onSelectDirectory,
    onSelectFile,
    onStage,
    onUnstage,
    onDiscard,
    readOnly = false,
  } = props;

  const indent = depth * 13;

  if (node.kind === 'dir') {
    const collapsed = Boolean(collapsedDirs[node.path]);
    const directoryActive = selectedDirectoryPath === node.path;
    const directoryContainsActiveFile = selectedPath ? selectedPath.startsWith(`${node.path}/`) : false;
    return (
      <>
        <Box
          onClick={() => onSelectDirectory(node.path)}
          title={`Select ${node.path}/`}
          style={{
            cursor: 'pointer',
            height: 24,
            display: 'grid',
            gridTemplateColumns: '22px minmax(0, 1fr) auto auto auto',
            gap: 6,
            alignItems: 'center',
            paddingLeft: 6 + indent,
            paddingRight: 6,
            borderRadius: 5,
            color: directoryActive || directoryContainsActiveFile ? 'var(--mantine-color-yellow-1)' : 'var(--mantine-color-dimmed)',
            background: directoryActive ? 'rgba(34, 139, 230, 0.28)' : 'transparent',
            border: directoryActive ? '1px solid rgba(74, 171, 247, 0.88)' : '1px solid transparent',
            boxShadow: directoryActive ? 'inset 3px 0 0 rgba(116, 192, 252, 0.95), 0 0 0 1px rgba(74, 171, 247, 0.18)' : undefined,
          }}
        >
          <ActionIcon
            size="xs"
            variant="subtle"
            aria-label={collapsed ? 'Expand directory' : 'Collapse directory'}
            onClick={(event) => {
              event.stopPropagation();
              onToggleDir(node.path);
            }}
          >
            {collapsed ? '▸' : '▾'}
          </ActionIcon>
          <Text
            size="xs"
            fw={directoryActive ? 900 : directoryContainsActiveFile ? 800 : 700}
            c={directoryActive ? undefined : directoryContainsActiveFile ? 'yellow.0' : undefined}
            truncate
            title={node.path}
          >
            {node.name}/
          </Text>
          <Badge size="xs" variant="light">{node.fileCount}</Badge>
          <Badge size="xs" color="green" variant="light">+{node.additions}</Badge>
          <Badge size="xs" color="red" variant="light">-{node.deletions}</Badge>
        </Box>
        {!collapsed ? node.children.map((child) => (
          <FileTreeRow
            key={child.path}
            node={child}
            depth={depth + 1}
            scope={scope}
            scopeActive={scopeActive}
            selectedPath={selectedPath}
            selectedDirectoryPath={selectedDirectoryPath}
            actionBusy={actionBusy}
            collapsedDirs={collapsedDirs}
            onToggleDir={onToggleDir}
            onSelectDirectory={onSelectDirectory}
            onSelectFile={onSelectFile}
            onStage={onStage}
            onUnstage={onUnstage}
            onDiscard={onDiscard}
            readOnly={readOnly}
          />
        )) : null}
      </>
    );
  }

  const active = selectedPath === node.path;
  const file = node.file;
  return (
    <Box
      onClick={() => onSelectFile(node.path)}
      title={node.path}
      style={{
        cursor: 'pointer',
        height: 26,
        display: 'grid',
        gridTemplateColumns: '28px minmax(0, 1fr) auto auto auto auto',
        gap: 6,
        alignItems: 'center',
        paddingLeft: 6 + indent,
        paddingRight: 6,
        borderRadius: 5,
        background: active ? 'rgba(245, 159, 0, 0.30)' : scopeActive ? 'rgba(34, 139, 230, 0.045)' : 'transparent',
        border: active ? '1px solid rgba(255, 212, 59, 0.88)' : scopeActive ? '1px solid rgba(74, 171, 247, 0.08)' : '1px solid transparent',
        boxShadow: active ? 'inset 3px 0 0 rgba(255, 212, 59, 0.95)' : undefined,
      }}
    >
      <Badge size="xs" variant="outline" style={{ minWidth: 24 }}>{statusCode(file)}</Badge>
      <Text size="xs" fw={active ? 900 : scopeActive ? 700 : 600} c={active ? 'yellow.0' : undefined} truncate>{node.name}</Text>
      <Text size="xs" c="green" fw={700}>+{node.additions}</Text>
      <Text size="xs" c="red" fw={700}>-{node.deletions}</Text>
      {readOnly ? null : scope === 'unstaged' ? (
        <Group gap={2} wrap="nowrap">
          <Tooltip label="Stage file" withArrow>
            <ActionIcon
              size="sm"
              variant="outline"
              color="blue"
              loading={actionBusy}
              style={{ width: 24, height: 22, minWidth: 24 }}
              aria-label="Stage file"
              onClick={(event) => {
                event.stopPropagation();
                void onStage(node.path);
              }}
            >
              <IconPlus size={14} />
            </ActionIcon>
          </Tooltip>
          <Tooltip label="Discard file changes" withArrow>
            <ActionIcon
              size="sm"
              variant="outline"
              color="red"
              loading={actionBusy}
              style={{ width: 24, height: 22, minWidth: 24 }}
              aria-label="Discard file changes"
              onClick={(event) => {
                event.stopPropagation();
                onDiscard(node.path);
              }}
            >
              <IconTrash size={14} />
            </ActionIcon>
          </Tooltip>
        </Group>
      ) : (
        <Tooltip label="Unstage file" withArrow>
          <ActionIcon
            size="sm"
            variant="outline"
            color="blue"
            loading={actionBusy}
            style={{ width: 24, height: 22, minWidth: 24 }}
            aria-label="Unstage file"
            onClick={(event) => {
              event.stopPropagation();
              void onUnstage(node.path);
            }}
          >
            <IconMinus size={14} />
          </ActionIcon>
        </Tooltip>
      )}
    </Box>
  );
}

function FileTree(props: {
  nodes: FileTreeNode[];
  scope: ReviewDiffScope;
  scopeActive: boolean;
  selectedPath: string | null;
  selectedDirectoryPath: string | null;
  actionBusy: boolean;
  collapsedDirs: Record<string, boolean>;
  onToggleDir: (path: string) => void;
  onSelectDirectory: (path: string) => void;
  onSelectFile: (path: string) => void;
  onStage: (path: string) => Promise<void>;
  onUnstage: (path: string) => Promise<void>;
  onDiscard: (path: string) => void;
  readOnly?: boolean;
}) {
  const { nodes, ...rest } = props;
  return (
    <Stack gap={1}>
      {nodes.map((node) => (
        <FileTreeRow key={node.path} node={node} depth={0} {...rest} />
      ))}
    </Stack>
  );
}

export function DiffPanel(props: DiffPanelProps) {
  const { runId, repoRef, state, onPersistState, forceViewerOpen = false, mode = 'worktree' } = props;
  const commitMode = mode === 'commit';
  const [statusBusy, setStatusBusy] = useState(false);
  const [diffBusy, setDiffBusy] = useState(false);
  const [actionBusy, setActionBusy] = useState(false);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [diffError, setDiffError] = useState<string | null>(null);
  const [stagedFiles, setStagedFiles] = useState<ReviewStatusFileEntry[]>([]);
  const [unstagedFiles, setUnstagedFiles] = useState<ReviewStatusFileEntry[]>([]);
  const [branchSummary, setBranchSummary] = useState<string>('');
  const [diff, setDiff] = useState<ReviewDiffResponse | ReviewCommitDiffResponse | null>(null);
  const [viewerOpen, setViewerOpen] = useState(false);
  const [resizing, setResizing] = useState(false);
  const [sidebarHidden, setSidebarHidden] = useState(false);
  const [sidebarWidth, setSidebarWidth] = useState(DEFAULT_SIDEBAR_WIDTH);
  const [sidebarWidthDraft, setSidebarWidthDraft] = useState<number | null>(null);
  const resizeFrame = useRef<number | null>(null);
  const refreshDiffRequestIdRef = useRef(0);
  const [diffManifest, setDiffManifest] = useState<ReviewDiffManifestResponse | ReviewCommitDiffManifestResponse | null>(null);
  const [filePatchByPath, setFilePatchByPath] = useState<Record<string, string>>({});
  const [filePatchBusyByPath, setFilePatchBusyByPath] = useState<Record<string, boolean>>({});
  const [collapsedByPath, setCollapsedByPath] = useState<Record<string, boolean>>({});
  const [collapsedTreeDirs, setCollapsedTreeDirs] = useState<Record<string, boolean>>({});
  const [activeScrollPath, setActiveScrollPath] = useState<string | null>(null);
  const [selectedDirectoryPath, setSelectedDirectoryPath] = useState<string | null>(null);
  const selectedDirectoryPathRef = useRef<string | null>(null);
  const [discardTarget, setDiscardTarget] = useState<{ path: string | null; label: string; paths: string[] } | null>(null);
  const [unstageAllTarget, setUnstageAllTarget] = useState<{ label: string; paths: string[] } | null>(null);
  const viewerGridRef = useRef<HTMLDivElement | null>(null);
  const liveSidebarWidthRef = useRef(DEFAULT_SIDEBAR_WIDTH);

  const stagedTotals = useMemo(() => sumCounts(stagedFiles), [stagedFiles]);
  const unstagedTotals = useMemo(() => sumCounts(unstagedFiles), [unstagedFiles]);
  const allTotals = useMemo(() => sumCounts([...stagedFiles, ...unstagedFiles]), [stagedFiles, unstagedFiles]);
  const stagedTree = useMemo(() => buildFileTree(stagedFiles), [stagedFiles]);
  const unstagedTree = useMemo(() => buildFileTree(unstagedFiles), [unstagedFiles]);
  const selectedScopeFiles = useMemo(
    () => diffManifest?.files ?? (state.selected_scope === 'staged' ? stagedFiles : unstagedFiles),
    [diffManifest, state.selected_scope, stagedFiles, unstagedFiles]
  );
  const selectedFile = state.selected_path
    ? selectedScopeFiles.find((file) => file.path === state.selected_path) ?? null
    : null;
  const selectedTitle = selectedFile?.path ?? (commitMode ? props.commitTitle || 'Commit diff' : state.selected_scope === 'staged' ? 'Staged diff' : 'Unstaged diff');
  const selectedScopeTotals = useMemo(() => sumCounts(selectedScopeFiles), [selectedScopeFiles]);
  const selectedTotals = selectedFile
    ? { additions: selectedFile.additions, deletions: selectedFile.deletions }
    : commitMode
      ? selectedScopeTotals
      : state.selected_scope === 'staged'
        ? stagedTotals
        : unstagedTotals;
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

    return inspectUnifiedPatch(diff.patch, state.selected_path);
  }, [state.selected_path, diff?.patch]);

  const scopeDiffRows = useMemo(() => {
    return (diffManifest?.files ?? []).map((file) => ({
      file,
      status: selectedScopeCountsByPath[file.path] ?? null,
      patch: filePatchByPath[file.path] ?? '',
    }));
  }, [diffManifest, filePatchByPath, selectedScopeCountsByPath]);

  const visibleScopeDiffRows = useMemo(() => {
    if (!selectedDirectoryPath) return scopeDiffRows;
    return scopeDiffRows.filter(({ file }) => file.path.startsWith(`${selectedDirectoryPath}/`));
  }, [scopeDiffRows, selectedDirectoryPath]);

  const groupedScopePatch = useMemo(() => {
    return visibleScopeDiffRows
      .filter(({ patch }) => patch.trim())
      .map(({ patch }) => patch.trimEnd())
      .join('\n');
  }, [visibleScopeDiffRows]);

  const groupedScopePatchLoading = useMemo(() => {
    return visibleScopeDiffRows.some(({ file }) => Boolean(filePatchBusyByPath[file.path]));
  }, [visibleScopeDiffRows, filePatchBusyByPath]);

  const hasScopeDiffRows = visibleScopeDiffRows.length > 0;
  const allScopeRowsCollapsed = hasScopeDiffRows && visibleScopeDiffRows.every(({ file }) => collapsedByPath[file.path] !== false);

  useEffect(() => {
    if (state.selected_path !== null) return;
    const firstVisiblePath = visibleScopeDiffRows.find(({ patch }) => patch.trim())?.file.path ?? visibleScopeDiffRows[0]?.file.path ?? null;
    if (activeScrollPath === firstVisiblePath) return;
    if (activeScrollPath && visibleScopeDiffRows.some(({ file }) => file.path === activeScrollPath)) return;
    setActiveScrollPath(firstVisiblePath);
  }, [state.selected_path, visibleScopeDiffRows, activeScrollPath]);

  function setAllScopeRowsCollapsed(collapsed: boolean) {
    setCollapsedByPath(
      Object.fromEntries(visibleScopeDiffRows.map(({ file }) => [file.path, collapsed]))
    );
  }

  function toggleScopeRowCollapsed(path: string) {
    setCollapsedByPath((current) => ({
      ...current,
      [path]: !(current[path] ?? false),
    }));
  }

  function toggleTreeDir(path: string) {
    setCollapsedTreeDirs((current) => ({
      ...current,
      [path]: !current[path],
    }));
  }

  function selectScopeFile(scope: ReviewDiffScope, path: string) {
    selectedDirectoryPathRef.current = null;
    setSelectedDirectoryPath(null);
    setActiveScrollPath(path);
    void patchState({ selected_scope: scope, selected_path: path });
  }

  function selectScopeDirectory(scope: ReviewDiffScope, path: string) {
    selectedDirectoryPathRef.current = path;
    setSelectedDirectoryPath(path);
    setActiveScrollPath(null);
    void patchState({ selected_scope: scope, selected_path: null });
  }

  function selectWholeScope(scope: ReviewDiffScope) {
    selectedDirectoryPathRef.current = null;
    setSelectedDirectoryPath(null);
    setActiveScrollPath(null);
    void patchState({ selected_scope: scope, selected_path: null });
  }

  const singleFileRenderedLineCount = useMemo(() => {
    return selectedFilePayloadInfo.selectedUnifiedLineCount;
  }, [selectedFilePayloadInfo]);

  async function refreshStatus() {
    if (commitMode) return;
    if (!repoRef.trim()) return;
    try {
      setStatusBusy(true);
      setStatusError(null);
      const json = await getReviewStatus(repoRef);
      setStagedFiles(json.staged);
      setUnstagedFiles(json.unstaged);
      setCollapsedTreeDirs({});
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
    nextState: DiffPanelState,
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

  async function refreshCommitFilePatches(
    nextState: DiffPanelState,
    files: ReviewDiffManifestFileEntry[],
    requestId: number
  ) {
    const commitSha = props.commitSha?.trim() ?? '';
    if (!repoRef.trim() || !commitSha || nextState.selected_path || files.length === 0) {
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
          const json = await getReviewCommitDiff({
            repo_ref: repoRef,
            commit: commitSha,
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

  async function refreshDiff(nextState: DiffPanelState) {
    if (!repoRef.trim() || !viewerOpen) return;

    const requestId = ++refreshDiffRequestIdRef.current;

    try {
      setDiffBusy(true);
      setDiffError(null);

      if (commitMode) {
        const commitSha = props.commitSha?.trim() ?? '';
        if (!commitSha) {
          setDiffManifest(null);
          setDiff(null);
          setFilePatchByPath({});
          setFilePatchBusyByPath({});
          setCollapsedByPath({});
          setStagedFiles([]);
          setUnstagedFiles([]);
          setDiffError('Commit SHA is required.');
          return;
        }

        const manifest = await getReviewCommitDiffManifest({
          repo_ref: repoRef,
          commit: commitSha,
        });
        if (refreshDiffRequestIdRef.current !== requestId) {
          return;
        }
        const directoryPath = nextState.selected_path ? null : selectedDirectoryPathRef.current;
        const visibleFiles = directoryPath
          ? manifest.files.filter((file) => file.path.startsWith(`${directoryPath}/`))
          : manifest.files;
        setDiffManifest(manifest);
        setStagedFiles(manifest.files);
        setUnstagedFiles([]);
        setBranchSummary('');

        if (nextState.selected_path) {
          const json = await getReviewCommitDiff({
            repo_ref: repoRef,
            commit: commitSha,
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
        await refreshCommitFilePatches(nextState, visibleFiles, requestId);
        return;
      }

      const manifest = await getReviewDiffManifest({
        repo_ref: repoRef,
        scope: nextState.selected_scope,
      });
      if (refreshDiffRequestIdRef.current !== requestId) {
        return;
      }
      const directoryPath = nextState.selected_path ? null : selectedDirectoryPathRef.current;
      const visibleFiles = directoryPath
        ? manifest.files.filter((file) => file.path.startsWith(`${directoryPath}/`))
        : manifest.files;
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
      await refreshScopeFilePatches(nextState, visibleFiles, requestId);
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

  async function patchState(patch: Partial<DiffPanelState>) {
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

  function requestDiscard(path: string | null) {
    const paths = path ? [path] : unstagedFiles.map((file) => file.path);
    setDiscardTarget({
      path,
      label: `${paths.length} unstaged file${paths.length === 1 ? '' : 's'}`,
      paths,
    });
  }

  async function confirmDiscard() {
    if (!repoRef.trim() || !runId || !discardTarget) return;
    const path = discardTarget.path;
    try {
      setActionBusy(true);
      setStatusError(null);
      await discardWorkflowReviewDiff(runId, { scope: 'unstaged', path });
      const nextState = {
        ...state,
        selected_scope: 'unstaged' as ReviewDiffScope,
        selected_path: state.selected_path === path || path === null ? null : state.selected_path,
      };
      setDiscardTarget(null);
      await onPersistState(nextState);
      await refreshStatus();
      await refreshDiff(nextState);
    } catch (err) {
      setStatusError(err instanceof Error ? err.message : String(err));
    } finally {
      setActionBusy(false);
    }
  }

  function requestUnstageAll() {
    const paths = stagedFiles.map((file) => file.path);
    setUnstageAllTarget({
      label: `${paths.length} staged file${paths.length === 1 ? '' : 's'}`,
      paths,
    });
  }

  async function confirmUnstageAll() {
    if (!repoRef.trim() || !unstageAllTarget) return;
    try {
      setActionBusy(true);
      setStatusError(null);
      await unstageReviewDiff({ repo_ref: repoRef, scope: 'staged', path: null });
      const nextState = {
        ...state,
        selected_scope: 'unstaged' as ReviewDiffScope,
        selected_path: null,
      };
      setUnstageAllTarget(null);
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
  }, [repoRef, viewerOpen, state.selected_scope, state.selected_path, state.context_lines, state.whole_file, mode, props.commitSha]);


  useEffect(() => {
    if (!viewerOpen) {
      return;
    }
    setSidebarHidden(false);
    setSidebarWidth(DEFAULT_SIDEBAR_WIDTH);
    setSidebarWidthDraft(null);
    liveSidebarWidthRef.current = DEFAULT_SIDEBAR_WIDTH;
    viewerGridRef.current?.style.setProperty('--diff-sidebar-width', `${DEFAULT_SIDEBAR_WIDTH}px`);
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
        const width = clampSidebarWidth(window.innerWidth - event.clientX);
        liveSidebarWidthRef.current = width;
        viewerGridRef.current?.style.setProperty('--diff-sidebar-width', `${width}px`);
      });
    }

    function handleUp() {
      setResizing(false);
      if (resizeFrame.current) {
        cancelAnimationFrame(resizeFrame.current);
        resizeFrame.current = null;
      }

      const committedWidth = clampSidebarWidth(liveSidebarWidthRef.current || sidebarWidth);
      setSidebarWidthDraft(null);
      setSidebarWidth(committedWidth);
      viewerGridRef.current?.style.setProperty('--diff-sidebar-width', `${committedWidth}px`);
    }

    window.addEventListener('mousemove', handleMove);
    window.addEventListener('mouseup', handleUp);
    return () => {
      window.removeEventListener('mousemove', handleMove);
      window.removeEventListener('mouseup', handleUp);
    };
  }, [resizing, sidebarWidthDraft, sidebarWidth]);

  const effectiveSidebarWidth = clampSidebarWidth(sidebarWidth);
  const showSidebar = !sidebarHidden;
  const compactCounts = effectiveSidebarWidth < 340;
  const highlightedTreePath = state.selected_path ?? activeScrollPath;
  const highlightedDirectoryPath = state.selected_path === null ? selectedDirectoryPath : null;

  useEffect(() => {
    if (forceViewerOpen && !viewerOpen) {
      setViewerOpen(true);
    }
  }, [forceViewerOpen, viewerOpen]);

  const viewerContent = (
    <Box
      ref={viewerGridRef}
      style={{
        '--diff-sidebar-width': `${effectiveSidebarWidth}px`,
        height: '100%',
        display: 'grid',
        gridTemplateColumns: showSidebar ? 'minmax(0, 1fr) 8px var(--diff-sidebar-width)' : '1fr',
      } as React.CSSProperties}
    >
      <Box p={6} style={{ minHeight: 0, overflow: 'hidden', display: 'flex', flexDirection: 'column' }}>
        <Card withBorder p={6} style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
          <Group justify="space-between" align="center" mb={4} wrap="nowrap">
            <Group gap={6} wrap="nowrap" style={{ minWidth: 0, flex: 1 }}>
              <Button size="xs" variant="default" onClick={() => commitMode ? void refreshDiff(state) : void refreshStatus()} loading={commitMode ? diffBusy : statusBusy}>Refresh</Button>
              <Button size="xs" variant="default" onClick={() => setSidebarHidden((value) => !value)}>
                {showSidebar ? 'Hide diff browser' : 'Show diff browser'}
              </Button>
              <Text fw={600} size="sm" truncate>{selectedTitle}</Text>
              {commitMode && props.commitSubtitle ? <Badge size="xs" variant="outline">{props.commitSubtitle}</Badge> : null}
              {!state.selected_path && scopeDiffRows.length > 0 ? (
                <Group gap={6} wrap="nowrap">
                  <Text size="xs" c="dimmed">Virtualized multi-file patch diff</Text>
                  <Badge size="xs" variant="light">
                    {scopeDiffRows.filter(({ file }) => collapsedByPath[file.path] === false).length}/{scopeDiffRows.length} files expanded
                  </Badge>
                  {groupedScopePatchLoading ? <Badge size="xs" variant="light">Loading patches…</Badge> : null}
                </Group>
              ) : null}
            </Group>
            <Group gap={6} wrap="nowrap">
              {!state.selected_path && scopeDiffRows.length > 0 ? (
                <>
                  <Button size="xs" variant="default" onClick={() => setAllScopeRowsCollapsed(false)}>Expand all</Button>
                  <Button size="xs" variant="default" onClick={() => setAllScopeRowsCollapsed(true)}>Collapse all</Button>
                </>
              ) : null}
              {state.selected_path ? (
                diff ? <Badge variant="light">{diff.from_ref} → {diff.to_ref}</Badge> : null
              ) : diffManifest ? (
                <Badge variant="light">{diffManifest.from_ref} → {diffManifest.to_ref}</Badge>
              ) : null}
              {branchSummary ? <Badge variant="light">{branchSummary}</Badge> : null}
              <Badge variant="light">{commitMode ? 'Commit' : state.selected_scope === 'staged' ? 'Staged' : 'Unstaged'}</Badge>
              <Badge color="green" variant="light">+{selectedTotals.additions}</Badge>
              <Badge color="red" variant="light">-{selectedTotals.deletions}</Badge>
            </Group>
          </Group>
          <Box style={{ flex: 1, minHeight: 0, overflow: 'hidden' }}>
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
                    <DiffCode patch={selectedFilePatch} diffStyle={state.diff_style} />
                  </Box>
                </ScrollArea>
              ) : (
                <Text c="dimmed" size="sm">No diff available.</Text>
              )
            ) : scopeDiffRows.length > 0 ? (
              <Box style={{ height: '100%', minHeight: 0, display: 'flex', flexDirection: 'column' }}>
                {groupedScopePatch.trim() ? (
                  <Box style={{ flex: 1, minHeight: 0 }}>
                    <DiffCode
                      patch={groupedScopePatch}
                      diffStyle={state.diff_style}
                      collapsedPaths={collapsedByPath}
                      onToggleFile={toggleScopeRowCollapsed}
                      onActiveFileChange={(path) => {
                        if (state.selected_path === null) {
                          setActiveScrollPath(path);
                        }
                      }}
                    />
                  </Box>
                ) : groupedScopePatchLoading ? (
                  <Group justify="center" py="xl"><Loader size="sm" /></Group>
                ) : (
                  <Text size="sm" c="dimmed">No expanded file diffs available.</Text>
                )}
              </Box>
            ) : (
              <Text c="dimmed" size="sm">No diff available.</Text>
            )}
          </Box>
        </Card>
      </Box>

      {showSidebar ? (
        <Box
          onMouseDown={() => {
            const width = clampSidebarWidth(sidebarWidth);
            liveSidebarWidthRef.current = width;
            viewerGridRef.current?.style.setProperty('--diff-sidebar-width', `${width}px`);
            setSidebarWidthDraft(width);
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
                <Checkbox
                  label="Only show changes"
                  checked={state.only_changes && !state.whole_file}
                  onChange={(event) => void patchState({
                    only_changes: event.currentTarget.checked,
                    whole_file: event.currentTarget.checked ? false : state.whole_file,
                  })}
                />
                <Checkbox
                  label="Whole file"
                  checked={state.whole_file}
                  onChange={(event) => void patchState({
                    whole_file: event.currentTarget.checked,
                    only_changes: event.currentTarget.checked ? false : state.only_changes,
                  })}
                />
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
              <Stack gap="md" pr="xs">
                <Card withBorder p={6}>
                  <Stack gap={4}>
                    <ScopeHeader
                      title={commitMode ? 'Commit' : 'Staged'}
                      active={state.selected_scope === 'staged' && state.selected_path === null && selectedDirectoryPath === null}
                      fileCount={stagedFiles.length}
                      additions={stagedTotals.additions}
                      deletions={stagedTotals.deletions}
                      compactCounts={compactCounts}
                      buttonLabel={commitMode ? undefined : '−'}
                      buttonTooltip={commitMode ? undefined : 'Unstage all'}
                      actionBusy={actionBusy}
                      onSelect={() => selectWholeScope('staged')}
                      onAction={commitMode ? async () => undefined : async () => requestUnstageAll()}
                    />
                    {stagedFiles.length > 0 ? (
                      <FileTree
                        nodes={stagedTree}
                        scope="staged"
                        scopeActive={state.selected_scope === 'staged' && state.selected_path === null && selectedDirectoryPath === null}
                        selectedPath={state.selected_scope === 'staged' ? highlightedTreePath : null}
                        selectedDirectoryPath={state.selected_scope === 'staged' ? highlightedDirectoryPath : null}
                        actionBusy={actionBusy}
                        collapsedDirs={collapsedTreeDirs}
                        onToggleDir={toggleTreeDir}
                        onSelectDirectory={(path) => selectScopeDirectory('staged', path)}
                        onSelectFile={(path) => selectScopeFile('staged', path)}
                        onStage={(path) => runStageAction('stage', 'staged', path)}
                        onUnstage={(path) => runStageAction('unstage', 'staged', path)}
                        onDiscard={requestDiscard}
                        readOnly={commitMode}
                      />
                    ) : (
                      <Text c="dimmed" size="xs" px="xs" py={4}>No staged files.</Text>
                    )}
                  </Stack>
                </Card>

                {!commitMode ? (
                <Card withBorder p={6}>
                  <Stack gap={4}>
                    <ScopeHeader
                      title="Unstaged"
                      active={state.selected_scope === 'unstaged' && state.selected_path === null && selectedDirectoryPath === null}
                      fileCount={unstagedFiles.length}
                      additions={unstagedTotals.additions}
                      deletions={unstagedTotals.deletions}
                      compactCounts={compactCounts}
                      buttonLabel="+"
                      buttonTooltip="Stage all"
                      actionBusy={actionBusy}
                      extraAction={unstagedFiles.length > 0 ? (
                        <Tooltip label="Discard all unstaged changes" withArrow>
                          <ActionIcon
                            size="sm"
                            variant="outline"
                            color="red"
                            aria-label="Discard all unstaged changes"
                            style={{ width: 24, height: 22, minWidth: 24 }}
                            loading={actionBusy}
                            onClick={() => requestDiscard(null)}
                          >
                            <IconTrash size={14} />
                          </ActionIcon>
                        </Tooltip>
                      ) : null}
                      onSelect={() => selectWholeScope('unstaged')}
                      onAction={() => runStageAction('stage', 'unstaged', null)}
                    />

                    {unstagedFiles.length > 0 ? (
                      <FileTree
                        nodes={unstagedTree}
                        scope="unstaged"
                        scopeActive={state.selected_scope === 'unstaged' && state.selected_path === null && selectedDirectoryPath === null}
                        selectedPath={state.selected_scope === 'unstaged' ? highlightedTreePath : null}
                        selectedDirectoryPath={state.selected_scope === 'unstaged' ? highlightedDirectoryPath : null}
                        actionBusy={actionBusy}
                        collapsedDirs={collapsedTreeDirs}
                        onToggleDir={toggleTreeDir}
                        onSelectDirectory={(path) => selectScopeDirectory('unstaged', path)}
                        onSelectFile={(path) => selectScopeFile('unstaged', path)}
                        onStage={(path) => runStageAction('stage', 'unstaged', path)}
                        onUnstage={(path) => runStageAction('unstage', 'unstaged', path)}
                        onDiscard={requestDiscard}
                        readOnly={commitMode}
                      />
                    ) : (
                      <Text c="dimmed" size="xs" px="xs" py={4}>No unstaged files.</Text>
                    )}
                  </Stack>
                </Card>
                ) : null}
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

      <Modal
        opened={Boolean(discardTarget)}
        onClose={() => setDiscardTarget(null)}
        title="Discard unstaged changes?"
        centered
      >
        <Stack gap="sm">
          <Text size="sm">
            This will permanently discard local unstaged changes for {discardTarget?.label ?? 'the selected files'}:
          </Text>
          <ScrollArea.Autosize mah={220} type="auto">
            <Stack gap={4}>
              {(discardTarget?.paths ?? []).map((path) => (
                <Text key={path} size="xs" ff="monospace" style={{ wordBreak: 'break-all' }}>
                  {path}
                </Text>
              ))}
            </Stack>
          </ScrollArea.Autosize>
          <Group justify="flex-end">
            <Button variant="default" onClick={() => setDiscardTarget(null)} disabled={actionBusy}>Cancel</Button>
            <Button color="red" onClick={() => void confirmDiscard()} loading={actionBusy}>Discard</Button>
          </Group>
        </Stack>
      </Modal>

      <Modal
        opened={Boolean(unstageAllTarget)}
        onClose={() => setUnstageAllTarget(null)}
        title="Unstage all changes?"
        centered
      >
        <Stack gap="sm">
          <Text size="sm">
            This will move staged changes back to unstaged for {unstageAllTarget?.label ?? 'the selected files'}:
          </Text>
          <ScrollArea.Autosize mah={220} type="auto">
            <Stack gap={4}>
              {(unstageAllTarget?.paths ?? []).map((path) => (
                <Text key={path} size="xs" ff="monospace" style={{ wordBreak: 'break-all' }}>
                  {path}
                </Text>
              ))}
            </Stack>
          </ScrollArea.Autosize>
          <Group justify="flex-end">
            <Button variant="default" onClick={() => setUnstageAllTarget(null)} disabled={actionBusy}>Cancel</Button>
            <Button color="blue" onClick={() => void confirmUnstageAll()} loading={actionBusy}>Unstage all</Button>
          </Group>
        </Stack>
      </Modal>

      {forceViewerOpen ? (
        <Box style={{ height: 'calc(100dvh - 88px)', minHeight: 0, overflow: 'hidden' }}>
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
