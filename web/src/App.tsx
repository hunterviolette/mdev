import { useEffect, useMemo, useState } from 'react';
import {
  Alert,
  AppShell,
  Badge,
  Button,
  Card,
  Code,
  Divider,
  Group,
  JsonInput,
  Loader,
  Select,
  SimpleGrid,
  Stack,
  Table,
  Text,
  TextInput,
  Title
} from '@mantine/core';
import {
  createRun,
  createTemplate,
  invokeContextExport,
  listRepoTree,
  listRunEvents,
  listRuns,
  listTemplates,
  type RepoTreeResponse,
  type WorkflowEvent,
  type WorkflowRun,
  type WorkflowTemplate,
  type WorkflowTemplateDefinition
} from './api';
import { RepoTree, type RepoTreeEntry } from './RepoTree';

const starterDefinition: WorkflowTemplateDefinition = {
  version: 1,
  globals: {
    inference: {},
    prompt_fragments: {
      context_export: {
        enabled: true
      }
    },
    capabilities: []
  },
  steps: [
    {
      id: 'context_export',
      name: 'Context export',
      step_type: 'utility',
      automation_mode: 'manual',
      execution: {
        changeset_apply: {},
        compile_checks: {}
      },
      prompt: {
        include_repo_context: false,
        include_changeset_schema: false,
        include_user_context: false
      },
      config: {},
      capabilities: [],
      transitions: []
    }
  ]
};

export function App() {
  const [templates, setTemplates] = useState<WorkflowTemplate[]>([]);
  const [runs, setRuns] = useState<WorkflowRun[]>([]);
  const [events, setEvents] = useState<WorkflowEvent[]>([]);
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);

  const [templateName, setTemplateName] = useState('Default workflow');
  const [templateDescription, setTemplateDescription] = useState('Workflow with manual context export');
  const [templateJson, setTemplateJson] = useState(JSON.stringify(starterDefinition, null, 2));

  const [runTitle, setRunTitle] = useState('Manual run');
  const [repoRef, setRepoRef] = useState('');
  const [templateId, setTemplateId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [treeGitRef, setTreeGitRef] = useState('WORKTREE');
  const [treeRootData, setTreeRootData] = useState<RepoTreeResponse | null>(null);
  const [treeChildrenByParent, setTreeChildrenByParent] = useState<Record<string, RepoTreeEntry[]>>({});
  const [loadingTreeDirs, setLoadingTreeDirs] = useState<Set<string>>(new Set());
  const [treeBusy, setTreeBusy] = useState(false);
  const [treeError, setTreeError] = useState<string | null>(null);
  const [selectedPaths, setSelectedPaths] = useState<string[]>([]);

  const [contextMode, setContextMode] = useState<'entire_repo' | 'tree_select'>('tree_select');
  const [contextSavePath, setContextSavePath] = useState('');
  const [skipBinary, setSkipBinary] = useState(true);
  const [skipGitignore, setSkipGitignore] = useState(true);
  const [includeStagedDiff, setIncludeStagedDiff] = useState(false);
  const [includeUnstagedDiff, setIncludeUnstagedDiff] = useState(false);
  const [contextStatus, setContextStatus] = useState<string | null>(null);
  const [contextBusy, setContextBusy] = useState(false);

  const selectedRun = useMemo(() => runs.find((run) => run.id === selectedRunId) ?? null, [runs, selectedRunId]);
  const rootTreeEntries = useMemo(() => treeChildrenByParent[''] ?? [], [treeChildrenByParent]);
  const selectedSet = useMemo(() => new Set(selectedPaths), [selectedPaths]);

  async function refresh() {
    const [templateData, runData] = await Promise.all([listTemplates(), listRuns()]);
    setTemplates(templateData);
    setRuns(runData);
    if (!selectedRunId && runData.length > 0) {
      setSelectedRunId(runData[0].id);
    }
  }

  async function refreshEvents(runId: string) {
    setEvents(await listRunEvents(runId));
  }

  async function refreshTree(run: WorkflowRun) {
    await loadTreeDir(run, '', true);
  }

  async function loadTreeSubtree(run: WorkflowRun, basePath: string): Promise<{ children: Record<string, RepoTreeEntry[]>; files: string[] }> {
    const data = await listRepoTree(run.repo_ref, treeGitRef, {
      basePath,
      skipBinary,
      skipGitignore
    });

    const children: Record<string, RepoTreeEntry[]> = {
      [basePath]: data.entries
    };
    const files: string[] = [];

    for (const entry of data.entries) {
      if (entry.kind === 'file') {
        files.push(entry.path);
      } else if (entry.has_children) {
        const nested = await loadTreeSubtree(run, entry.path);
        Object.assign(children, nested.children);
        files.push(...nested.files);
      }
    }

    return { children, files };
  }

  async function loadTreeDir(run: WorkflowRun, basePath: string, replaceRoot = false) {
    if (loadingTreeDirs.has(basePath)) {
      return;
    }

    setTreeError(null);
    if (replaceRoot) {
      setTreeBusy(true);
    }
    setLoadingTreeDirs((prev) => {
      const next = new Set(prev);
      next.add(basePath);
      return next;
    });

    try {
      const data = await listRepoTree(run.repo_ref, treeGitRef, {
        basePath,
        skipBinary,
        skipGitignore
      });

      if (replaceRoot) {
        setTreeRootData(data);
        setTreeChildrenByParent(() => {
          const nextChildren: Record<string, RepoTreeEntry[]> = { '': data.entries };
          const visiblePaths = new Set<string>();
          for (const entries of Object.values(nextChildren)) {
            for (const entry of entries) {
              visiblePaths.add(entry.path);
            }
          }
          setSelectedPaths((prev) => prev.filter((path) => visiblePaths.has(path)));
          return nextChildren;
        });
      } else {
        setTreeChildrenByParent((prev) => {
          const nextChildren = {
            ...prev,
            [basePath]: data.entries
          };
          const visiblePaths = new Set<string>();
          for (const entries of Object.values(nextChildren)) {
            for (const entry of entries) {
              visiblePaths.add(entry.path);
            }
          }
          setSelectedPaths((prevSelected) => prevSelected.filter((path) => visiblePaths.has(path)));
          return nextChildren;
        });
      }
    } catch (err) {
      setTreeError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoadingTreeDirs((prev) => {
        const next = new Set(prev);
        next.delete(basePath);
        return next;
      });
      if (replaceRoot) {
        setTreeBusy(false);
      }
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  useEffect(() => {
    if (selectedRunId) {
      void refreshEvents(selectedRunId);
    } else {
      setEvents([]);
    }
  }, [selectedRunId]);

  useEffect(() => {
    if (!selectedRun) {
      setTreeRootData(null);
      setTreeChildrenByParent({});
      setLoadingTreeDirs(new Set());
      setSelectedPaths([]);
      return;
    }

    void refreshTree(selectedRun);
  }, [selectedRun?.id, selectedRun?.repo_ref, treeGitRef, skipBinary, skipGitignore]);


  function toggleFile(path: string) {
    setSelectedPaths((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return Array.from(next).sort();
    });
  }

  function setPaths(paths: string[], checked: boolean) {
    setSelectedPaths((prev) => {
      const next = new Set(prev);
      for (const path of paths) {
        if (checked) next.add(path);
        else next.delete(path);
      }
      return Array.from(next).sort();
    });
  }

  async function toggleDirectory(entry: RepoTreeEntry, checked: boolean) {
    if (!selectedRun) {
      return;
    }

    setTreeError(null);
    setLoadingTreeDirs((prev) => {
      const next = new Set(prev);
      next.add(entry.path);
      return next;
    });

    try {
      const { children, files } = await loadTreeSubtree(selectedRun, entry.path);

      setTreeChildrenByParent((prev) => ({
        ...prev,
        ...children
      }));

      setSelectedPaths((prev) => {
        const next = new Set(prev);
        for (const path of files) {
          if (checked) next.add(path);
          else next.delete(path);
        }
        return Array.from(next).sort();
      });
    } catch (err) {
      setTreeError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoadingTreeDirs((prev) => {
        const next = new Set(prev);
        next.delete(entry.path);
        return next;
      });
    }
  }

  return (
    <AppShell padding="md">
      <AppShell.Main>
        <Stack>
          <Group justify="space-between">
            <Title order={2}>Workflow Web</Title>
            <Button variant="light" onClick={() => void refresh()}>Refresh</Button>
          </Group>

          <SimpleGrid cols={{ base: 1, md: 2 }}>
            <Card withBorder>
              <Stack>
                <Title order={4}>Create workflow template</Title>
                <TextInput label="Name" value={templateName} onChange={(e) => setTemplateName(e.currentTarget.value)} />
                <TextInput label="Description" value={templateDescription} onChange={(e) => setTemplateDescription(e.currentTarget.value)} />
                <JsonInput
                  label="Definition"
                  autosize
                  minRows={16}
                  value={templateJson}
                  onChange={setTemplateJson}
                  formatOnBlur
                />
                <Button
                  loading={busy}
                  onClick={async () => {
                    setBusy(true);
                    try {
                      await createTemplate({
                        name: templateName,
                        description: templateDescription,
                        definition: JSON.parse(templateJson) as WorkflowTemplateDefinition
                      });
                      await refresh();
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  Save template
                </Button>
              </Stack>
            </Card>

            <Card withBorder>
              <Stack>
                <Title order={4}>Start workflow run</Title>
                <Select
                  label="Template"
                  data={templates.map((t) => ({ value: t.id, label: t.name }))}
                  value={templateId}
                  onChange={setTemplateId}
                  clearable
                />
                <TextInput label="Run title" value={runTitle} onChange={(e) => setRunTitle(e.currentTarget.value)} />
                <TextInput
                  label="Repo path"
                  placeholder="C:/repo or /home/user/repo"
                  value={repoRef}
                  onChange={(e) => setRepoRef(e.currentTarget.value)}
                />
                <Button
                  loading={busy}
                  onClick={async () => {
                    setBusy(true);
                    try {
                      const run = await createRun({
                        template_id: templateId,
                        title: runTitle,
                        repo_ref: repoRef,
                        context: {}
                      });
                      await refresh();
                      setSelectedRunId(run.id);
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  Create run
                </Button>
              </Stack>
            </Card>
          </SimpleGrid>

          <SimpleGrid cols={{ base: 1, lg: 2 }}>
            <Card withBorder>
              <Stack>
                <Title order={4}>Workflow runs</Title>
                <Table striped highlightOnHover>
                  <Table.Thead>
                    <Table.Tr>
                      <Table.Th>Title</Table.Th>
                      <Table.Th>Status</Table.Th>
                      <Table.Th>Repo</Table.Th>
                    </Table.Tr>
                  </Table.Thead>
                  <Table.Tbody>
                    {runs.map((run) => (
                      <Table.Tr
                        key={run.id}
                        onClick={() => setSelectedRunId(run.id)}
                        style={{ cursor: 'pointer', background: run.id === selectedRunId ? 'rgba(255,255,255,0.06)' : undefined }}
                      >
                        <Table.Td>{run.title}</Table.Td>
                        <Table.Td><Badge>{run.status}</Badge></Table.Td>
                        <Table.Td><Code>{run.repo_ref}</Code></Table.Td>
                      </Table.Tr>
                    ))}
                  </Table.Tbody>
                </Table>
              </Stack>
            </Card>

            <Card withBorder>
              <Stack>
                <Title order={4}>Run events</Title>
                {selectedRun ? (
                  <Stack gap="xs">
                    <Text size="sm">Selected run: <Code>{selectedRun.title}</Code></Text>
                    {events.length === 0 ? (
                      <Text c="dimmed">No events yet.</Text>
                    ) : (
                      events.slice().reverse().map((event) => (
                        <Card key={event.id} withBorder>
                          <Stack gap={4}>
                            <Group justify="space-between">
                              <Badge variant="light">{event.kind}</Badge>
                              <Text size="xs" c="dimmed">{event.created_at}</Text>
                            </Group>
                            <Text size="sm">{event.message}</Text>
                            <Code block>{JSON.stringify(event.payload, null, 2)}</Code>
                          </Stack>
                        </Card>
                      ))
                    )}
                  </Stack>
                ) : (
                  <Text c="dimmed">Select a run to inspect events.</Text>
                )}
              </Stack>
            </Card>
          </SimpleGrid>

          <Card withBorder>
            <Stack>
              <Group justify="space-between">
                <Title order={4}>Context exporter</Title>
                {selectedRun ? <Badge variant="light">{selectedRun.title}</Badge> : null}
              </Group>

              {!selectedRun ? (
                <Alert color="gray">Create or select a workflow run first. The selected run provides the repo path for tree scanning and the run id for capability execution.</Alert>
              ) : (
                <>
                  <Group align="end">
                    <TextInput label="Repo" value={selectedRun.repo_ref} readOnly style={{ flex: 1 }} />
                    <TextInput label="Git ref" value={treeGitRef} onChange={(e) => setTreeGitRef(e.currentTarget.value)} />
                    <Button variant="light" onClick={() => void refreshTree(selectedRun)} loading={treeBusy}>Refresh tree</Button>
                  </Group>

                  <Group align="end">
                    <Select
                      label="Mode"
                      value={contextMode}
                      onChange={(value) => setContextMode((value as 'entire_repo' | 'tree_select') ?? 'tree_select')}
                      data={[
                        { value: 'entire_repo', label: 'ENTIRE REPO' },
                        { value: 'tree_select', label: 'TREE SELECT' }
                      ]}
                    />
                    <TextInput
                      label="Save path"
                      placeholder="/tmp/repo_context.txt"
                      value={contextSavePath}
                      onChange={(e) => setContextSavePath(e.currentTarget.value)}
                      style={{ flex: 1 }}
                    />
                    <Button
                      loading={contextBusy}
                      onClick={async () => {
                        if (!selectedRun) return;
                        setContextBusy(true);
                        setContextStatus(null);
                        try {
                          const response = await invokeContextExport(selectedRun.id, {
                            step_id: 'context_export',
                            payload: {
                              repo_ref: selectedRun.repo_ref,
                              git_ref: treeGitRef,
                              exclude_regex: [],
                              skip_binary: skipBinary,
                              skip_gitignore: skipGitignore,
                              include_staged_diff: includeStagedDiff,
                              include_unstaged_diff: includeUnstagedDiff,
                              include_files: contextMode === 'tree_select' ? selectedPaths : null,
                              save_path: contextSavePath
                            }
                          });
                          setContextStatus(`Context export completed: ${String(response.output_path ?? '')}`);
                          await refreshEvents(selectedRun.id);
                        } catch (err) {
                          setContextStatus(err instanceof Error ? err.message : String(err));
                        } finally {
                          setContextBusy(false);
                        }
                      }}
                      disabled={!contextSavePath || (contextMode === 'tree_select' && selectedPaths.length === 0)}
                    >
                      Generate context file
                    </Button>
                  </Group>

                  <Group>
                    <Button
                      color={skipBinary ? 'blue' : 'gray'}
                      variant={skipBinary ? 'filled' : 'outline'}
                      onClick={() => setSkipBinary((v) => !v)}
                    >
                      {skipBinary ? 'Skip binary: ON' : 'Skip binary: OFF'}
                    </Button>
                    <Button
                      color={skipGitignore ? 'blue' : 'gray'}
                      variant={skipGitignore ? 'filled' : 'outline'}
                      onClick={() => setSkipGitignore((v) => !v)}
                    >
                      {skipGitignore ? 'Skip .gitignore: ON' : 'Skip .gitignore: OFF'}
                    </Button>
                    <Button
                      color={includeStagedDiff ? 'blue' : 'gray'}
                      variant={includeStagedDiff ? 'filled' : 'outline'}
                      onClick={() => setIncludeStagedDiff((v) => !v)}
                    >
                      {includeStagedDiff ? 'Staged diff: ON' : 'Staged diff: OFF'}
                    </Button>
                    <Button
                      color={includeUnstagedDiff ? 'blue' : 'gray'}
                      variant={includeUnstagedDiff ? 'filled' : 'outline'}
                      onClick={() => setIncludeUnstagedDiff((v) => !v)}
                    >
                      {includeUnstagedDiff ? 'Unstaged diff: ON' : 'Unstaged diff: OFF'}
                    </Button>
                  </Group>

                  {contextStatus ? <Alert color="blue">{contextStatus}</Alert> : null}
                  {treeError ? <Alert color="red">{treeError}</Alert> : null}

                  <Divider label="Tree selection" labelPosition="center" />

                  <Group justify="space-between">
                    <Text size="sm" c="dimmed">
                      {treeRootData ? `Refreshed ${treeRootData.refreshed_at}` : 'No tree data loaded yet.'}
                    </Text>
                    <Text size="sm">Selected files: <Code>{selectedPaths.length}</Code></Text>
                  </Group>

                  {treeBusy && !treeRootData ? (
                    <Group><Loader size="sm" /><Text size="sm">Scanning repository…</Text></Group>
                  ) : (
                    <RepoTree
                      rootEntries={rootTreeEntries}
                      childrenByParent={treeChildrenByParent}
                      loadingDirs={loadingTreeDirs}
                      selected={selectedSet}
                      onLoadDir={(path) => {
                        if (selectedRun) {
                          void loadTreeDir(selectedRun, path, false);
                        }
                      }}
                      onToggleFile={toggleFile}
                      onToggleDir={(entry, checked) => {
                        void toggleDirectory(entry, checked);
                      }}
                      onSetPaths={setPaths}
                    />
                  )}
                </>
              )}
            </Stack>
          </Card>
        </Stack>
      </AppShell.Main>
    </AppShell>
  );
}
