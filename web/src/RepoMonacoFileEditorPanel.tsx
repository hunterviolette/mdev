import { useEffect, useRef, useState } from 'react';
import { Alert, Badge, Button, Card, Group, Loader, Stack, Switch, Text, Title, ActionIcon, Modal, TextInput, ScrollArea } from '@mantine/core';
import { Workspace, lazy as mountModernMonaco } from 'modern-monaco';
import {
  createWorkspaceFile,
  createWorkspaceFolder,
  deleteWorkspacePath,
  listRepoFiles,
  listRepoTree,
  readWorkspaceFile,
  writeWorkspaceFile,
} from './api';
import { RepoExplorerTree, type RepoTreeEntry } from './RepoTree';

type RepoMonacoFileEditorPanelProps = {
  repoRef: string;
  gitRef?: string;
};

const README_PATH = 'README.virtual.txt';
const README_CONTENT = '// Select a file from the explorer to open it.\n';

export function RepoMonacoFileEditorPanel(props: RepoMonacoFileEditorPanelProps) {
  const { repoRef, gitRef = 'WORKTREE' } = props;
  const [rootEntries, setRootEntries] = useState<RepoTreeEntry[]>([]);
  const [childrenByParent, setChildrenByParent] = useState<Record<string, RepoTreeEntry[]>>({});
  const [loadingDirs, setLoadingDirs] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [saving, setSaving] = useState(false);
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [hideBinary, setHideBinary] = useState(true);
  const [hideGitignored, setHideGitignored] = useState(true);
  const [workspaceFiles, setWorkspaceFiles] = useState<Record<string, string>>({});
  const [savedFiles, setSavedFiles] = useState<Record<string, string>>({});
  const [workspaceVersion, setWorkspaceVersion] = useState(0);
  const [openTabs, setOpenTabs] = useState<string[]>([]);
  const [dirtyPaths, setDirtyPaths] = useState<Record<string, boolean>>({});
  const [quickOpenOpen, setQuickOpenOpen] = useState(false);
  const [quickOpenQuery, setQuickOpenQuery] = useState('');
  const [quickOpenIndex, setQuickOpenIndex] = useState<string[]>([]);
  const [quickOpenLoading, setQuickOpenLoading] = useState(false);
  const [quickOpenActiveIndex, setQuickOpenActiveIndex] = useState(0);
  const quickOpenInputRef = useRef<HTMLInputElement | null>(null);
  const [workspace, setWorkspace] = useState(
    () =>
      new Workspace({
        name: `repo-editor:${repoRef || 'default'}:0`,
        initialFiles: {
          [README_PATH]: README_CONTENT,
        },
        entryFile: README_PATH,
      })
  );

  const workspaceRef = useRef<Workspace | null>(workspace);
  const workspaceRepoRef = useRef<string>(repoRef);
  const openRequestSeq = useRef(0);
  const mountRequestSeq = useRef(0);
  const entryPathRef = useRef<string>(README_PATH);
  const workspaceNameSeq = useRef(0);

  function normalizeWorkspacePath(path: string) {
    return path.replace(/\\/g, '/').trim().replace(/^\/+/, '');
  }

  function normalizeEditorText(text: string) {
    return text.replace(/\r\n/g, '\n');
  }

  function readWorkspaceText(contents: unknown) {
    if (typeof contents === 'string') {
      return contents;
    }
    if (contents instanceof Uint8Array) {
      return new TextDecoder().decode(contents);
    }
    if (contents instanceof ArrayBuffer) {
      return new TextDecoder().decode(new Uint8Array(contents));
    }
    if (ArrayBuffer.isView(contents)) {
      return new TextDecoder().decode(
        new Uint8Array(contents.buffer, contents.byteOffset, contents.byteLength)
      );
    }
    return String(contents ?? '');
  }

  function nextWorkspaceName() {
    workspaceNameSeq.current += 1;
    return `repo-editor:${repoRef || 'default'}:${workspaceNameSeq.current}`;
  }

  function markTabDirty(path: string, dirty: boolean) {
    const normalizedPath = normalizeWorkspacePath(path);
    setDirtyPaths((prev) => {
      const next = { ...prev };
      if (dirty) {
        next[normalizedPath] = true;
      } else {
        delete next[normalizedPath];
      }
      return next;
    });
  }

  function ensureTabOpen(path: string) {
    const normalizedPath = normalizeWorkspacePath(path);
    setOpenTabs((prev) => (prev.includes(normalizedPath) ? prev : [...prev, normalizedPath]));
  }

  async function syncCurrentEditorToState(targetPath?: string | null) {
    const currentWorkspace = workspaceRef.current;
    const activePath = targetPath ? normalizeWorkspacePath(targetPath) : selectedPath ? normalizeWorkspacePath(selectedPath) : null;
    if (!currentWorkspace || !activePath) {
      return;
    }

    try {
      const currentContents = await Promise.resolve(currentWorkspace.fs.readFile(activePath));
      const text = readWorkspaceText(currentContents);
      setWorkspaceFiles((prev) => {
        const previous = prev[activePath];
        const next = {
          ...prev,
          [activePath]: text,
        };
        if (previous !== undefined) {
          markTabDirty(activePath, previous !== text);
        }
        return next;
      });
    } catch {
    }
  }

  function fuzzyScore(path: string, query: string) {
    const candidate = path.toLowerCase();
    const q = query.trim().toLowerCase();
    if (!q) {
      return 10_000 + candidate.length;
    }

    const fileName = candidate.split('/').pop() ?? candidate;
    if (candidate === q) return 0;
    if (fileName === q) return 1;
    if (candidate.startsWith(q)) return 10;
    if (fileName.startsWith(q)) return 15;

    const containsIndex = candidate.indexOf(q);
    if (containsIndex >= 0) {
      return 100 + containsIndex;
    }

    let qi = 0;
    let first = -1;
    let last = -1;
    for (let i = 0; i < candidate.length && qi < q.length; i += 1) {
      if (candidate[i] === q[qi]) {
        if (first < 0) first = i;
        last = i;
        qi += 1;
      }
    }

    if (qi !== q.length) {
      return Number.MAX_SAFE_INTEGER;
    }

    const span = last - first + 1;
    return 500 + span + (candidate.length - q.length);
  }

  const quickOpenResults = [...quickOpenIndex]
    .map((path) => ({ path, score: fuzzyScore(path, quickOpenQuery) }))
    .filter((item) => item.score < Number.MAX_SAFE_INTEGER)
    .sort((a, b) => a.score - b.score || a.path.localeCompare(b.path))
    .slice(0, 40);

  async function ensureWorkspaceParentDirs(path: string) {
    const currentWorkspace = workspaceRef.current;
    if (!currentWorkspace) {
      throw new Error('Workspace not initialized');
    }

    const normalizedPath = normalizeWorkspacePath(path);
    const parts = normalizedPath.split('/').filter(Boolean);
    if (parts.length <= 1) {
      return;
    }

    let currentDir = '';
    for (const part of parts.slice(0, -1)) {
      currentDir = currentDir ? `${currentDir}/${part}` : part;
      try {
        await Promise.resolve(currentWorkspace.fs.createDirectory(currentDir));
      } catch {
      }
    }
  }

  function createWorkspace(files: Record<string, string>, entryFile?: string | null) {
    const normalizedFiles = Object.fromEntries(
      Object.entries(files).map(([path, contents]) => [normalizeWorkspacePath(path), contents])
    );
    const normalizedEntry = entryFile ? normalizeWorkspacePath(entryFile) : null;
    const resolvedEntry = normalizedEntry && normalizedFiles[normalizedEntry] !== undefined ? normalizedEntry : README_PATH;

    const next = new Workspace({
      name: nextWorkspaceName(),
      initialFiles: {
        [README_PATH]: README_CONTENT,
        ...normalizedFiles,
      },
      entryFile: resolvedEntry,
    });

    entryPathRef.current = resolvedEntry;
    workspaceRef.current = next;
    workspaceRepoRef.current = repoRef;
    mountRequestSeq.current += 1;
    setWorkspace(next);
    setWorkspaceVersion((value) => value + 1);
    return next;
  }

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      try {
        await mountModernMonaco({
          workspace,
          defaultTheme: 'one-dark-pro',
        });

        if (cancelled) {
          return;
        }

        if (entryPathRef.current === README_PATH) {
          await Promise.resolve(workspace.openTextDocument(README_PATH, README_CONTENT));
        } else {
          await Promise.resolve(workspace.openTextDocument(entryPathRef.current));
        }
      } catch (err: unknown) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [workspace]);

  useEffect(() => {
    const currentWorkspace = workspaceRef.current;
    const nextPath = selectedPath ? normalizeWorkspacePath(selectedPath) : README_PATH;

    if (!currentWorkspace) {
      return;
    }

    void (async () => {
      try {
        if (nextPath === README_PATH) {
          await Promise.resolve(currentWorkspace.openTextDocument(README_PATH, README_CONTENT));
        } else {
          await Promise.resolve(currentWorkspace.openTextDocument(nextPath));
        }
        entryPathRef.current = nextPath;
      } catch (err: unknown) {
        setError(err instanceof Error ? err.message : String(err));
      }
    })();
  }, [selectedPath, workspaceFiles]);


  async function snapshotCurrentEditorFiles(baseFiles: Record<string, string>) {
    const currentWorkspace = workspaceRef.current;
    if (!currentWorkspace || !selectedPath) {
      return baseFiles;
    }

    try {
      const currentContents = await Promise.resolve(currentWorkspace.fs.readFile(normalizeWorkspacePath(selectedPath)));
      return {
        ...baseFiles,
        [normalizeWorkspacePath(selectedPath)]: readWorkspaceText(currentContents),
      };
    } catch {
      return baseFiles;
    }
  }

  async function loadRoot() {
    if (!repoRef.trim()) {
      setRootEntries([]);
      setChildrenByParent({});
      return;
    }

    setBusy(true);
    setError(null);
    try {
      const response = await listRepoTree(repoRef, gitRef, {
        skipBinary: hideBinary,
        skipGitignore: hideGitignored,
      });
      setRootEntries(response.entries as RepoTreeEntry[]);
      setChildrenByParent({});
      setLoadingDirs(new Set());
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    void loadRoot();
  }, [repoRef, gitRef, hideBinary, hideGitignored]);

  useEffect(() => {
    if (workspaceRepoRef.current === repoRef) {
      return;
    }

    const next = new Workspace({
      name: nextWorkspaceName(),
      initialFiles: {
        [README_PATH]: README_CONTENT,
      },
      entryFile: README_PATH,
    });

    entryPathRef.current = README_PATH;
    workspaceRepoRef.current = repoRef;
    workspaceRef.current = next;
    setWorkspace(next);
    setWorkspaceVersion((value) => value + 1);
  }, [repoRef]);

  useEffect(() => {
    setWorkspaceFiles({});
    setSelectedPath(null);
    setError(null);
    openRequestSeq.current = 0;
  }, [repoRef]);

  useEffect(() => {
    setSavedFiles({});
  }, [repoRef]);

  useEffect(() => {
    setOpenTabs([]);
    setDirtyPaths({});
  }, [repoRef]);

  useEffect(() => {
    setQuickOpenOpen(false);
    setQuickOpenQuery('');
    setQuickOpenIndex([]);
    setQuickOpenActiveIndex(0);
  }, [repoRef, gitRef, hideBinary, hideGitignored]);

  useEffect(() => {
    if (!quickOpenOpen) {
      return;
    }
    setQuickOpenActiveIndex(0);
  }, [quickOpenQuery, quickOpenOpen]);

  async function loadDir(path: string) {
    if (!repoRef.trim()) return;

    setLoadingDirs((prev) => new Set(prev).add(path));
    try {
      const response = await listRepoTree(repoRef, gitRef, {
        basePath: path,
        skipBinary: hideBinary,
        skipGitignore: hideGitignored,
      });
      setChildrenByParent((prev) => ({
        ...prev,
        [path]: response.entries as RepoTreeEntry[],
      }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoadingDirs((prev) => {
        const next = new Set(prev);
        next.delete(path);
        return next;
      });
    }
  }

  async function openFile(path: string) {
    if (!repoRef.trim()) return;

    const requestId = ++openRequestSeq.current;
    setOpening(true);
    setError(null);

    try {
      const response = await readWorkspaceFile(repoRef, path);
      if (requestId !== openRequestSeq.current) {
        return;
      }

      const normalizedPath = normalizeWorkspacePath(response.path);
      const snapshotted = await snapshotCurrentEditorFiles(workspaceFiles);
      if (requestId !== openRequestSeq.current) {
        return;
      }

      const nextFiles = {
        ...snapshotted,
        [normalizedPath]: response.contents,
      };

      const currentWorkspace = workspaceRef.current;
      if (!currentWorkspace) {
        throw new Error('Workspace not initialized');
      }

      await ensureWorkspaceParentDirs(normalizedPath);
      await Promise.resolve(currentWorkspace.fs.writeFile(normalizedPath, response.contents));
      if (requestId !== openRequestSeq.current) {
        return;
      }

      setWorkspaceFiles(nextFiles);
      setSavedFiles((prev) => ({
        ...prev,
        [normalizedPath]: normalizeEditorText(response.contents),
      }));
      ensureTabOpen(normalizedPath);
      markTabDirty(normalizedPath, false);
      setSelectedPath(normalizedPath);
      entryPathRef.current = normalizedPath;
      await Promise.resolve(currentWorkspace.openTextDocument(normalizedPath));
    } catch (err) {
      if (requestId !== openRequestSeq.current) {
        return;
      }
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (requestId === openRequestSeq.current) {
        setOpening(false);
      }
    }
  }

  async function closeTab(path: string) {
    const normalizedPath = normalizeWorkspacePath(path);
    await syncCurrentEditorToState(selectedPath);

    if (dirtyPaths[normalizedPath] && !window.confirm(`${normalizedPath} has unsaved changes. Close anyway?`)) {
      return;
    }

    const remainingTabs = openTabs.filter((tab) => tab !== normalizedPath);

    setOpenTabs(remainingTabs);
    setDirtyPaths((prev) => {
      const next = { ...prev };
      delete next[normalizedPath];
      return next;
    });

    if (selectedPath === normalizedPath) {
      const nextSelected = remainingTabs.length ? remainingTabs[remainingTabs.length - 1] : null;
      setSelectedPath(nextSelected);
      entryPathRef.current = nextSelected ?? README_PATH;
    }
  }

  async function quickOpenBySearch() {
    setQuickOpenOpen(true);
    setError(null);

    if (quickOpenIndex.length > 0 || quickOpenLoading || !repoRef.trim()) {
      return;
    }

    try {
      setQuickOpenLoading(true);
      const response = await listRepoFiles(repoRef, gitRef, {
        skipBinary: hideBinary,
        skipGitignore: hideGitignored,
      });
      setQuickOpenIndex(response.files);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setQuickOpenLoading(false);
    }
  }

  async function openQuickOpenSelection(path: string) {
    await openFile(path);
    setQuickOpenOpen(false);
    setQuickOpenQuery('');
    setQuickOpenActiveIndex(0);
  }

  useEffect(() => {
    if (!quickOpenOpen) {
      return;
    }

    const frame = window.requestAnimationFrame(() => {
      quickOpenInputRef.current?.focus();
      quickOpenInputRef.current?.select();
    });

    return () => window.cancelAnimationFrame(frame);
  }, [quickOpenOpen, quickOpenLoading]);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (quickOpenOpen) {
        return;
      }

      if (!event.altKey) {
        return;
      }

      const key = event.key.toLowerCase();
      if (key === 's') {
        event.preventDefault();
        void saveCurrentFile();
        return;
      }

      if (key === 'w') {
        event.preventDefault();
        if (selectedPath) {
          void closeTab(selectedPath);
        }
        return;
      }

      if (key === 'e') {
        event.preventDefault();
        void quickOpenBySearch();
      }
    };

    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [selectedPath, quickOpenOpen, repoRef, gitRef, hideBinary, hideGitignored, quickOpenIndex, quickOpenLoading]);

  useEffect(() => {
    if (!selectedPath) {
      return;
    }

    const normalizedPath = normalizeWorkspacePath(selectedPath);
    const currentWorkspace = workspaceRef.current;
    if (!currentWorkspace) {
      return;
    }

    let cancelled = false;

    const tick = async () => {
      try {
        const contents = await Promise.resolve(currentWorkspace.fs.readFile(normalizedPath));
        if (cancelled) {
          return;
        }
        const text = normalizeEditorText(readWorkspaceText(contents));
        const saved = normalizeEditorText(savedFiles[normalizedPath] ?? '');
        markTabDirty(normalizedPath, text !== saved);
      } catch {
      }
    };

    void tick();
    const interval = window.setInterval(() => {
      void tick();
    }, 250);

    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [selectedPath, savedFiles, workspace]);

  async function saveCurrentFile() {
    if (!repoRef.trim() || !selectedPath) return;

    const currentWorkspace = workspaceRef.current;
    if (!currentWorkspace) return;

    try {
      setSaving(true);
      setError(null);
      const contents = await Promise.resolve(currentWorkspace.fs.readFile(normalizeWorkspacePath(selectedPath)));
      const text = readWorkspaceText(contents);

      const normalizedPath = normalizeWorkspacePath(selectedPath);
      setWorkspaceFiles((prev) => ({
        ...prev,
        [normalizedPath]: text,
      }));
      setSavedFiles((prev) => ({
        ...prev,
        [normalizedPath]: normalizeEditorText(text),
      }));
      markTabDirty(normalizedPath, false);

      await writeWorkspaceFile({
        repo_ref: repoRef,
        path: normalizedPath,
        contents: text,
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  async function handleCreateFile(parentPath: string | null) {
    if (!repoRef.trim()) return;
    const requested = window.prompt('New file path', parentPath ? `${parentPath}/new_file.txt` : 'new_file.txt');
    if (!requested || !requested.trim()) return;

    try {
      setError(null);
      const created = await createWorkspaceFile({
        repo_ref: repoRef,
        path: requested.trim(),
        contents: '',
      });

      const normalizedPath = normalizeWorkspacePath(created.path);
      const snapshotted = await snapshotCurrentEditorFiles(workspaceFiles);
      const nextFiles = {
        ...snapshotted,
        [normalizedPath]: '',
      };

      const currentWorkspace = workspaceRef.current;
      if (!currentWorkspace) {
        throw new Error('Workspace not initialized');
      }

      await ensureWorkspaceParentDirs(normalizedPath);
      await Promise.resolve(currentWorkspace.fs.writeFile(normalizedPath, ''));
      setWorkspaceFiles(nextFiles);
      setSavedFiles((prev) => ({
        ...prev,
        [normalizedPath]: '',
      }));
      ensureTabOpen(normalizedPath);
      markTabDirty(normalizedPath, false);
      setSelectedPath(normalizedPath);
      entryPathRef.current = normalizedPath;
      await Promise.resolve(currentWorkspace.openTextDocument(normalizedPath));
      await loadRoot();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function handleCreateFolder(parentPath: string | null) {
    if (!repoRef.trim()) return;
    const requested = window.prompt('New folder path', parentPath ? `${parentPath}/new_folder` : 'new_folder');
    if (!requested || !requested.trim()) return;

    try {
      setError(null);
      await createWorkspaceFolder({
        repo_ref: repoRef,
        path: requested.trim(),
      });
      await loadRoot();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function handleDeletePath(path: string) {
    if (!repoRef.trim()) return;
    if (!window.confirm(`Delete ${path}?`)) return;

    try {
      setError(null);
      await deleteWorkspacePath(repoRef, path);

      const normalizedPath = normalizeWorkspacePath(path);
      const snapshotted = await snapshotCurrentEditorFiles(workspaceFiles);
      const nextFiles = { ...snapshotted };
      delete nextFiles[normalizedPath];
      setSavedFiles((prev) => {
        const next = { ...prev };
        delete next[normalizedPath];
        return next;
      });

      const nextSelected = selectedPath === normalizedPath ? null : selectedPath;

      createWorkspace(nextFiles, nextSelected);
      setWorkspaceFiles(nextFiles);
      setOpenTabs((prev) => prev.filter((tab) => tab !== normalizedPath));
      setDirtyPaths((prev) => {
        const next = { ...prev };
        delete next[normalizedPath];
        return next;
      });
      setSelectedPath(nextSelected);
      await loadRoot();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <>
      <Modal
        opened={quickOpenOpen}
        onClose={() => setQuickOpenOpen(false)}
        title="Quick open"
        centered
        size="lg"
        styles={{ body: { paddingTop: 8 } }}
      >
        <Stack gap="xs">
          <TextInput
            ref={quickOpenInputRef}
            data-autofocus
            placeholder="Type a file name or path"
            value={quickOpenQuery}
            onChange={(event) => {
              setQuickOpenQuery(event.currentTarget.value);
              setQuickOpenActiveIndex(0);
            }}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                event.preventDefault();
                event.stopPropagation();
                setQuickOpenOpen(false);
                return;
              }

              if (event.key === 'ArrowDown') {
                event.preventDefault();
                event.stopPropagation();
                setQuickOpenActiveIndex((prev) => Math.min(prev + 1, Math.max(quickOpenResults.length - 1, 0)));
                return;
              }

              if (event.key === 'ArrowUp') {
                event.preventDefault();
                event.stopPropagation();
                setQuickOpenActiveIndex((prev) => Math.max(prev - 1, 0));
                return;
              }

              if (event.key === 'Enter') {
                const selected = quickOpenResults[quickOpenActiveIndex];
                if (selected) {
                  event.preventDefault();
                  event.stopPropagation();
                  void openQuickOpenSelection(selected.path);
                }
                return;
              }

              event.stopPropagation();
            }}
          />
          {quickOpenLoading ? <Text size="sm" c="dimmed">Indexing files…</Text> : null}
          <ScrollArea.Autosize mah={360} offsetScrollbars>
            <Stack gap={4}>
              {!quickOpenLoading && quickOpenResults.length === 0 ? (
                <Text size="sm" c="dimmed">No matching files</Text>
              ) : null}
              {quickOpenResults.map((item, index) => {
                const fileName = item.path.split('/').pop() || item.path;
                const isActive = index === quickOpenActiveIndex;
                return (
                  <div
                    key={item.path}
                    onMouseDown={(event) => {
                      event.preventDefault();
                      void openQuickOpenSelection(item.path);
                    }}
                    style={{
                      padding: '8px 10px',
                      borderRadius: 6,
                      background: isActive ? 'rgba(59,130,246,0.18)' : 'transparent',
                      border: isActive ? '1px solid rgba(59,130,246,0.45)' : '1px solid transparent',
                      cursor: 'pointer',
                    }}
                  >
                    <Text size="sm" fw={600}>{fileName}</Text>
                    <Text size="xs" c="dimmed">{item.path}</Text>
                  </div>
                );
              })}
            </Stack>
          </ScrollArea.Autosize>
        </Stack>
      </Modal>
      <Card withBorder>
      <Stack gap="md">
        <Group justify="space-between" align="flex-start" wrap="wrap">
          <Stack gap={2}>
            <Group gap="xs">
              <Title order={4}>File editor</Title>
              <Badge variant="light">Modern Monaco</Badge>
            </Group>
            <Text size="sm" c="dimmed">Explorer mode uses the shared tree core without fragment-selection checkboxes.</Text>
            <Text size="xs" c="dimmed">Repo: {repoRef || 'No repo selected'}</Text>
          </Stack>
          <Button variant="default" disabled={!selectedPath} onClick={() => void saveCurrentFile()} loading={saving}>
            Save file
          </Button>
        </Group>

        <Group gap="md">
          <Switch label="Hide binary" checked={hideBinary} onChange={(event) => setHideBinary(event.currentTarget.checked)} />
          <Switch label="Hide gitignored" checked={hideGitignored} onChange={(event) => setHideGitignored(event.currentTarget.checked)} />
          <Button variant="default" size="xs" onClick={() => void loadRoot()} loading={busy}>Refresh</Button>
          {opening ? <Text size="xs" c="dimmed">Opening…</Text> : null}
        </Group>

        {!repoRef.trim() ? (
          <Alert color="yellow">Select a workflow or provide a repo path first.</Alert>
        ) : null}

        {error ? <Alert color="red">{error}</Alert> : null}

        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'minmax(280px, 360px) minmax(0, 1fr)',
            gap: 16,
            minHeight: 620,
            alignItems: 'stretch',
          }}
        >
          <Card withBorder p="sm" style={{ minHeight: 620, overflow: 'hidden' }}>
            <Stack gap="sm" h="100%" style={{ minHeight: 0 }}>
              <Group justify="space-between">
                <Text fw={600}>Explorer</Text>
                {busy ? <Loader size="xs" /> : null}
              </Group>
              <RepoExplorerTree
                rootEntries={rootEntries}
                childrenByParent={childrenByParent}
                loadingDirs={loadingDirs}
                activePath={selectedPath}
                onLoadDir={(path) => void loadDir(path)}
                onOpenFile={(path) => void openFile(path)}
                onCreateFile={handleCreateFile}
                onCreateFolder={handleCreateFolder}
                onDeletePath={handleDeletePath}
                height={560}
              />
            </Stack>
          </Card>

          <Card withBorder p={0} style={{ minHeight: 620, overflow: 'hidden' }}>
            <Stack gap={0} h="100%" style={{ minHeight: 620 }}>
              <Stack gap={0}>
                <Group justify="space-between" p="sm" style={{ borderBottom: '1px solid rgba(255,255,255,0.08)' }}>
                  <div>
                    <Text fw={600}>{selectedPath ?? README_PATH}</Text>
                    <Text size="xs" c="dimmed">Alt+S save · Alt+W close tab · Alt+E quick open</Text>
                  </div>
                </Group>
                <Group gap="xs" p="xs" style={{ borderBottom: '1px solid rgba(255,255,255,0.08)', overflowX: 'auto', flexWrap: 'nowrap' }}>
                  {openTabs.length === 0 ? (
                    <Text size="xs" c="dimmed">No open tabs</Text>
                  ) : (
                    openTabs.map((tabPath) => {
                      const isActive = tabPath === selectedPath;
                      const isDirty = !!dirtyPaths[tabPath];
                      const label = tabPath.split('/').pop() || tabPath;

                      return (
                        <Group
                          key={tabPath}
                          gap={6}
                          wrap="nowrap"
                          style={{
                            padding: '4px 8px',
                            borderRadius: 6,
                            background: isActive ? 'rgba(59,130,246,0.18)' : 'rgba(255,255,255,0.04)',
                            border: isActive ? '1px solid rgba(59,130,246,0.45)' : '1px solid rgba(255,255,255,0.08)',
                            cursor: 'pointer',
                          }}
                          onClick={() => {
                            setSelectedPath(tabPath);
                            entryPathRef.current = tabPath;
                          }}
                        >
                          <Group gap={6} wrap="nowrap">
                            {isDirty ? <Text size="sm" c="yellow">●</Text> : null}
                            <Text size="sm">{label}</Text>
                          </Group>
                          <ActionIcon
                            size="sm"
                            variant="subtle"
                            onClick={(event) => {
                              event.stopPropagation();
                              void closeTab(tabPath);
                            }}
                          >
                            ×
                          </ActionIcon>
                        </Group>
                      );
                    })
                  )}
                </Group>
              </Stack>
              <div style={{ flex: 1, minHeight: 0, overflow: 'hidden' }}>
                <monaco-editor
                  key={`workspace-${workspaceVersion}`}
                  theme="one-dark-pro"
                  style={{ display: 'block', width: '100%', height: '620px' }}
                />
              </div>
            </Stack>
          </Card>
        </div>
      </Stack>
    </Card>
    </>
  );
}
