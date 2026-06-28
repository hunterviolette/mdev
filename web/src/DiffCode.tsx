import { useEffect, useMemo, useRef, useState } from 'react';
import { Badge, Box, Button, Group, Loader, Progress, Text } from '@mantine/core';
import { createHighlighter } from 'shiki';

export type DiffCodeStyle = 'unified' | 'split';

type PatchLineKind = 'context' | 'add' | 'delete' | 'hunk';

type PatchLine = {
  kind: PatchLineKind;
  text: string;
  oldLine?: number;
  newLine?: number;
};

type ParsedPatchFile = {
  path: string;
  oldPath: string;
  newPath: string;
  additions: number;
  deletions: number;
  unifiedLineCount: number;
  lines: PatchLine[];
};

type PatchInspection = {
  fileCount: number;
  containsSelectedFile: boolean;
  isExactSelectedFilePayload: boolean;
  selectedUnifiedLineCount: number;
};

type Token = {
  content: string;
  color?: string;
  fontStyle?: number;
};

type RenderRow =
  | {
      kind: 'file';
      path: string;
      additions: number;
      deletions: number;
      collapsed?: boolean;
    }
  | {
      kind: 'hunk';
      text: string;
    }
  | {
      kind: 'unified-line';
      lineKind: 'context' | 'add' | 'delete';
      oldLine?: number;
      newLine?: number;
      text: string;
      tokens?: Token[];
    }
  | {
      kind: 'split-line';
      lineKind: 'context' | 'add' | 'delete' | 'change';
      oldLine?: number;
      newLine?: number;
      oldText?: string;
      newText?: string;
      oldTokens?: Token[];
      newTokens?: Token[];
    };

const HUNK_RE = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;
const ROW_HEIGHT = 20;
const OVERSCAN_ROWS = 40;
const HIGHLIGHT_BATCH_SIZE = 160;
const highlighterByLanguage = new Map<string, Promise<any>>();

function stripGitPrefix(path: string): string {
  return path.replace(/^a\//, '').replace(/^b\//, '');
}

function pathFromDiffGit(line: string): { oldPath: string; newPath: string; path: string } {
  const match = line.match(/^diff --git\s+a\/(.*?)\s+b\/(.*)$/);
  const oldPath = match?.[1] ?? '';
  const newPath = match?.[2] ?? oldPath;
  return { oldPath, newPath, path: newPath || oldPath };
}

function languageForPath(path: string): string {
  const lower = path.toLowerCase();
  if (lower.endsWith('.ts') || lower.endsWith('.tsx')) return 'tsx';
  if (lower.endsWith('.js') || lower.endsWith('.jsx')) return 'jsx';
  if (lower.endsWith('.rs')) return 'rust';
  if (lower.endsWith('.json')) return 'json';
  if (lower.endsWith('.css')) return 'css';
  if (lower.endsWith('.html')) return 'html';
  if (lower.endsWith('.md')) return 'markdown';
  if (lower.endsWith('.sql')) return 'sql';
  if (lower.endsWith('.toml')) return 'toml';
  if (lower.endsWith('.yml') || lower.endsWith('.yaml')) return 'yaml';
  return 'text';
}

function isPatchMetadataLine(rawLine: string) {
  return (
    rawLine.startsWith('index ') ||
    rawLine.startsWith('new file mode ') ||
    rawLine.startsWith('deleted file mode ') ||
    rawLine.startsWith('similarity index ') ||
    rawLine.startsWith('rename from ') ||
    rawLine.startsWith('rename to ') ||
    rawLine.startsWith('\\ No newline')
  );
}

export function parseUnifiedPatch(patch: string): ParsedPatchFile[] {
  const files: ParsedPatchFile[] = [];
  let current: ParsedPatchFile | null = null;
  let oldLine = 0;
  let newLine = 0;

  for (const rawLine of patch.replace(/\r\n/g, '\n').split('\n')) {
    if (rawLine.startsWith('diff --git ')) {
      const info = pathFromDiffGit(rawLine);
      current = {
        path: info.path,
        oldPath: info.oldPath,
        newPath: info.newPath,
        additions: 0,
        deletions: 0,
        unifiedLineCount: 0,
        lines: [],
      };
      files.push(current);
      oldLine = 0;
      newLine = 0;
      continue;
    }

    if (!current) {
      if (!rawLine.trim()) continue;
      current = {
        path: '(patch)',
        oldPath: '(patch)',
        newPath: '(patch)',
        additions: 0,
        deletions: 0,
        unifiedLineCount: 0,
        lines: [],
      };
      files.push(current);
    }

    if (isPatchMetadataLine(rawLine)) continue;

    if (rawLine.startsWith('--- ')) {
      current.oldPath = stripGitPrefix(rawLine.slice(4).trim());
      continue;
    }

    if (rawLine.startsWith('+++ ')) {
      current.newPath = stripGitPrefix(rawLine.slice(4).trim());
      current.path = current.newPath || current.oldPath || current.path;
      continue;
    }

    const hunk = rawLine.match(HUNK_RE);
    if (hunk) {
      oldLine = Number(hunk[1]);
      newLine = Number(hunk[2]);
      current.lines.push({ kind: 'hunk', text: rawLine });
      continue;
    }

    if (rawLine.startsWith('+')) {
      current.lines.push({ kind: 'add', text: rawLine.slice(1), newLine });
      current.additions += 1;
      current.unifiedLineCount += 1;
      newLine += 1;
      continue;
    }

    if (rawLine.startsWith('-')) {
      current.lines.push({ kind: 'delete', text: rawLine.slice(1), oldLine });
      current.deletions += 1;
      current.unifiedLineCount += 1;
      oldLine += 1;
      continue;
    }

    if (rawLine.startsWith(' ')) {
      current.lines.push({ kind: 'context', text: rawLine.slice(1), oldLine, newLine });
      current.unifiedLineCount += 1;
      oldLine += 1;
      newLine += 1;
    }
  }

  return files.filter((file) => file.lines.length > 0);
}

export function inspectUnifiedPatch(patch: string, selectedPath: string | null | undefined): PatchInspection {
  const files = parseUnifiedPatch(patch);
  const selected = selectedPath
    ? files.find((file) => file.path === selectedPath || file.oldPath === selectedPath || file.newPath === selectedPath) ?? null
    : null;

  return {
    fileCount: files.length,
    containsSelectedFile: selectedPath ? Boolean(selected) : files.length > 0,
    isExactSelectedFilePayload: files.length === 1 && (!selectedPath || Boolean(selected)),
    selectedUnifiedLineCount: selected?.unifiedLineCount ?? files[0]?.unifiedLineCount ?? 0,
  };
}

function getHighlighter(path: string) {
  const lang = languageForPath(path);
  let promise = highlighterByLanguage.get(lang);
  if (!promise) {
    promise = createHighlighter({ themes: ['github-dark'], langs: [lang] });
    highlighterByLanguage.set(lang, promise);
  }
  return { lang, promise };
}

function waitForNextFrame() {
  return new Promise<void>((resolve) => window.requestAnimationFrame(() => resolve()));
}

function tokenStyle(token: Token): React.CSSProperties {
  return {
    color: token.color,
    fontStyle: token.fontStyle === 1 || token.fontStyle === 3 ? 'italic' : undefined,
    fontWeight: token.fontStyle === 2 || token.fontStyle === 3 ? 700 : undefined,
  };
}

function HighlightedText(props: { text: string; tokens?: Token[] }) {
  const { text, tokens } = props;
  if (!tokens || tokens.length === 0) return <>{text || ' '}</>;

  return (
    <>
      {tokens.map((token, index) => (
        <span key={index} style={tokenStyle(token)}>{token.content}</span>
      ))}
    </>
  );
}

function lineBackground(kind: 'context' | 'add' | 'delete' | 'change', side?: 'old' | 'new') {
  if (kind === 'add' && side !== 'old') return 'rgba(47, 158, 68, 0.14)';
  if (kind === 'delete' && side !== 'new') return 'rgba(224, 49, 49, 0.14)';
  if (kind === 'change') {
    if (side === 'old') return 'rgba(224, 49, 49, 0.14)';
    if (side === 'new') return 'rgba(47, 158, 68, 0.14)';
  }
  return 'transparent';
}

function tokenLine(highlighter: any, lang: string, text: string): Token[] {
  const tokens = highlighter.codeToTokens(text || ' ', { lang, theme: 'github-dark' });
  return tokens.tokens?.[0] ?? [];
}

function buildSplitPreparedRows(file: ParsedPatchFile, highlightedByIndex: Record<number, Token[]>): RenderRow[] {
  const rows: RenderRow[] = [];
  let pendingDeletes: Array<{ line: PatchLine; index: number }> = [];

  file.lines.forEach((line, index) => {
    if (line.kind === 'hunk') {
      while (pendingDeletes.length > 0) {
        const pending = pendingDeletes.shift();
        rows.push({
          kind: 'split-line',
          lineKind: 'delete',
          oldLine: pending?.line.oldLine,
          oldText: pending?.line.text,
          oldTokens: pending ? highlightedByIndex[pending.index] : undefined,
        });
      }
      return;
    }

    if (line.kind === 'delete') {
      pendingDeletes.push({ line, index });
      return;
    }

    if (line.kind === 'add') {
      const paired = pendingDeletes.shift();
      rows.push({
        kind: 'split-line',
        lineKind: paired ? 'change' : 'add',
        oldLine: paired?.line.oldLine,
        newLine: line.newLine,
        oldText: paired?.line.text,
        newText: line.text,
        oldTokens: paired ? highlightedByIndex[paired.index] : undefined,
        newTokens: highlightedByIndex[index],
      });
      return;
    }

    while (pendingDeletes.length > 0) {
      const pending = pendingDeletes.shift();
      rows.push({
        kind: 'split-line',
        lineKind: 'delete',
        oldLine: pending?.line.oldLine,
        oldText: pending?.line.text,
        oldTokens: pending ? highlightedByIndex[pending.index] : undefined,
      });
    }

    rows.push({
      kind: 'split-line',
      lineKind: 'context',
      oldLine: line.oldLine,
      newLine: line.newLine,
      oldText: line.text,
      newText: line.text,
      oldTokens: highlightedByIndex[index],
      newTokens: highlightedByIndex[index],
    });
  });

  while (pendingDeletes.length > 0) {
    const pending = pendingDeletes.shift();
    rows.push({
      kind: 'split-line',
      lineKind: 'delete',
      oldLine: pending?.line.oldLine,
      oldText: pending?.line.text,
      oldTokens: pending ? highlightedByIndex[pending.index] : undefined,
    });
  }

  return rows;
}

function buildUnifiedPreparedRows(file: ParsedPatchFile, highlightedByIndex: Record<number, Token[]>): RenderRow[] {
  return file.lines.flatMap((line, index): RenderRow[] => {
    if (line.kind === 'hunk') return [];
    return [{
      kind: 'unified-line',
      lineKind: line.kind,
      oldLine: line.oldLine,
      newLine: line.newLine,
      text: line.text,
      tokens: highlightedByIndex[index],
    }];
  });
}

async function prepareRows(
  files: ParsedPatchFile[],
  diffStyle: DiffCodeStyle,
  cancelled: () => boolean,
  appendRows: (rows: RenderRow[]) => void,
  setPreparedCount: (count: number) => void,
) {
  let preparedFiles = 0;

  for (const file of files) {
    if (cancelled()) return;

    const { lang, promise } = getHighlighter(file.path);
    const highlightedByIndex: Record<number, Token[]> = {};
    let highlighter: any | null = null;

    try {
      highlighter = await promise;
    } catch (err) {
      console.warn('Shiki highlighter failed; rendering plain text for diff file.', file.path, err);
    }

    const codeLineIndexes = file.lines
      .map((line, index) => ({ line, index }))
      .filter(({ line }) => line.kind === 'context' || line.kind === 'add' || line.kind === 'delete');

    for (let start = 0; start < codeLineIndexes.length; start += HIGHLIGHT_BATCH_SIZE) {
      if (cancelled()) return;

      const batch = codeLineIndexes.slice(start, start + HIGHLIGHT_BATCH_SIZE);
      if (highlighter) {
        for (const { line, index } of batch) {
          highlightedByIndex[index] = tokenLine(highlighter, lang, line.text);
        }
      }

      await waitForNextFrame();
    }

    if (cancelled()) return;

    appendRows([
      { kind: 'file', path: file.path, additions: file.additions, deletions: file.deletions, collapsed: false },
      ...(diffStyle === 'split'
        ? buildSplitPreparedRows(file, highlightedByIndex)
        : buildUnifiedPreparedRows(file, highlightedByIndex)),
    ]);
    preparedFiles += 1;
    setPreparedCount(preparedFiles);
    await waitForNextFrame();
  }
}

function UnifiedRow(props: { row: Extract<RenderRow, { kind: 'unified-line' }> }) {
  const { row } = props;
  const prefix = row.lineKind === 'add' ? '+' : row.lineKind === 'delete' ? '-' : ' ';

  return (
    <Box
      style={{
        display: 'grid',
        gridTemplateColumns: '56px 56px 20px minmax(0, 1fr)',
        height: ROW_HEIGHT,
        background: lineBackground(row.lineKind),
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
        fontSize: 12,
        lineHeight: `${ROW_HEIGHT}px`,
        whiteSpace: 'pre',
        overflow: 'hidden',
      }}
    >
      <Box px={6} c="dimmed" style={{ textAlign: 'right', userSelect: 'none' }}>{row.oldLine ?? ''}</Box>
      <Box px={6} c="dimmed" style={{ textAlign: 'right', userSelect: 'none' }}>{row.newLine ?? ''}</Box>
      <Box c="dimmed" style={{ userSelect: 'none' }}>{prefix}</Box>
      <Box px={4} style={{ overflow: 'hidden' }}>
        <HighlightedText text={row.text} tokens={row.tokens} />
      </Box>
    </Box>
  );
}

function SplitCell(props: {
  lineNo?: number;
  text?: string;
  tokens?: Token[];
  side: 'old' | 'new';
  lineKind: 'context' | 'add' | 'delete' | 'change';
}) {
  const { lineNo, text, tokens, side, lineKind } = props;

  return (
    <Box
      style={{
        display: 'grid',
        gridTemplateColumns: '56px minmax(0, 1fr)',
        height: ROW_HEIGHT,
        background: text === undefined ? 'transparent' : lineBackground(lineKind, side),
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
        fontSize: 12,
        lineHeight: `${ROW_HEIGHT}px`,
        whiteSpace: 'pre',
        overflow: 'hidden',
      }}
    >
      <Box px={6} c="dimmed" style={{ textAlign: 'right', userSelect: 'none' }}>{lineNo ?? ''}</Box>
      <Box px={4} style={{ overflow: 'hidden' }}>{text === undefined ? '' : <HighlightedText text={text} tokens={tokens} />}</Box>
    </Box>
  );
}

function SplitRow(props: { row: Extract<RenderRow, { kind: 'split-line' }> }) {
  const { row } = props;

  return (
    <Box style={{ display: 'grid', gridTemplateColumns: 'minmax(0, 1fr) minmax(0, 1fr)', height: ROW_HEIGHT }}>
      <SplitCell lineNo={row.oldLine} text={row.oldText} tokens={row.oldTokens} side="old" lineKind={row.lineKind} />
      <SplitCell lineNo={row.newLine} text={row.newText} tokens={row.newTokens} side="new" lineKind={row.lineKind} />
    </Box>
  );
}

function FileHeaderRow(props: {
  row: Extract<RenderRow, { kind: 'file' }>;
  onToggleFile?: (path: string) => void;
  sticky?: boolean;
}) {
  const { row, onToggleFile, sticky = false } = props;

  return (
    <Box
      px={6}
      style={{
        height: ROW_HEIGHT,
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        background: sticky ? 'rgba(32, 32, 32, 0.98)' : 'rgba(255,255,255,0.055)',
        borderTop: sticky ? '0' : '1px solid rgba(255,255,255,0.08)',
        borderBottom: '1px solid rgba(255,255,255,0.12)',
        boxShadow: sticky ? '0 1px 6px rgba(0,0,0,0.35)' : undefined,
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
        fontSize: 12,
        overflow: 'hidden',
      }}
    >
      {onToggleFile ? (
        <Button
          size="compact-xs"
          variant="subtle"
          onClick={() => onToggleFile(row.path)}
          style={{ height: 16, minHeight: 16, paddingInline: 4 }}
        >
          {row.collapsed ? 'Expand' : 'Collapse'}
        </Button>
      ) : null}
      <Text size="xs" fw={700} truncate style={{ flex: 1 }}>{row.path}</Text>
      <Text size="xs" c="green">+{row.additions}</Text>
      <Text size="xs" c="red">-{row.deletions}</Text>
    </Box>
  );
}

function RenderDiffRow(props: { row: RenderRow; diffStyle: DiffCodeStyle; onToggleFile?: (path: string) => void; hiddenDuplicateFilePath?: string | null }) {
  const { row, diffStyle, onToggleFile, hiddenDuplicateFilePath } = props;

  if (row.kind === 'file') {
    if (row.path === hiddenDuplicateFilePath) {
      return <Box style={{ height: ROW_HEIGHT }} />;
    }
    return <FileHeaderRow row={row} onToggleFile={onToggleFile} />;
  }

  if (row.kind === 'hunk') {
    return (
      <Box
        px={8}
        style={{
          height: ROW_HEIGHT,
          background: 'rgba(34, 139, 230, 0.14)',
          fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
          fontSize: 12,
          lineHeight: `${ROW_HEIGHT}px`,
          whiteSpace: 'pre',
          overflow: 'hidden',
        }}
      >
        {row.text}
      </Box>
    );
  }

  if (diffStyle === 'split' && row.kind === 'split-line') return <SplitRow row={row} />;
  if (row.kind === 'unified-line') return <UnifiedRow row={row} />;

  return null;
}

function applyCollapsedRows(rows: RenderRow[], collapsedPaths?: Record<string, boolean>): RenderRow[] {
  if (!collapsedPaths || Object.keys(collapsedPaths).length === 0) {
    return rows;
  }

  const next: RenderRow[] = [];
  let currentCollapsed = false;

  for (const row of rows) {
    if (row.kind === 'file') {
      currentCollapsed = Boolean(collapsedPaths[row.path]);
      next.push({ ...row, collapsed: currentCollapsed });
      continue;
    }

    if (!currentCollapsed) {
      next.push(row);
    }
  }

  return next;
}

function VirtualizedDiffRows(props: { rows: RenderRow[]; diffStyle: DiffCodeStyle; collapsedPaths?: Record<string, boolean>; onToggleFile?: (path: string) => void; onActiveFileChange?: (path: string | null) => void }) {
  const { rows, diffStyle, collapsedPaths, onToggleFile, onActiveFileChange } = props;
  const visibleInputRows = useMemo(() => applyCollapsedRows(rows, collapsedPaths), [rows, collapsedPaths]);
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(520);

  useEffect(() => {
    const node = viewportRef.current;
    if (!node) return;

    const update = () => {
      setViewportHeight(Math.max(160, node.clientHeight || 520));
      setScrollTop(node.scrollTop);
    };

    update();
    const resizeObserver = new ResizeObserver(update);
    resizeObserver.observe(node);
    node.addEventListener('scroll', update, { passive: true });

    return () => {
      resizeObserver.disconnect();
      node.removeEventListener('scroll', update);
    };
  }, []);

  const stickySlotHeight = ROW_HEIGHT;
  const totalHeight = visibleInputRows.length * ROW_HEIGHT;
  const contentScrollTop = Math.max(0, scrollTop - stickySlotHeight);
  const rawStart = Math.max(0, Math.floor(contentScrollTop / ROW_HEIGHT));
  const start = Math.max(0, rawStart - OVERSCAN_ROWS);
  const visibleCount = Math.ceil(viewportHeight / ROW_HEIGHT) + OVERSCAN_ROWS * 2;
  const end = Math.min(visibleInputRows.length, start + visibleCount);
  const visibleRows = visibleInputRows.slice(start, end);

  const activeFileInfo = (() => {
    const activeIndex = Math.min(visibleInputRows.length - 1, rawStart);
    for (let index = activeIndex; index >= 0; index -= 1) {
      const row = visibleInputRows[index];
      if (row?.kind === 'file') return { row, index };
    }

    const firstFileIndex = visibleInputRows.findIndex((row) => row.kind === 'file');
    if (firstFileIndex >= 0) {
      const row = visibleInputRows[firstFileIndex];
      if (row.kind === 'file') return { row, index: firstFileIndex };
    }

    return null;
  })();

  const stickyActiveFileInfo = activeFileInfo && !activeFileInfo.row.collapsed ? activeFileInfo : null;
  const hiddenDuplicateFilePath = stickyActiveFileInfo?.row.path ?? null;

  useEffect(() => {
    onActiveFileChange?.(activeFileInfo?.row.path ?? null);
  }, [activeFileInfo?.row.path, onActiveFileChange]);

  return (
    <Box
      ref={viewportRef}
      style={{
        height: '100%',
        minHeight: 0,
        overflow: 'auto',
        border: '1px solid rgba(255,255,255,0.08)',
        borderRadius: 6,
        position: 'relative',
      }}
    >
      {stickyActiveFileInfo ? (
        <Box style={{ position: 'sticky', top: 0, zIndex: 5 }}>
          <FileHeaderRow row={stickyActiveFileInfo.row} onToggleFile={onToggleFile} sticky />
        </Box>
      ) : null}

      <Box style={{ height: totalHeight, minWidth: diffStyle === 'split' ? 900 : 760, position: 'relative' }}>
        <Box style={{ position: 'absolute', top: start * ROW_HEIGHT, left: 0, right: 0 }}>
          {visibleRows.map((row, index) => (
            <RenderDiffRow
              key={start + index}
              row={row}
              diffStyle={diffStyle}
              onToggleFile={onToggleFile}
              hiddenDuplicateFilePath={hiddenDuplicateFilePath}
            />
          ))}
        </Box>
      </Box>
    </Box>
  );
}

export function DiffCode(props: { patch: string; diffStyle: DiffCodeStyle; collapsedPaths?: Record<string, boolean>; onToggleFile?: (path: string) => void; onActiveFileChange?: (path: string | null) => void }) {
  const files = useMemo(() => parseUnifiedPatch(props.patch), [props.patch]);
  const [rows, setRows] = useState<RenderRow[]>([]);
  const [preparedFiles, setPreparedFiles] = useState(0);
  const [done, setDone] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setRows([]);
    setPreparedFiles(0);
    setDone(false);

    void prepareRows(
      files,
      props.diffStyle,

      () => cancelled,
      (nextRows) => setRows((current) => [...current, ...nextRows]),
      setPreparedFiles,
    ).finally(() => {
      if (!cancelled) setDone(true);
    });

    return () => {
      cancelled = true;
    };
  }, [files, props.diffStyle]);

  if (files.length === 0) {
    return <Text c="dimmed" size="sm">No diff available.</Text>;
  }

  const progressValue = files.length === 0 ? 0 : Math.round((preparedFiles / files.length) * 100);

  if (rows.length === 0) {
    return (
      <Box style={{ height: '100%', minHeight: 120, display: 'flex', flexDirection: 'column', gap: 4 }}>
        <Box px={6} py={3} style={{ borderBottom: '1px solid rgba(255,255,255,0.08)' }}>
          <Group justify="space-between" gap={6} mb={3}>
            <Group gap={6}>
              <Loader size="xs" />
              <Text size="xs" c="dimmed">Preparing highlighted diff…</Text>
            </Group>
            <Badge size="xs" variant="light">{preparedFiles}/{files.length} files</Badge>
          </Group>
          <Progress value={progressValue} size={2} animated />
        </Box>
        <Group justify="center" style={{ flex: 1 }}>
          <Text size="xs" c="dimmed">Waiting for first highlighted file…</Text>
        </Group>
      </Box>
    );
  }

  return (
    <Box style={{ display: 'flex', flexDirection: 'column', gap: done ? 0 : 3, height: '100%', minHeight: 0 }}>
      {!done ? (
        <Box px={6} py={2} style={{ borderBottom: '1px solid rgba(255,255,255,0.08)' }}>
          <Group justify="space-between" gap={6} style={{ minHeight: 18 }}>
            <Group gap={6}>
              <Loader size="xs" />
              <Text size="xs" c="dimmed">Preparing…</Text>
            </Group>
            <Group gap={4}>
              <Badge size="xs" variant="light">{preparedFiles}/{files.length} files</Badge>
              <Badge size="xs" variant="light">{progressValue}%</Badge>
            </Group>
          </Group>
          <Progress value={progressValue} size={2} animated />
        </Box>
      ) : null}
      <VirtualizedDiffRows rows={rows} diffStyle={props.diffStyle} collapsedPaths={props.collapsedPaths} onToggleFile={props.onToggleFile} onActiveFileChange={props.onActiveFileChange} />
    </Box>
  );
}
