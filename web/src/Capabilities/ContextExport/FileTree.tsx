import { useMemo, useState } from 'react';
import { ActionIcon, Box, Button, Group, ScrollArea, Stack, Text, TextInput, Tooltip } from '@mantine/core';
import {
  IconChevronDown,
  IconChevronRight,
  IconFile,
  IconFolder,
  IconFolderOpen,
  IconSearch
} from '@tabler/icons-react';
import type { RepoTreeEntry } from '../../api';

export type FileTreePathState = 'included' | 'excluded' | 'mixed' | 'neutral';

export type FileTreeEntry = RepoTreeEntry & {
  state?: FileTreePathState;
  reason?: string;
  selected?: boolean;
};

export type FileTreeProps = {
  entries: FileTreeEntry[];
  onDirectoryClick?: (path: string) => void;
  onFileClick?: (path: string) => void;
  searchable?: boolean;
  showExpandCollapse?: boolean;
  showLegend?: boolean;
};

function normalize(path: string) {
  return path.trim().replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
}

function parentPath(path: string) {
  const index = path.lastIndexOf('/');
  return index === -1 ? '' : path.slice(0, index);
}

function nameFromPath(path: string) {
  const index = path.lastIndexOf('/');
  return index === -1 ? path : path.slice(index + 1);
}

function isWithin(path: string, directory: string) {
  return path === directory || path.startsWith(`${directory}/`);
}

function statusColor(state: FileTreePathState) {
  if (state === 'included') return 'var(--mantine-color-green-6)';
  if (state === 'excluded') return 'var(--mantine-color-red-6)';
  if (state === 'mixed') return 'var(--mantine-color-yellow-6)';
  return 'transparent';
}

function statusTextColor(state: FileTreePathState, selected: boolean) {
  if (state === 'excluded') return 'red.5';
  if (selected && state === 'included') return 'green.5';
  return undefined;
}

function createTreeEntries(entries: FileTreeEntry[]) {
  const byPath = new Map<string, FileTreeEntry>();

  for (const sourceEntry of entries) {
    const path = normalize(sourceEntry.path);
    if (!path) continue;

    byPath.set(path, {
      ...sourceEntry,
      path,
      name: sourceEntry.name || nameFromPath(path)
    });

    const parts = path.split('/');
    for (let index = 1; index < parts.length; index += 1) {
      const directoryPath = parts.slice(0, index).join('/');
      if (!byPath.has(directoryPath)) {
        byPath.set(directoryPath, {
          path: directoryPath,
          name: parts[index - 1],
          kind: 'dir',
          has_children: true
        });
      }
    }
  }

  const children = new Map<string, FileTreeEntry[]>();

  for (const entry of byPath.values()) {
    const parent = parentPath(entry.path);
    const siblings = children.get(parent) ?? [];
    siblings.push(entry);
    children.set(parent, siblings);
  }

  for (const siblings of children.values()) {
    siblings.sort((left, right) => {
      if (left.kind !== right.kind) return left.kind === 'dir' ? -1 : 1;
      return left.name.localeCompare(right.name);
    });
  }

  return {
    entries: Array.from(byPath.values()),
    children
  };
}

export function FileTree(props: FileTreeProps) {
  const [collapsedPaths, setCollapsedPaths] = useState<Set<string>>(new Set());
  const [filter, setFilter] = useState('');

  const tree = useMemo(() => createTreeEntries(props.entries), [props.entries]);

  const directoryPaths = useMemo(
    () => tree.entries.filter((entry) => entry.kind === 'dir').map((entry) => entry.path),
    [tree.entries]
  );

  const normalizedFilter = filter.trim().toLowerCase();

  const pathMatchesFilter = (path: string) => {
    if (!normalizedFilter) return true;
    return path.toLowerCase().includes(normalizedFilter);
  };

  const directoryContainsMatch = (path: string) => {
    if (!normalizedFilter) return true;
    return tree.entries.some((entry) => isWithin(entry.path, path) && pathMatchesFilter(entry.path));
  };

  const toggleExpanded = (path: string) => {
    setCollapsedPaths((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const expandAll = () => {
    setCollapsedPaths(new Set());
  };

  const collapseAll = () => {
    setCollapsedPaths(new Set(directoryPaths));
  };

  const renderEntries = (parent: string, depth: number): React.ReactNode => {
    return (tree.children.get(parent) ?? []).flatMap((entry) => {
      const isDirectory = entry.kind === 'dir';
      const visible = isDirectory ? directoryContainsMatch(entry.path) : pathMatchesFilter(entry.path);
      if (!visible) return [];

      const isExpanded = !collapsedPaths.has(entry.path) || Boolean(normalizedFilter);
      const state = entry.state ?? 'neutral';
      const selected = Boolean(entry.selected);
      const onClick = isDirectory ? props.onDirectoryClick : props.onFileClick;

      return [
        <Stack key={entry.path} gap={0}>
          <Tooltip
            label={entry.reason ?? entry.path}
            disabled={!entry.reason}
            openDelay={350}
            position="right"
          >
            <Group
              gap={5}
              wrap="nowrap"
              px={6}
              style={{
                minHeight: 28,
                marginLeft: depth * 16,
                borderLeft: depth > 0 ? '1px solid var(--mantine-color-default-border)' : undefined,
                borderRadius: 4,
                cursor: onClick ? 'pointer' : 'default',
                background: selected ? 'var(--mantine-color-default-hover)' : undefined
              }}
              onClick={() => onClick?.(entry.path)}
              onMouseEnter={(event) => {
                if (onClick) event.currentTarget.style.background = 'var(--mantine-color-default-hover)';
              }}
              onMouseLeave={(event) => {
                if (onClick) {
                  event.currentTarget.style.background = selected
                    ? 'var(--mantine-color-default-hover)'
                    : 'transparent';
                }
              }}
            >
              <Box
                style={{
                  width: 3,
                  height: 16,
                  borderRadius: 2,
                  background: statusColor(state),
                  flexShrink: 0
                }}
              />
              {isDirectory ? (
                <ActionIcon
                  variant="transparent"
                  size="sm"
                  onClick={(event) => {
                    event.stopPropagation();
                    toggleExpanded(entry.path);
                  }}
                >
                  {isExpanded ? <IconChevronDown size={13} /> : <IconChevronRight size={13} />}
                </ActionIcon>
              ) : (
                <Box w={28} />
              )}
              <Box c="dimmed" style={{ display: 'flex', flexShrink: 0 }}>
                {isDirectory ? (
                  isExpanded ? <IconFolderOpen size={15} /> : <IconFolder size={15} />
                ) : (
                  <IconFile size={14} />
                )}
              </Box>
              <Text
                size="sm"
                ff="monospace"
                truncate
                fw={selected ? 600 : 400}
                c={statusTextColor(state, selected)}
                style={{ flex: 1 }}
              >
                {entry.name}
              </Text>
              {state !== 'neutral' ? (
                <Box
                  aria-label={state}
                  style={{
                    width: 7,
                    height: 7,
                    borderRadius: '50%',
                    background: statusColor(state),
                    flexShrink: 0
                  }}
                />
              ) : null}
            </Group>
          </Tooltip>
          {isDirectory && isExpanded ? renderEntries(entry.path, depth + 1) : null}
        </Stack>
      ];
    });
  };

  return (
    <Stack gap="xs" h="100%" style={{ minHeight: 0, flex: 1 }}>
      {props.searchable !== false || props.showExpandCollapse !== false ? (
        <Group gap="xs" wrap="nowrap">
          {props.searchable !== false ? (
            <TextInput
              value={filter}
              onChange={(event) => setFilter(event.currentTarget.value)}
              placeholder="Filter paths"
              leftSection={<IconSearch size={14} />}
              size="xs"
              style={{ flex: 1 }}
            />
          ) : null}
          {props.showExpandCollapse !== false ? (
            <>
              <Button size="xs" variant="subtle" onClick={expandAll}>Expand all</Button>
              <Button size="xs" variant="subtle" onClick={collapseAll}>Collapse all</Button>
            </>
          ) : null}
        </Group>
      ) : null}
      {props.showLegend ? (
        <Group gap="md" px={4}>
          <Group gap={5}>
            <Box w={7} h={7} bg="green.6" style={{ borderRadius: '50%' }} />
            <Text size="xs" c="dimmed">Included</Text>
          </Group>
          <Group gap={5}>
            <Box w={7} h={7} bg="red.6" style={{ borderRadius: '50%' }} />
            <Text size="xs" c="dimmed">Excluded</Text>
          </Group>
          <Group gap={5}>
            <Box w={7} h={7} bg="yellow.6" style={{ borderRadius: '50%' }} />
            <Text size="xs" c="dimmed">Mixed</Text>
          </Group>
        </Group>
      ) : null}
      <ScrollArea
        style={{
          flex: 1,
          minHeight: 0,
          border: '1px solid var(--mantine-color-default-border)',
          borderRadius: 6,
          background: 'var(--mantine-color-body)'
        }}
        offsetScrollbars
      >
        <Stack gap={0} p={6}>
          {renderEntries('', 0)}
        </Stack>
      </ScrollArea>
    </Stack>
  );
}
