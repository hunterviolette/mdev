import { useMemo, useState } from 'react';
import { ActionIcon, Box, Button, Checkbox, Group, Loader, ScrollArea, Stack, Text } from '@mantine/core';
import { IconChevronDown, IconChevronRight, IconFile, IconFolder, IconFolderPlus, IconPlus, IconTrash } from '@tabler/icons-react';

export type RepoTreeEntry = {
  name: string;
  path: string;
  kind: 'file' | 'dir';
  has_children: boolean;
};

type RepoTreeCoreProps = {
  rootEntries: RepoTreeEntry[];
  childrenByParent: Record<string, RepoTreeEntry[]>;
  loadingDirs: Set<string>;
  height?: number;
  rowMode: 'fragment' | 'explorer';
  selectedPaths?: Set<string>;
  selectedDirs?: Set<string>;
  activePath?: string | null;
  onLoadDir: (path: string) => void;
  onToggleFile?: (path: string) => void;
  onToggleDir?: (entry: RepoTreeEntry, checked: boolean) => void;
  onSetPaths?: (paths: string[], checked: boolean) => void;
  onOpenFile?: (path: string) => void;
  onCreateFile?: (parentPath: string | null) => void;
  onCreateFolder?: (parentPath: string | null) => void;
  onDeletePath?: (path: string) => void;
};

type RepoFragmentTreeProps = {
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

type RepoExplorerTreeProps = {
  rootEntries: RepoTreeEntry[];
  childrenByParent: Record<string, RepoTreeEntry[]>;
  loadingDirs: Set<string>;
  activePath?: string | null;
  onLoadDir: (path: string) => void;
  onOpenFile: (path: string) => void;
  onCreateFile?: (parentPath: string | null) => void;
  onCreateFolder?: (parentPath: string | null) => void;
  onDeletePath?: (path: string) => void;
  height?: number;
};

export function RepoTree(props: RepoFragmentTreeProps) {
  return (
    <RepoTreeCore
      rowMode="fragment"
      rootEntries={props.rootEntries}
      childrenByParent={props.childrenByParent}
      loadingDirs={props.loadingDirs}
      selectedPaths={props.selected}
      selectedDirs={props.selectedDirs}
      onLoadDir={props.onLoadDir}
      onToggleFile={props.onToggleFile}
      onToggleDir={props.onToggleDir}
      onSetPaths={props.onSetPaths}
      height={props.height}
    />
  );
}

export function RepoFragmentTree(props: RepoFragmentTreeProps) {
  return <RepoTree {...props} />;
}

export function RepoExplorerTree(props: RepoExplorerTreeProps) {
  return (
    <RepoTreeCore
      rowMode="explorer"
      rootEntries={props.rootEntries}
      childrenByParent={props.childrenByParent}
      loadingDirs={props.loadingDirs}
      activePath={props.activePath ?? null}
      onLoadDir={props.onLoadDir}
      onOpenFile={props.onOpenFile}
      onCreateFile={props.onCreateFile}
      onCreateFolder={props.onCreateFolder}
      onDeletePath={props.onDeletePath}
      height={props.height}
    />
  );
}

export function collectLoadedFilePaths(parentPath: string, childrenByParent: Record<string, RepoTreeEntry[]>): string[] {
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

function RepoTreeCore({
  rootEntries,
  childrenByParent,
  loadingDirs,
  height = 360,
  rowMode,
  selectedPaths = new Set<string>(),
  selectedDirs = new Set<string>(),
  activePath = null,
  onLoadDir,
  onToggleFile,
  onToggleDir,
  onSetPaths,
  onOpenFile,
  onCreateFile,
  onCreateFolder,
  onDeletePath,
}: RepoTreeCoreProps) {
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

  const toolbar = rowMode === 'explorer' ? (
    <Group justify="space-between" mb="xs">
      <Text size="sm" fw={600}>Repository</Text>
      <Group gap={4}>
        <ActionIcon variant="subtle" size="sm" onClick={() => onCreateFile?.(null)} disabled={!onCreateFile}>
          <IconPlus size={14} />
        </ActionIcon>
        <ActionIcon variant="subtle" size="sm" onClick={() => onCreateFolder?.(null)} disabled={!onCreateFolder}>
          <IconFolderPlus size={14} />
        </ActionIcon>
      </Group>
    </Group>
  ) : null;

  return (
    <ScrollArea h={height} offsetScrollbars>
      <Stack gap={2}>
        {toolbar}
        {rootEntries.map((entry) => (
          <RepoTreeRow
            key={entry.path}
            entry={entry}
            depth={0}
            expanded={expanded}
            childrenByParent={childrenByParent}
            loadingDirs={loadingDirs}
            rowMode={rowMode}
            selectedPaths={selectedPaths}
            selectedDirs={selectedDirs}
            activePath={activePath}
            onToggleExpanded={toggleExpanded}
            onToggleFile={onToggleFile}
            onToggleDir={onToggleDir}
            onSetPaths={onSetPaths}
            onLoadDir={onLoadDir}
            onOpenFile={onOpenFile}
            onCreateFile={onCreateFile}
            onCreateFolder={onCreateFolder}
            onDeletePath={onDeletePath}
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
  rowMode: 'fragment' | 'explorer';
  selectedPaths: Set<string>;
  selectedDirs: Set<string>;
  activePath: string | null;
  onToggleExpanded: (entry: RepoTreeEntry) => void;
  onToggleFile?: (path: string) => void;
  onToggleDir?: (entry: RepoTreeEntry, checked: boolean) => void;
  onSetPaths?: (paths: string[], checked: boolean) => void;
  onLoadDir: (path: string) => void;
  onOpenFile?: (path: string) => void;
  onCreateFile?: (parentPath: string | null) => void;
  onCreateFolder?: (parentPath: string | null) => void;
  onDeletePath?: (path: string) => void;
};

function RepoTreeRow({
  entry,
  depth,
  expanded,
  childrenByParent,
  loadingDirs,
  rowMode,
  selectedPaths,
  selectedDirs,
  activePath,
  onToggleExpanded,
  onToggleFile,
  onToggleDir,
  onSetPaths,
  onLoadDir,
  onOpenFile,
  onCreateFile,
  onCreateFolder,
  onDeletePath,
}: RepoTreeRowProps) {
  const isExpanded = expanded.has(entry.path);
  const isFile = entry.kind === 'file';
  const childEntries = childrenByParent[entry.path] ?? [];
  const descendantFiles = useMemo(() => collectLoadedFilePaths(entry.path, childrenByParent), [entry.path, childrenByParent]);

  if (isFile) {
    if (rowMode === 'explorer') {
      const isActive = activePath === entry.path;
      return (
        <Group
          gap={6}
          wrap="nowrap"
          justify="space-between"
          style={{
            paddingLeft: depth * 16,
            borderRadius: 6,
            background: isActive ? 'rgba(76, 110, 245, 0.18)' : 'transparent',
          }}
        >
          <Group
            gap={6}
            wrap="nowrap"
            style={{ flex: 1, minWidth: 0, cursor: 'pointer', padding: '4px 6px' }}
            onClick={() => onOpenFile?.(entry.path)}
          >
            <ActionIcon variant="subtle" size="sm" disabled>
              <IconFile size={14} />
            </ActionIcon>
            <Text size="sm" ff="monospace" truncate>{entry.name}</Text>
          </Group>
          <Group gap={2} wrap="nowrap">
            <ActionIcon variant="subtle" size="sm" onClick={() => onDeletePath?.(entry.path)} disabled={!onDeletePath}>
              <IconTrash size={14} />
            </ActionIcon>
          </Group>
        </Group>
      );
    }

    return (
      <Group gap={6} wrap="nowrap" style={{ paddingLeft: depth * 16 }}>
        <ActionIcon variant="subtle" size="sm" disabled>
          <IconFile size={14} />
        </ActionIcon>
        <Checkbox
          checked={selectedPaths.has(entry.path)}
          onChange={() => onToggleFile?.(entry.path)}
          label={<Text size="sm" ff="monospace">{entry.name}</Text>}
        />
      </Group>
    );
  }

  const allSelected = descendantFiles.length > 0 && descendantFiles.every((path) => selectedPaths.has(path));
  const partiallySelected = !allSelected && descendantFiles.some((path) => selectedPaths.has(path));
  const isDirActive = activePath === entry.path;

  if (rowMode === 'explorer') {
    return (
      <>
        <Group
          gap={6}
          wrap="nowrap"
          justify="space-between"
          style={{
            paddingLeft: depth * 16,
            borderRadius: 6,
            background: isDirActive ? 'rgba(76, 110, 245, 0.12)' : 'transparent',
          }}
        >
          <Group gap={6} wrap="nowrap" style={{ flex: 1, minWidth: 0 }}>
            <ActionIcon variant="subtle" size="sm" onClick={() => onToggleExpanded(entry)}>
              {isExpanded ? <IconChevronDown size={14} /> : <IconChevronRight size={14} />}
            </ActionIcon>
            <Group
              gap={6}
              wrap="nowrap"
              style={{ flex: 1, minWidth: 0, cursor: 'pointer', padding: '4px 0' }}
              onClick={() => onToggleExpanded(entry)}
            >
              <IconFolder size={14} />
              <Text size="sm" fw={600} truncate>{entry.name}</Text>
            </Group>
          </Group>
          <Group gap={2} wrap="nowrap">
            <ActionIcon variant="subtle" size="sm" onClick={() => onCreateFile?.(entry.path)} disabled={!onCreateFile}>
              <IconPlus size={14} />
            </ActionIcon>
            <ActionIcon variant="subtle" size="sm" onClick={() => onCreateFolder?.(entry.path)} disabled={!onCreateFolder}>
              <IconFolderPlus size={14} />
            </ActionIcon>
            <ActionIcon variant="subtle" size="sm" onClick={() => onDeletePath?.(entry.path)} disabled={!onDeletePath}>
              <IconTrash size={14} />
            </ActionIcon>
          </Group>
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
            rowMode={rowMode}
            selectedPaths={selectedPaths}
            selectedDirs={selectedDirs}
            activePath={activePath}
            onToggleExpanded={onToggleExpanded}
            onToggleFile={onToggleFile}
            onToggleDir={onToggleDir}
            onSetPaths={onSetPaths}
            onLoadDir={onLoadDir}
            onOpenFile={onOpenFile}
            onCreateFile={onCreateFile}
            onCreateFolder={onCreateFolder}
            onDeletePath={onDeletePath}
          />
        ))}
      </>
    );
  }

  return (
    <>
      <Group gap={6} wrap="nowrap" style={{ paddingLeft: depth * 16 }}>
        <ActionIcon variant="subtle" size="sm" onClick={() => onToggleExpanded(entry)}>
          {isExpanded ? <IconChevronDown size={14} /> : <IconChevronRight size={14} />}
        </ActionIcon>

        <Checkbox
          checked={allSelected || selectedDirs.has(entry.path)}
          indeterminate={partiallySelected}
          onChange={(event) => onToggleDir?.(entry, event.currentTarget.checked)}
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
          rowMode={rowMode}
          selectedPaths={selectedPaths}
          selectedDirs={selectedDirs}
          activePath={activePath}
          onToggleExpanded={onToggleExpanded}
          onToggleFile={onToggleFile}
          onToggleDir={onToggleDir}
          onSetPaths={onSetPaths}
          onLoadDir={onLoadDir}
          onOpenFile={onOpenFile}
          onCreateFile={onCreateFile}
          onCreateFolder={onCreateFolder}
          onDeletePath={onDeletePath}
        />
      ))}
    </>
  );
}
