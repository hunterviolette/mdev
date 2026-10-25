import { useEffect, useMemo, useState } from 'react';
import { Alert, Badge, Box, Button, Group, Loader, Paper, ScrollArea, Select, SimpleGrid, Stack, Table, Text, TextInput, Title } from '@mantine/core';
import { getWorkflowCumulativeDiff } from './api';

type Metrics = {
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  input_tokens_estimated: boolean;
  output_tokens_estimated: boolean;
  input_tokens_incomplete: boolean;
  output_tokens_incomplete: boolean;
  runtime_seconds: number;
  stage_executions: number;
  completed_stages: number;
  failed_stages: number;
  stage_error_rate: number;
  changeset_attempts: number;
  changeset_applied: number;
  files_changed: number;
  lines_added: number | null;
  lines_removed: number | null;
};

type WorkflowRow = {
  run_id: string;
  title: string;
  status: string;
  pool_type: string;
  archived_at: string | null;
  created_at: string;
  updated_at: string;
  metrics: Metrics;
};

type Response = {
  supervisor_id: string;
  workflow_count: number;
  totals: Metrics;
  workflows: WorkflowRow[];
  unattributed: Metrics;
};

type SortField = 'tokens' | 'runtime' | 'stages' | 'errors' | 'diff';

const integer = (value: number) => new Intl.NumberFormat().format(value);
const tokenExplanation = 'Theoretical token usage is estimated from known input and output content using o200k_base. Provider-reported usage takes precedence where available. Actual provider context and tokenization may differ.';
const tokenIncompleteExplanation = 'Usage was unavailable for some invocations; the displayed total includes only recorded values.';
const tokenFlags = (metrics: Metrics) => ({
  estimated: metrics.input_tokens_estimated || metrics.output_tokens_estimated,
  incomplete: metrics.input_tokens_incomplete || metrics.output_tokens_incomplete,
});
const tokenText = (value: number, estimated: boolean, incomplete: boolean) => `${integer(value)}${estimated ? '*' : ''}${incomplete ? '†' : ''}`;
const totalTokenText = (metrics: Metrics) => {
  const flags = tokenFlags(metrics);
  return tokenText(metrics.total_tokens, flags.estimated, flags.incomplete);
};
const tokenDetail = (metrics: Metrics) => `${tokenText(metrics.input_tokens, metrics.input_tokens_estimated, metrics.input_tokens_incomplete)} input / ${tokenText(metrics.output_tokens, metrics.output_tokens_estimated, metrics.output_tokens_incomplete)} output`;
const tokenTitle = (metrics: Metrics) => {
  const flags = tokenFlags(metrics);
  return [flags.estimated ? tokenExplanation : '', flags.incomplete ? tokenIncompleteExplanation : ''].filter(Boolean).join(' ') || undefined;
};

const duration = (seconds: number) => {
  const total = Math.floor(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const remaining = total % 60;
  return hours ? `${hours}h ${minutes}m` : minutes ? `${minutes}m ${remaining}s` : `${remaining}s`;
};
const lines = (value: number | null, prefix: string) => value === null ? 'N/A' : `${prefix}${integer(value)}`;

export function SupervisorImpactPanel({ supervisorId, navigate }: { supervisorId: string; navigate?: (path: string) => void }) {
  const [from, setFrom] = useState('');
  const [to, setTo] = useState('');
  const [status, setStatus] = useState('all');
  const [visibility, setVisibility] = useState('all');
  const [sort, setSort] = useState<SortField>('tokens');
  const [data, setData] = useState<Response | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [cumulative, setCumulative] = useState<Record<string, { files: number; additions: number; deletions: number } | null>>({});
  const [diffLoading, setDiffLoading] = useState(false);

  useEffect(() => {
    const controller = new AbortController();
    const params = new URLSearchParams({ status, visibility });
    if (from) params.set('from', from);
    if (to) params.set('to', to);
    setLoading(true);
    setError(null);
    fetch(`/api/supervisor-runs/${encodeURIComponent(supervisorId)}/impact?${params}`, { signal: controller.signal })
      .then(async response => {
        if (!response.ok) throw new Error(await response.text());
        return response.json() as Promise<Response>;
      })
      .then(setData)
      .catch(error => { if (!controller.signal.aborted) setError(error instanceof Error ? error.message : String(error)); })
      .finally(() => { if (!controller.signal.aborted) setLoading(false); });
    return () => controller.abort();
  }, [supervisorId, from, to, status, visibility, refresh]);

  useEffect(() => {
    let active = true;
    setCumulative({});
    setDiffLoading(Boolean(data?.workflows.some(workflow => workflow.pool_type === 'feature' || workflow.pool_type === 'manual')));
    if (!data) return () => { active = false; };
    const runIds = data.workflows.filter(workflow => workflow.pool_type === 'feature' || workflow.pool_type === 'manual').map(workflow => workflow.run_id);
    void Promise.all(runIds.map(async runId => {
      try {
        const result = await getWorkflowCumulativeDiff(runId);
        return [runId, result.ok ? { files: result.files, additions: result.additions, deletions: result.deletions } : null] as const;
      } catch {
        return [runId, null] as const;
      }
    })).then(results => {
      if (active) {
        setCumulative(Object.fromEntries(results));
        setDiffLoading(false);
      }
    });
    return () => { active = false; };
  }, [data]);

  const workflows = useMemo(() => (data?.workflows ?? []).map(workflow => {
    const diff = cumulative[workflow.run_id];
    return {
      ...workflow,
      metrics: {
        ...workflow.metrics,
        lines_added: diff?.additions ?? null,
        lines_removed: diff?.deletions ?? null,
        files_changed: diff?.files ?? 0
      }
    };
  }).sort((a, b) => {
    const score = (row: WorkflowRow) => {
      switch (sort) {
        case 'runtime': return row.metrics.runtime_seconds;
        case 'stages': return row.metrics.stage_executions;
        case 'errors': return row.metrics.failed_stages;
        case 'diff': return (row.metrics.lines_added ?? 0) + (row.metrics.lines_removed ?? 0);
        default: return row.metrics.total_tokens;
      }
    };
    return score(b) - score(a);
  }), [data, cumulative, sort]);
  const diffTotals = useMemo(() => {
    const ids = data?.workflows.filter(workflow => workflow.pool_type === 'feature' || workflow.pool_type === 'manual').map(workflow => workflow.run_id) ?? [];
    const values = ids.map(id => cumulative[id]).filter((value): value is NonNullable<typeof value> => value != null);
    return {
      files: values.reduce((sum, value) => sum + value.files, 0),
      additions: values.reduce((sum, value) => sum + value.additions, 0),
      deletions: values.reduce((sum, value) => sum + value.deletions, 0),
      available: values.length,
      missing: ids.length - values.length,
    };
  }, [data, cumulative]);
  const diffReady = !diffLoading && diffTotals.available > 0;
  const diffValue = (value: number, prefix = '') => diffLoading ? 'Loading…' : diffReady ? `${prefix}${integer(value)}` : 'N/A';
  const totals = data?.totals;
  return <Stack gap="md">
    <Group justify="space-between" align="center">
      <Title order={4}>Supervisor Impact</Title>
      <Button size="xs" variant="subtle" loading={loading} onClick={() => setRefresh(value => value + 1)}>Refresh</Button>
    </Group>
    <SimpleGrid cols={{ base: 2, md: 4 }}>
      <TextInput label="From" type="date" value={from} onChange={event => setFrom(event.currentTarget.value)} />
      <TextInput label="To" type="date" value={to} onChange={event => setTo(event.currentTarget.value)} />
      <Select label="Workflow status" value={status} onChange={value => setStatus(value ?? 'all')} data={[{ value: 'all', label: 'All statuses' }, { value: 'running', label: 'Running' }, { value: 'completed', label: 'Completed' }, { value: 'failed', label: 'Failed' }, { value: 'queued', label: 'Queued' }, { value: 'paused', label: 'Paused' }, { value: 'cancelled', label: 'Cancelled' }, { value: 'waiting', label: 'Waiting' }]} />
      <Select label="Visibility" value={visibility} onChange={value => setVisibility(value ?? 'all')} data={[{ value: 'all', label: 'Current + archived' }, { value: 'current', label: 'Current only' }, { value: 'archived', label: 'Archived only' }]} />
    </SimpleGrid>
    {error && <Alert color="red">{error}</Alert>}
    {loading && !data && <Loader size="sm" />}
    {totals && <Stack gap="md">
      <Paper withBorder p="lg" radius="md">
        <Stack gap="md">
          <Box>
            <Text fw={700} size="sm">Cumulative workflow diff</Text>
            <Text size="xs" c="dimmed">Combined code changes from feature and manual workflows only. Integration and refine workflows are excluded. Files are counted per workflow.</Text>
          </Box>
          <SimpleGrid cols={{ base: 1, sm: 3 }} spacing="sm">
            <Paper withBorder radius="md" p="md"><Text size="xs" c="dimmed">Files changed</Text><Text fw={800} fz={26}>{diffValue(diffTotals.files)}</Text></Paper>
            <Paper withBorder radius="md" p="md"><Text size="xs" c="dimmed">Lines added</Text><Text fw={800} fz={26} c="teal">{diffValue(diffTotals.additions, '+')}</Text></Paper>
            <Paper withBorder radius="md" p="md"><Text size="xs" c="dimmed">Lines removed</Text><Text fw={800} fz={26} c="red">{diffValue(diffTotals.deletions, '−')}</Text></Paper>
          </SimpleGrid>
          <Group justify="space-between" gap="xs">
            <Text size="xs" c="dimmed">{diffLoading ? 'Loading feature and manual workflow diffs…' : `${diffTotals.available} of ${diffTotals.available + diffTotals.missing} feature/manual workflow diffs available`}</Text>
            {diffLoading && <Loader size="xs" />}
          </Group>
          {!diffLoading && diffTotals.missing > 0 && <Alert color="yellow" variant="light" title="Partial diff totals">{`${diffTotals.missing} workflow diff${diffTotals.missing === 1 ? ' is' : 's are'} unavailable. The figures above include only successful responses.`}</Alert>}
        </Stack>
      </Paper>
      <Paper withBorder p="lg" radius="md">
        <Stack gap="md">
          <Box>
            <Text fw={700} size="sm">Execution summary</Text>
            <Text size="xs" c="dimmed">Totals across the workflows matching the selected filters</Text>
          </Box>
          <SimpleGrid cols={{ base: 2, sm: 3, lg: 5 }} spacing="sm">
            {[
              ['Total tokens', totalTokenText(totals), tokenDetail(totals), undefined],
              ['Stage runtime', duration(totals.runtime_seconds), 'Sum of completed stage intervals', undefined],
              ['Executions', integer(totals.stage_executions), `${integer(totals.completed_stages)} completed`, undefined],
              ['Failed stages', integer(totals.failed_stages), `${(totals.stage_error_rate * 100).toFixed(1)}% of executions`, totals.failed_stages ? 'red' : undefined],
              ['Changesets', integer(totals.changeset_attempts), `${integer(totals.changeset_applied)} applied`, undefined]
            ].map(([label, value, detail, color]) => <Paper key={label} withBorder p="md" radius="md" title={label === 'Total tokens' ? tokenTitle(totals) : undefined}>
              <Stack gap={4}><Text size="xs" c="dimmed">{label}</Text><Text fw={800} fz={24} c={color} style={{ overflowWrap: 'anywhere' }}>{value}</Text><Text size="xs" c="dimmed">{detail}</Text></Stack>
            </Paper>)}
          </SimpleGrid>
        </Stack>
      </Paper>
    </Stack>}
    <Group justify="space-between">
      <Box><Text fw={700}>Workflow breakdown ({data?.workflow_count ?? 0})</Text><Text size="xs" c="dimmed">Individual execution metrics and cumulative repository changes</Text></Box>
      <Select size="xs" w={180} value={sort} onChange={value => setSort((value ?? 'tokens') as SortField)} data={[{ value: 'tokens', label: 'Most tokens' }, { value: 'runtime', label: 'Longest runtime' }, { value: 'stages', label: 'Most executions' }, { value: 'errors', label: 'Most errors' }, { value: 'diff', label: 'Largest cumulative diff' }]} />
    </Group>
    <ScrollArea>
      <Table striped highlightOnHover withTableBorder withColumnBorders miw={1050} verticalSpacing="sm">
        <Table.Thead><Table.Tr><Table.Th>Workflow</Table.Th><Table.Th>Status</Table.Th><Table.Th>Pool type</Table.Th><Table.Th ta="right">Tokens</Table.Th><Table.Th ta="right">Runtime</Table.Th><Table.Th ta="right">Stages</Table.Th><Table.Th ta="right">Errors</Table.Th><Table.Th ta="right">Lines added</Table.Th><Table.Th ta="right">Lines removed</Table.Th><Table.Th ta="right">Files</Table.Th></Table.Tr></Table.Thead>
        <Table.Tbody>{workflows.map(workflow => <Table.Tr key={workflow.run_id}>
          <Table.Td><Button size="compact-sm" variant="subtle" onClick={() => navigate ? navigate(`/workflows/${encodeURIComponent(workflow.run_id)}/impact`) : window.location.assign(`/workflows/${encodeURIComponent(workflow.run_id)}/impact`)}>{workflow.title}</Button></Table.Td>
          <Table.Td><Group gap={4} wrap="nowrap"><Badge size="xs" variant="light">{workflow.status}</Badge>{workflow.archived_at && <Badge size="xs" color="gray">Archived</Badge>}</Group></Table.Td>
          <Table.Td><Badge size="xs" variant="outline" color="gray">{workflow.pool_type.replace(/_/g, ' ')}</Badge></Table.Td>
          <Table.Td ta="right"><Text span title={tokenTitle(workflow.metrics)}>{totalTokenText(workflow.metrics)}</Text></Table.Td>
          <Table.Td ta="right">{duration(workflow.metrics.runtime_seconds)}</Table.Td>
          <Table.Td ta="right">{integer(workflow.metrics.stage_executions)}</Table.Td>
          <Table.Td ta="right"><Text c={workflow.metrics.failed_stages ? 'red' : undefined}>{integer(workflow.metrics.failed_stages)}</Text></Table.Td>
          <Table.Td ta="right"><Text c="teal" fw={600}>{lines(workflow.metrics.lines_added, '+')}</Text></Table.Td>
          <Table.Td ta="right"><Text c="red" fw={600}>{lines(workflow.metrics.lines_removed, '−')}</Text></Table.Td>
          <Table.Td ta="right">{cumulative[workflow.run_id] ? integer(workflow.metrics.files_changed) : 'N/A'}</Table.Td>
        </Table.Tr>)}
</Table.Tbody>
      </Table>
    </ScrollArea>
    {!loading && workflows.length === 0 && <Text c="dimmed" size="sm">No workflow activity matches the selected filters.</Text>}
  </Stack>;
}
