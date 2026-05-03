import { Component, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  Code,
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
import { parsePatchFiles, type FileDiffMetadata } from '@pierre/diffs';
import { FileDiff, PatchDiff, Virtualizer } from '@pierre/diffs/react';
import {
  getReviewCommitDiff,
  getReviewCommitDiffManifest,
  getReviewCommitReport,
  getReviewCommitOptions,
  type ReviewCommitDiffManifestResponse,
  type ReviewCommitDiffResponse,
  type ReviewCommitSummary,
  type ReviewDiffManifestFileEntry,
  type ReviewCommitReportResponse,
  type ReviewCommitRefOption,
} from './api';

const COMMIT_PAGE_SIZE = 75;
const DEFAULT_COMMIT_ROW_HEIGHT = 44;
const LARGE_SINGLE_FILE_RENDER_LINE_LIMIT = 8000;

type DiffStyle = 'unified' | 'split';
type CommitReportType = 'commits' | 'analytics';
type CommitAnalyticsMode = 'activity' | 'net';
type CommitAggregationWindow = 'daily' | 'monthly' | 'yearly';
type CommitAnalyticsColorBy = 'extension' | 'author';

type CommitAnalyticsGroupBucket = {
  key: string;
  label: string;
  additions: number;
  deletions: number;
  net: number;
};

function commitAnalyticsGroups(month: ReviewCommitReportResponse['months'][number]): CommitAnalyticsGroupBucket[] {
  return month.groups ?? month.extensions.map((extension) => ({
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

type CommitReviewState = {
  selected_path: string | null;
  diff_style: DiffStyle;
  only_changes: boolean;
  context_lines: number;
  whole_file: boolean;
};

const DEFAULT_REVIEW_STATE: CommitReviewState = {
  selected_path: null,
  diff_style: 'unified',
  only_changes: true,
  context_lines: 10,
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

function clampContextLines(value: number | string | null | undefined): number {
  const numeric = typeof value === 'number' ? value : Number(value ?? 10);
  if (!Number.isFinite(numeric)) return 10;
  return Math.max(0, Math.min(1000, Math.round(numeric)));
}

function sumCounts(files: ReviewDiffManifestFileEntry[]) {
  return files.reduce(
    (acc, file) => {
      acc.additions += file.additions;
      acc.deletions += file.deletions;
      return acc;
    },
    { additions: 0, deletions: 0 }
  );
}

type SafePatchDiffProps = {
  patch: string;
  diffStyle: DiffStyle;
};

type SafePatchDiffState = {
  error: string | null;
};

class SafePatchDiff extends Component<SafePatchDiffProps, SafePatchDiffState> {
  state: SafePatchDiffState = { error: null };

  static getDerivedStateFromError(error: unknown): SafePatchDiffState {
    return { error: error instanceof Error ? error.message : String(error) };
  }

  componentDidCatch() {}

  componentDidUpdate(prevProps: SafePatchDiffProps) {
    if ((prevProps.patch !== this.props.patch || prevProps.diffStyle !== this.props.diffStyle) && this.state.error) {
      this.setState({ error: null });
    }
  }

  render() {
    if (this.state.error) {
      return (
        <Stack gap="sm">
          <Alert color="yellow">The rich diff renderer failed for this patch. Showing the raw patch instead.</Alert>
          <Text size="xs" c="dimmed">{this.state.error}</Text>
          <Code block>{this.props.patch}</Code>
        </Stack>
      );
    }

    return (
      <PatchDiff
        patch={this.props.patch}
        options={{ theme: { dark: 'pierre-dark', light: 'pierre-light' }, diffStyle: this.props.diffStyle }}
      />
    );
  }
}

type SafeFileDiffProps = {
  fileDiff: FileDiffMetadata;
  patch: string;
  diffStyle: DiffStyle;
};

type SafeFileDiffState = {
  error: string | null;
};

class SafeFileDiff extends Component<SafeFileDiffProps, SafeFileDiffState> {
  state: SafeFileDiffState = { error: null };

  static getDerivedStateFromError(error: unknown): SafeFileDiffState {
    return { error: error instanceof Error ? error.message : String(error) };
  }

  componentDidCatch() {}

  componentDidUpdate(prevProps: SafeFileDiffProps) {
    if ((prevProps.fileDiff !== this.props.fileDiff || prevProps.patch !== this.props.patch || prevProps.diffStyle !== this.props.diffStyle) && this.state.error) {
      this.setState({ error: null });
    }
  }

  render() {
    if (this.state.error) {
      return (
        <Stack gap="sm">
          <Alert color="yellow">The rich diff renderer failed for this patch. Showing the raw patch instead.</Alert>
          <Text size="xs" c="dimmed">{this.state.error}</Text>
          <Code block>{this.props.patch}</Code>
        </Stack>
      );
    }

    return (
      <FileDiff
        fileDiff={this.props.fileDiff}
        options={{ theme: { dark: 'pierre-dark', light: 'pierre-light' }, diffStyle: this.props.diffStyle }}
      />
    );
  }
}

function CommitAnalyticsChart(props: { report: ReviewCommitReportResponse | null; mode: CommitAnalyticsMode }) {
  const { report, mode } = props;
  const months = report?.months ?? [];
  const [hoveredMonth, setHoveredMonth] = useState<string | null>(null);
  const [tooltipPosition, setTooltipPosition] = useState({ x: 0, y: 0 });
  const chartContainerRef = useRef<HTMLDivElement | null>(null);
  const [chartContainerWidth, setChartContainerWidth] = useState(0);

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
      setChartContainerWidth(Math.floor(node.getBoundingClientRect().width));
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

  const availableWidth = Math.max(1280, chartContainerWidth - 8);
  const width = Math.max(availableWidth, months.length * 190 + 160);
  const height = Math.max(660, Math.min(820, Math.round(width * 0.38)));
  const margin = { top: 34, right: 36, bottom: 92, left: 92 };
  const plotWidth = width - margin.left - margin.right;
  const plotHeight = height - margin.top - margin.bottom;
  const domainSpan = Math.max(1, chart.domainMax - chart.domainMin);
  const yScale = (value: number) => margin.top + ((chart.domainMax - value) / domainSpan) * plotHeight;
  const zeroY = yScale(0);
  const barBand = plotWidth / Math.max(1, months.length);
  const barWidth = Math.max(72, Math.min(180, barBand * 0.58));

  return (
    <Stack gap="sm" style={{ height: '100%', minHeight: 0 }}>
      <Group gap="xs">
        <Badge variant="light">{report?.commits.length ?? 0} commits shown</Badge>
        <Badge color="green" variant="light">+{months.reduce((sum, month) => sum + month.additions, 0)}</Badge>
        <Badge color="red" variant="light">-{months.reduce((sum, month) => sum + month.deletions, 0)}</Badge>
        <Badge variant="light">{months.length} months</Badge>
        <Badge variant="light">{mode === 'net' ? 'Net LOC' : 'Additions / deletions'}</Badge>
      </Group>

      <Box ref={chartContainerRef} style={{ position: 'relative', flex: 1, minHeight: 620, width: '100%', alignSelf: 'stretch' }}>
        <ScrollArea type="auto" style={{ height: '100%', minHeight: 620, width: '100%' }}>
          <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`} role="img" aria-label="Monthly LOC by file extension" preserveAspectRatio="none" style={{ display: 'block' }}>
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
                  <text x={x + barWidth / 2} y={height - margin.bottom + 28} textAnchor="middle" fontSize="13" fill="rgba(255,255,255,0.78)">{month.month}</text>
                  <text x={x + barWidth / 2} y={height - margin.bottom + 50} textAnchor="middle" fontSize="12" fontWeight={700} fill={month.net >= 0 ? 'rgba(105,219,124,0.98)' : 'rgba(255,135,135,0.98)'}>{month.net >= 0 ? '+' : ''}{month.net}</text>
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

      <Group gap={6}>
        {chart.orderedExtensions.slice(0, 20).map((extension) => (
          <Badge key={extension} size="xs" variant="outline" style={{ borderColor: colorForExtension(extension), color: colorForExtension(extension) }}>{extension}</Badge>
        ))}
      </Group>
    </Stack>
  );
}

function CommitFileRow(props: {
  file: ReviewDiffManifestFileEntry;
  active: boolean;
  onSelect: () => void;
}) {
  const { file, active, onSelect } = props;
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
          <Text size="sm" fw={active ? 700 : 500} style={{ wordBreak: 'break-word' }}>{file.path}</Text>
        </Group>
        <Group gap={6} wrap="nowrap">
          <Badge color="green" variant="light">+{file.additions}</Badge>
          <Badge color="red" variant="light">-{file.deletions}</Badge>
          <Button
            size="compact-xs"
            variant={active ? 'filled' : 'light'}
            onClick={(event) => {
              event.stopPropagation();
              onSelect();
            }}
          >
            {active ? 'Viewing' : 'View diff'}
          </Button>
        </Group>
      </Group>
    </Box>
  );
}

export function CommitSummaryPanel(props: CommitSummaryPanelProps) {
  const { repoRef } = props;
  const [commits, setCommits] = useState<ReviewCommitSummary[]>([]);
  const [commitReportType, setCommitReportType] = useState<CommitReportType>('commits');
  const [commitReport, setCommitReport] = useState<ReviewCommitReportResponse | null>(null);
  const [commitRefOptions, setCommitRefOptions] = useState<ReviewCommitRefOption[]>([]);
  const [commitAnalyticsMode, setCommitAnalyticsMode] = useState<CommitAnalyticsMode>('activity');
  const [commitAggregationWindow, setCommitAggregationWindow] = useState<CommitAggregationWindow>('monthly');
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
  const [expandedSha, setExpandedSha] = useState<string | null>(null);
  const [selectedCommit, setSelectedCommit] = useState<ReviewCommitSummary | null>(null);
  const [manifestBySha, setManifestBySha] = useState<Record<string, ReviewCommitDiffManifestResponse>>({});
  const [diffManifest, setDiffManifest] = useState<ReviewCommitDiffManifestResponse | null>(null);
  const [diff, setDiff] = useState<ReviewCommitDiffResponse | null>(null);
  const [filePatchByPath, setFilePatchByPath] = useState<Record<string, string>>({});
  const [filePatchBusyByPath, setFilePatchBusyByPath] = useState<Record<string, boolean>>({});
  const [collapsedByPath, setCollapsedByPath] = useState<Record<string, boolean>>({});
  const [reviewState, setReviewState] = useState<CommitReviewState>(DEFAULT_REVIEW_STATE);
  const [reviewOpen, setReviewOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [diffBusy, setDiffBusy] = useState(false);
  const [manifestBusySha, setManifestBusySha] = useState<string | null>(null);
  const [nextCommitOffset, setNextCommitOffset] = useState<number | null>(0);
  const [loadingMoreCommits, setLoadingMoreCommits] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const refreshDiffRequestIdRef = useRef(0);
  const commitScrollViewportRef = useRef<HTMLDivElement | null>(null);
  const commitItemNodeByShaRef = useRef<Record<string, HTMLDivElement | null>>({});
  const commitItemResizeObserverByShaRef = useRef<Record<string, ResizeObserver | undefined>>({});
  const commitItemMeasureRefByShaRef = useRef<Record<string, (node: HTMLDivElement | null) => void>>({});
  const [commitListViewportHeight, setCommitListViewportHeight] = useState(0);
  const [commitListScrollTop, setCommitListScrollTop] = useState(0);
  const [commitItemHeightBySha, setCommitItemHeightBySha] = useState<Record<string, number>>({});

  const commitDatasetFilters = useMemo(() => ({
    ref_name: commitReportRefName.trim() || null,
    aggregation_window: commitAggregationWindow,
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
    commitAggregationWindow,
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
    return Math.max(0, commitLinearHeight - commitListViewportHeight * 2);
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

  const selectedFilePatch = useMemo(() => {
    if (!reviewState.selected_path || !diff?.patch?.trim()) return '';
    return diff.patch;
  }, [reviewState.selected_path, diff?.patch]);

  const selectedFilePayloadInfo = useMemo(() => {
    if (!reviewState.selected_path || !diff?.patch?.trim()) {
      return { fileCount: 0, containsSelectedFile: false, isExactSelectedFilePayload: false, selectedUnifiedLineCount: 0 };
    }

    try {
      const files = parsePatchFiles(diff.patch).flatMap((patch) => patch.files ?? []);
      const selected = files.find((file) => file.name === reviewState.selected_path) ?? null;
      return {
        fileCount: files.length,
        containsSelectedFile: files.some((file) => file.name === reviewState.selected_path),
        isExactSelectedFilePayload: files.length === 1 && selected?.name === reviewState.selected_path,
        selectedUnifiedLineCount: selected?.unifiedLineCount ?? 0,
      };
    } catch {
      return { fileCount: 0, containsSelectedFile: false, isExactSelectedFilePayload: false, selectedUnifiedLineCount: 0 };
    }
  }, [reviewState.selected_path, diff?.patch]);

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

  const commitDiffRows = useMemo(() => {
    return (diffManifest?.files ?? []).map((file) => ({
      file,
      parsed: parsedFileDiffByPath[file.path] ?? null,
      patch: filePatchByPath[file.path] ?? '',
    }));
  }, [diffManifest, parsedFileDiffByPath, filePatchByPath]);

  const manifestFiles = diffManifest?.files ?? [];
  const manifestTotals = useMemo(() => sumCounts(manifestFiles), [manifestFiles]);
  const hasCommitDiffRows = commitDiffRows.length > 0;
  const allCommitRowsCollapsed = hasCommitDiffRows && commitDiffRows.every(({ file }) => collapsedByPath[file.path] !== false);

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
          commitAggregationWindow,
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
    downloadTextFile(`commit-analytics-${safeRef}-${commitAggregationWindow}-${commitAnalyticsColorBy}.csv`, csv, 'text/csv;charset=utf-8');
  }

  async function loadCommitPage(offset: number, append: boolean) {
    if (!repoRef.trim()) return;
    try {
      if (append) setLoadingMoreCommits(true);
      else {
        setBusy(true);
        setCommitReportBusy(true);
        setNextCommitOffset(0);
      }
      setError(null);
      const json = await getReviewCommitReport({
        repo_ref: repoRef,
        limit: COMMIT_PAGE_SIZE,
        offset: append ? offset : 0,
        ...commitDatasetFilters,
      });
      setCommitReport(json);
      setCommits((current) => {
        if (!append) return json.commits;
        const seen = new Set(current.map((commit) => commit.sha));
        return [...current, ...json.commits.filter((commit) => !seen.has(commit.sha))];
      });
      setNextCommitOffset(json.has_more ? json.next_offset ?? offset + json.commits.length : null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
      setCommitReportBusy(false);
      setLoadingMoreCommits(false);
    }
  }

  async function refreshCommits() {
    await loadCommitPage(0, false);
  }

  async function loadMoreCommits() {
    if (nextCommitOffset === null || busy || loadingMoreCommits) return;
    await loadCommitPage(nextCommitOffset, true);
  }

  async function refreshCommitReport() {
    await loadCommitPage(0, false);
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

  async function refreshCommitDiff(commit: ReviewCommitSummary, nextState: CommitReviewState) {
    if (!repoRef.trim()) return;
    const requestId = ++refreshDiffRequestIdRef.current;

    try {
      setDiffBusy(true);
      setError(null);
      const manifest = await ensureManifest(commit);
      if (refreshDiffRequestIdRef.current !== requestId) return;
      setDiffManifest(manifest);
      setSelectedCommit(commit);

      if (nextState.selected_path) {
        const json = await getReviewCommitDiff({
          repo_ref: repoRef,
          commit: commit.sha,
          path: nextState.selected_path,
          context_lines: nextState.whole_file ? 1000 : clampContextLines(nextState.context_lines),
          whole_file: nextState.whole_file,
        });
        if (refreshDiffRequestIdRef.current !== requestId) return;
        setDiff(json);
        setFilePatchByPath({});
        setFilePatchBusyByPath({});
        setCollapsedByPath({});
        return;
      }

      setDiff(null);
      setFilePatchByPath({});
      setFilePatchBusyByPath(Object.fromEntries(manifest.files.map((file) => [file.path, true])));
      setCollapsedByPath(Object.fromEntries(manifest.files.map((file) => [file.path, false])));

      const patchEntries = await Promise.all(
        manifest.files.map(async (file) => {
          try {
            const json = await getReviewCommitDiff({
              repo_ref: repoRef,
              commit: commit.sha,
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

      if (refreshDiffRequestIdRef.current !== requestId) return;
      setFilePatchByPath(Object.fromEntries(patchEntries));
      setFilePatchBusyByPath({});
    } catch (err) {
      if (refreshDiffRequestIdRef.current !== requestId) return;
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (refreshDiffRequestIdRef.current === requestId) setDiffBusy(false);
    }
  }

  async function openCommitReview(commit: ReviewCommitSummary, selectedPath: string | null) {
    const nextState = { ...reviewState, selected_path: selectedPath };
    setSelectedCommit(commit);
    setReviewState(nextState);
    setReviewOpen(true);
    await refreshCommitDiff(commit, nextState);
  }

  async function patchReviewState(patch: Partial<CommitReviewState>) {
    const nextState: CommitReviewState = {
      ...reviewState,
      ...patch,
      context_lines: patch.context_lines === undefined ? reviewState.context_lines : clampContextLines(patch.context_lines),
    };
    setReviewState(nextState);
    if (selectedCommit && reviewOpen) await refreshCommitDiff(selectedCommit, nextState);
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

  function setAllCommitRowsCollapsed(collapsed: boolean) {
    setCollapsedByPath(Object.fromEntries(commitDiffRows.map(({ file }) => [file.path, collapsed])));
  }

  function toggleCommitRowCollapsed(path: string) {
    setCollapsedByPath((current) => ({ ...current, [path]: !(current[path] ?? false) }));
  }

  useEffect(() => {
    if (!repoRef.trim()) return;
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
    return () => {
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

  const reviewContent = selectedCommit ? (
    <Box style={{ height: '100%', display: 'grid', gridTemplateColumns: 'minmax(0, 1fr) 360px', gap: 12, minHeight: 0 }}>
      <Card withBorder p="sm" style={{ minHeight: 0, display: 'flex', flexDirection: 'column' }}>
        <Group justify="space-between" align="center" mb="sm">
          <Stack gap={2} style={{ minWidth: 0 }}>
            <Group gap="xs" wrap="nowrap">
              <Badge variant="outline">{selectedCommit.short_sha}</Badge>
              {diffManifest ? <Badge variant="light">{diffManifest.from_ref.slice(0, 8)} → {diffManifest.to_ref.slice(0, 8)}</Badge> : null}
            </Group>
            <Text fw={700} style={{ wordBreak: 'break-word' }}>{reviewState.selected_path ?? selectedCommit.subject}</Text>
          </Stack>
          <Group gap="xs">
            {!reviewState.selected_path && hasCommitDiffRows ? (
              <Button size="xs" variant="default" onClick={() => setAllCommitRowsCollapsed(!allCommitRowsCollapsed)}>
                {allCommitRowsCollapsed ? 'Expand all' : 'Collapse all'}
              </Button>
            ) : null}
            <Button size="xs" variant="default" onClick={() => setReviewOpen(false)}>Close</Button>
          </Group>
        </Group>
        <Divider mb="sm" />
        <Box style={{ flex: 1, minHeight: 0 }}>
          {diffBusy ? (
            <Group justify="center" py="xl"><Loader /></Group>
          ) : error ? (
            <Alert color="red">{error}</Alert>
          ) : reviewState.selected_path ? (
            !selectedFilePayloadInfo.containsSelectedFile ? (
              <Alert color="yellow" title="Selected file diff unavailable">
                <Stack gap="sm">
                  <Text size="sm">The selected file was not found in the current diff payload.</Text>
                  <Group><Button size="xs" variant="filled" onClick={() => void patchReviewState({ selected_path: null })}>Open whole commit</Button></Group>
                </Stack>
              </Alert>
            ) : !selectedFilePayloadInfo.isExactSelectedFilePayload ? (
              <Alert color="yellow" title="Selected file diff is stale">
                <Stack gap="sm">
                  <Text size="sm">The current payload contains {selectedFilePayloadInfo.fileCount} file diffs, so it is not safe to render in selected-file mode.</Text>
                  <Group><Button size="xs" variant="filled" onClick={() => void patchReviewState({ selected_path: null })}>Open whole commit</Button></Group>
                </Stack>
              </Alert>
            ) : selectedFilePayloadInfo.selectedUnifiedLineCount > LARGE_SINGLE_FILE_RENDER_LINE_LIMIT ? (
              <Alert color="yellow" title="Large single-file diff">
                <Stack gap="sm">
                  <Text size="sm">This patch is too large for the non-virtualized single-file renderer.</Text>
                  <Group>
                    <Button size="xs" variant="filled" onClick={() => void patchReviewState({ selected_path: null })}>Open whole commit</Button>
                    <Button size="xs" variant="default" onClick={() => void patchReviewState({ whole_file: false })}>Reduce context</Button>
                  </Group>
                </Stack>
              </Alert>
            ) : selectedFilePatch ? (
              <ScrollArea h="100%" type="auto">
                <Box p={0} style={{ overflow: 'hidden' }}>
                  <SafePatchDiff patch={selectedFilePatch} diffStyle={reviewState.diff_style} />
                </Box>
              </ScrollArea>
            ) : (
              <Text size="sm" c="dimmed">No diff available for this commit selection.</Text>
            )
          ) : hasCommitDiffRows ? (
            <ScrollArea h="100%" type="auto">
              <Box p="xs" style={{ minHeight: '100%' }}>
                <Virtualizer contentStyle={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
                  {commitDiffRows.map(({ file, parsed, patch }) => {
                    const collapsed = collapsedByPath[file.path] ?? false;
                    return (
                      <Card key={file.path} withBorder p={0} style={{ overflow: 'hidden' }}>
                        <Box px="sm" py="xs" style={{ position: 'sticky', top: 0, zIndex: 2, background: 'var(--mantine-color-body)', borderBottom: '1px solid rgba(255,255,255,0.08)' }}>
                          <Group justify="space-between" wrap="nowrap" gap="xs">
                            <Group gap="xs" wrap="nowrap" style={{ minWidth: 0, flex: 1 }}>
                              <Badge variant="outline">{statusCode(file)}</Badge>
                              <Button size="compact-xs" variant="subtle" onClick={() => toggleCommitRowCollapsed(file.path)}>{collapsed ? 'Expand' : 'Collapse'}</Button>
                              <Text size="sm" fw={600} style={{ wordBreak: 'break-word' }}>{file.path}</Text>
                            </Group>
                            <Group gap="xs" wrap="nowrap">
                              <Badge color="green" variant="light">+{file.additions}</Badge>
                              <Badge color="red" variant="light">-{file.deletions}</Badge>
                            </Group>
                          </Group>
                        </Box>
                        {!collapsed ? (
                          parsed ? (
                            <Box p={0} style={{ overflow: 'hidden' }}><SafeFileDiff fileDiff={parsed} patch={patch} diffStyle={reviewState.diff_style} /></Box>
                          ) : filePatchBusyByPath[file.path] ? (
                            <Box p="md"><Group justify="center" py="lg"><Loader size="sm" /></Group></Box>
                          ) : patch ? (
                            <Box p="md"><Code block>{patch}</Code></Box>
                          ) : (
                            <Box p="md"><Text c="dimmed" size="sm">No diff available.</Text></Box>
                          )
                        ) : null}
                      </Card>
                    );
                  })}
                </Virtualizer>
              </Box>
            </ScrollArea>
          ) : (
            <Text size="sm" c="dimmed">No diff available for this commit selection.</Text>
          )}
        </Box>
      </Card>

      <Card withBorder p="sm" style={{ minHeight: 0, display: 'flex', flexDirection: 'column' }}>
        <Group justify="space-between" mb="sm">
          <Text fw={700}>Diff browser</Text>
          <Button size="compact-xs" variant="subtle" onClick={() => setReviewOpen(false)}>Hide</Button>
        </Group>
        <SegmentedControl
          value={reviewState.diff_style}
          onChange={(value) => void patchReviewState({ diff_style: value as DiffStyle })}
          data={[{ label: 'Unified', value: 'unified' }, { label: 'Split', value: 'split' }]}
          fullWidth
        />
        <Group gap="md" mt="sm">
          <Checkbox checked={reviewState.only_changes} onChange={(event) => void patchReviewState({ only_changes: event.currentTarget.checked })} label="Only show changes" />
          <Checkbox checked={reviewState.whole_file} onChange={(event) => void patchReviewState({ whole_file: event.currentTarget.checked })} label="Whole file" />
        </Group>
        <NumberInput
          label="Context lines"
          min={0}
          max={1000}
          step={1}
          value={reviewState.context_lines}
          disabled={reviewState.whole_file}
          onChange={(value) => void patchReviewState({ context_lines: clampContextLines(value) })}
          mt="sm"
        />
        <Divider my="sm" />
        <Button
          variant={reviewState.selected_path === null ? 'filled' : 'default'}
          onClick={() => void patchReviewState({ selected_path: null })}
          style={{ justifyContent: 'space-between' }}
        >
          <Group gap="xs" wrap="nowrap">
            <Text fw={700} size="sm">Commit</Text>
            <Badge variant="light">{manifestFiles.length}</Badge>
            <Badge color="green" variant="light">+{manifestTotals.additions}</Badge>
            <Badge color="red" variant="light">-{manifestTotals.deletions}</Badge>
          </Group>
        </Button>
        <ScrollArea mt="sm" style={{ flex: 1, minHeight: 0 }} type="auto">
          <Stack gap="xs">
            {manifestFiles.map((file) => (
              <CommitFileRow
                key={file.path}
                file={file}
                active={reviewState.selected_path === file.path}
                onSelect={() => void patchReviewState({ selected_path: file.path })}
              />
            ))}
          </Stack>
        </ScrollArea>
      </Card>
    </Box>
  ) : null;

  return (
    <>
      <Box style={{ height: 'calc(100dvh - 96px)', minHeight: 0, overflow: 'hidden' }}>
        <Card withBorder p="sm" style={{ height: '100%', minHeight: 0, overflow: 'hidden', display: 'flex', flexDirection: 'column' }}>
          <Group justify="space-between" align="flex-start" mb="sm">
            <Stack gap={6} style={{ flex: 1 }}>
              <Stack gap={0}>
                <Text fw={700}>Commit summary</Text>
                <Text size="xs" c="dimmed">Compact history and monthly net LOC analytics</Text>
              </Stack>
              <Group gap="xs" align="end">
                <SegmentedControl
                  size="xs"
                  value={commitReportType}
                  onChange={(value) => setCommitReportType(value as CommitReportType)}
                  data={[{ label: 'Commits', value: 'commits' }, { label: 'Analytics', value: 'analytics' }]}
                />
                <SegmentedControl
                  size="xs"
                  value={commitAnalyticsMode}
                  onChange={(value) => setCommitAnalyticsMode(value as CommitAnalyticsMode)}
                  data={[{ label: 'Activity', value: 'activity' }, { label: 'Net', value: 'net' }]}
                  disabled={commitReportType !== 'analytics'}
                />
                <SegmentedControl
                  size="xs"
                  value={commitAggregationWindow}
                  onChange={(value) => setCommitAggregationWindow(value as CommitAggregationWindow)}
                  data={[{ label: 'Daily', value: 'daily' }, { label: 'Monthly', value: 'monthly' }, { label: 'Yearly', value: 'yearly' }]}
                  disabled={commitReportType !== 'analytics'}
                />
                <SegmentedControl
                  size="xs"
                  value={commitAnalyticsColorBy}
                  onChange={(value) => setCommitAnalyticsColorBy(value as CommitAnalyticsColorBy)}
                  data={[{ label: 'Ext', value: 'extension' }, { label: 'Author', value: 'author' }]}
                  disabled={commitReportType !== 'analytics'}
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
                <TextInput size="xs" label="Since" placeholder="2025-01-01" value={commitReportSince} onChange={(event) => setCommitReportSince(event.currentTarget.value)} />
                <TextInput size="xs" label="Until" placeholder="2025-02-28" value={commitReportUntil} onChange={(event) => setCommitReportUntil(event.currentTarget.value)} />
                <TextInput size="xs" label="Include paths" placeholder="api/src, web/src" value={commitReportIncludePathsText} onChange={(event) => setCommitReportIncludePathsText(event.currentTarget.value)} />
                <TextInput size="xs" label="Exclude paths" placeholder="target, node_modules" value={commitReportExcludePathsText} onChange={(event) => setCommitReportExcludePathsText(event.currentTarget.value)} />
                <TextInput size="xs" label="Include ext" placeholder="rs, ts, tsx" value={commitReportIncludeExtensionsText} onChange={(event) => setCommitReportIncludeExtensionsText(event.currentTarget.value)} />
                <TextInput size="xs" label="Exclude ext" placeholder="lock, map" value={commitReportExcludeExtensionsText} onChange={(event) => setCommitReportExcludeExtensionsText(event.currentTarget.value)} />
                <TextInput size="xs" label="Include regex" style={{ minWidth: 220 }} value={commitReportIncludeRegexText} onChange={(event) => setCommitReportIncludeRegexText(event.currentTarget.value)} />
                <TextInput size="xs" label="Exclude regex" style={{ minWidth: 320 }} value={commitReportExcludeRegexText} onChange={(event) => setCommitReportExcludeRegexText(event.currentTarget.value)} />

              </Group>
            </Stack>
            <Button
              size="xs"
              variant="default"
              loading={busy || commitReportBusy}
              onClick={() => void refreshCommitReport()}
            >
              Apply filters
            </Button>
            <Button
              size="xs"
              variant="default"
              disabled={!commitReport}
              onClick={exportCommitAnalyticsCsv}
            >
              Export CSV
            </Button>
          </Group>
          {error && !reviewOpen ? <Alert color="red" mb="sm">{error}</Alert> : null}
          <Divider mb="sm" />
          {commitReportType === 'analytics' ? (
            (commitReportBusy || busy) && !commitReport ? (
              <Group justify="center" py="xl"><Loader /></Group>
            ) : (
              <Box style={{ flex: 1, minHeight: 0 }}>
                <CommitAnalyticsChart report={commitReport} mode={commitAnalyticsMode} />
              </Box>
            )
          ) : busy && commits.length === 0 ? (
            <Group justify="center" py="xl"><Loader /></Group>
          ) : (
            <ScrollArea style={{ flex: 1, minHeight: 0, width: '100%', overflow: 'hidden' }} type="auto" scrollbarSize={commitLinearHeight <= commitListViewportHeight * 2 ? 0 : undefined} viewportRef={commitScrollViewportRef}>
              <Box style={{ position: 'relative', minHeight: commitVirtualHeight, width: '100%' }}>
                <Box style={{ position: 'absolute', top: commitVisibleTop, left: 0, right: 0, display: 'grid', gridTemplateColumns: 'repeat(2, minmax(0, 1fr))', gap: 8, alignItems: 'start', width: '100%' }}>
                  {[leftCommits, rightCommits].map((columnCommits, columnIndex) => (
                    <Stack key={columnIndex} gap="xs" style={{ width: '100%' }}>
                      {columnCommits.map((commit, rowIndex) => {
                  const expanded = expandedSha === commit.sha;
                  const manifest = manifestBySha[commit.sha];
                  const fileCount = commit.files_changed;
                  const additions = commit.additions;
                  const deletions = commit.deletions;
                  const hasStats = typeof fileCount === 'number' && typeof additions === 'number' && typeof deletions === 'number';
                  return (
                    <Card ref={setCommitItemMeasureRef(commit.sha)} key={commit.sha} withBorder p={6} style={{ background: selectedCommit?.sha === commit.sha && reviewOpen ? 'rgba(34, 139, 230, 0.10)' : undefined }}>
                      <Group justify="space-between" align="center" wrap="nowrap" gap="xs">
                        <Group gap={6} wrap="nowrap" style={{ minWidth: 0, flex: 1 }}>
                          <Button size="compact-xs" variant="subtle" onClick={() => void toggleExpanded(commit)}>{expanded ? '▾' : '▸'}</Button>
                          <Badge size="xs" variant="outline">{commit.short_sha}</Badge>
                          <Badge size="xs" variant="light">{formatCommitDate(commit.authored_at)}</Badge>
                          <Text size="sm" fw={600} truncate style={{ minWidth: 0, flex: 1 }} title={commit.subject}>{commit.subject}</Text>
                          <Text size="xs" c="dimmed" truncate style={{ maxWidth: 120 }}>{commit.author_name}</Text>
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
                          <Button size="compact-xs" variant="filled" onClick={() => void openCommitReview(commit, null)}>Review</Button>
                        </Group>
                      </Group>
                      {expanded ? (
                        manifestBusySha === commit.sha && !manifest ? (
                          <Group justify="center" py="sm"><Loader size="sm" /></Group>
                        ) : manifest ? (
                          <Stack gap={4} mt={6}>
                            {manifest.files.map((file) => (
                              <Box
                                key={`${commit.sha}:${file.path}`}
                                onClick={() => void openCommitReview(commit, file.path)}
                                style={{
                                  cursor: 'pointer',
                                  padding: '4px 6px',
                                  borderRadius: 6,
                                  background: selectedCommit?.sha === commit.sha && reviewState.selected_path === file.path && reviewOpen ? 'rgba(34, 139, 230, 0.16)' : 'rgba(255,255,255,0.02)',
                                  border: '1px solid rgba(255,255,255,0.05)'
                                }}
                              >
                                <Group justify="space-between" wrap="nowrap">
                                  <Group gap={6} wrap="nowrap" style={{ minWidth: 0, flex: 1 }}>
                                    <Badge size="xs" variant="outline">{statusCode(file)}</Badge>
                                    <Text size="xs" truncate>{file.path}</Text>
                                  </Group>
                                  <Group gap={4} wrap="nowrap">
                                    <Badge size="xs" color="green" variant="light">+{file.additions}</Badge>
                                    <Badge size="xs" color="red" variant="light">-{file.deletions}</Badge>
                                  </Group>
                                </Group>
                              </Box>
                            ))}
                          </Stack>
                        ) : null
                      ) : null}
                    </Card>
                  );
                      })}
                    </Stack>
                  ))}
                </Box>
              </Box>
              {nextCommitOffset !== null ? (
                <Button size="xs" variant="default" loading={loadingMoreCommits} onClick={() => void loadMoreCommits()}>Load older commits</Button>
              ) : commits.length > 0 ? (
                <Text size="xs" c="dimmed" ta="center">End of commit history</Text>
              ) : null}
            </ScrollArea>
          )}
        </Card>
      </Box>
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
