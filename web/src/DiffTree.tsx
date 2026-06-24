import { useMemo } from 'react';
import { Badge, Box, Button, Stack, Text } from '@mantine/core';

export type DiffTreeFile = {
  path: string;
  status: string;
  additions: number;
  deletions: number;
};

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
      file: DiffTreeFile;
      additions: number;
      deletions: number;
    };

function buildFileTree(files: DiffBrowserTreeFile[]): FileTreeNode[] {
  const root = new Map<string, FileTreeNode>();
  const childMaps = new WeakMap<Extract<FileTreeNode, { kind: 'dir' }>, Map<string, FileTreeNode>>();

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

  function childrenFor(dir: Extract<FileTreeNode, { kind: 'dir' }>) {
    let map = childMaps.get(dir);
    if (!map) {
      map = new Map(dir.children.map((child) => [child.name, child]));
      childMaps.set(dir, map);
    }
    return map;
  }

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

      while (compacted.children.length === 1 && compacted.children[0].kind === 'dir') {
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

function DiffTreeRow(props: {
  node: FileTreeNode;
  depth: number;
  selectedPath: string | null;
  collapsedDirs: Record<string, boolean>;
  actionLabel?: string;
  activeActionLabel?: string;
  onToggleDir: (path: string) => void;
  onSelectFile: (path: string) => void;
  onFileAction?: (path: string) => void;
}) {
  const {
    node,
    depth,
    selectedPath,
    collapsedDirs,
    actionLabel,
    activeActionLabel,
    onToggleDir,
    onSelectFile,
    onFileAction,
  } = props;
  const indent = depth * 13;

  if (node.kind === 'dir') {
    const collapsed = Boolean(collapsedDirs[node.path]);
    return (
      <>
        <Box
          onClick={() => onToggleDir(node.path)}
          style={{
            cursor: 'pointer',
            height: 24,
            display: 'grid',
            gridTemplateColumns: 'minmax(0, 1fr) auto auto auto',
            gap: 6,
            alignItems: 'center',
            paddingLeft: 6 + indent,
            paddingRight: 6,
            borderRadius: 5,
            color: 'var(--mantine-color-dimmed)',
          }}
        >
          <Text size="xs" fw={700} truncate title={node.path}>
            {collapsed ? '▸' : '▾'} {node.name}/
          </Text>
          <Badge size="xs" variant="light">{node.fileCount}</Badge>
          <Badge size="xs" color="green" variant="light">+{node.additions}</Badge>
          <Badge size="xs" color="red" variant="light">-{node.deletions}</Badge>
        </Box>
        {!collapsed ? node.children.map((child) => (
          <DiffTreeRow
            key={child.path}
            node={child}
            depth={depth + 1}
            selectedPath={selectedPath}
            collapsedDirs={collapsedDirs}
            actionLabel={actionLabel}
            activeActionLabel={activeActionLabel}
            onToggleDir={onToggleDir}
            onSelectFile={onSelectFile}
            onFileAction={onFileAction}
          />
        )) : null}
      </>
    );
  }

  const active = selectedPath === node.path;
  return (
    <Box
      onClick={() => onSelectFile(node.path)}
      title={node.path}
      style={{
        cursor: 'pointer',
        height: 26,
        display: 'grid',
        gridTemplateColumns: onFileAction ? '28px minmax(0, 1fr) auto auto auto' : '28px minmax(0, 1fr) auto auto',
        gap: 6,
        alignItems: 'center',
        paddingLeft: 6 + indent,
        paddingRight: 6,
        borderRadius: 5,
        background: active ? 'rgba(34, 139, 230, 0.16)' : 'transparent',
        border: active ? '1px solid rgba(34, 139, 230, 0.34)' : '1px solid transparent',
      }}
    >
      <Badge size="xs" variant="outline" style={{ minWidth: 24 }}>{node.file.status}</Badge>
      <Text size="xs" fw={active ? 800 : 600} truncate>{node.name}</Text>
      <Text size="xs" c="green" fw={700}>+{node.additions}</Text>
      <Text size="xs" c="red" fw={700}>-{node.deletions}</Text>
      {onFileAction ? (
        <Button
          size="compact-xs"
          variant={active ? 'filled' : 'subtle'}
          onClick={(event) => {
            event.stopPropagation();
            onFileAction(node.path);
          }}
        >
          {active ? activeActionLabel ?? actionLabel ?? 'Open' : actionLabel ?? 'Open'}
        </Button>
      ) : null}
    </Box>
  );
}

export function DiffTree(props: {
  files: DiffBrowserTreeFile[];
  selectedPath: string | null;
  collapsedDirs: Record<string, boolean>;
  actionLabel?: string;
  activeActionLabel?: string;
  onToggleDir: (path: string) => void;
  onSelectFile: (path: string) => void;
  onFileAction?: (path: string) => void;
}) {
  const { files, ...rest } = props;
  const nodes = useMemo(() => buildFileTree(files), [files]);

  if (files.length === 0) {
    return <Text c="dimmed" size="xs" px="xs" py={4}>No files.</Text>;
  }

  return (
    <Stack gap={1}>
      {nodes.map((node) => (
        <DiffTreeRow key={node.path} node={node} depth={0} {...rest} />
      ))}
    </Stack>
  );
}
