import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Alert, Badge, Box, Button, Checkbox, Code, Collapse, Divider, Grid, Group, HoverCard, Loader, Modal, Paper, Progress, ScrollArea, SegmentedControl, Select, SimpleGrid, Stack, Table, Tabs, Text, TextInput, Title } from '@mantine/core';
import { getImpactDetail, getImpactEvents, getImpactInference, getInferenceInputs, getWorkflowImpact, type ImpactDetail, type ImpactEffect, type ImpactEvent, type ImpactInferenceRecord, type ImpactInvocation, type InferenceInputRecord, type WorkflowImpact } from './api';
import { DiffTree, type DiffTreeFile } from './DiffTree';
import { getWorkflowReviewStatus, type ReviewStatusResponse, type ReviewStatusFileEntry } from './api';

const { ImpactFileExplorer } = (() => {
type Line = { sign: '+' | '-' | ' '; value: string };
const split = (value: string, sign: Line['sign']): Line[] => value.split('\n').map(value => ({ sign, value }));
const isApplied = (effect: ImpactEffect) => effect.status === 'applied';

function actionLines(effect: ImpactEffect): Line[] {
  const operation = effect.operation as Record<string, any> | null;
  if (!operation) return [];
  const actions = Array.isArray(operation.changes) ? operation.changes : [operation];
  const result: Line[] = [];
  for (const action of actions) {
    const kind = action.action || operation.op || effect.action;
    const before = action.match?.text;
    const after = action.replacement ?? action.text;
    if (kind === 'replace_block') {
      if (typeof before === 'string') result.push(...split(before, '-'));
      if (typeof after === 'string') result.push(...split(after, '+'));
    } else if (kind === 'insert_before' || kind === 'insert_after') {
      if (typeof after === 'string') result.push(...split(after, '+'));
    } else if ((kind === 'write' || kind === 'create') && typeof operation.contents === 'string') {
      result.push(...split(operation.contents, '+'));
    }
  }
  return result;
}

function ImpactFileExplorer({ files, effects }: { files: string[]; effects: ImpactEffect[] }) {
  const [active, setActive] = useState<string | null>(null);
  const [includeFailures, setIncludeFailures] = useState(false);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const applied = useMemo(() => effects.filter(isApplied), [effects]);
  const treeFiles = useMemo<DiffTreeFile[]>(() => files.map(path => {
    const rows = applied.filter(effect => effect.path_before === path || effect.path_after === path);
    const counts = rows.flatMap(actionLines);
    const last = rows[rows.length - 1];
    const kind = last?.action.toLowerCase() || '';
    const status = kind.includes('delete') ? 'D' : kind.includes('write') || kind.includes('create') ? 'A' : rows.length ? 'M' : '—';
    return { path, status, additions: counts.filter(line => line.sign === '+').length, deletions: counts.filter(line => line.sign === '-').length };
  }), [files, applied]);
  const visible = active ? effects.filter(effect => (effect.path_before === active || effect.path_after === active) && (includeFailures || isApplied(effect))) : [];
  const selected = treeFiles.find(item => item.path === active);
  return <>
    <Text size="xs" c="dimmed">Recorded changes · applied only · select a file to inspect</Text>
    <ScrollArea.Autosize mah={350}>
      <DiffTree files={treeFiles} selectedPath={null} collapsedDirs={collapsed} onToggleDir={path => setCollapsed(current => ({ ...current, [path]: !current[path] }))} onSelectFile={setActive} />
    </ScrollArea.Autosize>
    <Modal
      opened={active !== null}
      onClose={() => { setActive(null); setIncludeFailures(false); }}
      title="Review file changes"
      size="95%"
      centered
      styles={{ content: { height: '88vh', display: 'flex', flexDirection: 'column' }, body: { flex: 1, minHeight: 0, overflow: 'hidden', padding: 0 } }}
    >
      <Box style={{ display: 'flex', height: '100%', minHeight: 0 }}>
        <Box w={300} miw={220} style={{ flexShrink: 0, borderRight: '1px solid var(--mantine-color-default-border)', display: 'flex', flexDirection: 'column', minHeight: 0 }}>
          <Box p="sm" style={{ borderBottom: '1px solid var(--mantine-color-default-border)' }}>
            <Text fw={700} size="sm">Affected files</Text>
            <Text size="xs" c="dimmed">{treeFiles.length} files · select to review</Text>
          </Box>
          <ScrollArea style={{ flex: 1, minHeight: 0 }} p="xs">
            <DiffTree files={treeFiles} selectedPath={active} collapsedDirs={collapsed} onToggleDir={path => setCollapsed(current => ({ ...current, [path]: !current[path] }))} onSelectFile={setActive} />
          </ScrollArea>
        </Box>
        <Box style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', minHeight: 0 }}>
          <Box p="md" style={{ borderBottom: '1px solid var(--mantine-color-default-border)' }}>
            <Text fw={700} size="sm" style={{ overflowWrap: 'anywhere' }}>{active}</Text>
            <Group justify="space-between" mt="xs" align="center">
              <Group gap="xs"><Badge variant="outline">{selected?.status || '—'}</Badge><Text size="sm" c="teal">+{selected?.additions ?? 0}</Text><Text size="sm" c="red">−{selected?.deletions ?? 0}</Text></Group>
              <Checkbox size="xs" label="Include failed operations" checked={includeFailures} onChange={event => setIncludeFailures(event.currentTarget.checked)} />
            </Group>
            <Text size="xs" c="dimmed" mt="xs">Recorded action text, not a net Git diff. Repeated successful edits contribute repeatedly.</Text>
          </Box>
          <ScrollArea style={{ flex: 1, minHeight: 0 }} p="md">
            <Stack gap="md">
              {visible.length === 0 && <Text size="sm" c="dimmed">No {includeFailures ? 'recorded' : 'applied'} operations for this file.</Text>}
              {visible.map((effect, index) => {
                const lines = actionLines(effect);
                return <Paper key={`${effect.attempt_id}-${index}`} withBorder p="sm" radius="sm">
                  <Stack gap="xs">
                    <Group justify="space-between"><Badge size="xs" color={isApplied(effect) ? 'teal' : 'red'}>{effect.action} · {effect.status}</Badge><Text size="xs" c="dimmed">{new Date(effect.created_at).toLocaleString()}</Text></Group>
                    <Text size="xs" c="dimmed">Changeset {effect.attempt_id.slice(0, 8)} · {effect.attempt_status}</Text>
                    {effect.error && <Text size="xs" c="red">{effect.error}</Text>}
                    {lines.length ? <Box ff="monospace" fz={11} style={{ whiteSpace: 'pre', overflowX: 'auto' }}>{lines.map((line, lineIndex) => <Box key={lineIndex} px="xs" style={{ color: line.sign === '+' ? 'var(--mantine-color-teal-4)' : line.sign === '-' ? 'var(--mantine-color-red-4)' : 'var(--mantine-color-dimmed)', backgroundColor: line.sign === '+' ? 'rgba(40,160,100,.09)' : line.sign === '-' ? 'rgba(220,60,70,.09)' : 'transparent' }}>{line.sign} {line.value || ' '}</Box>)}</Box> : <Text size="xs" c="dimmed">No recorded line-level text for this operation.</Text>}
                  </Stack>
                </Paper>;
              })}
            </Stack>
          </ScrollArea>
        </Box>
      </Box>
    </Modal>
  </>;
}
return { ImpactFileExplorer };
})();

function InferenceTokenUsage({ accounting, inputTokens, outputTokens }: {
  accounting?: {
    reported_usage?: { input_tokens: number; output_tokens: number } | null;
    theoretical_usage?: { input_tokens: number; output_tokens: number; tokenizer: string; input_scope: string } | null;
  } | null;
  inputTokens: number | null;
  outputTokens: number | null;
}) {
  const reportedInput = accounting?.reported_usage?.input_tokens ?? inputTokens;
  const reportedOutput = accounting?.reported_usage?.output_tokens ?? outputTokens;
  const theoretical = accounting?.theoretical_usage;
  const hasReported = reportedInput != null || reportedOutput != null;
  const input = reportedInput ?? theoretical?.input_tokens;
  const output = reportedOutput ?? theoretical?.output_tokens;
  const estimatedInput = reportedInput == null && theoretical?.input_tokens != null;
  const estimatedOutput = reportedOutput == null && theoretical?.output_tokens != null;
  const format = (value: number | null | undefined, estimated: boolean) => value == null ? '—' : `${value.toLocaleString()}${estimated ? '*' : ''}`;
  return <Text size="xs" c="dimmed" title={estimatedInput || estimatedOutput ? inferenceTokenExplanation : undefined}>
    {hasReported ? 'Reported: ' : theoretical ? 'Theoretical: ' : ''}{format(input, estimatedInput)} in / {format(output, estimatedOutput)} out tokens
  </Text>;
}

const inferenceTokenExplanation = 'Theoretical token usage is estimated from known input and output content using o200k_base. Provider-reported usage takes precedence where available. Actual provider context and tokenization may differ.';

function inferenceTokenTotals(records: ImpactInvocation[]) {
  let input = 0;
  let output = 0;
  let inputKnown = 0;
  let outputKnown = 0;
  let inputEstimated = false;
  let outputEstimated = false;
  for (const record of records) {
    const reportedInput = record.accounting?.reported_usage?.input_tokens ?? record.input_tokens;
    const reportedOutput = record.accounting?.reported_usage?.output_tokens ?? record.output_tokens;
    const theoreticalInput = record.accounting?.theoretical_usage?.input_tokens;
    const theoreticalOutput = record.accounting?.theoretical_usage?.output_tokens;
    const inputValue = reportedInput ?? theoreticalInput;
    const outputValue = reportedOutput ?? theoreticalOutput;
    if (inputValue != null) {
      input += inputValue;
      inputKnown++;
      if (reportedInput == null) inputEstimated = true;
    }
    if (outputValue != null) {
      output += outputValue;
      outputKnown++;
      if (reportedOutput == null) outputEstimated = true;
    }
  }
  return {
    input: inputKnown ? input : null,
    output: outputKnown ? output : null,
    inputEstimated,
    outputEstimated,
    inputIncomplete: inputKnown < records.length,
    outputIncomplete: outputKnown < records.length,
  };
}

function InferenceTokenValue({ value, estimated, incomplete }: { value: number | null; estimated: boolean; incomplete?: boolean }) {
  const explanation = [estimated ? inferenceTokenExplanation : '', incomplete ? 'Usage was unavailable for some invocations; the displayed total includes only recorded values.' : ''].filter(Boolean).join(' ');
  return <Text span title={explanation || undefined} style={{ whiteSpace: 'nowrap' }}>{value == null ? '—' : value.toLocaleString()}{estimated && <Text span c="dimmed">*</Text>}{incomplete && <Text span c="dimmed">†</Text>}</Text>;
}

const { ImpactInferenceSummary } = (() => {
type Props = { runId: string; records: ImpactInvocation[]; responses: number; onHistory: () => void };
const failed = (r: ImpactInvocation) => /fail|error|cancel/i.test(r.status) || !!r.error;
const elapsed = (start: string, end: string | null) => {
  if (!end) return 'Not recorded';
  const milliseconds = Date.parse(end) - Date.parse(start);
  if (!Number.isFinite(milliseconds) || milliseconds < 0) return 'Not recorded';
  return milliseconds < 1000 ? `${milliseconds} ms` : milliseconds < 60000 ? `${(milliseconds / 1000).toFixed(1)} s` : `${Math.floor(milliseconds / 60000)}m ${Math.round(milliseconds % 60000 / 1000)}s`;
};
const cost = (rows: ImpactInvocation[]) => {
  const priced = rows.filter(r => r.cost != null);
  if (!priced.length) return 'Unavailable';
  const currencies = [...new Set(priced.map(r => r.currency || 'unknown'))];
  return currencies.length !== 1 ? 'Mixed currencies' : `${currencies[0]} ${priced.reduce((n, r) => n + (r.cost || 0), 0).toFixed(4)}`;
};
function InferenceTokenUsage({ accounting, inputTokens, outputTokens }: {
  accounting: ImpactInvocation['accounting'];
  inputTokens: number | null;
  outputTokens: number | null;
}) {
  const reported = accounting?.reported_usage;
  const theoretical = accounting?.theoretical_usage;
  const reportedInput = reported?.input_tokens ?? inputTokens;
  const reportedOutput = reported?.output_tokens ?? outputTokens;
  const hasReported = reportedInput != null || reportedOutput != null;
  const primaryInput = reportedInput ?? theoretical?.input_tokens;
  const primaryOutput = reportedOutput ?? theoretical?.output_tokens;
  const format = (value: number | null | undefined, estimated: boolean) => value == null ? '—' : `${value.toLocaleString()}${estimated ? '*' : ''}`;
  const estimatedInput = reportedInput == null && theoretical?.input_tokens != null;
  const estimatedOutput = reportedOutput == null && theoretical?.output_tokens != null;
  return <Text size="xs" c="dimmed" title={estimatedInput || estimatedOutput ? inferenceTokenExplanation : undefined}>
    {hasReported ? 'Reported: ' : theoretical ? 'Theoretical: ' : ''}{format(primaryInput, estimatedInput)} in / {format(primaryOutput, estimatedOutput)} out tokens
  </Text>;
}

function InvocationRow({ runId, item }: { runId: string; item: ImpactInvocation }) {
  const [opened, setOpened] = useState(false);
  const [detail, setDetail] = useState<ImpactInferenceRecord | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  useEffect(() => { if (!opened || detail || loading || error) return; let active = true; setLoading(true); getImpactInference(runId, { invocation_id: item.id, limit: 1 }).then(response => { if (active) setDetail(response.invocations[0] || null); }).catch(e => { if (active) setError(String(e)); }).finally(() => { if (active) setLoading(false); }); return () => { active = false; }; }, [opened, runId, item.id]);
  return <Paper withBorder radius="sm" p="xs"><Stack gap="xs">
    <Box component="button" type="button" onClick={() => setOpened(v => !v)} style={{ cursor: 'pointer', width: '100%', border: 0, background: 'transparent', color: 'inherit', textAlign: 'left' }}>
      <Group justify="space-between" wrap="wrap" gap="xs"><Group gap="xs"><Text size="xs">{opened ? '▾' : '▸'} {item.stage_type} · {item.model || 'Unknown model'}</Text><Badge size="xs" color={failed(item) ? 'red' : 'teal'}>{item.status}</Badge></Group><Text size="xs" c="dimmed">{elapsed(item.created_at, item.completed_at)}</Text></Group>
      <Text size="xs" c="dimmed">{new Date(item.created_at).toLocaleString()} · {item.cost == null ? 'Cost unavailable' : `${item.currency || ''} ${item.cost}`}</Text><InferenceTokenUsage accounting={item.accounting} inputTokens={item.input_tokens} outputTokens={item.output_tokens} />
    </Box>
    <Collapse in={opened}>{loading && <Loader size="xs" />}{error && <Text size="xs" c="red">{error}</Text>}{detail && <Stack gap="sm"><InferenceTokenUsage accounting={detail.accounting} inputTokens={detail.input_tokens} outputTokens={detail.output_tokens} /><Text size="xs" fw={700}>Inputs ({detail.inputs.length})</Text>{detail.inputs.map((input, index) => <Box key={index}><Text size="xs" c="dimmed">{input.label || input.input_type}</Text><Box component="pre" p="xs" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', maxHeight: 250, overflowY: 'auto', fontSize: 11, background: 'var(--mantine-color-default-hover)' }}>{input.content}</Box></Box>)}<Text size="xs" fw={700}>Output</Text><Box component="pre" p="xs" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', maxHeight: 350, overflowY: 'auto', fontSize: 11, background: 'var(--mantine-color-default-hover)' }}>{detail.response_text || 'No text output recorded'}</Box>{detail.provider_metadata != null && <><Text size="xs" fw={700}>Provider metadata</Text><Box component="pre" p="xs" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', maxHeight: 180, overflowY: 'auto', fontSize: 11 }}>{JSON.stringify(detail.provider_metadata, null, 2)}</Box></>}{detail.error_message && <Text size="xs" c="red">{detail.error_message}</Text>}</Stack>}</Collapse>
  </Stack></Paper>;
}
function InferenceInputsView({ runId }: { runId: string }) {
  const [rows, setRows] = useState<InferenceInputRecord[]>([]);
  const [draft, setDraft] = useState('');
  const [search, setSearch] = useState('');
  const [hideDefaults, setHideDefaults] = useState(true);
  const [stage, setStage] = useState<string | null>(null);
  const [offset, setOffset] = useState(0);
  const [total, setTotal] = useState(0);
  const [nextOffset, setNextOffset] = useState<number | null>(null);
  const [stages, setStages] = useState<string[]>([]);
  const [expandedInputs, setExpandedInputs] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    setRows([]);
    setOffset(0);
    setTotal(0);
    setNextOffset(null);
    setExpandedInputs([]);
    setStages([]);
  }, [runId]);

  function resetFilteredResults() {
    setRows([]);
    setOffset(0);
    setTotal(0);
    setNextOffset(null);
    setExpandedInputs([]);
    setError(null);
  }

  function updateSearch(value: string) {
    resetFilteredResults();
    setSearch(value);
  }

  function updateStage(value: string | null) {
    resetFilteredResults();
    setStage(value);
  }

  function updateHideDefaults(value: boolean) {
    resetFilteredResults();
    setHideDefaults(value);
  }

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    getInferenceInputs(runId, {
      search,
      hide_defaults: hideDefaults,
      stage_type: stage || undefined,
      offset,
      limit: 100,
    }).then(result => {
      if (!active) return;
      setRows(current => offset === 0 ? result.inputs : [...current, ...result.inputs.filter(item => !current.some(row => row.id === item.id))]);
      setTotal(result.total);
      setNextOffset(result.next_offset);
      setStages(current => [...new Set([...current, ...result.inputs.map(item => item.stage_type)])].sort());
    }).catch(reason => {
      if (active) setError(String(reason));
    }).finally(() => {
      if (active) setLoading(false);
    });
    return () => { active = false; };
  }, [runId, search, hideDefaults, stage, offset, revision]);

  return <Stack gap="sm">
    <Group align="center" gap="sm" wrap="nowrap" style={{ width: '100%', overflowX: 'auto', paddingBottom: 2 }}>
      <TextInput aria-label="Search input text" placeholder="Search input text, label, or source" value={draft} onChange={event => setDraft(event.currentTarget.value)} onKeyDown={event => { if (event.key === 'Enter') updateSearch(draft.trim()); }} size="sm" style={{ flex: '1 1 240px', minWidth: 180 }} />
      <Button size="sm" variant="light" onClick={() => updateSearch(draft.trim())}>Search</Button>
      <Select aria-label="Stage" placeholder="All stages" clearable searchable value={stage} onChange={updateStage} data={stages.map(value => ({ value, label: value }))} size="sm" w={160} style={{ flexShrink: 0 }} />
      <Checkbox label="Hide default inputs" checked={hideDefaults} onChange={event => updateHideDefaults(event.currentTarget.checked)} styles={{ root: { flexShrink: 0 }, label: { whiteSpace: 'nowrap' } }} />
      <Button size="sm" variant="subtle" disabled={loading} onClick={() => { resetFilteredResults(); setRevision(value => value + 1); }}>Refresh</Button>
    </Group>
    <Text size="xs" c="dimmed">{rows.length} of {total} matching inputs</Text>
    {error && <Text size="xs" c="red">{error}</Text>}
    <ScrollArea.Autosize mah={480} type="auto" offsetScrollbars>
      <Table striped highlightOnHover verticalSpacing="sm" miw={1000}>
        <Table.Thead><Table.Tr><Table.Th>Time</Table.Th><Table.Th>Session</Table.Th><Table.Th>Stage</Table.Th><Table.Th>Status</Table.Th><Table.Th>Origin</Table.Th><Table.Th>Source</Table.Th><Table.Th>Input text</Table.Th></Table.Tr></Table.Thead>
        <Table.Tbody>{rows.map(item => {
          const expanded = expandedInputs.includes(item.id);
          const content = expanded || item.content.length <= 450 ? item.content : `${item.content.slice(0, 450)}…`;
          return <Table.Tr key={item.id}>
            <Table.Td style={{ verticalAlign: 'top', whiteSpace: 'nowrap' }}><Text size="xs">{new Date(item.created_at).toLocaleString()}</Text></Table.Td>
            <Table.Td style={{ verticalAlign: 'top' }}><Text size="xs" fw={600}>Session {item.session_id.slice(0, 12)}</Text><Text size="xs" c="dimmed" style={{ overflowWrap: 'anywhere' }}>{item.session_id}</Text></Table.Td>
            <Table.Td style={{ verticalAlign: 'top' }}><Text size="xs">{item.stage_type}</Text><Text size="xs" c="dimmed">{item.step_id}</Text></Table.Td>
            <Table.Td style={{ verticalAlign: 'top' }}><Badge size="xs" color={item.invocation_status === 'failed' ? 'red' : 'teal'} title={item.invocation_error || undefined}>{item.invocation_status === 'failed' ? 'Error' : item.invocation_status}</Badge>{item.invocation_error && <Text size="xs" c="red" style={{ overflowWrap: 'anywhere' }}>{item.invocation_error}</Text>}</Table.Td>
            <Table.Td style={{ verticalAlign: 'top' }}><Badge size="xs" variant="light" color={item.input_origin === 'user_custom' ? 'blue' : 'gray'}>{item.input_origin.replace(/_/g, ' ')}</Badge></Table.Td>
            <Table.Td style={{ verticalAlign: 'top' }}><Text size="xs">{item.label}</Text><Text size="xs" c="dimmed">{item.input_source}</Text></Table.Td>
            <Table.Td style={{ minWidth: 300, maxWidth: 650, verticalAlign: 'top' }}><Text size="xs" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{content}</Text>{item.content.length > 450 && <Button size="compact-xs" variant="subtle" onClick={() => setExpandedInputs(current => expanded ? current.filter(id => id !== item.id) : [...current, item.id])}>{expanded ? 'Show less' : 'Show full input'}</Button>}</Table.Td>
          </Table.Tr>;
        })}</Table.Tbody>
      </Table>
    </ScrollArea.Autosize>
    {!loading && !rows.length && <Text size="sm" c="dimmed">No recorded inputs match the filters.</Text>}
    {loading && <Loader size="xs" />}
    {nextOffset !== null && !loading && <Group justify="flex-end"><Button size="xs" variant="light" onClick={() => setOffset(nextOffset)}>Load more</Button></Group>}
  </Stack>;
}

function ImpactInferenceSummary({ runId, records, responses, onHistory }: Props) {
  const [view, setView] = useState<'sessions' | 'inputs'>('sessions');
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [inputCounts, setInputCounts] = useState<Record<string, number>>({});
  const [inputCountsLoaded, setInputCountsLoaded] = useState(false);
  useEffect(() => {
    let active = true;
    setInputCounts({});
    setInputCountsLoaded(false);
    getInferenceInputs(runId, { limit: 1, hide_defaults: true }).then(result => {
      if (!active) return;
      setInputCounts(result.non_default_by_session || {});
      setInputCountsLoaded(true);
    }).catch(() => {
      if (active) setInputCountsLoaded(false);
    });
    return () => { active = false; };
  }, [runId, records]);
  const totals = useMemo(() => inferenceTokenTotals(records), [records]);
  const nonDefaultInputs = inputCountsLoaded ? [...new Set(records.map(record => record.session_id))].reduce((sum, id) => sum + (inputCounts[id] || 0), 0) : null;
  const providers = useMemo(() => {
    const groups = new Map<string, Map<string, ImpactInvocation[]>>();
    for (const record of records) {
      const provider = record.provider?.trim() || record.transport || 'Unknown provider';
      if (!groups.has(provider)) groups.set(provider, new Map());
      const sessions = groups.get(provider)!;
      if (!sessions.has(record.session_id)) sessions.set(record.session_id, []);
      sessions.get(record.session_id)!.push(record);
    }
    return [...groups.entries()].map(([name, sessions]) => ({ name, sessions: [...sessions.entries()].map(([id, calls]) => ({ id, calls: calls.sort((a, b) => a.created_at.localeCompare(b.created_at)) })) }));
  }, [records]);
  const errorCount = records.filter(failed).length;
  return <Paper withBorder radius="md" p="lg"><Stack gap="md">
    <Group justify="space-between" align="center" wrap="wrap" gap="sm">
      <Text fw={700}>Inference</Text>
      <Group gap="sm">
        <SegmentedControl
          size="xs"
          value={view}
          onChange={(value) => setView(value as 'sessions' | 'inputs')}
          data={[{ label: 'Sessions', value: 'sessions' }, { label: 'Inputs', value: 'inputs' }]}
        />
        <Button size="xs" variant="subtle" onClick={onHistory}>History →</Button>
      </Group>
    </Group>
    <Group gap="xl" wrap="wrap" align="flex-start">
      <Box><Text size="xs" c="dimmed">Responses</Text><Text fw={700}>{responses}</Text></Box>
      <Box><Text size="xs" c="dimmed">Invocations</Text><Text fw={700}>{records.length}</Text></Box>
      <Box><Text size="xs" c="dimmed">Non-default inputs</Text><Text fw={700}>{nonDefaultInputs == null ? '—' : nonDefaultInputs.toLocaleString()}</Text></Box>
      <Box><Text size="xs" c="dimmed">Input tokens</Text><Text fw={700}><InferenceTokenValue value={totals.input} estimated={totals.inputEstimated} incomplete={totals.inputIncomplete} /></Text></Box>
      <Box><Text size="xs" c="dimmed">Output tokens</Text><Text fw={700}><InferenceTokenValue value={totals.output} estimated={totals.outputEstimated} incomplete={totals.outputIncomplete} /></Text></Box>
      <Box><Text size="xs" c="dimmed">Provider errors</Text><Text fw={700} c={errorCount ? 'red' : undefined}>{errorCount}</Text></Box>
      <Box><Text size="xs" c="dimmed">Recorded cost</Text><Text fw={700}>{cost(records)}</Text></Box>
    </Group>
    {view === 'inputs' ? <InferenceInputsView runId={runId} /> : <>
    {!providers.length && <Text size="sm" c="dimmed">No recorded inference sessions.</Text>}
    {providers.map(provider => {
      const calls = provider.sessions.flatMap(session => session.calls);
      const providerErrors = calls.filter(failed).length;
      return <Stack gap="xs" key={provider.name}>
        <Group justify="space-between" gap="xs">
          <Text fw={700} size="sm">{provider.name}</Text>
          <Text size="xs" c="dimmed">{provider.sessions.length} sessions · {calls.length} calls{providerErrors ? ` · ${providerErrors} errors` : ''}</Text>
        </Group>
        <Paper withBorder radius="sm" style={{ overflow: 'hidden' }}>
          <Stack gap={0}>
            {provider.sessions.map((session, index) => {
              const key = `${provider.name}:${session.id}`;
              const open = expanded[key] ?? (provider.sessions.length === 1);
              const start = session.calls[0]?.created_at;
              const finish = session.calls.filter(record => record.completed_at).map(record => record.completed_at!).sort().slice(-1)[0] || null;
              const errors = session.calls.filter(failed).length;
              const sessionTokens = inferenceTokenTotals(session.calls);
              const sessionInputs = inputCountsLoaded ? inputCounts[session.id] || 0 : null;
              const models = [...new Set(session.calls.map(record => record.model?.trim()).filter((model): model is string => !!model && model.toLowerCase() !== 'unknown' && model.toLowerCase() !== 'unknown model'))];
              return <Box key={key} style={{ borderTop: index ? '1px solid var(--mantine-color-default-border)' : undefined }}>
                <Box component="button" type="button" onClick={() => setExpanded(current => ({ ...current, [key]: !open }))} px="sm" py={9} style={{ cursor: 'pointer', width: '100%', border: 0, background: 'transparent', color: 'inherit', textAlign: 'left' }}>
                  <Group gap="sm" wrap="nowrap">
                    <Text size="xs" c="dimmed" w={12}>{open ? '▾' : '▸'}</Text>
                    <Box style={{ flex: 1, minWidth: 0 }}>
                      <Text size="sm" fw={600} truncate>Session {session.id.slice(0, 12)}{models.length ? ` · ${models.join(', ')}` : ''}</Text>
                      <Group gap="xs" wrap="wrap" mt={2}>
                        <Text size="xs" c="dimmed">{session.calls.length} {session.calls.length === 1 ? 'call' : 'calls'}</Text>
                        <Text size="xs" c="dimmed">·</Text>
                        <Text size="xs" c="dimmed">{sessionInputs == null ? '—' : sessionInputs.toLocaleString()} non-default inputs</Text>
                        <Text size="xs" c="dimmed">·</Text>
                        <Text size="xs" c="dimmed"><InferenceTokenValue value={sessionTokens.input} estimated={sessionTokens.inputEstimated} incomplete={sessionTokens.inputIncomplete} /> in</Text>
                        <Text size="xs" c="dimmed">·</Text>
                        <Text size="xs" c="dimmed"><InferenceTokenValue value={sessionTokens.output} estimated={sessionTokens.outputEstimated} incomplete={sessionTokens.outputIncomplete} /> out</Text>
                      </Group>
                    </Box>
                    {errors > 0 && <Badge size="xs" color="red" variant="light">{errors} errors</Badge>}
                    <Text size="xs" c="dimmed" ta="right" w={85} style={{ whiteSpace: 'nowrap' }}>{start ? elapsed(start, finish) : 'Unavailable'}</Text>
                  </Group>
                </Box>
                <Collapse in={open}>
                  <Stack gap="xs" px="sm" pb="sm" pt={4} style={{ borderTop: '1px solid var(--mantine-color-default-border)' }}>
                    {session.calls.map(call => <InvocationRow key={call.id} runId={runId} item={call} />)}
                  </Stack>
                </Collapse>
              </Box>;
            })}
          </Stack>
        </Paper>
      </Stack>;
    })}
    <Text size="xs" c="dimmed">Session duration spans the first invocation to the last recorded completion. Hover over * for theoretical token methodology and † for incomplete totals.</Text>
    </>}
  </Stack></Paper>;
}
return { ImpactInferenceSummary };
})();

const { ImpactEvidence } = (() => {
type Props = { runId: string; data: WorkflowImpact; onInspect: (kind: 'code' | 'inference') => void };
const fmt = (n: number | null | undefined) => n == null ? '—' : n.toLocaleString();
type DiffLine = { mark: '+' | '-' | ' '; text: string };
const lines = (value: string, mark: DiffLine['mark']): DiffLine[] => value.split('\n').map(text => ({ mark, text }));
const diff = (effect: ImpactEffect): DiffLine[] => {
  const op = effect.operation || {};
  const actions = Array.isArray(op.changes) ? op.changes : [op];
  const result: DiffLine[] = [];
  for (const change of actions) {
    const action = change.action || op.op || effect.action;
    const before = change.match?.text;
    const after = change.replacement ?? change.text;
    if (action === 'replace_block') {
      if (typeof before === 'string') result.push(...lines(before, '-'));
      if (typeof after === 'string') result.push(...lines(after, '+'));
    } else if (action === 'insert_before' || action === 'insert_after') {
      if (typeof after === 'string') result.push(...lines(after, '+'));
    } else if (action === 'write' && typeof op.contents === 'string') {
      result.push(...lines(op.contents, '+'));
    } else if (action === 'delete') {
      result.push({ mark: ' ', text: 'File deleted; previous content not recorded' });
    } else if (action === 'move') {
      result.push({ mark: ' ', text: `Moved ${op.from || effect.path_before || ''} → ${op.to || effect.path_after || ''}` });
    }
  }
  return result;
};
const rate = (passed: number, total: number) => total > 0 ? `${(100 * passed / total).toFixed(1)}%` : '—';
function DiffView({ effect }: { effect: ImpactEffect }) {
  const content = diff(effect);
  if (!content.length) return <Text size="xs" c="dimmed">No recorded text delta for this action.</Text>;
  return <ScrollArea.Autosize mah={320}><Box ff="monospace" fz={11} style={{ whiteSpace: 'pre', overflowX: 'auto' }}>{content.map((line, i) => <Box key={i} px="xs" style={{ color: line.mark === '+' ? 'var(--mantine-color-teal-4)' : line.mark === '-' ? 'var(--mantine-color-red-4)' : 'var(--mantine-color-dimmed)', background: line.mark === '+' ? 'rgba(40,160,100,.09)' : line.mark === '-' ? 'rgba(220,60,70,.09)' : 'transparent' }}>{line.mark} {line.text || ' '}</Box>)}</Box></ScrollArea.Autosize>;
}

const money = (records: ImpactInvocation[]) => { const priced = records.filter(r => r.cost != null); const currencies = [...new Set(priced.map(r => r.currency || 'unknown'))]; return priced.length === 0 ? 'Unavailable' : currencies.length !== 1 ? 'Mixed currencies' : `${currencies[0]} ${priced.reduce((sum, r) => sum + (r.cost || 0), 0).toFixed(4)}`; };

function ImpactEvidence({ runId, data, onInspect }: Props) {
  const [detail, setDetail] = useState<ImpactDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [selectedProvider, setSelectedProvider] = useState<string | null>(null);
  const [selectedSession, setSelectedSession] = useState<string>('all');
  const [selectedStage, setSelectedStage] = useState<string>('all');
  useEffect(() => { let active = true; setDetail(null); setError(null); getImpactDetail(runId).then(value => { if (active) setDetail(value); }).catch(e => { if (active) setError(String(e)); }); return () => { active = false; }; }, [runId, data.changeset_attempts, data.inference_outputs]);
  const effects = detail?.effects || [];
  const all = detail?.inference || [];
  const attempts = detail?.attempts || [];
  const operations = attempts.reduce((sum, item) => sum + item.total_ops, 0);
  const appliedOperations = attempts.reduce((sum, item) => sum + item.applied_ops, 0);
  const actionTotal = attempts.reduce((sum, item) => sum + item.total_actions, 0);
  const actionApplied = attempts.reduce((sum, item) => sum + item.applied_actions, 0);
  const recordedLines = effects.filter(e => e.status === 'applied').flatMap(diff).filter(line => line.mark !== ' ');
  const addedLines = recordedLines.filter(line => line.mark === '+').length;
  const removedLines = recordedLines.filter(line => line.mark === '-').length;
  const files = useMemo(() => [...new Set([...data.file_paths, ...effects.flatMap(e => [e.path_before, e.path_after]).filter((p): p is string => !!p)])].sort(), [data.file_paths, effects]);
  const file = selectedFile && files.includes(selectedFile) ? selectedFile : files[0];
  const fileEffects = file ? effects.filter(e => e.path_before === file || e.path_after === file) : [];
  const sessions = [...new Set(all.map(i => i.session_id))];
  const stages = [...new Set(all.map(i => i.stage_type))];
  const scoped = all.filter(i => (selectedSession === 'all' || i.session_id === selectedSession) && (selectedStage === 'all' || i.stage_type === selectedStage));
  const providers = [...new Set(scoped.map(i => `${i.provider}|${i.transport}`))];
  const provider = selectedProvider && providers.includes(selectedProvider) ? selectedProvider : providers[0];
  const selectedInvocations = scoped.filter(i => `${i.provider}|${i.transport}` === provider);
  const models = [...new Set(selectedInvocations.map(i => i.model))];
  return <GridLayout>
    <Paper withBorder radius="md" p="lg"><Stack gap="md">
      <Group justify="space-between"><Text fw={700}>Code impact</Text><Button size="xs" variant="subtle" onClick={() => onInspect('code')}>History →</Button></Group>
      <Group align="stretch" gap="lg" wrap="nowrap"><Stack gap="md" style={{ flex: '0 0 38%', minWidth: 0 }}><Title order={2}>{data.files_changed}<Text span size="xs" c="dimmed"> files</Text></Title><SimpleGrid cols={3}><Metric label="Created" value={data.files_created} /><Metric label="Modified" value={data.files_modified} /><Metric label="Deleted" value={data.files_deleted} /></SimpleGrid><SimpleGrid cols={2}><Metric label="Recorded + lines" value={addedLines ? `+${fmt(addedLines)}` : data.lines_added == null ? '—' : `+${fmt(data.lines_added)}`} /><Metric label="Recorded − lines" value={removedLines ? `−${fmt(removedLines)}` : data.lines_removed == null ? '—' : `−${fmt(data.lines_removed)}`} /></SimpleGrid><Text size="xs" c="dimmed">Recorded action text is not a complete historical repository diff.</Text><Box><Group justify="space-between"><Text size="xs" fw={700}>Changesets applied</Text><Text size="xs">{data.changeset_applied}/{data.changeset_attempts} · {rate(data.changeset_applied, data.changeset_attempts)}</Text></Group><Box h={7} mt={5} bg="dark.5" style={{ borderRadius: 8, overflow: 'hidden' }}><Box h="100%" bg="teal" style={{ width: rate(data.changeset_applied, data.changeset_attempts) === '—' ? '0%' : rate(data.changeset_applied, data.changeset_attempts) }} /></Box></Box><Box><Group justify="space-between"><Text size="xs" fw={700}>Operations applied</Text><Text size="xs">{appliedOperations}/{operations} · {rate(appliedOperations, operations)}</Text></Group><Box h={7} mt={5} bg="dark.5" style={{ borderRadius: 8, overflow: 'hidden' }}><Box h="100%" bg="blue" style={{ width: rate(appliedOperations, operations) === '—' ? '0%' : rate(appliedOperations, operations) }} /></Box></Box><Box><Group justify="space-between"><Text size="xs" fw={700}>Actions applied</Text><Text size="xs">{actionApplied}/{actionTotal} · {rate(actionApplied, actionTotal)}</Text></Group><Box h={7} mt={5} bg="dark.5" style={{ borderRadius: 8, overflow: 'hidden' }}><Box h="100%" bg="cyan" style={{ width: rate(actionApplied, actionTotal) === '—' ? '0%' : rate(actionApplied, actionTotal) }} /></Box></Box></Stack>
      <Stack gap="xs" style={{ flex: 1, minWidth: 0 }}><Text size="xs" fw={700}>Affected files</Text><ImpactFileExplorer files={files} effects={effects} />{false && file && <ScrollArea.Autosize mah={270}><Stack gap="xs">{fileEffects.map((e, index) => <Paper key={`${e.attempt_id}-${index}`} p="xs" withBorder><Group justify="space-between"><Badge size="xs" color={e.status === 'applied' ? 'teal' : 'red'}>{e.action} · {e.status}</Badge><Text size="xs" c="dimmed">{new Date(e.created_at).toLocaleString()}</Text></Group><Text size="xs" c="dimmed">Attempt {e.attempt_id.slice(0, 8)} · {e.attempt_status}</Text>{e.error && <Text size="xs" c="red">{e.error}</Text>}<DiffView effect={e} /></Paper>)}</Stack></ScrollArea.Autosize>}</Stack></Group>
      {error && <Paper withBorder radius="sm" p="sm"><Stack gap={4}><Text size="sm" fw={700} c="red">Detailed Impact data unavailable</Text><Text size="xs" c="dimmed">The summary metrics and affected-file names remain available. Individual changeset actions, historical diffs, inference sessions, and provider statistics require the detail API.</Text><Text size="xs" c="red" style={{ overflowWrap: 'anywhere' }}>{error}</Text></Stack></Paper>}
    </Stack></Paper>
    <ImpactInferenceSummary runId={runId} records={all} responses={data.inference_outputs} onHistory={() => onInspect('inference')} />
  </GridLayout>;
}
function Metric({ label, value }: { label: string; value: string | number }) { return <Box><Text size="xs" c="dimmed">{label}</Text><Text size="sm" fw={700}>{value}</Text></Box>; }
function GridLayout({ children }: { children: React.ReactNode }) { return <Box style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(min(100%, 510px), 1fr))', gap: 14 }}>{children}</Box>; }
return { ImpactEvidence };
})();

const { ImpactStageFlow } = (() => {
type Props = { runId: string; data: WorkflowImpact; onInspect: (target: 'stage' | 'capability' | 'history', name?: string) => void };
type Outcome = { id: string; name: string; step: string; failed: boolean; completed: boolean; durationMs: number | null };
type Capability = Outcome & { stageExecution: string | null };
const detail = (e: ImpactEvent): Record<string, unknown> => e.payload && typeof e.payload === 'object' && !Array.isArray(e.payload) ? e.payload as Record<string, unknown> : {};
const label = (value: string) => value.replace(/_/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
const stageName = (e: ImpactEvent) => {
  const p = detail(e);
  const named = p.stage_type || p.step_type;
  if (typeof named === 'string' && named.trim()) return named.toLowerCase();
  return (e.step_id || 'Unassigned').replace(/-[0-9a-f]{8}-[0-9a-f-]{27,}$/i, '').toLowerCase();
};
const capabilityName = (e: ImpactEvent) => {
  const p = detail(e);
  for (const key of ['capability', 'capability_type', 'capability_name']) if (typeof p[key] === 'string' && p[key]) return p[key] as string;
  if (/inference/i.test(e.kind)) return 'Inference';
  if (/changeset|apply/i.test(e.kind)) return 'Changeset';
  if (/compile/i.test(e.kind)) return 'Compile commands';
  if (/checkpoint|operator/i.test(e.kind)) return 'Operator checkpoint';
  return 'Other capability';
};
const isError = (e: ImpactEvent) => /fail|error|reject/i.test(e.kind) || /error|fatal/i.test(e.level);
const isDone = (e: ImpactEvent) => /complet|success|succeed/i.test(e.kind);
const percent = (success: number, total: number) => total ? `${(100 * success / total).toFixed(1)}%` : '—';
const recordedDuration = (e: ImpactEvent): number | null => {
  const value = detail(e).duration_ms;
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null;
};
const durationTotal = (items: Outcome[]) => {
  const recorded = items.filter(item => item.durationMs !== null);
  if (!recorded.length) return '—';
  const ms = recorded.reduce((sum, item) => sum + (item.durationMs || 0), 0);
  const value = ms < 1000 ? `${Math.round(ms)} ms` : ms < 60000 ? `${(ms / 1000).toFixed(1)} s` : ms < 3600000 ? `${Math.floor(ms / 60000)}m ${((ms % 60000) / 1000).toFixed(1)}s` : `${Math.floor(ms / 3600000)}h ${Math.floor(ms % 3600000 / 60000)}m`;
  return recorded.length < items.length ? `${value} recorded` : value;
};

type DiffSummary = { files: number; additions: number; deletions: number };

const summarizeDiff = (files: ReviewStatusFileEntry[]): DiffSummary => ({
  files: new Set(files.map(file => file.path)).size,
  additions: files.reduce((sum, file) => sum + file.additions, 0),
  deletions: files.reduce((sum, file) => sum + file.deletions, 0)
});

function DiffMetric({ label, value, color }: { label: string; value: string; color?: string }) {
  return <Paper withBorder radius="md" p="sm" style={{ minWidth: 0 }}>
    <Text size="xs" c="dimmed">{label}</Text>
    <Text fw={800} fz={24} c={color} style={{ overflowWrap: 'anywhere' }}>{value}</Text>
  </Paper>;
}

function DiffBreakdown({ title, summary }: { title: string; summary: DiffSummary }) {
  return <Paper withBorder radius="md" p="sm" style={{ minWidth: 0 }}>
    <Stack gap="sm">
      <Text fw={700} size="sm">{title}</Text>
      <SimpleGrid cols={3} spacing="xs">
        <Box><Text size="xs" c="dimmed">Files</Text><Text fw={700} fz="lg">{summary.files.toLocaleString()}</Text></Box>
        <Box><Text size="xs" c="dimmed">Added</Text><Text fw={700} fz="lg" c="teal">+{summary.additions.toLocaleString()}</Text></Box>
        <Box><Text size="xs" c="dimmed">Removed</Text><Text fw={700} fz="lg" c="red">−{summary.deletions.toLocaleString()}</Text></Box>
      </SimpleGrid>
    </Stack>
  </Paper>;
}

function ImpactRepositoryDiff({ summaries, reviewError }: { summaries: { staged: DiffSummary; unstaged: DiffSummary; composite: DiffSummary } | null; reviewError: boolean }) {
  return <Paper withBorder radius="md" p="lg" h="100%">
    <Stack gap="md">
      <Box>
        <Text fw={700} size="sm">Repository diff</Text>
        <Text size="xs" c="dimmed">Current repository changes</Text>
      </Box>
      {summaries ? <>
        <SimpleGrid cols={3} spacing="xs">
          <DiffMetric label="Files changed" value={summaries.composite.files.toLocaleString()} />
          <DiffMetric label="Lines added" value={`+${summaries.composite.additions.toLocaleString()}`} color="teal" />
          <DiffMetric label="Lines removed" value={`−${summaries.composite.deletions.toLocaleString()}`} color="red" />
        </SimpleGrid>
        <SimpleGrid cols={{ base: 1, sm: 2 }} spacing="sm">
          <DiffBreakdown title="Staged" summary={summaries.staged} />
          <DiffBreakdown title="Unstaged" summary={summaries.unstaged} />
        </SimpleGrid>
      </> : <Text size="xs" c="dimmed">{reviewError ? 'Repository diff unavailable' : 'Loading repository diff…'}</Text>}
    </Stack>
  </Paper>;
}

function ImpactExecutionSummary({ data, runtime, cost, onHistory }: { data: WorkflowImpact; runtime: string; cost: string; onHistory: () => void }) {
  return <Paper withBorder radius="md" p="lg" h="100%">
    <Stack gap="md">
      <Group justify="space-between" align="start" gap="xs">
        <Box>
          <Text fw={700} size="sm">Execution summary</Text>
          <Text size="xs" c="dimmed">Totals across all stages</Text>
        </Box>
        <Button size="xs" variant="subtle" onClick={onHistory}>View history →</Button>
      </Group>
      <SimpleGrid cols={3} spacing="xs">
        <DiffMetric label="Total executions" value={data.stage_executions.toLocaleString()} />
        <DiffMetric label="Completed" value={data.completed_stages.toLocaleString()} color="teal" />
        <DiffMetric label="Failed" value={data.failed_stages.toLocaleString()} color="red" />
        <DiffMetric label="Total cost" value={cost} />
        <DiffMetric label="Total runtime" value={runtime} />
      </SimpleGrid>
    </Stack>
  </Paper>;
}

function ImpactStageFlow({ runId, data, onInspect }: Props) {
  const [events, setEvents] = useState<ImpactEvent[]>([]);
  const [reviewStatus, setReviewStatus] = useState<ReviewStatusResponse | null>(null);
  const [reviewError, setReviewError] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setReviewStatus(null);
    setReviewError(false);
    getWorkflowReviewStatus(runId).then(status => {
      if (!cancelled) setReviewStatus(status);
    }).catch(() => {
      if (!cancelled) setReviewError(true);
    });
    return () => { cancelled = true; };
  }, [runId, data.stage_executions, data.capability_activations]);
  const diffSummaries = useMemo(() => {
    if (!reviewStatus) return null;
    const staged = summarizeDiff(reviewStatus.staged);
    const unstaged = summarizeDiff(reviewStatus.unstaged);
    const composite: DiffSummary = {
      files: new Set([...reviewStatus.staged, ...reviewStatus.unstaged].map(file => file.path)).size,
      additions: staged.additions + unstaged.additions,
      deletions: staged.deletions + unstaged.deletions
    };
    return { staged, unstaged, composite };
  }, [reviewStatus]);
  const [inference, setInference] = useState<ImpactInvocation[] | null>(null);
  useEffect(() => {
    let cancelled = false;
    setInference(null);
    getImpactDetail(runId).then(result => {
      if (!cancelled) setInference(result.inference);
    }).catch(() => {
      if (!cancelled) setInference([]);
    });
    return () => { cancelled = true; };
  }, [runId, data.inference_outputs]);
  const [pending, setPending] = useState(true);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    setPending(true);
    setError(null);
    setEvents([]);
    const load = async () => {
      const all: ImpactEvent[] = [];
      let cursor: number | undefined;
      const seen = new Set<number>();
      while (!cancelled) {
        const page = await getImpactEvents(runId, { after: cursor, limit: 250 });
        all.push(...page.events);
        if (!page.has_more) break;
        if (page.next_cursor == null || seen.has(page.next_cursor)) throw new Error('History cursor did not advance');
        cursor = page.next_cursor;
        seen.add(cursor);
      }
      if (!cancelled) setEvents(all);
    };
    load().catch(e => { if (!cancelled) setError(String(e)); }).finally(() => { if (!cancelled) setPending(false); });
    return () => { cancelled = true; };
  }, [runId, data.stage_executions, data.capability_activations]);
  const stages = useMemo(() => {
    const byExecution = new Map<string, Outcome>();
    const byInvocation = new Map<string, Capability>();
    for (const e of events) {
      const step = e.step_id || '';
      if (e.stage_execution_id) {
        const id = e.stage_execution_id;
        const item = byExecution.get(id) || { id, name: stageName(e), step, failed: false, completed: false, durationMs: null };
        item.failed ||= isError(e) || detail(e).ok === false;
        item.completed ||= isDone(e);
        if ((e.kind === 'stage_execution_completed' || e.kind === 'stage_execution_failed') && recordedDuration(e) !== null) item.durationMs = recordedDuration(e);
        byExecution.set(id, item);
      }
      if (e.capability_invocation_id) {
        const id = e.capability_invocation_id;
        const item = byInvocation.get(id) || { id, name: capabilityName(e), step, stageExecution: e.stage_execution_id, failed: false, completed: false, durationMs: null };
        if (!item.stageExecution && e.stage_execution_id) item.stageExecution = e.stage_execution_id;
        item.failed ||= isError(e) || detail(e).ok === false;
        item.completed ||= isDone(e);
        if ((/_completed$|_failed$/.test(e.kind)) && recordedDuration(e) !== null) item.durationMs = recordedDuration(e);
        byInvocation.set(id, item);
      }
    }
    const names = new Map<string, { name: string; executions: Outcome[]; capabilities: Capability[] }>();
    for (const execution of byExecution.values()) {
      if (!names.has(execution.name)) names.set(execution.name, { name: execution.name, executions: [], capabilities: [] });
      names.get(execution.name)!.executions.push(execution);
    }
    for (const capability of byInvocation.values()) {
      const execution = capability.stageExecution ? byExecution.get(capability.stageExecution) : undefined;
      const related = execution || [...byExecution.values()].find(s => s.step && s.step === capability.step);
      if (!related) continue;
      names.get(related.name)?.capabilities.push(capability);
    }
    return [...names.values()];
  }, [events]);
  const runtime = durationTotal(stages.flatMap(stage => stage.executions));
  const cost = useMemo(() => {
    if (inference === null) return 'Loading…';
    if (!inference.length) return '—';
    const priced = inference.filter(item => item.cost !== null && Number.isFinite(item.cost));
    if (!priced.length) return 'Unavailable';
    const currencies = [...new Set(priced.map(item => item.currency || 'unknown'))];
    if (currencies.length !== 1) return 'Mixed currencies';
    const total = priced.reduce((sum, item) => sum + (item.cost || 0), 0);
    const amount = total.toFixed(4);
    const currency = currencies[0];
    const formatted = currency === 'USD' ? `$${amount}` : `${currency} ${amount}`;
    return priced.length < inference.length ? `${formatted} recorded` : formatted;
  }, [inference]);
  return <Stack gap="md">
    <SimpleGrid cols={{ base: 1, lg: 2 }} spacing="md" verticalSpacing="md">
      <ImpactRepositoryDiff summaries={diffSummaries} reviewError={reviewError} />
      <ImpactExecutionSummary data={data} runtime={runtime} cost={cost} onHistory={() => onInspect('history')} />
    </SimpleGrid>
    <Paper withBorder radius="md" p="lg"><Stack gap="md">
    <Box>
      <Text fw={700} size="sm">Stage execution flow</Text>
      <Text size="xs" c="dimmed">Execution outcomes and capabilities by stage</Text>
    </Box>
    <Divider />
    <Text fw={700} size="sm">Stage breakdown</Text>
    {error && <Text c="red" size="xs">Unable to load stage details: {error}</Text>}
    {pending && <Text size="xs" c="dimmed">Loading stage and capability breakdown…</Text>}
    <Box style={{ overflowX: 'auto' }}><Group align="flex-start" wrap="nowrap" gap="sm">{stages.map(stage => {
      const completed = stage.executions.filter(s => !s.failed && s.completed).length;
      const failed = stage.executions.filter(s => s.failed).length;
      const rate = stage.executions.length ? completed / stage.executions.length * 100 : 0;
      const groups = new Map<string, Capability[]>();
      for (const capability of stage.capabilities) {
        if (!groups.has(capability.name)) groups.set(capability.name, []);
        groups.get(capability.name)!.push(capability);
      }
      return <Paper key={stage.name} withBorder radius="md" p="sm" style={{ flex: '1 0 210px', minWidth: 0 }}><Stack gap="xs">
        <Box>
          <Group justify="space-between" gap="xs"><Text size="xs" fw={700}>{label(stage.name)}</Text><Text size="xs" c="dimmed">{stage.executions.length} attempts</Text></Group>
          <Group justify="space-between" gap="xs"><Text size="xs" c="dimmed">Total stage time</Text><Text size="xs" fw={600} title="Sum of recorded stage execution durations across attempts">{durationTotal(stage.executions)}</Text></Group>
          <Text fw={700} fz="xl" c="teal" mt="xs">{percent(completed, stage.executions.length)} <Text component="span" size="xs" c="dimmed">success</Text></Text>
          <Progress.Root size={7} radius="sm" mt="xs">{completed > 0 && <Progress.Section value={rate} color="teal" />}{failed > 0 && <Progress.Section value={failed / stage.executions.length * 100} color="red" />}</Progress.Root>
          <Group justify="space-between" mt="xs"><Text size="xs" c="teal">{completed} successful</Text><Text size="xs" c={failed ? 'red' : 'dimmed'}>{failed} failed</Text></Group>
          <Text size="xs" c="dimmed" mt={5}>{stage.capabilities.length} capability invocations · {groups.size} types</Text>
        </Box>
        <ScrollArea.Autosize mah={210} type="auto" offsetScrollbars><Stack gap="xs" pt="xs" style={{ borderTop: '1px solid var(--mantine-color-default-border)' }}>
          {[...groups.entries()].sort((a, b) => b[1].length - a[1].length).map(([name, calls]) => {
            const success = calls.filter(c => !c.failed && c.completed).length;
            const errors = calls.filter(c => c.failed).length;
            return <Box key={name} component="button" type="button" onClick={() => onInspect('capability', name)} style={{ border: 0, background: 'transparent', color: 'inherit', cursor: 'pointer', textAlign: 'left' }}>
              <Group justify="space-between" gap="xs"><Text size="xs" fw={600}>{label(name)}</Text><Text size="xs" c="dimmed">{calls.length} runs · {percent(success, calls.length)}</Text></Group>
              <Group justify="space-between" gap="xs"><Text size="xs" c="dimmed">{success} succeeded{errors ? ` · ${errors} failed` : ''}</Text><Text size="xs" fw={600} title="Sum of recorded capability invocation durations across runs">{durationTotal(calls)}</Text></Group>
              <Progress.Root size={3} radius="xl" mt={4} color="gray.7">{success > 0 && <Progress.Section value={success / calls.length * 100} color="teal.8" />}{errors > 0 && <Progress.Section value={errors / calls.length * 100} color="red.8" />}</Progress.Root>
            </Box>;
          })}
          {groups.size === 0 && <Text size="xs" c="dimmed">No linked capability invocations.</Text>}
        </Stack></ScrollArea.Autosize>
      </Stack></Paper>;
    })}</Group></Box>
    {!pending && !error && !stages.length && <Text size="sm" c="dimmed">No recorded stage executions.</Text>}
    </Stack></Paper>
  </Stack>;
}
return { ImpactStageFlow };
})();

const { ImpactDashboard } = (() => {
type Target = 'stage' | 'capability' | 'inference' | 'code' | 'history';
type Props = { runId: string; data: WorkflowImpact; events: ImpactEvent[]; hasMore: boolean; onInspect: (target: Target, name?: string) => void };
type Aggregate = { name: string; count: number; failed: number; completed: number };
const titleCase = (text: string) => text.replace(/_/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
const payload = (event: ImpactEvent): Record<string, unknown> => event.payload && typeof event.payload === 'object' && !Array.isArray(event.payload) ? event.payload as Record<string, unknown> : {};
const stageType = (event: ImpactEvent) => { const value = payload(event).stage_type || payload(event).step_type; if (typeof value === 'string' && value) return value.toLowerCase(); const step = event.step_id || 'Unassigned'; return step.replace(/-[0-9a-f]{8}-[0-9a-f-]{27,}$/i, '').toLowerCase(); };
const capabilityType = (event: ImpactEvent) => { const detail = payload(event); for (const key of ['capability', 'capability_type', 'capability_name']) if (typeof detail[key] === 'string' && detail[key]) return detail[key] as string; if (/inference/i.test(event.kind)) return 'Inference'; if (/changeset|apply/i.test(event.kind)) return 'Changesets'; if (/compile/i.test(event.kind)) return 'Compile'; return 'Other capability'; };
const grouped = (events: ImpactEvent[], type: 'stage' | 'capability'): Aggregate[] => { const items = new Map<string, { name: string; failed: boolean; completed: boolean }>(); for (const event of events) { const id = type === 'stage' ? event.stage_execution_id : event.capability_invocation_id; if (!id) continue; const name = type === 'stage' ? stageType(event) : capabilityType(event); const item = items.get(id) || { name, failed: false, completed: false }; item.failed ||= /fail|error|reject/i.test(event.kind) || /error|fatal/i.test(event.level); item.completed ||= /complet|success|succeed/i.test(event.kind); items.set(id, item); } const totals = new Map<string, Aggregate>(); for (const item of items.values()) { const value = totals.get(item.name) || { name: item.name, count: 0, failed: 0, completed: 0 }; value.count++; if (item.failed) value.failed++; else if (item.completed) value.completed++; totals.set(item.name, value); } return [...totals.values()]; };
function Stat({ label, value, color }: { label: string; value: string | number; color?: string }) { return <Box><Text size="xs" c="dimmed">{label}</Text><Text fw={700} fz="lg" c={color}>{typeof value === 'number' ? value.toLocaleString() : value}</Text></Box>; }
function Header({ title, subtitle, onClick }: { title: string; subtitle: string; onClick: () => void }) { return <Group justify="space-between" align="start"><Box><Text fw={700} size="sm">{title}</Text><Text size="xs" c="dimmed">{subtitle}</Text></Box><Button size="xs" variant="subtle" onClick={onClick}>Details →</Button></Group>; }
function ImpactDashboard({ runId, data, events, hasMore, onInspect }: Props) {
  const [selectedSession, setSelectedSession] = useState('all');
  useEffect(() => { setSelectedSession('all'); }, [runId]);
  const stages = grouped(events, 'stage');
  const capabilities = grouped(events, 'capability').sort((a, b) => b.count - a.count);
  const measuredStages = stages.reduce((sum, item) => sum + item.count, 0);
  const measuredCapabilities = capabilities.reduce((sum, item) => sum + item.count, 0);
  return <Stack gap="md">
    <ImpactStageFlow runId={runId} data={data} onInspect={onInspect} />
    <ImpactEvidence runId={runId} data={data} onInspect={onInspect} />

  </Stack>;
}
return { ImpactDashboard };
})();



type Granularity = 'stage' | 'capability' | 'event';
type TimeRange = 'all' | '5m' | '30m' | '1h' | '24h' | 'custom';
type ReplayItem = { id: string; title: string; subtitle: string; time: string; status: string; event: ImpactEvent; count: number };

const formatNumber = (value: number) => value.toLocaleString();
const formatTime = (value: string) => { const date = new Date(value); return Number.isNaN(date.getTime()) ? value : date.toLocaleString(); };
const displayName = (value: string) => value.replace(/_/g, ' ').replace(/\b\w/g, letter => letter.toUpperCase());
const record = (value: unknown): Record<string, unknown> => value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
const textValue = (value: unknown) => typeof value === 'string' ? value : '';
const stageName = (event: ImpactEvent) => {
  const payload = record(event.payload);
  const explicit = textValue(payload.stage_type) || textValue(payload.step_type);
  if (explicit) return explicit.toLowerCase();
  const step = event.step_id || '';
  const match = step.match(/^(.+)-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i);
  return (match ? match[1] : step || 'Unassigned stage').toLowerCase();
};
const capabilityName = (event: ImpactEvent) => {
  const payload = record(event.payload);
  const named = textValue(payload.capability) || textValue(payload.capability_type) || textValue(payload.capability_name);
  if (named) return named;
  const kind = event.kind.toLowerCase();
  if (kind.includes('inference')) return 'Inference';
  if (kind.includes('changeset') || kind.includes('apply')) return 'Changesets';
  if (kind.includes('compile')) return 'Compile';
  if (kind.includes('checkpoint') || kind.includes('operator')) return 'Operator checkpoints';
  return event.capability_invocation_id ? 'Other capability' : 'Workflow';
};
const isFailure = (event: ImpactEvent) => /fail|error|reject/i.test(event.kind) || /error|fatal/i.test(event.level);
const statusColor = (status: string) => status === 'failed' ? 'red' : status === 'completed' ? 'teal' : 'blue';
const inputCategory = (origin: string, source: string) => {
  if (origin === 'user_custom') return 'user';
  if (/error|retry|recovery|compile/i.test(`${origin} ${source}`)) return 'recovery';
  return 'generated';
};

function summarize(events: ImpactEvent[], granularity: Granularity): ReplayItem[] {
  if (granularity === 'event') return events.map(event => ({ id: event.id, title: displayName(event.kind), subtitle: event.message, time: event.created_at, status: isFailure(event) ? 'failed' : 'completed', event, count: 1 })).sort((a, b) => b.event.sequence_no - a.event.sequence_no);
  const groups = new Map<string, ReplayItem>();
  for (const event of events) {
    const key = granularity === 'stage' ? event.stage_execution_id : event.capability_invocation_id;
    if (!key) continue;
    const current = groups.get(key);
    if (!current) {
      groups.set(key, { id: key, title: granularity === 'stage' ? stageName(event) : capabilityName(event), subtitle: event.message, time: event.created_at, status: 'running', event, count: 0 });
    }
    const item = groups.get(key)!;
    item.count++;
    if (granularity === 'stage' && /^stage_execution_completed$/.test(event.kind)) item.status = 'completed';
    if (granularity === 'stage' && /^stage_execution_failed$/.test(event.kind)) item.status = 'failed';
    if (granularity === 'capability' && /completed|succeeded|success/i.test(event.kind)) item.status = 'completed';
    if (isFailure(event)) item.status = 'failed';
  }
  return [...groups.values()].sort((a, b) => b.event.sequence_no - a.event.sequence_no);
}

function Metric({ label, value, hint }: { label: string; value: number | string; hint?: string }) {
  return <Box style={{ minWidth: 0 }}><Text size="xs" c="dimmed">{label}</Text><Text fw={650} fz="xl" style={{ fontVariantNumeric: 'tabular-nums' }}>{typeof value === 'number' ? formatNumber(value) : value}</Text>{hint && <Text size="xs" c="dimmed">{hint}</Text>}</Box>;
}

const internalField = /(^|_)(id|uuid|hash|trace|span|metadata|raw|payload|stack|headers)(_|$)/i;
const sensitiveField = /secret|password|credential|authorization|api[_-]?key|access[_-]?token|refresh[_-]?token|private[_-]?key/i;
const friendlyLabels: Record<string, string> = {
  stage_type: 'Stage', step_type: 'Step', capability_type: 'Capability',
  capability_name: 'Capability', file_path: 'File', path: 'Path',
  error_message: 'Error', duration_ms: 'Duration (ms)',
  input_tokens: 'Input tokens', output_tokens: 'Output tokens',
};

function readableValue(value: unknown, depth = 0): string {
  if (value == null || value === '') return 'Not provided';
  if (typeof value === 'boolean') return value ? 'Yes' : 'No';
  if (typeof value === 'number') return formatNumber(value);
  if (typeof value === 'string') return value;
  if (Array.isArray(value)) {
    if (!value.length) return 'None';
    if (depth >= 2 || value.length > 8) return `${formatNumber(value.length)} items`;
    return value.map(item => readableValue(item, depth + 1)).join(', ');
  }
  const entries = Object.entries(record(value)).filter(([key]) => !internalField.test(key) && !sensitiveField.test(key));
  if (!entries.length) return 'Additional technical details recorded';
  if (depth >= 2) return `${entries.length} details`;
  return entries.map(([key, item]) => `${friendlyLabels[key] || displayName(key)}: ${readableValue(item, depth + 1)}`).join(' · ');
}

function Payload({ value, raw = false }: { value: unknown; raw?: boolean }) {
  const entries = Object.entries(record(value)).filter(([key]) => !internalField.test(key) && !sensitiveField.test(key));
  const serialized = typeof value === 'string' ? value : JSON.stringify(value, null, 2);

  if (raw) {
    return <ScrollArea.Autosize mah={480} type="auto"><Code block style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', fontSize: 12 }}>{serialized || 'Not recorded'}</Code></ScrollArea.Autosize>;
  }

  if (!entries.length) {
    return <Text size="sm" c="dimmed" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{typeof value === 'string' ? (value || 'Not recorded') : 'No additional user-facing details recorded.'}</Text>;
  }

  return <Stack gap="xs">
    {entries.map(([key, item]) => <Box key={key}>
      <Text size="xs" c="dimmed">{friendlyLabels[key] || displayName(key)}</Text>
      <Text size="sm" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{readableValue(item)}</Text>
    </Box>)}
  </Stack>;
}

function InferenceInspector({ runId, event }: { runId: string; event: ImpactEvent }) {
  const [inference, setInference] = useState<ImpactInferenceRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [origin, setOrigin] = useState('all');
  const [view, setView] = useState<'readable' | 'raw'>('readable');
  const [chosenId, setChosenId] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    setInference([]);
    setChosenId(null);
    void getImpactInference(runId, event.capability_invocation_id ? { capability_invocation_id: event.capability_invocation_id } : { step_id: event.step_id || undefined })
      .then(result => { if (active) setInference(result.invocations); })
      .catch(err => { if (active) setError(err instanceof Error ? err.message : String(err)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [runId, event.capability_invocation_id, event.step_id]);
  const selected = inference.find(item => item.id === chosenId) || inference[0];
  const inputs = selected?.inputs.filter(input => origin === 'all' || inputCategory(input.input_origin, input.input_source) === origin) || [];
  return <Stack gap="sm">
    <Group justify="space-between" align="center"><Text fw={600} size="sm">Inference details</Text>{inference.length > 1 && <Badge size="xs" variant="light">{inference.length} invocations</Badge>}</Group>
    {loading && <Loader size="xs" />}
    {error && <Alert color="red">{error}</Alert>}
    {!loading && !selected && <Text size="sm" c="dimmed">No inference invocation linked to this selection.</Text>}
    {selected && <>
      {inference.length > 1 && <Select size="xs" data={inference.map(item => ({ value: item.id, label: `${formatTime(item.created_at)} · ${item.model} · ${item.status}` }))} value={selected.id} onChange={setChosenId} allowDeselect={false} />}
      <Group gap="xs" align="center"><Badge size="sm" variant="light">{selected.provider}</Badge>{selected.model && <Text size="xs" c="dimmed">{selected.model}</Text>}<Text size="xs" c="dimmed">{formatTime(selected.created_at)}</Text><InferenceTokenUsage accounting={selected.accounting} inputTokens={selected.input_tokens} outputTokens={selected.output_tokens} /></Group>
      {selected.error_message && <Alert color="red">{selected.error_message}</Alert>}
      <SegmentedControl size="xs" fullWidth value={view} onChange={value => setView(value as 'readable' | 'raw')} data={[{ label: 'Readable', value: 'readable' }, { label: 'Raw payload', value: 'raw' }]} />
      <Select size="xs" value={origin} onChange={value => setOrigin(value || 'all')} allowDeselect={false} data={[{ value: 'all', label: 'All recorded inputs' }, { value: 'user', label: 'User instructions' }, { value: 'generated', label: 'Generated and inherited inputs' }, { value: 'recovery', label: 'Errors and recovery inputs' }]} />
      <Text size="xs" c="dimmed">Inputs · {inputs.length} of {selected.inputs.length}</Text>
      {inputs.map(input => <Paper key={`${selected.id}-${input.sequence_no}`} withBorder p="sm"><Group justify="space-between" mb="xs"><Text size="sm" fw={600}>{input.label || input.input_type}</Text><Badge size="xs" variant="outline">{input.input_origin}</Badge></Group><Text size="xs" c="dimmed" mb="xs">{input.input_source}</Text><Payload value={input.content} raw={view === 'raw'} /></Paper>)}
      {!inputs.length && <Text size="sm" c="dimmed">No recorded inputs match this filter.</Text>}
      <Divider label="Inference response" labelPosition="left" />
      {view === 'raw' ? <Payload value={{ response: selected.response_payload, metadata: selected.provider_metadata }} raw /> : <Payload value={selected.response_text} />}
    </>}
  </Stack>;
}

export function ImpactPanel({ runId }: { runId: string | null }) {
  const [data, setData] = useState<WorkflowImpact | null>(null);
  const [events, setEvents] = useState<ImpactEvent[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<'analytics' | 'replay'>('analytics');
  const [granularity, setGranularity] = useState<Granularity>('stage');
  const [range, setRange] = useState<TimeRange>('all');
  const [from, setFrom] = useState('');
  const [to, setTo] = useState('');
  const [stage, setStage] = useState('all');
  const [capability, setCapability] = useState('all');
  const [search, setSearch] = useState('');
  const [errorsOnly, setErrorsOnly] = useState(false);
  const [live, setLive] = useState(true);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [cursor, setCursor] = useState<number | null>(null);
  const [newEvents, setNewEvents] = useState(0);
  const generation = useRef(0);
  const eventsRef = useRef<ImpactEvent[]>([]);
  const cursorRef = useRef<number | null>(null);

  const timeFilters = useMemo(() => {
    if (range === 'custom') {
      const start = from ? new Date(from) : null;
      const end = to ? new Date(to) : null;
      return {
        from: start && !Number.isNaN(start.getTime()) ? start.toISOString() : undefined,
        to: end && !Number.isNaN(end.getTime()) ? end.toISOString() : undefined,
      };
    }
    if (range === 'all') return {};
    const minutes = range === '5m' ? 5 : range === '30m' ? 30 : range === '1h' ? 60 : 1440;
    return { from: new Date(Date.now() - minutes * 60000).toISOString() };
  }, [range, from, to]);

  const load = useCallback(async (append = false) => {
    if (!runId) return;
    const request = ++generation.current;
    setLoading(true);
    setError(null);
    try {
      const filters = { ...timeFilters, limit: 250, ...(append && cursorRef.current !== null ? { after: cursorRef.current } : {}) };
      const [history, summary] = await Promise.all([getImpactEvents(runId, filters), append ? Promise.resolve(null) : getWorkflowImpact(runId)]);
      if (request !== generation.current) return;
      const next = append ? [...eventsRef.current, ...history.events] : history.events;
      const unique = [...new Map(next.map(event => [event.id, event])).values()].sort((a, b) => a.sequence_no - b.sequence_no);
      eventsRef.current = unique;
      cursorRef.current = history.next_cursor ?? (append ? cursorRef.current : null);
      setEvents(unique);
      setCursor(cursorRef.current);
      setHasMore(history.has_more);
      if (summary) setData(summary);
    } catch (err) {
      if (request === generation.current) setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (request === generation.current) setLoading(false);
    }
  }, [runId, timeFilters]);

  useEffect(() => {
    eventsRef.current = [];
    cursorRef.current = null;
    setEvents([]);
    setData(null);
    setSelectedId(null);
    setNewEvents(0);
    void load();
    return () => { generation.current++; };
  }, [load]);

  useEffect(() => {
    if (!runId || !live) return;
    let active = true;
    let busy = false;
    const poll = async () => {
      if (busy || !active) return;
      busy = true;
      try {
        const history = await getImpactEvents(runId, { ...timeFilters, after: cursorRef.current ?? undefined, limit: 250 });
        if (!active) return;
        if (history.events.length) {
          const previous = new Set(eventsRef.current.map(event => event.id));
          const added = history.events.filter(event => !previous.has(event.id));
          const merged = [...eventsRef.current, ...added].sort((a, b) => a.sequence_no - b.sequence_no);
          eventsRef.current = merged;
          cursorRef.current = history.next_cursor ?? cursorRef.current;
          setEvents(merged);
          setCursor(cursorRef.current);
          setHasMore(history.has_more);
          if (selectedId && added.length > 0) setNewEvents(count => count + added.length);
        }
        setData(await getWorkflowImpact(runId));
      } catch (err) {
        if (active) setError(err instanceof Error ? err.message : String(err));
      } finally { busy = false; }
    };
    const interval = window.setInterval(() => { void poll(); }, 3000);
    return () => { active = false; window.clearInterval(interval); };
  }, [runId, live, timeFilters, selectedId]);

  const stageOptions = useMemo(() => [...new Set(events.filter(event => event.step_id || event.stage_execution_id || record(event.payload).stage_type).map(stageName))].sort().map(value => ({ value, label: displayName(value) })), [events]);
  const capabilityOptions = useMemo(() => [...new Set(events.filter(event => event.capability_invocation_id).map(capabilityName))].sort().map(value => ({ value, label: displayName(value) })), [events]);
  const filtered = useMemo(() => events.filter(event => {
    if (stage !== 'all' && stageName(event) !== stage) return false;
    if (capability !== 'all' && capabilityName(event) !== capability) return false;
    if (errorsOnly && !isFailure(event)) return false;
    if (search && !`${event.kind} ${event.message} ${JSON.stringify(event.payload)}`.toLowerCase().includes(search.toLowerCase())) return false;
    return true;
  }), [events, stage, capability, errorsOnly, search]);
  const items = useMemo(() => summarize(filtered, granularity), [filtered, granularity]);
  const selected = items.find(item => item.id === selectedId) || items[0];
  const selectedEvents = selected ? filtered.filter(event => granularity === 'event' ? event.id === selected.id : granularity === 'stage' ? event.stage_execution_id === selected.id : event.capability_invocation_id === selected.id) : [];
  const latest = events[events.length - 1];
  const failures = filtered.filter(isFailure).length;
  const matchingCount = items.length;

  if (!runId) return <Alert color="blue">Select a workflow to inspect its impact and execution history.</Alert>;

  return <Stack gap="lg">
    <Group justify="flex-end" align="center"><Group gap="xs"><Badge variant="light" color={live ? 'teal' : 'gray'}>{live ? 'Live' : 'Paused'}</Badge><Button variant="subtle" size="xs" onClick={() => setLive(value => !value)}>{live ? 'Pause updates' : 'Resume updates'}</Button><Button variant="light" size="xs" loading={loading} onClick={() => void load()}>Refresh</Button></Group></Group>
    {error && <Alert color="red" withCloseButton onClose={() => setError(null)}>{error}</Alert>}
    {tab === 'analytics' && data && <ImpactDashboard runId={runId} data={data} events={events} hasMore={hasMore} onInspect={(target, name) => { setSelectedId(null); setStage('all'); setCapability('all'); setSearch(''); setErrorsOnly(false); setRange('all'); if (target === 'stage') { setGranularity('stage'); if (name) setStage(name); } else if (target === 'capability') { setGranularity('capability'); if (name) setCapability(name); } else if (target === 'inference') { setGranularity('event'); setSearch('inference'); } else if (target === 'code') { setGranularity('event'); setSearch('changeset'); } else { setGranularity('stage'); } setTab('replay'); }} />}
    {tab === 'replay' && <Group justify="space-between"><Text fw={700}>Execution inspector</Text><Button size="xs" variant="light" onClick={() => setTab('analytics')}>← Back to dashboard</Button></Group>}
    {tab === 'replay' && <Stack gap="sm">
      <Paper withBorder p="md" radius="md"><Stack gap="sm"><Group justify="space-between"><Text fw={600} size="sm">Replay controls</Text><Text size="xs" c="dimmed">{formatNumber(matchingCount)} matching executions · {failures} error events</Text></Group><SegmentedControl fullWidth value={granularity} onChange={value => { setGranularity(value as Granularity); setSelectedId(null); }} data={[{ label: 'Stages', value: 'stage' }, { label: 'Capabilities', value: 'capability' }, { label: 'Events / I-O', value: 'event' }]} /><SimpleGrid cols={{ base: 1, sm: 3 }}><Select size="xs" label="Time range" value={range} onChange={value => setRange(value as TimeRange || 'all')} allowDeselect={false} data={[{ value: 'all', label: 'Entire workflow' }, { value: '5m', label: 'Last 5 minutes' }, { value: '30m', label: 'Last 30 minutes' }, { value: '1h', label: 'Last hour' }, { value: '24h', label: 'Last 24 hours' }, { value: 'custom', label: 'Custom range' }]} /><Select size="xs" label="Stage" value={stage} onChange={value => { setStage(value || 'all'); setSelectedId(null); }} data={[{ value: 'all', label: 'All stages' }, ...stageOptions]} /><Select size="xs" label="Capability" value={capability} onChange={value => { setCapability(value || 'all'); setSelectedId(null); }} data={[{ value: 'all', label: 'All capabilities' }, ...capabilityOptions]} /></SimpleGrid>{range === 'custom' && <SimpleGrid cols={2}><TextInput label="From" type="datetime-local" size="xs" value={from} onChange={event => setFrom(event.currentTarget.value)} /><TextInput label="To" type="datetime-local" size="xs" value={to} onChange={event => setTo(event.currentTarget.value)} /></SimpleGrid>}<Group align="end"><TextInput size="xs" style={{ flex: 1 }} placeholder="Search events and payloads" value={search} onChange={event => setSearch(event.currentTarget.value)} /><Checkbox size="xs" label="Errors only" checked={errorsOnly} onChange={event => setErrorsOnly(event.currentTarget.checked)} /></Group></Stack></Paper>
      {newEvents > 0 && <Button variant="light" size="xs" onClick={() => { setSelectedId(latest?.id || null); setGranularity('event'); setNewEvents(0); }}>{newEvents} new events · Jump to latest</Button>}
      <Grid gutter="sm" align="start">
        <Grid.Col span={{ base: 12, md: 4 }}>
        <Paper withBorder radius="md" p="sm"><Group justify="space-between" mb="sm"><Text fw={600} size="sm">Execution history</Text><Badge variant="light">{items.length} entries</Badge></Group><ScrollArea.Autosize mah={720} type="auto"><Stack gap={4}>{items.map(item => <Box key={item.id} component="button" type="button" onClick={() => { setSelectedId(item.id); setNewEvents(0); }} style={{ width: '100%', textAlign: 'left', background: selected?.id === item.id ? 'var(--mantine-color-default-hover)' : 'transparent', border: 'none', borderRadius: 6, padding: '7px 9px', cursor: 'pointer', color: 'inherit' }}><Group justify="space-between" gap="xs" wrap="nowrap"><Text size="sm" fw={600} lineClamp={1}>{item.title}</Text><Badge size="xs" color={statusColor(item.status)} variant="light">{item.status}</Badge></Group><Text size="xs" c="dimmed" lineClamp={2}>{item.subtitle}</Text><Group gap="xs" mt={4}><Text size="xs" c="dimmed">{formatTime(item.time)}</Text>{item.count > 1 && <Text size="xs" c="dimmed">{item.count} events</Text>}</Group></Box>)}{!items.length && <Text size="sm" c="dimmed" p="md">No matching executions in the loaded history.</Text>}</Stack></ScrollArea.Autosize>{hasMore && <Button fullWidth size="xs" variant="subtle" mt="sm" loading={loading} onClick={() => void load(true)}>Load more events</Button>}</Paper>
        </Grid.Col>
        <Grid.Col span={{ base: 12, md: 8 }}>
        <Paper withBorder radius="md" p="sm"><ScrollArea.Autosize mah={720} type="auto">{selected ? <Stack gap="sm"><Group justify="space-between" align="center" gap="xs"><Box style={{ minWidth: 0 }}><Text size="sm" fw={600}>{displayName(selected.title)}</Text><Text size="xs" c="dimmed">{formatTime(selected.time)}</Text></Box><Group gap={6}><Badge size="xs" variant="light" color={statusColor(selected.status)}>{selected.status}</Badge><Badge size="xs" variant="outline">{selectedEvents.length} events</Badge></Group></Group>{!selectedEvents.some(event => /inference/i.test(event.kind)) && <Text size="xs" c="dimmed">{selected.subtitle}</Text>}<Group gap="xs"><Button size="xs" variant="light" disabled={items.indexOf(selected) === 0} onClick={() => setSelectedId(items[items.indexOf(selected) - 1].id)}>Previous</Button><Button size="xs" variant="light" disabled={items.indexOf(selected) === items.length - 1} onClick={() => setSelectedId(items[items.indexOf(selected) + 1].id)}>Next</Button>{granularity === 'stage' && <Button size="xs" variant="subtle" onClick={() => { setStage(stageName(selected.event)); setGranularity('capability'); setSelectedId(null); }}>Explore capabilities</Button>}{granularity === 'capability' && <Button size="xs" variant="subtle" onClick={() => { setGranularity('event'); setSelectedId(null); }}>Explore events</Button>}</Group>{selectedEvents.some(event => /inference/i.test(event.kind)) ? (
  <InferenceInspector runId={runId} event={selected.event} />
) : (
  <>
    <Divider label="Recorded events" labelPosition="left" />
    {selectedEvents.map(event => <Box key={event.id}>
      <Group justify="space-between" align="start">
        <Text size="sm" fw={600}>{displayName(event.kind)}</Text>
        <Text size="xs" c="dimmed">#{event.sequence_no}</Text>
      </Group>
      <Text size="xs" c="dimmed">{formatTime(event.created_at)}</Text>
      <Text size="sm" my="xs">{event.message}</Text>
      <Payload value={event.payload} />
      <Divider my="sm" />
    </Box>)}
  </>
)}</Stack> : <Text c="dimmed" size="sm">Select an execution to inspect its history.</Text>}</ScrollArea.Autosize></Paper>
        </Grid.Col>
      </Grid>
      <Group justify="space-between"><Text size="xs" c="dimmed">Sequence cursor: {cursor ?? '—'} · {hasMore ? 'More history available' : 'End of loaded history'}</Text><Text size="xs" c="dimmed">Recorded events are read-only</Text></Group>
    </Stack>}
    {loading && !data && <Loader size="sm" />}
  </Stack>;
}
