import { useEffect, useMemo, useState } from 'react';
import { Alert, Button, Card, Group, Loader, Stack, Text, Textarea, Title } from '@mantine/core';
import { listRepoTree } from './api';
import { RepoTree, type RepoTreeEntry } from './RepoTree';

type RepoFileEditorPanelProps = {
  repoRef: string;
  gitRef?: string;
};

export function RepoFileEditorPanel(props: RepoFileEditorPanelProps) {
  const { repoRef, gitRef = 'WORKTREE' } = props;
  const [rootEntries, setRootEntries] = useState<RepoTreeEntry[]>([]);
  const [childrenByParent, setChildrenByParent] = useState<Record<string, RepoTreeEntry[]>>({});
  const [loadingDirs, setLoadingDirs] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [draft, setDraft] = useState('');

  useEffect(() => {
    setRootEntries([]);
    setChildrenByParent({});
    setLoadingDirs(new Set());
    setSelectedPath(null);
    setDraft('');
    setError(null);

    if (!repoRef.trim()) {
      return;
    }

    let cancelled = false;
    setBusy(true);
    void listRepoTree(repoRef, gitRef, { skipBinary: true, skipGitignore: false })
      .then((response) => {
        if (cancelled) return;
        setRootEntries(response.entries as RepoTreeEntry[]);
      })
      .catch((err) => {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setBusy(false);
      });

    return () => {
      cancelled = true;
    };
  }, [repoRef, gitRef]);

  async function loadDir(path: string) {
    if (!repoRef.trim()) return;
    setLoadingDirs((prev) => new Set(prev).add(path));
    try {
      const response = await listRepoTree(repoRef, gitRef, {
        basePath: path,
        skipBinary: true,
        skipGitignore: false,
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

  function openFile(path: string) {
    setSelectedPath(path);
    setDraft([
      `// ${path}`,
      '// File read/save endpoints are not wired yet for the web shell.',
      '// This panel is ready to be connected once /api/file read+write routes exist.',
      '',
    ].join('\n'));
  }

  const selectedSet = useMemo(() => new Set(selectedPath ? [selectedPath] : []), [selectedPath]);

  return (
    <Card withBorder>
      <Stack gap="md">
        <Group justify="space-between" align="flex-start" wrap="wrap">
          <Stack gap={2}>
            <Title order={4}>File editor</Title>
            <Text size="sm" c="dimmed">Browse the repo with the existing tree component and prepare for lightweight manual file edits.</Text>
            <Text size="xs" c="dimmed">Repo: {repoRef || 'No repo selected'}</Text>
          </Stack>
          <Button variant="default" disabled>
            Save file
          </Button>
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
            minHeight: 560,
          }}
        >
          <Card withBorder p="sm">
            <Stack gap="sm" h="100%">
              <Group justify="space-between">
                <Text fw={600}>Repository</Text>
                {busy ? <Loader size="xs" /> : null}
              </Group>
              <RepoTree
                rootEntries={rootEntries}
                childrenByParent={childrenByParent}
                loadingDirs={loadingDirs}
                selected={selectedSet}
                onLoadDir={(path) => void loadDir(path)}
                onToggleFile={openFile}
                onToggleDir={() => {}}
                onSetPaths={() => {}}
                height={500}
              />
            </Stack>
          </Card>

          <Card withBorder p="sm">
            <Stack gap="sm" h="100%">
              <Group justify="space-between" wrap="wrap">
                <div>
                  <Text fw={600}>{selectedPath ?? 'No file selected'}</Text>
                  <Text size="xs" c="dimmed">This editor is wired to the tree now; connect file read/write API next.</Text>
                </div>
              </Group>
              <Textarea
                value={draft}
                onChange={(event) => setDraft(event.currentTarget.value)}
                autosize
                minRows={28}
                placeholder="Select a file from the tree to begin editing."
                styles={{ input: { fontFamily: 'monospace' } }}
              />
            </Stack>
          </Card>
        </div>
      </Stack>
    </Card>
  );
}
