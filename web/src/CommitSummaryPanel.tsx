import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Divider,
  Group,
  Loader,
  Modal,
  NumberInput,
  ScrollArea,
  Select,
  SegmentedControl,
  Stack,
  Text,
  TextInput,
} from '@mantine/core';
import { DiffPanel, type DiffPanelState } from './DiffPanel';
import {
  getReviewCommitDiffManifest,
  getReviewCommits,
  getReviewCommitAnalytics,
  getReviewCommitOptions,
  type ReviewCommitDiffManifestResponse,
  type ReviewCommitSummary,
  type ReviewDiffManifestFileEntry,
  type ReviewCommitAnalyticsResponse,
  type ReviewCommitRefOption,
} from './api';

const COMMIT_PAGE_SIZE = 75;
const DEFAULT_COMMIT_ROW_HEIGHT = 44;

type DiffStyle = 'unified' | 'split';
type CommitReportType = 'commits' | 'analytics';
type CommitAnalyticsMode = 'activity' | 'net';
type CommitChartDisplayMode = 'fit' | 'scroll';
type CommitAggregationWindow = 'daily' | 'monthly' | 'yearly';
type CommitAggregationPreset = CommitAggregationWindow | 'custom_days';
type CommitAnalyticsColorBy = 'extension' | 'author';

type CommitAnalyticsGroupBucket = {
  key: string;
  label: string;
  additions: number;
  deletions: number;
  net: number;
};

function commitAnalyticsGroups(month: ReviewCommitAnalyticsResponse['months'][number]): CommitAnalyticsGroupBucket[] {
  if (Array.isArray(month.groups) && month.groups.length > 0) {
    return month.groups;
  }

  return (month.extensions ?? []).map((extension) => ({
    key: extension.extension,
    label: extension.extension,
    additions: extension.additions,
    deletions: extension.deletions,
    net: extension.net,
  }));
}

function splitCommitFilterText(value: string) {
  return value
    .split(/[\n,]/)
    .map((item) => item.trim())
    .filter(Boolean);
}

function todayDateInputValue() {
  const date = new Date();
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

function dateFromInputValue(value: string): Date | null {
  if (!value.trim()) return null;
  const [year, month, day] = value.split('-').map(Number);
  if (!year || !month || !day) return null;
  return new Date(year, month - 1, day);
}

function inputValueFromDate(value: Date | null): string {
  if (!value) return '';
  const year = value.getFullYear();
  const month = String(value.getMonth() + 1).padStart(2, '0');
  const day = String(value.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

function csvCell(value: string | number | null | undefined) {
  const text = String(value ?? '');
  if (/[",\n\r]/.test(text)) return `"${text.replace(/"/g, '""')}"`;
  return text;
}

function downloadTextFile(filename: string, contents: string, contentType: string) {
  const blob = new Blob([contents], { type: contentType });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = filename;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}

type CommitSummaryPanelProps = {
  repoRef: string;
};

type CommitReviewState = DiffPanelState;

const DEFAULT_REVIEW_STATE: CommitReviewState = {
  selected_scope: 'staged',
  selected_path: null,
  diff_style: 'split',
  only_changes: true,
  context_lines: 4,
  whole_file: false,
};

function formatCommitDate(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, { month: 'short', day: '2-digit', hour: '2-digit', minute: '2-digit' }).format(date);
}

function statusCode(file: ReviewDiffManifestFileEntry): string {
  if (file.untracked) return 'U';
  return `${file.index_status}${file.worktree_status}`.replace(/\./g, '').trim() || 'M';
}




function CommitAnalyticsChart(props: { report: ReviewCommitAnalyticsResponse | null; mode: CommitAnalyticsMode; displayMode: CommitChartDisplayMode; aggregationPreset: CommitAggregationPreset; aggregationDays: number; onFullscreen?: () => void; onExportCsv?: () => void }) {
  const { report, mode, displayMode, aggregationPreset, aggregationDays, onFullscreen, onExportCsv } = props;
  const months = report?.months ?? [];
  const [hoveredMonth, setHoveredMonth] = useState<string | null>(null);
  const [tooltipPosition, setTooltipPosition] = useState({ x: 0, y: 0 });
  const chartContainerRef = useRef<HTMLDivElement | null>(null);
  const [chartContainerWidth, setChartContainerWidth] = useState(0);
  const [chartContainerHeight, setChartContainerHeight] = useState(0);

  const chart = useMemo(() => {
    const extensions = new Map<string, number>();
    for (const month of months) {
      const groups = commitAnalyticsGroups(month);
      if (groups.length === 0) extensions.set('[no file stats]', (extensions.get('[no file stats]') ?? 0) + 1);
      for (const group of groups) {
        const value = mode === 'net'
          ? Math.abs(group.net)
          : group.additions + group.deletions;
        extensions.set(group.label, (extensions.get(group.label) ?? 0) + value);
      }
    }

    const orderedExtensions = [...extensions.entries()]
      .sort((a, b) => b[1] - a[1])
      .map(([extension]) => extension);

    const maxPositive = Math.max(
      1,
      ...months.map((month) => {
        const groups = commitAnalyticsGroups(month);
        if (groups.length === 0) return 1;
        return groups.reduce((sum, group) => {
          return sum + (mode === 'net' ? Math.max(0, group.net) : group.additions);
        }, 0);
      })
    );

    const maxNegative = Math.max(
      1,
      ...months.map((month) => {
        const groups = commitAnalyticsGroups(month);
        if (groups.length === 0) return 0;
        return groups.reduce((sum, group) => {
          return sum + (mode === 'net' ? Math.max(0, -group.net) : group.deletions);
        }, 0);
      })
    );

    const rawDomainMin = maxNegative > 0 ? -maxNegative : 0;
    const rawDomainMax = maxPositive > 0 ? maxPositive : 0;
    const rawSpan = Math.max(1, rawDomainMax - rawDomainMin);
    const padding = Math.max(1, Math.ceil(rawSpan * 0.08));
    const domainMin = rawDomainMin < 0 ? rawDomainMin - padding : 0;
    const domainMax = rawDomainMax > 0 ? rawDomainMax + padding : padding;
    const tickStep = Math.max(1, (domainMax - domainMin) / 4);
    const ticks = Array.from({ length: 5 }, (_, index) => Math.round(domainMin + tickStep * index))
      .filter((value, index, values) => values.indexOf(value) === index);

    return { orderedExtensions, domainMin, domainMax, ticks };
  }, [months, mode]);

  const colorForExtension = (extension: string) => {
    const palette = [
      '#4dabf7',
      '#69db7c',
      '#ffd43b',
      '#da77f2',
      '#ffa94d',
      '#38d9a9',
      '#748ffc',
      '#f783ac',
      '#a9e34b',
      '#66d9e8',
      '#b197fc',
      '#ff8787',
      '#8ce99a',
      '#fcc2d7',
      '#c0eb75',
      '#d0bfff'
    ];
    const index = Math.max(0, chart.orderedExtensions.indexOf(extension));
    return palette[index % palette.length];
  };

  const hoveredMonthData = hoveredMonth ? months.find((month) => month.month === hoveredMonth) ?? null : null;
  const hoveredRows = hoveredMonthData
    ? [...commitAnalyticsGroups(hoveredMonthData)]
      .sort((a, b) => {
        const aValue = mode === 'net' ? Math.abs(a.net) : a.additions + a.deletions;
        const bValue = mode === 'net' ? Math.abs(b.net) : b.additions + b.deletions;
        return bValue - aValue;
      })
    : [];

  useEffect(() => {
    const node = chartContainerRef.current;
    if (!node) return;

    const updateWidth = () => {
      const rect = node.getBoundingClientRect();
      setChartContainerWidth(Math.floor(rect.width));
      setChartContainerHeight(Math.floor(rect.height));
    };

    updateWidth();

    if (typeof ResizeObserver === 'undefined') {
      window.addEventListener('resize', updateWidth);
      return () => window.removeEventListener('resize', updateWidth);
    }

    const observer = new ResizeObserver(updateWidth);
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  if (months.length === 0) {
    return <Text size="sm" c="dimmed">No commit analytics available for the selected filters.</Text>;
  }

  const bucketLabel = aggregationPreset === 'daily'
    ? 'days'
    : aggregationPreset === 'monthly'
      ? 'months'
      : aggregationPreset === 'yearly'
        ? 'years'
        : `${Math.max(1, Math.min(9999, Math.round(aggregationDays || 1)))}-day periods`;

  const totalCommits = report?.totals?.commits ?? months.reduce((sum, month) => sum + month.commits, 0);

  const availableWidth = Math.max(420, chartContainerWidth - 8);
  const naturalWidth = Math.max(1280, months.length * 190 + 160);
  const width = displayMode === 'fit' ? availableWidth : naturalWidth;
  const fallbackHeight = displayMode === 'fit'
    ? Math.max(500, Math.min(720, Math.round(availableWidth * 0.4)))
    : Math.max(620, Math.min(820, Math.round(width * 0.36)));
  const height = displayMode === 'fit'
    ? Math.max(500, chartContainerHeight || fallbackHeight)
    : fallbackHeight;
  const margin = {
    top: 28,
    right: 36,
    bottom: displayMode === 'fit' ? 112 : 88,
    left: 92,
  };
  const compactFitLabels = displayMode === 'fit' && months.length > 14;
  const xLabelEvery = compactFitLabels
    ? Math.max(1, Math.ceil(months.length / 10))
    : 1;
  const plotWidth = width - margin.left - margin.right;
  const plotHeight = height - margin.top - margin.bottom;
  const domainSpan = Math.max(1, chart.domainMax - chart.domainMin);
  const yScale = (value: number) => margin.top + ((chart.domainMax - value) / domainSpan) * plotHeight;
  const zeroY = yScale(0);
  const barBand = plotWidth / Math.max(1, months.length);
  const barWidth = displayMode === 'fit'
    ? Math.max(18, Math.min(barBand * 0.82, 150))
    : Math.max(72, Math.min(180, barBand * 0.58));

  return (
    <Stack gap="sm" style={{ height: '100%', minHeight: 0 }}>
      <Group justify="space-between" align="center" gap="xs">
        <Group gap="xs">
          <Badge variant="light">{totalCommits} commits</Badge>
          <Badge color="green" variant="light">+{months.reduce((sum, month) => sum + month.additions, 0)}</Badge>
          <Badge color="red" variant="light">-{months.reduce((sum, month) => sum + month.deletions, 0)}</Badge>
          <Badge variant="light">{months.length} {bucketLabel}</Badge>
          <Badge variant="light">{mode === 'net' ? 'Net change' : 'Total change'}</Badge>
        </Group>
        <Group gap="xs">
          {onExportCsv ? (
            <Button
              size="compact-xs"
              variant="subtle"
              onClick={onExportCsv}
            >
              Export CSV
            </Button>
          ) : null}
          {onFullscreen ? (
            <Button
              size="compact-xs"
              variant="subtle"
              onClick={onFullscreen}
            >
              Fullscreen
            </Button>
          ) : null}
        </Group>
      </Group>

      <Box ref={chartContainerRef} style={{ position: 'relative', flex: 1, minHeight: 500, width: '100%', alignSelf: 'stretch' }}>
        <ScrollArea type="auto" style={{ height: '100%', minHeight: 500, width: '100%' }}>
          <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`} role="img" aria-label="Commit LOC analytics" preserveAspectRatio="none" style={{ display: 'block', minWidth: displayMode === 'fit' ? '100%' : undefined }}>
            <rect x={margin.left} y={margin.top} width={plotWidth} height={plotHeight} fill="rgba(255,255,255,0.015)" />
            <line x1={margin.left} y1={zeroY} x2={width - margin.right} y2={zeroY} stroke="rgba(255,255,255,0.5)" strokeWidth="1.4" />
            <line x1={margin.left} y1={margin.top} x2={margin.left} y2={height - margin.bottom} stroke="rgba(255,255,255,0.32)" strokeWidth="1.2" />

            {chart.ticks.map((tick) => {
              const y = yScale(tick);
              return (
                <g key={tick}>
                  <line x1={margin.left} y1={y} x2={width - margin.right} y2={y} stroke={tick === 0 ? 'rgba(255,255,255,0.5)' : 'rgba(255,255,255,0.09)'} strokeWidth={tick === 0 ? 1.4 : 1} />
                  <text x={margin.left - 12} y={y + 4} textAnchor="end" fontSize="13" fill="rgba(255,255,255,0.7)">{tick}</text>
                </g>
              );
            })}

            <text x={22} y={margin.top + plotHeight / 2} textAnchor="middle" fontSize="14" fill="rgba(255,255,255,0.78)" transform={`rotate(-90 22 ${margin.top + plotHeight / 2})`}>LOC</text>

            {months.map((month, monthIndex) => {
              const x = margin.left + monthIndex * barBand + (barBand - barWidth) / 2;
              let positiveBase = 0;
              let negativeBase = 0;
              const groupsForMonth = commitAnalyticsGroups(month);
              const effectiveGroupsForMonth = groupsForMonth.length > 0 ? groupsForMonth : [{ key: '[no file stats]', label: '[no file stats]', additions: 1, deletions: 0, net: 1 }];
              const groupMap = new Map(effectiveGroupsForMonth.map((group) => [group.label, group]));

              return (
                <g key={month.month}>
                  <rect
                    x={margin.left + monthIndex * barBand}
                    y={margin.top}
                    width={barBand}
                    height={plotHeight}
                    fill={hoveredMonth === month.month ? 'rgba(255,255,255,0.045)' : 'transparent'}
                    onMouseEnter={() => setHoveredMonth(month.month)}
                    onMouseMove={(event) => {
                      setHoveredMonth(month.month);
                      setTooltipPosition({ x: event.clientX, y: event.clientY });
                    }}
                    onMouseLeave={() => setHoveredMonth(null)}
                  />
                  {chart.orderedExtensions.map((extensionName) => {
                    const extension = groupMap.get(extensionName);
                    if (!extension) return null;

                    const positiveValue = mode === 'net' ? Math.max(0, extension.net) : extension.additions;
                    const negativeValue = mode === 'net' ? Math.max(0, -extension.net) : extension.deletions;
                    const color = colorForExtension(extensionName);
                    const positiveY0 = yScale(positiveBase);
                    const positiveY1 = yScale(positiveBase + positiveValue);
                    const positiveHeight = Math.max(1, positiveY0 - positiveY1);
                    const negativeY0 = yScale(-negativeBase);
                    const negativeY1 = yScale(-(negativeBase + negativeValue));
                    const negativeHeight = Math.max(1, negativeY1 - negativeY0);
                    positiveBase += positiveValue;
                    negativeBase += negativeValue;

                    return (
                      <g key={`${month.month}:${extensionName}`} pointerEvents="none">
                        {positiveValue > 0 ? (
                          <g>
                            <rect
                              x={x}
                              y={positiveY1}
                              width={barWidth}
                              height={positiveHeight}
                              fill={color}
                              opacity={0.9}
                              stroke="rgba(0,0,0,0.24)"
                              strokeWidth="0.6"
                            />
                            {positiveHeight >= 18 && barWidth >= 56 ? (
                              <text x={x + barWidth / 2} y={positiveY1 + positiveHeight / 2 + 4} textAnchor="middle" fontSize="11" fontWeight={700} fill="rgba(0,0,0,0.72)">
                                {mode === 'net' ? `+${positiveValue}` : `+${positiveValue}`}
                              </text>
                            ) : null}
                          </g>
                        ) : null}
                        {negativeValue > 0 ? (
                          <g>
                            <rect
                              x={x}
                              y={negativeY0}
                              width={barWidth}
                              height={negativeHeight}
                              fill={color}
                              opacity={0.48}
                              stroke="rgba(0,0,0,0.24)"
                              strokeWidth="0.6"
                            />
                            {negativeHeight >= 18 && barWidth >= 56 ? (
                              <text x={x + barWidth / 2} y={negativeY0 + negativeHeight / 2 + 4} textAnchor="middle" fontSize="11" fontWeight={700} fill="rgba(255,255,255,0.92)">
                                -{negativeValue}
                              </text>
                            ) : null}
                          </g>
                        ) : null}
                      </g>
                    );
                  })}
                  {monthIndex % xLabelEvery === 0 ? (
                    displayMode === 'fit' ? (
                      <g transform={`translate(${x + barWidth / 2} ${height - margin.bottom + 34}) rotate(-35)`}>
                        <text textAnchor="end" fontSize="11" fill="rgba(255,255,255,0.78)">{month.month}</text>
                        <text y="17" textAnchor="end" fontSize="10" fontWeight={700} fill={month.net >= 0 ? 'rgba(105,219,124,0.98)' : 'rgba(255,135,135,0.98)'}>{month.net >= 0 ? '+' : ''}{month.net}</text>
                      </g>
                    ) : (
                      <>
                        <text x={x + barWidth / 2} y={height - margin.bottom + 26} textAnchor="middle" fontSize="12" fill="rgba(255,255,255,0.78)">{month.month}</text>
                        <text x={x + barWidth / 2} y={height - margin.bottom + 44} textAnchor="middle" fontSize="11" fontWeight={700} fill={month.net >= 0 ? 'rgba(105,219,124,0.98)' : 'rgba(255,135,135,0.98)'}>{month.net >= 0 ? '+' : ''}{month.net}</text>
                      </>
                    )
                  ) : null}
                </g>
              );
            })}
          </svg>
        </ScrollArea>

        {hoveredMonthData ? (
          <Box
            style={{
              position: 'fixed',
              left: Math.min(tooltipPosition.x + 18, window.innerWidth - 360),
              top: Math.min(tooltipPosition.y + 18, window.innerHeight - 420),
              zIndex: 1000,
              width: 340,
              maxHeight: 390,
              overflow: 'auto',
              padding: 12,
              borderRadius: 8,
              background: 'rgba(20,20,20,0.96)',
              border: '1px solid rgba(255,255,255,0.18)',
              boxShadow: '0 12px 36px rgba(0,0,0,0.45)',
              pointerEvents: 'none'
            }}
          >
            <Group justify="space-between" mb={6}>
              <Text size="sm" fw={800}>{hoveredMonthData.month}</Text>
              <Badge size="sm" color={hoveredMonthData.net >= 0 ? 'green' : 'red'} variant="light">{hoveredMonthData.net >= 0 ? '+' : ''}{hoveredMonthData.net} net</Badge>
            </Group>
            <Group gap={6} mb={8}>
              <Badge size="xs" color="green" variant="light">+{hoveredMonthData.additions}</Badge>
              <Badge size="xs" color="red" variant="light">-{hoveredMonthData.deletions}</Badge>
              <Badge size="xs" variant="light">{hoveredMonthData.files_changed} files</Badge>
              <Badge size="xs" variant="light">{hoveredMonthData.commits} commits</Badge>
            </Group>
            <Stack gap={4}>
              {hoveredRows.map((extension) => {
                const positiveValue = mode === 'net' ? Math.max(0, extension.net) : extension.additions;
                const negativeValue = mode === 'net' ? Math.max(0, -extension.net) : extension.deletions;
                return (
                  <Box key={extension.key} style={{ display: 'grid', gridTemplateColumns: '18px 58px 1fr 72px', gap: 8, alignItems: 'center' }}>
                    <Box style={{ width: 12, height: 12, borderRadius: 3, background: colorForExtension(extension.label) }} />
                    <Text size="xs" fw={700} truncate title={extension.label}>{extension.label}</Text>
                    <Text size="xs" c="dimmed">+{extension.additions} / -{extension.deletions}</Text>
                    <Text size="xs" fw={700} ta="right" c={extension.net >= 0 ? 'green' : 'red'}>
                      {mode === 'net' ? `${extension.net >= 0 ? '+' : ''}${extension.net}` : `+${positiveValue} / -${negativeValue}`}
                    </Text>
                  </Box>
                );
              })}
            </Stack>
          </Box>
        ) : null}
      </Box>

      <Group gap={6} mt={2} style={{ flex: '0 0 auto' }}>
        {chart.orderedExtensions.slice(0, 20).map((extension) => (
          <Badge key={extension} size="xs" variant="outline" style={{ borderColor: colorForExtension(extension), color: colorForExtension(extension) }}>{extension}</Badge>
        ))}
      </Group>
    </Stack>
  );
}

type CommitFileLike = {
  path: string;
  additions?: number;
  deletions?: number;
  status?: string;
};

type CommitFileTreeNode = {
  name: string;
  path: string;
  type: "directory" | "file";
  additions: number;
  deletions: number;
  children: CommitFileTreeNode[];
  file?: CommitFileLike;
};

function buildCommitFileTree(files: CommitFileLike[]): CommitFileTreeNode[] {
  const root: CommitFileTreeNode[] = [];

  for (const file of files) {
    const parts = file.path.split("/").filter(Boolean);
    let level = root;
    let currentPath = "";

    for (let index = 0; index < parts.length; index += 1) {
      const name = parts[index];
      const isFile = index === parts.length - 1;
      currentPath = currentPath ? `${currentPath}/${name}` : name;

      let node = level.find((child) => child.name === name && child.type === (isFile ? "file" : "directory"));

      if (!node) {
        node = {
          name,
          path: currentPath,
          type: isFile ? "file" : "directory",
          additions: 0,
          deletions: 0,
          children: [],
          file: isFile ? file : undefined,
        };
        level.push(node);
      }

      node.additions += file.additions ?? 0;
      node.deletions += file.deletions ?? 0;

      if (!isFile) {
        level = node.children;
      }
    }
  }

  return root.sort(sortCommitFileTreeNodes);
}

function sortCommitFileTreeNodes(a: CommitFileTreeNode, b: CommitFileTreeNode) {
  if (a.type !== b.type) {
    return a.type === "directory" ? -1 : 1;
  }

  return a.name.localeCompare(b.name);
}

function CommitFileTree(props: {
  nodes: CommitFileTreeNode[];
  depth?: number;
  commitSha: string;
  selectedPath: string | null;
  onOpenFile: (path: string) => void;
  collapsedPaths?: Set<string>;
  onToggleDirectory?: (path: string) => void;
}) {
  const { nodes, depth = 0, commitSha, selectedPath, onOpenFile, collapsedPaths, onToggleDirectory } = props;

  return (
    <Stack gap={3}>
      {nodes.map((node) => {
        const selected = node.type === 'file' && selectedPath === node.file?.path;
        const changed = node.file;
        const isDirectory = node.type === 'directory';
        const collapsed = isDirectory && collapsedPaths?.has(node.path);

        return (
          <Box key={`${commitSha}:${node.path}`}>
            <Box
              onClick={changed ? () => onOpenFile(changed.path) : isDirectory ? () => onToggleDirectory?.(node.path) : undefined}
              style={{
                cursor: changed || isDirectory ? 'pointer' : 'default',
                padding: isDirectory ? '3px 4px' : '7px 8px',
                paddingLeft: 8 + depth * 16,
                borderRadius: isDirectory ? 0 : 8,
                background: selected ? 'rgba(34, 139, 230, 0.16)' : isDirectory ? 'transparent' : 'rgba(255,255,255,0.025)',
                border: isDirectory ? '0' : '1px solid rgba(255,255,255,0.06)'
              }}
            >
              <Group justify={isDirectory ? 'flex-start' : 'space-between'} wrap="nowrap" gap="xs">
                <Group gap={6} wrap="nowrap" style={{ minWidth: 0, flex: 1 }}>
                  {isDirectory ? (
                    <Text size="xs" c="dimmed" style={{ width: 14, textAlign: 'center', lineHeight: 1 }}>{collapsed ? '▸' : '▾'}</Text>
                  ) : (
                    <Badge size="xs" variant="outline">{changed?.status || 'M'}</Badge>
                  )}
                  <Text size="xs" fw={isDirectory ? 800 : 700} c={isDirectory ? 'dimmed' : undefined} truncate title={changed?.path ?? node.path}>{node.name}</Text>
                  {isDirectory ? (
                    <Group gap={6} wrap="nowrap">
                      <Text c="green" fw={700} style={{ fontSize: 10, lineHeight: 1.1 }}>+{node.additions}</Text>
                      <Text c="red" fw={700} style={{ fontSize: 10, lineHeight: 1.1 }}>-{node.deletions}</Text>
                    </Group>
                  ) : null}
                </Group>
                {!isDirectory ? (
                  <Group gap={4} wrap="nowrap">
                    <Badge size="xs" color="green" variant="light">+{node.additions}</Badge>
                    <Badge size="xs" color="red" variant="light">-{node.deletions}</Badge>
                  </Group>
                ) : null}
              </Group>
            </Box>
            {node.children.length > 0 && !collapsed ? (
              <Box mt={isDirectory ? 2 : 4}>
                <CommitFileTree nodes={node.children} depth={depth + 1} commitSha={commitSha} selectedPath={selectedPath} onOpenFile={onOpenFile} collapsedPaths={collapsedPaths} onToggleDirectory={onToggleDirectory} />
              </Box>
            ) : null}
          </Box>
        );
      })}
    </Stack>
  );
}

export function CommitSummaryPanel(props: CommitSummaryPanelProps) {
  const { repoRef } = props;
  const [commits, setCommits] = useState<ReviewCommitSummary[]>([]);
  const [commitReportType, setCommitReportType] = useState<CommitReportType>('commits');
  const [commitReport, setCommitReport] = useState<ReviewCommitAnalyticsResponse | null>(null);
  const [commitRefOptions, setCommitRefOptions] = useState<ReviewCommitRefOption[]>([]);
  const [commitAnalyticsMode, setCommitAnalyticsMode] = useState<CommitAnalyticsMode>('activity');
  const [commitChartDisplayMode, setCommitChartDisplayMode] = useState<CommitChartDisplayMode>('fit');
  const [commitChartFullscreenOpen, setCommitChartFullscreenOpen] = useState(false);
  const [commitAggregationPreset, setCommitAggregationPreset] = useState<CommitAggregationPreset>('monthly');
  const [commitAggregationDays, setCommitAggregationDays] = useState<number>(10);
  const [commitAggregationDaysText, setCommitAggregationDaysText] = useState('10');
  const [commitAnalyticsColorBy, setCommitAnalyticsColorBy] = useState<CommitAnalyticsColorBy>('extension');
  const [commitReportBusy, setCommitReportBusy] = useState(false);
  const [commitReportRefName, setCommitReportRefName] = useState('');
  const [commitReportSince, setCommitReportSince] = useState('');
  const [commitReportUntil, setCommitReportUntil] = useState('');
  const [commitReportIncludePathsText, setCommitReportIncludePathsText] = useState('');
  const [commitReportExcludePathsText, setCommitReportExcludePathsText] = useState('');
  const [commitReportIncludeExtensionsText, setCommitReportIncludeExtensionsText] = useState('');
  const [commitReportExcludeExtensionsText, setCommitReportExcludeExtensionsText] = useState('');
  const [commitReportIncludeRegexText, setCommitReportIncludeRegexText] = useState('');
  const [commitReportExcludeRegexText, setCommitReportExcludeRegexText] = useState('(^|/)Cargo\\.lock$\n(^|/)package-lock\\.json$\n(^|/)pnpm-lock\\.yaml$\n(^|/)yarn\\.lock$');
  const [advancedCommitFiltersOpen, setAdvancedCommitFiltersOpen] = useState(false);
  const [commitDatePreset, setCommitDatePreset] = useState<'all' | '30d' | '90d' | '6m' | '1y' | 'custom'>('all');
  const [expandedSha, setExpandedSha] = useState<string | null>(null);
  const [selectedCommit, setSelectedCommit] = useState<ReviewCommitSummary | null>(null);
  const [hoveredCommit, setHoveredCommit] = useState<ReviewCommitSummary | null>(null);
  const [collapsedCommitPreviewPaths, setCollapsedCommitPreviewPaths] = useState<Record<string, string[]>>({});
  const commitPreviewClearTimeoutRef = useRef<number | null>(null);
  const commitInspectorHoveredRef = useRef(false);
  const [manifestBySha, setManifestBySha] = useState<Record<string, ReviewCommitDiffManifestResponse>>({});
  const [reviewState, setReviewState] = useState<CommitReviewState>(DEFAULT_REVIEW_STATE);
  const [reviewOpen, setReviewOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [manifestBusySha, setManifestBusySha] = useState<string | null>(null);
  const [nextCommitCursor, setNextCommitCursor] = useState<string | null>(null);
  const [loadingMoreCommits, setLoadingMoreCommits] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const commitScrollViewportRef = useRef<HTMLDivElement | null>(null);
  const commitItemNodeByShaRef = useRef<Record<string, HTMLDivElement | null>>({});
  const commitItemResizeObserverByShaRef = useRef<Record<string, ResizeObserver | undefined>>({});
  const commitItemMeasureRefByShaRef = useRef<Record<string, (node: HTMLDivElement | null) => void>>({});
  const [commitListViewportHeight, setCommitListViewportHeight] = useState(0);
  const [commitListScrollTop, setCommitListScrollTop] = useState(0);
  const [commitItemHeightBySha, setCommitItemHeightBySha] = useState<Record<string, number>>({});
  const appliedCommitDatasetFiltersRef = useRef<Record<string, unknown> | null>(null);
  const [appliedCommitDatasetFiltersKey, setAppliedCommitDatasetFiltersKey] = useState('');

  const sanitizeCommitAggregationDays = useCallback((value: string | number) => {
    const digits = String(value).replace(/[^0-9]/g, '').slice(0, 4);
    const raw = Number(digits);
    if (!Number.isFinite(raw) || raw <= 0) return 1;
    return Math.max(1, Math.min(9999, Math.round(raw)));
  }, []);

  const updateCommitAggregationDaysText = useCallback((value: string | number) => {
    const digits = String(value).replace(/[^0-9]/g, '').slice(0, 4);
    setCommitAggregationDaysText(digits);
    if (digits.trim()) {
      setCommitAggregationDays(sanitizeCommitAggregationDays(digits));
    }
  }, [sanitizeCommitAggregationDays]);

  const commitCommitAggregationDaysText = useCallback(() => {
    const next = sanitizeCommitAggregationDays(commitAggregationDaysText || commitAggregationDays);
    setCommitAggregationDays(next);
    setCommitAggregationDaysText(String(next));
  }, [commitAggregationDays, commitAggregationDaysText, sanitizeCommitAggregationDays]);

  const effectiveAggregationWindow: CommitAggregationWindow = commitAggregationPreset === 'custom_days' ? 'daily' : commitAggregationPreset;
  const effectiveAggregationDays = commitAggregationPreset === 'custom_days' ? Math.max(1, Math.min(9999, Math.round(commitAggregationDays || 1))) : 1;

  const commitDatasetFilters = useMemo(() => ({
    ref_name: commitReportRefName.trim() || null,
    aggregation_window: effectiveAggregationWindow,
    aggregation_days: effectiveAggregationDays,
    color_by: commitAnalyticsColorBy,
    since: commitReportSince.trim() || null,
    until: commitReportUntil.trim() || null,
    include_paths: splitCommitFilterText(commitReportIncludePathsText),
    exclude_paths: splitCommitFilterText(commitReportExcludePathsText),
    include_extensions: splitCommitFilterText(commitReportIncludeExtensionsText),
    exclude_extensions: splitCommitFilterText(commitReportExcludeExtensionsText),
    include_regex: splitCommitFilterText(commitReportIncludeRegexText),
    exclude_regex: splitCommitFilterText(commitReportExcludeRegexText),
  }), [
    commitReportRefName,
    effectiveAggregationWindow,
    effectiveAggregationDays,
    commitAnalyticsColorBy,
    commitReportSince,
    commitReportUntil,
    commitReportIncludePathsText,
    commitReportExcludePathsText,
    commitReportIncludeExtensionsText,
    commitReportExcludeExtensionsText,
    commitReportIncludeRegexText,
    commitReportExcludeRegexText,
  ]);

  const commitDatasetFiltersKey = useMemo(() => JSON.stringify(commitDatasetFilters), [commitDatasetFilters]);
  const commitDatasetFiltersDirty = Boolean(appliedCommitDatasetFiltersKey && appliedCommitDatasetFiltersKey !== commitDatasetFiltersKey);
  const formatCommitDateInput = useCallback((date: Date) => {
    const local = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
    return local.toISOString().slice(0, 10);
  }, []);

  const applyCommitDatePreset = useCallback((preset: 'all' | '30d' | '90d' | '6m' | '1y' | 'custom') => {
    setCommitDatePreset(preset);

    if (preset === 'custom') return;

    if (preset === 'all') {
      setCommitReportSince('');
      setCommitReportUntil('');
      return;
    }

    const until = new Date();
    const since = new Date(until);
    if (preset === '30d') {
      since.setDate(since.getDate() - 30);
    } else if (preset === '90d') {
      since.setDate(since.getDate() - 90);
    } else if (preset === '6m') {
      since.setMonth(since.getMonth() - 6);
    } else if (preset === '1y') {
      since.setFullYear(since.getFullYear() - 1);
    }

    setCommitReportSince(formatCommitDateInput(since));
    setCommitReportUntil(formatCommitDateInput(until));
  }, [formatCommitDateInput]);

  const activeAdvancedCommitFilterCount = useMemo(() => [
    commitReportIncludePathsText,
    commitReportExcludePathsText,
    commitReportIncludeExtensionsText,
    commitReportExcludeExtensionsText,
    commitReportIncludeRegexText,
    commitReportExcludeRegexText,
  ].filter((value) => value.trim()).length, [
    commitReportIncludePathsText,
    commitReportExcludePathsText,
    commitReportIncludeExtensionsText,
    commitReportExcludeExtensionsText,
    commitReportIncludeRegexText,
    commitReportExcludeRegexText,
  ]);


  const orderedCommits = useMemo(() => {
    return commits.sort((a, b) => {
      const aTime = new Date(a.authored_at).getTime();
      const bTime = new Date(b.authored_at).getTime();
      return bTime - aTime;
    });
  }, [commits]);

  const commitItemGap = 8;

  const commitItemHeights = useMemo(() => {
    return orderedCommits.map((commit) => commitItemHeightBySha[commit.sha] ?? DEFAULT_COMMIT_ROW_HEIGHT);
  }, [orderedCommits, commitItemHeightBySha]);

  const commitCumulativeTops = useMemo(() => {
    const tops = [0];
    for (const height of commitItemHeights) {
      tops.push(tops[tops.length - 1] + height);
    }
    return tops;
  }, [commitItemHeights]);

  const commitLinearHeight = commitCumulativeTops[commitCumulativeTops.length - 1] ?? 0;

  const findCommitIndexAtOffset = useCallback((offset: number) => {
    if (orderedCommits.length === 0) return 0;

    const clampedOffset = Math.max(0, Math.min(offset, Math.max(0, commitLinearHeight - 1)));
    let low = 0;
    let high = orderedCommits.length;

    while (low < high) {
      const mid = Math.floor((low + high + 1) / 2);
      if ((commitCumulativeTops[mid] ?? 0) <= clampedOffset) {
        low = mid;
      } else {
        high = mid - 1;
      }
    }

    return Math.min(low, Math.max(0, orderedCommits.length - 1));
  }, [commitCumulativeTops, commitLinearHeight, orderedCommits.length]);

  const buildVisibleCommitColumn = useCallback((startOffset: number) => {
    if (orderedCommits.length === 0) {
      return { commits: [] as ReviewCommitSummary[], nextOffset: 0 };
    }

    const viewportHeight = Math.max(1, commitListViewportHeight || 520);
    const startIndex = findCommitIndexAtOffset(startOffset);
    const commitsForColumn: ReviewCommitSummary[] = [];
    let index = startIndex;
    let usedHeight = Math.max(0, startOffset - (commitCumulativeTops[startIndex] ?? 0));

    while (index < orderedCommits.length && usedHeight < viewportHeight) {
      commitsForColumn.push(orderedCommits[index]);
      usedHeight += commitItemHeights[index] ?? DEFAULT_COMMIT_ROW_HEIGHT;
      index += 1;
    }

    return {
      commits: commitsForColumn,
      nextOffset: Math.min(commitCumulativeTops[index] ?? commitLinearHeight, commitLinearHeight),
    };
  }, [commitCumulativeTops, commitItemHeights, commitLinearHeight, commitListViewportHeight, findCommitIndexAtOffset, orderedCommits]);

  const maxCommitScrollTop = useMemo(() => {
    return Math.max(0, commitLinearHeight - commitListViewportHeight);
  }, [commitLinearHeight, commitListViewportHeight]);

  const effectiveCommitScrollTop = useMemo(() => {
    return Math.min(Math.max(0, commitListScrollTop), maxCommitScrollTop);
  }, [commitListScrollTop, maxCommitScrollTop]);

  const leftCommitColumn = useMemo(() => {
    return buildVisibleCommitColumn(effectiveCommitScrollTop);
  }, [buildVisibleCommitColumn, effectiveCommitScrollTop]);

  const rightCommitColumn = useMemo(() => {
    return buildVisibleCommitColumn(leftCommitColumn.nextOffset);
  }, [buildVisibleCommitColumn, leftCommitColumn.nextOffset]);

  const leftCommits = leftCommitColumn.commits;
  const rightCommits = rightCommitColumn.commits;

  const commitVirtualHeight = commitLinearHeight <= commitListViewportHeight * 2
    ? commitListViewportHeight
    : commitListViewportHeight + maxCommitScrollTop;
  const commitVisibleTop = effectiveCommitScrollTop;

  const updateCommitItemHeight = useCallback((sha: string, node: HTMLDivElement | null) => {
    if (!node) return;
    const nextHeight = Math.max(1, Math.ceil(node.getBoundingClientRect().height) + commitItemGap);
    setCommitItemHeightBySha((current) => {
      const previousHeight = current[sha];
      if (previousHeight !== undefined && Math.abs(previousHeight - nextHeight) <= 1) return current;
      return { ...current, [sha]: nextHeight };
    });
  }, []);

  const setCommitItemMeasureRef = useCallback((sha: string) => {
    if (!commitItemMeasureRefByShaRef.current[sha]) {
      commitItemMeasureRefByShaRef.current[sha] = (node: HTMLDivElement | null) => {
        const previousNode = commitItemNodeByShaRef.current[sha];
        if (previousNode === node) return;

        commitItemResizeObserverByShaRef.current[sha]?.disconnect();
        delete commitItemResizeObserverByShaRef.current[sha];
        commitItemNodeByShaRef.current[sha] = node;

        if (!node) return;

        let animationFrame: number | null = null;
        const measure = () => {
          if (animationFrame !== null) return;
          animationFrame = window.requestAnimationFrame(() => {
            animationFrame = null;
            updateCommitItemHeight(sha, node);
          });
        };

        measure();
        const resizeObserver = new ResizeObserver(measure);
        resizeObserver.observe(node);
        commitItemResizeObserverByShaRef.current[sha] = resizeObserver;
      };
    }

    return commitItemMeasureRefByShaRef.current[sha];
  }, [updateCommitItemHeight]);

  function exportCommitAnalyticsCsv() {
    if (!commitReport) return;
    const buckets = commitReport.months ?? [];
    const rows = [[
      'repo_ref',
      'ref_name',
      'aggregation_window',
      'color_by',
      'period',
      'group_key',
      'group_label',
      'additions',
      'deletions',
      'net',
      'period_additions',
      'period_deletions',
      'period_net',
      'period_files_changed',
      'period_commits'
    ]];

    for (const bucket of buckets) {
      const groups = commitAnalyticsGroups(bucket);
      for (const group of groups) {
        rows.push([
          repoRef,
          commitReportRefName,
          commitAggregationPreset === 'custom_days' ? `${effectiveAggregationDays}_days` : effectiveAggregationWindow,
          commitAnalyticsColorBy,
          bucket.month,
          group.key,
          group.label,
          String(group.additions),
          String(group.deletions),
          String(group.net),
          String(bucket.additions),
          String(bucket.deletions),
          String(bucket.net),
          String(bucket.files_changed),
          String(bucket.commits)
        ]);
      }
    }

    const csv = rows.map((row) => row.map(csvCell).join(',')).join('\n') + '\n';
    const safeRef = (commitReportRefName || 'ref').replace(/[^a-z0-9_.-]+/gi, '_');
    const aggregationLabel = commitAggregationPreset === 'custom_days' ? `${effectiveAggregationDays}-days` : effectiveAggregationWindow;
    downloadTextFile(`commit-analytics-${safeRef}-${aggregationLabel}-${commitAnalyticsColorBy}.csv`, csv, 'text/csv;charset=utf-8');
  }

  async function loadCommitPage(cursor: string | null, append: boolean) {
    if (!repoRef.trim()) return;
    try {
      if (append) setLoadingMoreCommits(true);
      else {
        setBusy(true);
        setNextCommitCursor(null);
      }
      setError(null);

      const filters = appliedCommitDatasetFiltersRef.current ?? commitDatasetFilters;
      const json = await getReviewCommits({
        repo_ref: repoRef,
        limit: COMMIT_PAGE_SIZE,
        cursor: append ? cursor : null,
        ...filters,
      });

      setCommits((current) => {
        if (!append) return json.commits;
        const seen = new Set(current.map((commit) => commit.sha));
        return [...current, ...json.commits.filter((commit) => !seen.has(commit.sha))];
      });
      const fallbackCursor = json.next_offset == null ? null : String(json.next_offset);
      setNextCommitCursor(json.has_more ? (json.next_cursor ?? fallbackCursor) : null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
      setLoadingMoreCommits(false);
    }
  }

  async function refreshCommits() {
    await loadCommitPage(null, false);
  }

  async function loadMoreCommits() {
    if (nextCommitCursor === null || busy || loadingMoreCommits) return;
    await loadCommitPage(nextCommitCursor, true);
  }

  async function refreshCommitReport(nextFilters: Record<string, unknown> = commitDatasetFilters) {
    if (!repoRef.trim()) return;
    try {
      setCommitReportBusy(true);
      setError(null);
      appliedCommitDatasetFiltersRef.current = nextFilters;
      setAppliedCommitDatasetFiltersKey(JSON.stringify(nextFilters));
      const json = await getReviewCommitAnalytics({
        repo_ref: repoRef,
        ...nextFilters,
      });
      setCommitReport(json);
      await loadCommitPage(null, false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setCommitReportBusy(false);
    }
  }

  function applyCommitDatasetFilters() {
    void refreshCommitReport(commitDatasetFilters);
  }

  async function ensureManifest(commit: ReviewCommitSummary) {
    const cached = manifestBySha[commit.sha];
    if (cached) return cached;
    setManifestBusySha(commit.sha);
    try {
      const manifest = await getReviewCommitDiffManifest({ repo_ref: repoRef, commit: commit.sha });
      setManifestBySha((current) => ({ ...current, [commit.sha]: manifest }));
      return manifest;
    } finally {
      setManifestBusySha(null);
    }
  }

  async function openCommitReview(commit: ReviewCommitSummary, selectedPath: string | null) {
    const nextState: CommitReviewState = {
      ...reviewState,
      selected_scope: 'staged',
      selected_path: selectedPath,
    };
    setSelectedCommit(commit);
    setReviewState(nextState);
    setReviewOpen(true);
  }

  async function toggleExpanded(commit: ReviewCommitSummary) {
    const next = expandedSha === commit.sha ? null : commit.sha;
    setExpandedSha(next);
    if (next) {
      try {
        await ensureManifest(commit);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    }
  }

  function selectCommitForDetails(commit: ReviewCommitSummary) {
    setCollapsedCommitPreviewPaths((current) => {
      if (!current[commit.sha]) return current;
      const next = { ...current };
      delete next[commit.sha];
      return next;
    });
    setSelectedCommit((current) => current?.sha === commit.sha ? null : commit);
  }

  function toggleCommitPreviewDirectory(commitSha: string, path: string) {
    setCollapsedCommitPreviewPaths((current) => {
      const paths = new Set(current[commitSha] ?? []);
      if (paths.has(path)) paths.delete(path);
      else paths.add(path);
      return { ...current, [commitSha]: [...paths] };
    });
  }

  function previewCommitDetails(commit: ReviewCommitSummary) {
    if (commitPreviewClearTimeoutRef.current !== null) {
      window.clearTimeout(commitPreviewClearTimeoutRef.current);
      commitPreviewClearTimeoutRef.current = null;
    }
    setHoveredCommit(commit);
  }

  function clearCommitPreview(commit: ReviewCommitSummary) {
    if (commitPreviewClearTimeoutRef.current !== null) {
      window.clearTimeout(commitPreviewClearTimeoutRef.current);
    }
    commitPreviewClearTimeoutRef.current = window.setTimeout(() => {
      commitPreviewClearTimeoutRef.current = null;
      if (commitInspectorHoveredRef.current) return;
      setHoveredCommit((current) => current?.sha === commit.sha ? null : current);
    }, 140);
  }

  useEffect(() => {
    if (!repoRef.trim()) return;
    appliedCommitDatasetFiltersRef.current = null;
    setAppliedCommitDatasetFiltersKey('');
    setCommitReport(null);
    let cancelled = false;
    void getReviewCommitOptions({ repo_ref: repoRef }).then((json) => {
      if (cancelled) return;
      const refs = json.refs.filter((item) => item.value !== '__WORKTREE__');
      setCommitRefOptions(refs);
      setCommitReportRefName((current) => current || json.default_ref || refs[0]?.value || '');
      setCommitReportSince((current) => current || '');
      setCommitReportUntil((current) => current || '');
    }).catch((err) => {
      if (!cancelled) setError(err instanceof Error ? err.message : String(err));
    });
    return () => {
      cancelled = true;
    };
  }, [repoRef]);

  useEffect(() => {
    if (!repoRef.trim() || !commitReportRefName.trim() || appliedCommitDatasetFiltersKey) return;
    const timeout = window.setTimeout(() => {
      void refreshCommitReport(commitDatasetFilters);
    }, 250);
    return () => window.clearTimeout(timeout);
  }, [repoRef, commitReportRefName, appliedCommitDatasetFiltersKey]);



  useEffect(() => {
    const viewport = commitScrollViewportRef.current;
    if (!viewport) return;

    const update = () => {
      setCommitListViewportHeight(viewport.clientHeight);
      setCommitListScrollTop(viewport.scrollTop);
    };

    update();
    requestAnimationFrame(update);
    viewport.addEventListener('scroll', update, { passive: true });
    const resizeObserver = new ResizeObserver(update);
    resizeObserver.observe(viewport);

    return () => {
      viewport.removeEventListener('scroll', update);
      resizeObserver.disconnect();
    };
  }, [busy, commits.length]);

  useEffect(() => {
    const viewport = commitScrollViewportRef.current;
    if (!viewport) return;
    if (commitReportType !== 'commits') return;
    if (busy || loadingMoreCommits || nextCommitCursor === null) return;

    const remaining = maxCommitScrollTop - effectiveCommitScrollTop;
    const threshold = Math.max(600, commitListViewportHeight * 0.75);
    if (remaining <= threshold) {
      void loadMoreCommits();
    }
  }, [commitReportType, busy, loadingMoreCommits, nextCommitCursor, maxCommitScrollTop, effectiveCommitScrollTop, commitListViewportHeight]);

  useEffect(() => {
    return () => {
      if (commitPreviewClearTimeoutRef.current !== null) {
        window.clearTimeout(commitPreviewClearTimeoutRef.current);
        commitPreviewClearTimeoutRef.current = null;
      }
      Object.values(commitItemResizeObserverByShaRef.current).forEach((observer) => observer?.disconnect());
      commitItemResizeObserverByShaRef.current = {};
      commitItemMeasureRefByShaRef.current = {};
    };
  }, []);

  useEffect(() => {
    const liveShas = new Set(orderedCommits.map((commit) => commit.sha));
    setCommitItemHeightBySha((current) => {
      const next: Record<string, number> = {};
      for (const [sha, height] of Object.entries(current)) {
        if (liveShas.has(sha)) next[sha] = height;
      }
      return Object.keys(next).length === Object.keys(current).length ? current : next;
    });
  }, [orderedCommits]);

  useEffect(() => {
    const viewport = commitScrollViewportRef.current;
    if (!viewport) return;
    if (viewport.scrollTop > maxCommitScrollTop) {
      viewport.scrollTop = maxCommitScrollTop;
      setCommitListScrollTop(maxCommitScrollTop);
    }
  }, [maxCommitScrollTop]);

  const inspectorCommit = selectedCommit ?? hoveredCommit;
  const collapsedInspectorPaths = useMemo(() => new Set(inspectorCommit ? collapsedCommitPreviewPaths[inspectorCommit.sha] ?? [] : []), [collapsedCommitPreviewPaths, inspectorCommit]);

  const reviewContent = selectedCommit ? (
    <DiffPanel
      runId={null}
      repoRef={repoRef}
      state={reviewState}
      onPersistState={async (nextState) => setReviewState(nextState)}
      forceViewerOpen
      mode="commit"
      commitSha={selectedCommit.sha}
      commitTitle={selectedCommit.subject}
      commitSubtitle={selectedCommit.short_sha}
      onClose={() => setReviewOpen(false)}
    />
  ) : null;

  return (
    <>
      <Modal
        opened={advancedCommitFiltersOpen}
        onClose={() => setAdvancedCommitFiltersOpen(false)}
        title="Commit filters"
        size="lg"
        centered
      >
        <Stack gap="sm">
          <Text size="xs" c="dimmed">
            Narrow commit history by paths, extensions, or regex. Empty fields leave the default history intact.
          </Text>
          <Group grow align="flex-start">
            <TextInput
              size="xs"
              label="Include paths"
              placeholder="api/src, web/src"
              value={commitReportIncludePathsText}
              onChange={(event) => setCommitReportIncludePathsText(event.currentTarget.value)}
            />
            <TextInput
              size="xs"
              label="Exclude paths"
              placeholder="target, node_modules"
              value={commitReportExcludePathsText}
              onChange={(event) => setCommitReportExcludePathsText(event.currentTarget.value)}
            />
          </Group>
          <Group grow align="flex-start">
            <TextInput
              size="xs"
              label="Include extensions"
              placeholder="rs, ts, tsx"
              value={commitReportIncludeExtensionsText}
              onChange={(event) => setCommitReportIncludeExtensionsText(event.currentTarget.value)}
            />
            <TextInput
              size="xs"
              label="Exclude extensions"
              placeholder="lock, map"
              value={commitReportExcludeExtensionsText}
              onChange={(event) => setCommitReportExcludeExtensionsText(event.currentTarget.value)}
            />
          </Group>
          <TextInput
            size="xs"
            label="Include regex"
            value={commitReportIncludeRegexText}
            onChange={(event) => setCommitReportIncludeRegexText(event.currentTarget.value)}
          />
          <TextInput
            size="xs"
            label="Exclude regex"
            value={commitReportExcludeRegexText}
            onChange={(event) => setCommitReportExcludeRegexText(event.currentTarget.value)}
          />
          <Group justify="space-between">
            <Button
              size="xs"
              variant="subtle"
              color="red"
              onClick={() => {
                setCommitReportIncludePathsText('');
                setCommitReportExcludePathsText('');
                setCommitReportIncludeExtensionsText('');
                setCommitReportExcludeExtensionsText('');
                setCommitReportIncludeRegexText('');
                setCommitReportExcludeRegexText('');
              }}
            >
              Clear filters
            </Button>
            <Group gap="xs">
              <Button size="xs" variant="default" onClick={() => setAdvancedCommitFiltersOpen(false)}>
                Cancel
              </Button>
              <Button
                size="xs"
                onClick={() => {
                  setAdvancedCommitFiltersOpen(false);
                  void refreshCommitReport();
                }}
              >
                Apply filters
              </Button>
            </Group>
          </Group>
        </Stack>
      </Modal>
      <Box style={{ height: 'calc(100dvh - 96px)', minHeight: 0, overflow: 'hidden' }}>
        <Card withBorder p="sm" style={{ height: '100%', minHeight: 0, overflow: 'hidden', display: 'flex', flexDirection: 'column' }}>
          <Group justify="space-between" align="flex-start" mb={6}>
            <Stack gap={6} style={{ flex: 1 }}>
              <Group gap="xs" align="center">
                <Text fw={700} size="sm">Commit summary</Text>
                <Text size="xs" c="dimmed">History and LOC analytics</Text>
              </Group>
              <Group gap="xs" align="end" wrap="wrap">
                <SegmentedControl
                  size="xs"
                  value={commitReportType}
                  onChange={(value) => setCommitReportType(value as CommitReportType)}
                  data={[{ label: 'Commits', value: 'commits' }, { label: 'Analytics', value: 'analytics' }]}
                />
                <Select
                  size="xs"
                  label="Ref"
                  searchable
                  data={commitRefOptions}
                  value={commitReportRefName || null}
                  onChange={(value) => setCommitReportRefName(value || '')}
                  style={{ minWidth: 180 }}
                />
                <Select
                  size="xs"
                  label="Preset"
                  value={commitDatePreset}
                  onChange={(value) => applyCommitDatePreset((value || 'all') as 'all' | '30d' | '90d' | '6m' | '1y' | 'custom')}
                  data={[
                    { label: 'All time', value: 'all' },
                    { label: 'Last 30 days', value: '30d' },
                    { label: 'Last 90 days', value: '90d' },
                    { label: 'Last 6 months', value: '6m' },
                    { label: 'Last year', value: '1y' },
                    { label: 'Custom', value: 'custom' },
                  ]}
                  style={{ width: 140 }}
                />
                <TextInput
                  size="xs"
                  type="date"
                  label="Since"
                  value={commitReportSince}
                  onChange={(event) => {
                    setCommitDatePreset('custom');
                    setCommitReportSince(event.currentTarget.value);
                  }}
                  style={{ width: 132 }}
                />
                <TextInput
                  size="xs"
                  type="date"
                  label="Until"
                  value={commitReportUntil}
                  onChange={(event) => {
                    setCommitDatePreset('custom');
                    setCommitReportUntil(event.currentTarget.value);
                  }}
                  style={{ width: 132 }}
                />
                <Button
                  size="xs"
                  variant={activeAdvancedCommitFilterCount > 0 ? 'light' : 'default'}
                  onClick={() => setAdvancedCommitFiltersOpen(true)}
                >
                  Filters{activeAdvancedCommitFilterCount > 0 ? ` · ${activeAdvancedCommitFilterCount}` : ''}
                </Button>
                <Button
                  size="xs"
                  variant={commitDatasetFiltersDirty ? 'filled' : 'default'}
                  color={commitDatasetFiltersDirty ? 'green' : undefined}
                  loading={busy || commitReportBusy}
                  onClick={applyCommitDatasetFilters}
                >
                  Apply
                </Button>

              </Group>
              {commitReportType === 'analytics' ? (
                <Group gap="xs" align="end" wrap="wrap">
                  <Select
                    size="xs"
                    label="Measure"
                    value={commitAnalyticsMode}
                    onChange={(value) => setCommitAnalyticsMode((value || 'activity') as CommitAnalyticsMode)}
                    data={[
                      { label: 'Total change', value: 'activity' },
                      { label: 'Net change', value: 'net' },
                    ]}
                    style={{ width: 150 }}
                  />
                  <Box>
                    <Text size="xs" fw={700} mb={4}>Aggregation</Text>
                    <Group gap={0} wrap="nowrap">
                      <Select
                        size="xs"
                        aria-label="Aggregation"
                        value={commitAggregationPreset}
                        onChange={(value) => setCommitAggregationPreset((value || 'monthly') as CommitAggregationPreset)}
                        data={[
                          { label: 'Day', value: 'daily' },
                          { label: 'Month', value: 'monthly' },
                          { label: 'Year', value: 'yearly' },
                          { label: 'Custom', value: 'custom_days' },
                        ]}
                        style={{ width: commitAggregationPreset === 'custom_days' ? 112 : 140 }}
                        styles={{
                          input: commitAggregationPreset === 'custom_days' ? {
                            borderTopRightRadius: 0,
                            borderBottomRightRadius: 0,
                          } : undefined,
                        }}
                      />
                      {commitAggregationPreset === 'custom_days' ? (
                        <TextInput
                          size="xs"
                          aria-label="Custom aggregation days"
                          value={commitAggregationDaysText}
                          onChange={(event) => updateCommitAggregationDaysText(event.currentTarget.value)}
                          onBlur={commitCommitAggregationDaysText}
                          onKeyDown={(event) => {
                            if (event.key === 'Enter') {
                              event.currentTarget.blur();
                            }
                          }}
                          inputMode="numeric"
                          maxLength={4}
                          rightSection="days"
                          rightSectionWidth={32}
                          style={{ width: 82, marginLeft: -1 }}
                          styles={{
                            input: {
                              borderTopLeftRadius: 0,
                              borderBottomLeftRadius: 0,
                              paddingLeft: 8,
                              paddingRight: 34,
                              textAlign: 'right',
                            },
                            section: {
                              color: 'var(--mantine-color-dimmed)',
                              fontSize: 11,
                              pointerEvents: 'none',
                            },
                          }}
                        />
                      ) : null}
                    </Group>
                  </Box>
                  <Select
                    size="xs"
                    label="View"
                    value={commitChartDisplayMode}
                    onChange={(value) => setCommitChartDisplayMode((value || 'fit') as CommitChartDisplayMode)}
                    data={[
                      { label: 'Fit', value: 'fit' },
                      { label: 'Scroll', value: 'scroll' },
                    ]}
                    style={{ width: 110 }}
                  />

                  <Select
                    size="xs"
                    label="Color"
                    value={commitAnalyticsColorBy}
                    onChange={(value) => setCommitAnalyticsColorBy((value || 'extension') as CommitAnalyticsColorBy)}
                    data={[
                      { label: 'File extension', value: 'extension' },
                      { label: 'Author', value: 'author' },
                    ]}
                    style={{ width: 125 }}
                  />

                </Group>
              ) : null}
            </Stack>
          </Group>
          {error && !reviewOpen ? <Alert color="red" mb="sm">{error}</Alert> : null}
          <Divider mb={6} />
          {commitReportType === 'analytics' ? (
            (commitReportBusy || busy) && !commitReport ? (
              <Group justify="center" py="xl"><Loader /></Group>
            ) : (
              <Box style={{ flex: 1, minHeight: 0 }}>
                <CommitAnalyticsChart report={commitReport} mode={commitAnalyticsMode} displayMode={commitChartDisplayMode} aggregationPreset={commitAggregationPreset} aggregationDays={effectiveAggregationDays} onFullscreen={() => setCommitChartFullscreenOpen(true)} onExportCsv={exportCommitAnalyticsCsv} />
              </Box>
            )
          ) : busy && commits.length === 0 ? (
            <Group justify="center" py="xl"><Loader /></Group>
          ) : (
            <Box style={{ flex: 1, minHeight: 0, display: 'grid', gridTemplateColumns: 'minmax(520px, 1fr) minmax(360px, 0.62fr)', gap: 10 }}>
              <ScrollArea style={{ minHeight: 0, width: '100%' }} type="auto" viewportRef={commitScrollViewportRef}>
                <Stack gap="xs" pr={4}>
                  {orderedCommits.map((commit) => {
                    const selected = selectedCommit?.sha === commit.sha;
                    const previewed = hoveredCommit?.sha === commit.sha;
                    const fileCount = commit.files_changed;
                    const additions = commit.additions;
                    const deletions = commit.deletions;
                    const hasStats = typeof fileCount === 'number' && typeof additions === 'number' && typeof deletions === 'number';

                    return (
                      <Card
                        ref={setCommitItemMeasureRef(commit.sha)}
                        key={commit.sha}
                        withBorder
                        p={7}
                        onMouseEnter={() => previewCommitDetails(commit)}
                        onMouseLeave={() => clearCommitPreview(commit)}
                        onFocus={() => previewCommitDetails(commit)}
                        onBlur={() => clearCommitPreview(commit)}
                        onClick={() => void selectCommitForDetails(commit)}
                        style={{
                          cursor: 'pointer',
                          background: selected
                            ? 'rgba(34, 139, 230, 0.14)'
                            : previewed
                              ? 'rgba(255,255,255,0.045)'
                              : undefined,
                          borderColor: selected
                            ? 'rgba(74, 171, 247, 0.55)'
                            : previewed
                              ? 'rgba(255,255,255,0.18)'
                              : undefined,
                        }}
                      >
                        <Group justify="space-between" align="center" wrap="nowrap" gap="xs">
                          <Group gap={6} wrap="nowrap" style={{ minWidth: 0, flex: 1 }}>
                            <Badge size="xs" variant="outline">{commit.short_sha}</Badge>
                            <Badge size="xs" variant="light">{formatCommitDate(commit.authored_at)}</Badge>
                            <Text size="sm" fw={700} truncate style={{ minWidth: 0, flex: 1 }} title={commit.subject}>{commit.subject}</Text>
                            <Text size="xs" c="dimmed" truncate style={{ maxWidth: 140 }}>{commit.author_name}</Text>
                          </Group>
                          <Group gap={4} wrap="nowrap">
                            {hasStats ? (
                              <>
                                <Badge size="xs" variant="light">{fileCount}f</Badge>
                                <Badge size="xs" color="green" variant="light">+{additions}</Badge>
                                <Badge size="xs" color="red" variant="light">-{deletions}</Badge>
                              </>
                            ) : (
                              <Badge size="xs" variant="light">stats unavailable</Badge>
                            )}
                            <Button
                              size="compact-xs"
                              variant="filled"
                              onClick={(event) => {
                                event.stopPropagation();
                                void openCommitReview(commit, null);
                              }}
                            >
                              Review
                            </Button>
                          </Group>
                        </Group>

                      </Card>
                    );
                  })}
                  {loadingMoreCommits ? (
                    <Group justify="center" py="sm"><Loader size="sm" /></Group>
                  ) : commits.length > 0 && nextCommitCursor === null ? (
                    <Text size="xs" c="dimmed" ta="center">End of commit history</Text>
                  ) : null}
                </Stack>
              </ScrollArea>

              <Card
                withBorder
                p="sm"
                onMouseEnter={() => {
                  commitInspectorHoveredRef.current = true;
                  if (commitPreviewClearTimeoutRef.current !== null) {
                    window.clearTimeout(commitPreviewClearTimeoutRef.current);
                    commitPreviewClearTimeoutRef.current = null;
                  }
                }}
                onMouseLeave={() => {
                  commitInspectorHoveredRef.current = false;
                  if (!selectedCommit) setHoveredCommit(null);
                }}
                style={{ minHeight: 0, overflow: 'hidden', display: 'flex', flexDirection: 'column' }}
              >
                {inspectorCommit ? (() => {
                  const files = inspectorCommit.files ?? [];
                  const pinned = selectedCommit?.sha === inspectorCommit.sha;
                  return (
                    <Stack gap="sm" style={{ minHeight: 0, flex: 1 }}>
                      <Stack gap={4}>
                        <Group justify="space-between" wrap="nowrap" gap="xs">
                          <Group gap={6} wrap="nowrap" style={{ minWidth: 0 }}>
                            <Badge size="xs" variant="outline">{inspectorCommit.short_sha}</Badge>
                            <Text size="sm" fw={800} truncate title={inspectorCommit.subject}>{inspectorCommit.subject}</Text>
                          </Group>
                          <Group gap={6} wrap="nowrap">
                            <Button size="compact-xs" variant={pinned ? 'light' : 'default'} onClick={() => selectCommitForDetails(inspectorCommit)}>{pinned ? 'Unpin' : 'Pin'}</Button>
                            <Button size="compact-xs" onClick={() => void openCommitReview(inspectorCommit, null)}>Review all</Button>
                          </Group>
                        </Group>
                        <Group gap={6}>
                          <Badge size="xs" variant="light">{formatCommitDate(inspectorCommit.authored_at)}</Badge>
                          <Badge size="xs" variant="light">{inspectorCommit.author_name}</Badge>
                          {typeof inspectorCommit.files_changed === 'number' ? <Badge size="xs" variant="light">{inspectorCommit.files_changed} files</Badge> : null}
                          {typeof inspectorCommit.additions === 'number' ? <Badge size="xs" color="green" variant="light">+{inspectorCommit.additions}</Badge> : null}
                          {typeof inspectorCommit.deletions === 'number' ? <Badge size="xs" color="red" variant="light">-{inspectorCommit.deletions}</Badge> : null}
                          <Badge size="xs" variant={pinned ? 'filled' : 'light'}>{pinned ? 'Pinned' : 'Preview'}</Badge>
                        </Group>
                      </Stack>

                      <Divider />

                      {files.length > 0 ? (
                        <ScrollArea style={{ flex: 1, minHeight: 0 }} type="auto">
                          <CommitFileTree
                            nodes={buildCommitFileTree(files)}
                            commitSha={inspectorCommit.sha}
                            selectedPath={reviewOpen ? reviewState.selected_path : null}
                            onOpenFile={(path) => void openCommitReview(inspectorCommit, path)}
                            collapsedPaths={collapsedInspectorPaths}
                            onToggleDirectory={(path) => toggleCommitPreviewDirectory(inspectorCommit.sha, path)}
                          />
                        </ScrollArea>
                      ) : (
                        <Text size="sm" c="dimmed">No per-file stats were returned for this commit.</Text>
                      )}
                    </Stack>
                  );
                })() : (
                  <Stack gap={4} justify="center" align="center" style={{ flex: 1 }}>
                    <Text size="sm" fw={800}>Hover a commit</Text>
                    <Text size="xs" c="dimmed" ta="center">Changed files and per-file additions/deletions preview here. Click a commit to pin it.</Text>
                  </Stack>
                )}
              </Card>
            </Box>
          )}
        </Card>
      </Box>
      <Modal
        opened={commitChartFullscreenOpen}
        onClose={() => setCommitChartFullscreenOpen(false)}
        fullScreen
        padding="md"
        radius={0}
        title={(
          <Group gap="xs">
            <Text fw={800}>Commit analytics</Text>
            <SegmentedControl
              size="xs"
              value={commitAnalyticsMode}
              onChange={(value) => setCommitAnalyticsMode(value as CommitAnalyticsMode)}
              data={[{ label: 'Total change', value: 'activity' }, { label: 'Net change', value: 'net' }]}
            />
            <SegmentedControl
              size="xs"
              value={commitChartDisplayMode}
              onChange={(value) => setCommitChartDisplayMode(value as CommitChartDisplayMode)}
              data={[{ label: 'Fit', value: 'fit' }, { label: 'Scroll', value: 'scroll' }]}
            />
          </Group>
        )}
        styles={{
          content: { display: 'flex', flexDirection: 'column' },
          body: { flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' },
        }}
      >
        <Box style={{ flex: 1, minHeight: 0 }}>
          <CommitAnalyticsChart report={commitReport} mode={commitAnalyticsMode} displayMode={commitChartDisplayMode} aggregationPreset={commitAggregationPreset} aggregationDays={effectiveAggregationDays} onExportCsv={exportCommitAnalyticsCsv} />
        </Box>
      </Modal>
      <Modal
        opened={reviewOpen}
        onClose={() => setReviewOpen(false)}
        withCloseButton={false}
        fullScreen
        padding={0}
        radius={0}
        styles={{
          content: { inset: 0, width: '100vw', maxWidth: '100vw', height: '100vh', maxHeight: '100vh', margin: 0, display: 'flex', flexDirection: 'column' },
          body: { flex: 1, padding: 0, minHeight: 0 },
        }}
      >
        <Box p="sm" style={{ height: '100vh', minHeight: 0 }}>{reviewContent}</Box>
      </Modal>
    </>
  );
}
