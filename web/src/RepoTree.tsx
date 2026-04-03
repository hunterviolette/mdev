import { useState } from 'react';
import { ActionIcon, Checkbox, Group, Loader, ScrollArea, Stack, Text } from '@mantine/core';
import { IconChevronDown, IconChevronRight, IconFile, IconFolder } from '@tabler/icons-react';

export type RepoTreeEntry = {
  name: string;
  path: string;
  kind: 'file' | 'dir';
  has_children: boolean;
};

type RepoTreeProps = {
  rootEntries: RepoTreeEntry[];
  childrenByParent: Record<string, RepoTreeEntry[]>;
  loadingDirs: Set<string>;
  selected: Set<string>;
  selectedDirs?: Set<string>;
  onLoadDir: (path: string) => void;
  onToggleFile: (path: string) => void;
  onToggleDir: (entry: RepoTreeEntry, checked: boolean) => void;
  onSetPaths: (paths: string[], checked: boolean) => void;
  height?: number;
};

export function RepoTree({
  rootEntries,
  childrenByParent,
  loadingDirs,
  selected,
  selectedDirs = new Set(),
  onLoadDir,
  onToggleFile,
  onToggleDir,
  onSetPaths,
  height = 360
}: RepoTreeProps) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  const toggleExpanded = (entry: RepoTreeEntry) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(entry.path)) {
        next.delete(entry.path);
      } else {
        next.add(entry.path);
        if (entry.kind === 'dir' && entry.has_children && !childrenByParent[entry.path] && !loadingDirs.has(entry.path)) {
          onLoadDir(entry.path);
        }
      }
      return next;
    });
  };

  return (
    <ScrollArea h={height} offsetScrollbars>
      <Stack gap={2}>
        {rootEntries.map((entry) => (
          <RepoTreeRow
            key={entry.path}
            entry={entry}
            depth={0}
            expanded={expanded}
            childrenByParent={childrenByParent}
            loadingDirs={loadingDirs}
            selected={selected}
            selectedDirs={selectedDirs}
            onToggleExpanded={toggleExpanded}
            onToggleFile={onToggleFile}
            onToggleDir={onToggleDir}
            onSetPaths={onSetPaths}
            onLoadDir={onLoadDir}
          />
        ))}
      </Stack>
    </ScrollArea>
  );
}

type RepoTreeRowProps = {
  entry: RepoTreeEntry;
  depth: number;
  expanded: Set<string>;
  childrenByParent: Record<string, RepoTreeEntry[]>;
  loadingDirs: Set<string>;
  selected: Set<string>;
  selectedDirs: Set<string>;
  onToggleExpanded: (entry: RepoTreeEntry) => void;
  onToggleFile: (path: string) => void;
  onToggleDir: (entry: RepoTreeEntry, checked: boolean) => void;
  onSetPaths: (paths: string[], checked: boolean) => void;
  onLoadDir: (path: string) => void;
};

function RepoTreeRow({
  entry,
  depth,
  expanded,
  childrenByParent,
  loadingDirs,
  selected,
  selectedDirs,
  onToggleExpanded,
  onToggleFile,
  onToggleDir,
  onSetPaths,
  onLoadDir
}: RepoTreeRowProps) {
  const isExpanded = expanded.has(entry.path);
  const isFile = entry.kind === 'file';

  if (isFile) {
    return (
      <Group gap={6} wrap="nowrap" style={{ paddingLeft: depth * 16 }}>
        <ActionIcon variant="subtle" size="sm" disabled>
          <IconFile size={14} />
        </ActionIcon>
        <Checkbox
          checked={selected.has(entry.path)}
          onChange={() => onToggleFile(entry.path)}
          label={<Text size="sm" ff="monospace">{entry.name}</Text>}
        />
      </Group>
    );
  }

  const childEntries = childrenByParent[entry.path] ?? [];
  const descendantFiles = collectLoadedFilePaths(entry.path, childrenByParent);
  const selectedCount = descendantFiles.filter((path) => selected.has(path)).length;
  const explicitlySelected = selectedDirs.has(entry.path);
  const allSelected = explicitlySelected || (descendantFiles.length > 0 && selectedCount === descendantFiles.length);
  const partiallySelected = !explicitlySelected && selectedCount > 0 && selectedCount < descendantFiles.length;

  return (
    <>
      <Group gap={6} wrap="nowrap" style={{ paddingLeft: depth * 16 }}>
        <ActionIcon
          variant="subtle"
          size="sm"
          onClick={() => onToggleExpanded(entry)}
          disabled={!entry.has_children && childEntries.length === 0}
        >
          {isExpanded ? <IconChevronDown size={14} /> : <IconChevronRight size={14} />}
        </ActionIcon>

        <Checkbox
          checked={allSelected}
          indeterminate={partiallySelected}
          onChange={(event) => onToggleDir(entry, event.currentTarget.checked)}
          label={
            <Group gap={6} wrap="nowrap">
              <IconFolder size={14} />
              <Text size="sm" fw={600}>{entry.name}</Text>
              {descendantFiles.length > 0 ? <Text size="xs" c="dimmed">({descendantFiles.length})</Text> : null}
            </Group>
          }
        />
      </Group>

      {isExpanded && loadingDirs.has(entry.path) ? (
        <Group gap={6} wrap="nowrap" style={{ paddingLeft: (depth + 1) * 16 }}>
          <Loader size="xs" />
          <Text size="xs" c="dimmed">Loading…</Text>
        </Group>
      ) : null}

      {isExpanded && childEntries.map((child) => (
        <RepoTreeRow
          key={child.path}
          entry={child}
          depth={depth + 1}
          expanded={expanded}
          childrenByParent={childrenByParent}
          loadingDirs={loadingDirs}
          selected={selected}
          selectedDirs={selectedDirs}
          onToggleExpanded={onToggleExpanded}
          onToggleFile={onToggleFile}
          onToggleDir={onToggleDir}
          onSetPaths={onSetPaths}
          onLoadDir={onLoadDir}
        />
      ))}
    </>
  );
}

function collectLoadedFilePaths(parentPath: string, childrenByParent: Record<string, RepoTreeEntry[]>): string[] {
  const children = childrenByParent[parentPath] ?? [];
  const out: string[] = [];

  for (const child of children) {
    if (child.kind === 'file') {
      out.push(child.path);
    } else {
      out.push(...collectLoadedFilePaths(child.path, childrenByParent));
    }
  }

  return out;
}
