import { useEffect, useMemo, useRef, useState } from 'react';
import {
  Alert,
  Badge,
  Box,
  Button,
  Card,
  CopyButton,
  FileButton,
  Group,
  Modal,
  ScrollArea,
  Select,
  SimpleGrid,
  Stack,
  Switch,
  Text,
  Textarea,
  TextInput,
  Title,
} from '@mantine/core';
import {
  confirmRepoSyncPairing,
  getRepoSyncPeerMessages,
  getRepoSyncStatus,
  previewRepoSyncManual,
  reconnectRepoSyncMapping,
  sendRepoSyncManual,
  sendRepoSyncPeerMessage,
  startRepoSyncPairing,
  unpairRepoSyncMapping,
  upsertRepoSyncMapping,
  type RepoSyncDirection,
  type RepoSyncMapping,
  type RepoSyncPairingSession,
  type RepoSyncPeerMessage,
  type RepoSyncPeerMessageBlock,
  type RepoSyncStatus,
} from '../api';

type ComposerMode = 'text' | 'code';

function fileAsBase64(file: File) {
  return new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error(`Failed to read ${file.name}`));
    reader.onload = () => {
      const value = typeof reader.result === 'string' ? reader.result : '';
      const comma = value.indexOf(',');
      resolve(comma >= 0 ? value.slice(comma + 1) : value);
    };
    reader.readAsDataURL(file);
  });
}

function remainingLabel(expiresAt: number) {
  const remaining = Math.max(0, expiresAt - Date.now());
  const minutes = Math.floor(remaining / 60000);
  const seconds = Math.floor((remaining % 60000) / 1000);
  return `${minutes}:${seconds.toString().padStart(2, '0')}`;
}

function peerMessageCopyText(message: RepoSyncPeerMessage) {
  return message.blocks
    .map((block) => {
      if (block.type === 'text') return block.text;
      if (block.type === 'code') {
        const language = block.language.trim();
        return language
          ? `\`\`\`${language}\n${block.text}\n\`\`\``
          : `\`\`\`\n${block.text}\n\`\`\``;
      }
      return block.name;
    })
    .join('\n\n');
}

function renderPeerMessageBlock(
  block: RepoSyncPeerMessageBlock,
  index: number,
  expanded: boolean
) {
  if (block.type === 'text') {
    return (
      <Text
        key={index}
        size="sm"
        lineClamp={expanded ? undefined : 6}
        style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}
      >
        {block.text}
      </Text>
    );
  }

  if (block.type === 'code') {
    return (
      <Box key={index}>
        {block.language ? <Text size="xs" c="dimmed">{block.language}</Text> : null}
        <Box
          component="pre"
          p="sm"
          m={0}
          style={{
            overflowX: 'auto',
            overflowY: expanded ? 'auto' : 'hidden',
            whiteSpace: 'pre-wrap',
            maxHeight: expanded ? 360 : '8.4em',
            borderRadius: 6,
            background: 'var(--mantine-color-dark-8)',
          }}
        >
          <Text component="code" size="xs" ff="monospace">
            {block.text}
          </Text>
        </Box>
      </Box>
    );
  }

  const href = `data:${block.mime_type || 'application/octet-stream'};base64,${block.data_base64}`;

  if (block.type === 'image') {
    return (
      <Stack key={index} gap="xs">
        <Text size="xs" c="dimmed">{block.name}</Text>
        <Box
          component="img"
          src={href}
          alt={block.name}
          style={{
            display: 'block',
            maxWidth: '100%',
            maxHeight: 360,
            objectFit: 'contain',
            borderRadius: 6,
          }}
        />
        <Button
          component="a"
          href={href}
          download={block.name}
          size="xs"
          variant="light"
          w="fit-content"
        >
          Download image
        </Button>
      </Stack>
    );
  }

  return (
    <Group key={index} justify="space-between" wrap="nowrap">
      <Stack gap={0} style={{ minWidth: 0 }}>
        <Text size="sm" fw={600} truncate>{block.name}</Text>
        <Text size="xs" c="dimmed">{block.mime_type || 'application/octet-stream'}</Text>
      </Stack>
      <Button component="a" href={href} download={block.name} size="xs" variant="light">
        Download
      </Button>
    </Group>
  );
}

type RepoSyncProps = {
  opened: boolean;
  onClose: () => void;
};

function emptyMapping(): RepoSyncMapping {
  return {
    id: '',
    workflow_run_id: '',
    peer_ipv4: '',
    peer_port: null,
    direction: 'both',
    peer_certificate_pem: '',
    enabled: true,
    sync_mode: 'manual',
    connected: false,
  };
}

export function RepoSync({ opened, onClose }: RepoSyncProps) {
  const [status, setStatus] = useState<RepoSyncStatus | null>(null);
  const [mapping, setMapping] = useState<RepoSyncMapping>(() => emptyMapping());
  const [pairing, setPairing] = useState<RepoSyncPairingSession | null>(null);
  const [passphrase, setPassphrase] = useState('');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [manualPreview, setManualPreview] = useState<{ file_count: number; total_bytes: number } | null>(null);
  const [peerMessages, setPeerMessages] = useState<RepoSyncPeerMessage[]>([]);
  const [peerDraft, setPeerDraft] = useState('');
  const [peerComposerMode, setPeerComposerMode] = useState<ComposerMode>('text');
  const [peerCodeLanguage, setPeerCodeLanguage] = useState('');
  const [peerAttachments, setPeerAttachments] = useState<RepoSyncPeerMessageBlock[]>([]);
  const [peerBusy, setPeerBusy] = useState(false);
  const [peerError, setPeerError] = useState<string | null>(null);
  const [expandedPeerMessages, setExpandedPeerMessages] = useState<Set<string>>(
    () => new Set()
  );
  const peerChatBottomRef = useRef<HTMLDivElement | null>(null);

  const workflowRunId = useMemo(() => {
    const match = window.location.pathname.match(/^\/workflows\/([^/]+)/);
    return match?.[1] ? decodeURIComponent(match[1]) : '';
  }, [opened]);

  async function refresh() {
    if (!workflowRunId) {
      setStatus(null);
      setMapping(emptyMapping());
      return;
    }

    const next = await getRepoSyncStatus(workflowRunId);
    setStatus(next);
    setMapping(next.mappings[0] ?? { ...emptyMapping(), workflow_run_id: workflowRunId });
    setPairing((current) => {
      if (!current) return current;
      return next.pairings.find((item) => item.id === current.id) ?? null;
    });
  }

  useEffect(() => {
    if (!opened) return;
    setError(null);
    setMessage(null);
    void refresh().catch((cause) => {
      setError(cause instanceof Error ? cause.message : String(cause));
    });
  }, [opened, workflowRunId]);

  useEffect(() => {
    if (!opened || !pairing || pairing.complete) return;

    let requestInFlight = false;
    const timer = window.setInterval(() => {
      if (requestInFlight) return;
      requestInFlight = true;
      void refresh()
        .catch((cause) => {
          setError(cause instanceof Error ? cause.message : String(cause));
        })
        .finally(() => {
          requestInFlight = false;
        });
    }, 1000);

    return () => window.clearInterval(timer);
  }, [opened, pairing?.id, pairing?.complete]);

  const paired = useMemo(
    () => Boolean(mapping.peer_certificate_pem?.trim()),
    [mapping.peer_certificate_pem]
  );

  const canSendPeerMessage = useMemo(
    () => Boolean(peerDraft.trim()) || peerAttachments.length > 0,
    [peerDraft, peerAttachments]
  );

  useEffect(() => {
    if (!opened || !mapping.connected || !mapping.id) {
      setPeerMessages([]);
      setPeerDraft('');
      setPeerAttachments([]);
      setExpandedPeerMessages(new Set());
      setPeerError(null);
      return;
    }

    let requestInFlight = false;
    let disposed = false;

    const refreshPeerMessages = async () => {
      if (requestInFlight) return;
      requestInFlight = true;
      try {
        const next = await getRepoSyncPeerMessages(mapping.id);
        if (!disposed) {
          setPeerMessages(next);
          setPeerError(null);
        }
      } catch (cause) {
        if (!disposed) {
          setPeerError(cause instanceof Error ? cause.message : String(cause));
        }
      } finally {
        requestInFlight = false;
      }
    };

    setPeerMessages([]);
    setPeerDraft('');
    setPeerAttachments([]);
    setPeerError(null);
    void refreshPeerMessages();

    const timer = window.setInterval(() => {
      void refreshPeerMessages();
    }, 1200);

    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [opened, mapping.connected, mapping.id]);

  useEffect(() => {
    if (!mapping.connected || peerMessages.length === 0) return;
    peerChatBottomRef.current?.scrollIntoView({ behavior: 'smooth', block: 'end' });
  }, [mapping.connected, peerMessages]);

  async function addPeerFiles(files: File[]) {
    setPeerError(null);
    try {
      const blocks = await Promise.all(
        files.map(async (file): Promise<RepoSyncPeerMessageBlock> => {
          if (file.size > 8 * 1024 * 1024) {
            throw new Error(`${file.name} exceeds the 8 MiB attachment limit.`);
          }

          const data_base64 = await fileAsBase64(file);
          const mime_type = file.type || 'application/octet-stream';

          if (mime_type.startsWith('image/')) {
            return {
              type: 'image',
              name: file.name,
              mime_type,
              data_base64,
            };
          }

          return {
            type: 'file',
            name: file.name,
            mime_type,
            data_base64,
          };
        })
      );

      setPeerAttachments((current) => [...current, ...blocks]);
    } catch (cause) {
      setPeerError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  function togglePeerMessageExpanded(messageId: string) {
    setExpandedPeerMessages((current) => {
      const next = new Set(current);
      if (next.has(messageId)) {
        next.delete(messageId);
      } else {
        next.add(messageId);
      }
      return next;
    });
  }

  function pastePeerImages(event: React.ClipboardEvent<HTMLTextAreaElement>) {
    const images = Array.from(event.clipboardData.items)
      .filter((item) => item.kind === 'file' && item.type.startsWith('image/'))
      .map((item) => item.getAsFile())
      .filter((file): file is File => Boolean(file));

    if (images.length === 0) return;

    event.preventDefault();
    void addPeerFiles(images);
  }

  async function sendPeerMessage() {
    if (!mapping.id || !mapping.connected || !canSendPeerMessage) return;

    setPeerBusy(true);
    setPeerError(null);
    try {
      const blocks: RepoSyncPeerMessageBlock[] = [];

      if (peerDraft.trim()) {
        blocks.push(
          peerComposerMode === 'code'
            ? {
                type: 'code',
                language: peerCodeLanguage.trim(),
                text: peerDraft,
              }
            : {
                type: 'text',
                text: peerDraft,
              }
        );
      }

      blocks.push(...peerAttachments);

      await sendRepoSyncPeerMessage(mapping.id, blocks);
      setPeerDraft('');
      setPeerAttachments([]);
      setPeerMessages(await getRepoSyncPeerMessages(mapping.id));
    } catch (cause) {
      setPeerError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setPeerBusy(false);
    }
  }

  async function saveMapping() {
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      const saved = await upsertRepoSyncMapping({
        id: mapping.id.trim() || undefined,
        workflow_run_id: workflowRunId,
        peer_ipv4: mapping.peer_ipv4.trim(),
        direction: mapping.direction,
        enabled: mapping.enabled,
        sync_mode: mapping.sync_mode,
      });
      setMapping(saved);
      setMessage('Mapping saved');
      await refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function beginPairing() {
    const phrase = passphrase.trim();
    if (phrase.length < 6) {
      setError('Pairing passphrase must be at least 6 characters.');
      return;
    }

    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      const saved = await upsertRepoSyncMapping({
        id: mapping.id.trim() || undefined,
        workflow_run_id: workflowRunId,
        peer_ipv4: mapping.peer_ipv4.trim(),
        direction: mapping.direction,
        enabled: true,
        sync_mode: 'manual',
      });
      setMapping(saved);
      const session = await startRepoSyncPairing(saved.id, phrase);
      setPairing(session);
      setMessage('Pairing started. Enter the same passphrase on the other computer.');
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function finishPairing() {
    if (!pairing) return;
    setBusy(true);
    setError(null);
    try {
      const session = await confirmRepoSyncPairing(pairing.id);
      setPairing(session);
      setMessage('Confirmed locally. Waiting for confirmation from the other computer.');
      await refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function reconnect() {
    if (!mapping.id) return;
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      const connected = await reconnectRepoSyncMapping(mapping.id);
      setMapping(connected);
      setMessage('Connected to trusted peer.');
      await refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function unpair() {
    if (!mapping.id) return;
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      await unpairRepoSyncMapping(mapping.id);
      setMapping({ ...emptyMapping(), workflow_run_id: workflowRunId });
      setPairing(null);
      setPassphrase('');
      setMessage('Trusted peer removed. Pairing will be required to connect again.');
      await refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function previewManualSync() {
    if (!workflowRunId) return;
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      const preview = await previewRepoSyncManual(workflowRunId);
      setManualPreview({
        file_count: preview.file_count,
        total_bytes: preview.total_bytes,
      });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function sendManualSync() {
    if (!workflowRunId) return;
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      const result = await sendRepoSyncManual(workflowRunId);
      setMessage(`Manual sync sent ${result.file_count} files to the trusted peer.`);
      setManualPreview(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      opened={opened}
      onClose={onClose}
      title="Repository sync"
      size={mapping.connected ? '90%' : 'xl'}
      centered
    >
      <SimpleGrid cols={{ base: 1, md: mapping.connected ? 2 : 1 }} spacing="md">
        <Stack gap="md">
        {error ? <Alert color="red">{error}</Alert> : null}
        {message ? <Alert color="green">{message}</Alert> : null}

        <Group justify="space-between">
          <Stack gap={2}>
            <Title order={5}>Machine identity</Title>
            <Text size="sm" c="dimmed">Each installation maintains its own TLS identity.</Text>
          </Stack>
          <Badge color={status?.identity_ready ? 'green' : 'gray'} variant="light">
            {status?.identity_ready ? 'Identity ready' : 'Not initialized'}
          </Badge>
        </Group>

        {status?.certificate_fingerprint ? (
          <Card withBorder>
            <Text size="xs" c="dimmed" tt="uppercase" fw={700}>Local fingerprint</Text>
            <Text size="sm" ff="monospace" style={{ wordBreak: 'break-all' }}>
              {status.certificate_fingerprint}
            </Text>
          </Card>
        ) : null}

        <Card withBorder>
          <Stack gap="xs">
            <Text size="xs" c="dimmed" tt="uppercase" fw={700}>This computer</Text>
            {status?.local_ipv4 ? (
              <Group justify="space-between" align="center">
                <Text size="lg" fw={700} ff="monospace">
                  {status.local_ipv4}
                </Text>
                <CopyButton value={status.local_ipv4}>
                  {({ copied, copy }) => (
                    <Button size="xs" variant="light" onClick={copy}>
                      {copied ? 'IPv4 copied' : 'Copy IPv4'}
                    </Button>
                  )}
                </CopyButton>
              </Group>
            ) : (
              <Text size="sm" c="dimmed">No LAN IPv4 address detected.</Text>
            )}
          </Stack>
        </Card>

        <Card withBorder>
          <Stack gap="md">
            <Group justify="space-between">
              <Title order={5}>Sync peer</Title>
              <Badge color={paired ? 'green' : 'gray'} variant="light">
                {paired ? 'Paired' : 'Unpaired'}
              </Badge>
            </Group>

            {!workflowRunId ? (
              <Alert color="yellow">Open Repo Sync from a workflow before pairing.</Alert>
            ) : paired ? (
              <>
                <Card withBorder>
                  <Stack gap="xs">
                    <Group justify="space-between">
                      <Text fw={700}>Trusted peer</Text>
                      <Badge color={mapping.connected ? 'green' : 'yellow'} variant="light">
                        {mapping.connected ? 'Connected' : 'Disconnected'}
                      </Badge>
                    </Group>
                    <Text size="sm" ff="monospace">{mapping.peer_ipv4}</Text>
                    <Text size="xs" c="dimmed">
                      This workflow remembers the peer certificate until you explicitly unpair it.
                    </Text>
                  </Stack>
                </Card>

                <Select
                  label="Direction"
                  value={mapping.direction}
                  data={[
                    { value: 'send', label: 'Send only' },
                    { value: 'receive', label: 'Receive only' },
                    { value: 'both', label: 'Send and receive' },
                  ]}
                  onChange={(value) => {
                    if (!value) return;
                    setMapping((current) => ({ ...current, direction: value as RepoSyncDirection }));
                  }}
                />

                <Select
                  label="Sync mode"
                  value={mapping.sync_mode}
                  data={[
                    { value: 'manual', label: 'Manual hard sync' },
                    { value: 'auto_apply', label: 'Auto Apply ChangeSets' },
                  ]}
                  onChange={(value) => {
                    if (!value) return;
                    setMapping((current) => ({
                      ...current,
                      sync_mode: value as RepoSyncMapping['sync_mode'],
                    }));
                  }}
                />

                {mapping.sync_mode === 'auto_apply' ? (
                  <Alert color="blue">
                    Successful local ChangeSets are mirrored only while this trusted peer is connected.
                  </Alert>
                ) : mapping.direction === 'receive' ? (
                  <Alert color="gray">
                    This workflow is receive-only. It can accept a manual repository sync from the trusted peer but cannot send one.
                  </Alert>
                ) : (
                  <Card withBorder>
                    <Stack gap="md">
                      <Stack gap={2}>
                        <Text fw={700}>Manual Hard Sync</Text>
                        <Text size="sm" c="dimmed">
                          Send this workflow's Context Exporter scope to the trusted peer as an explicit repository synchronization.
                        </Text>
                      </Stack>

                      {manualPreview ? (
                        <Card withBorder>
                          <Stack gap="xs">
                            <Group justify="space-between">
                              <Text size="sm" c="dimmed">Files to send</Text>
                              <Text fw={700}>{manualPreview.file_count}</Text>
                            </Group>
                            <Group justify="space-between">
                              <Text size="sm" c="dimmed">Payload size</Text>
                              <Text fw={700}>{manualPreview.total_bytes.toLocaleString()} bytes</Text>
                            </Group>
                          </Stack>
                        </Card>
                      ) : null}

                      {!mapping.connected ? (
                        <Alert color="yellow">
                          Reconnect to the trusted peer before sending a manual sync.
                        </Alert>
                      ) : null}

                      <Group justify="flex-end">
                        <Button
                          variant="default"
                          loading={busy}
                          disabled={!mapping.connected}
                          onClick={() => void previewManualSync()}
                        >
                          Preview Sync
                        </Button>
                        <Button
                          loading={busy}
                          disabled={!mapping.connected || !manualPreview}
                          onClick={() => void sendManualSync()}
                        >
                          Send Sync
                        </Button>
                      </Group>
                    </Stack>
                  </Card>
                )}

                <Group justify="space-between">
                  <Button color="red" variant="subtle" loading={busy} onClick={() => void unpair()}>
                    Unpair
                  </Button>
                  <Group>
                    <Button variant="default" loading={busy} onClick={() => void saveMapping()}>
                      Save settings
                    </Button>
                    <Button
                      loading={busy}
                      disabled={mapping.connected}
                      onClick={() => void reconnect()}
                    >
                      {mapping.connected ? 'Connected' : 'Reconnect'}
                    </Button>
                  </Group>
                </Group>
              </>
            ) : (
              <>
                <TextInput
                  label="Peer IPv4"
                  placeholder="192.168.1.42"
                  value={mapping.peer_ipv4}
                  onChange={(event) => setMapping((current) => ({ ...current, peer_ipv4: event.currentTarget.value }))}
                />

                <TextInput
                  label="Pairing passphrase"
                  description="Enter the same short passphrase on both computers. It is used only during the five-minute pairing window."
                  placeholder="maple-river-7421"
                  type="password"
                  value={passphrase}
                  onChange={(event) => setPassphrase(event.currentTarget.value)}
                />

                <Select
                  label="Direction"
                  value={mapping.direction}
                  data={[
                    { value: 'send', label: 'Send only' },
                    { value: 'receive', label: 'Receive only' },
                    { value: 'both', label: 'Send and receive' },
                  ]}
                  onChange={(value) => {
                    if (!value) return;
                    setMapping((current) => ({ ...current, direction: value as RepoSyncDirection }));
                  }}
                />

                <Group justify="flex-end">
                  <Button loading={busy} onClick={() => void beginPairing()}>
                    Start pairing
                  </Button>
                </Group>
              </>
            )}
          </Stack>
        </Card>

        {pairing ? (
          <Card withBorder>
            <Stack gap="md">
              <Group justify="space-between">
                <Title order={5}>Pairing</Title>
                <Badge color="yellow" variant="light">Pending</Badge>
              </Group>

              <Text size="sm" c="dimmed">
                Waiting for the other mdev instance. Enter the same pairing passphrase on both computers. Certificates and sync ports are exchanged automatically.
              </Text>

              {status?.local_ipv4 ? (
                <Card withBorder>
                  <Stack gap="xs">
                    <Text size="xs" c="dimmed" tt="uppercase" fw={700}>Local sync address</Text>
                    <Group justify="space-between" align="center">
                      <Text fw={700} ff="monospace">
                        {status.local_ipv4}:{pairing.local_port}
                      </Text>
                      <CopyButton value={`${status.local_ipv4}:${pairing.local_port}`}>
                        {({ copied, copy }) => (
                          <Button size="xs" variant="light" onClick={copy}>
                            {copied ? 'Address copied' : 'Copy address'}
                          </Button>
                        )}
                      </CopyButton>
                    </Group>
                  </Stack>
                </Card>
              ) : null}

              {pairing.verification_code ? (
                <Card withBorder>
                  <Stack gap="xs" align="center">
                    <Badge color="green" variant="light">Peer found</Badge>
                    <Text size="xs" c="dimmed" tt="uppercase" fw={700}>Verification code</Text>
                    <Title order={3} ff="monospace">{pairing.verification_code}</Title>
                    <Text size="sm" c="dimmed" ta="center">
                      Confirm only when the other computer displays the exact same code.
                    </Text>
                  </Stack>
                </Card>
              ) : (
                <Alert color="blue">
                  Waiting for the other computer to start pairing with the same passphrase.
                </Alert>
              )}

              <Group justify="flex-end">
                <Button
                  disabled={!pairing.verification_code}
                  loading={busy}
                  onClick={() => void finishPairing()}
                >
                  Codes match on both machines
                </Button>
              </Group>
            </Stack>
          </Card>
        ) : null}
        </Stack>
        {mapping.connected && mapping.id ? (
          <Card withBorder h="100%">
            <Stack gap="md" h="100%">
              <Group justify="space-between">
                <Stack gap={0}>
                  <Title order={5}>Peer chat</Title>
                  <Text size="xs" c="dimmed">
                    Messages live only in memory and expire after 15 minutes.
                  </Text>
                </Stack>
                <Badge color="green" variant="light">Connected</Badge>
              </Group>

              {peerError ? <Alert color="red">{peerError}</Alert> : null}

              <ScrollArea h={480} offsetScrollbars>
                <Stack gap="sm" pr="xs">
                  {peerMessages.length === 0 ? (
                    <Text size="sm" c="dimmed" ta="center" py="xl">
                      Send logs, code, screenshots, or files to the connected peer.
                    </Text>
                  ) : null}

                  {peerMessages.map((item) => (
                    <Card key={item.id} withBorder p="sm">
                      <Stack gap="xs">
                        <Group justify="space-between" align="flex-start" wrap="nowrap">
                          <Text size="xs" fw={700}>
                            {item.author === 'self' ? 'You' : 'Peer'}
                          </Text>
                          <Group gap={4} wrap="nowrap">
                            <Text size="xs" c="dimmed">
                              {new Date(item.sent_at_unix_ms).toLocaleTimeString()}
                            </Text>
                            <Badge size="xs" variant="light">
                              {remainingLabel(item.expires_at_unix_ms)}
                            </Badge>
                            <CopyButton value={peerMessageCopyText(item)}>
                              {({ copied, copy }) => (
                                <Button size="compact-xs" variant="subtle" onClick={copy}>
                                  {copied ? 'Copied' : 'Copy'}
                                </Button>
                              )}
                            </CopyButton>
                            <Button
                              size="compact-xs"
                              variant="subtle"
                              onClick={() => togglePeerMessageExpanded(item.id)}
                            >
                              {expandedPeerMessages.has(item.id) ? 'Collapse' : 'Expand'}
                            </Button>
                          </Group>
                        </Group>
                        {item.blocks.map((block, index) =>
                          renderPeerMessageBlock(
                            block,
                            index,
                            expandedPeerMessages.has(item.id)
                          )
                        )}
                      </Stack>
                    </Card>
                  ))}
                  <Box ref={peerChatBottomRef} />
                </Stack>
              </ScrollArea>

              {peerAttachments.length > 0 ? (
                <Card withBorder p="xs">
                  <Stack gap="xs">
                    {peerAttachments.map((block, index) => (
                      <Group key={index} justify="space-between">
                        <Text size="xs" truncate>
                          {block.type === 'file' || block.type === 'image'
                            ? block.name
                            : block.type}
                        </Text>
                        <Button
                          size="xs"
                          variant="subtle"
                          color="red"
                          onClick={() =>
                            setPeerAttachments((current) =>
                              current.filter((_, itemIndex) => itemIndex !== index)
                            )
                          }
                        >
                          Remove
                        </Button>
                      </Group>
                    ))}
                  </Stack>
                </Card>
              ) : null}

              <Stack gap="xs">
                <Group grow align="flex-end">
                  <Select
                    label="Block type"
                    value={peerComposerMode}
                    data={[
                      { value: 'text', label: 'Text' },
                      { value: 'code', label: 'Code' },
                    ]}
                    onChange={(value) =>
                      setPeerComposerMode((value as ComposerMode | null) ?? 'text')
                    }
                  />

                  {peerComposerMode === 'code' ? (
                    <TextInput
                      label="Language"
                      placeholder="rust"
                      value={peerCodeLanguage}
                      onChange={(event) => setPeerCodeLanguage(event.currentTarget.value)}
                    />
                  ) : (
                    <Box />
                  )}
                </Group>

                <Textarea
                  placeholder={
                    peerComposerMode === 'code'
                      ? 'Paste code or logs...'
                      : 'Message the connected peer... Enter to send, Shift+Enter for a new line.'
                  }
                  value={peerDraft}
                  onChange={(event) => setPeerDraft(event.currentTarget.value)}
                  onPaste={pastePeerImages}
                  onKeyDown={(event) => {
                    if (
                      event.key === 'Enter' &&
                      !event.shiftKey &&
                      !event.nativeEvent.isComposing
                    ) {
                      event.preventDefault();
                      if (!peerBusy && canSendPeerMessage) {
                        void sendPeerMessage();
                      }
                    }
                  }}
                  autosize
                  minRows={3}
                  maxRows={8}
                />

                <Group justify="space-between">
                  <FileButton multiple onChange={(files) => void addPeerFiles(files)}>
                    {(props) => (
                      <Button {...props} variant="default">
                        Attach files
                      </Button>
                    )}
                  </FileButton>

                  <Button
                    loading={peerBusy}
                    disabled={!canSendPeerMessage}
                    onClick={() => void sendPeerMessage()}
                  >
                    Send
                  </Button>
                </Group>
              </Stack>
            </Stack>
          </Card>
        ) : null}
      </SimpleGrid>
    </Modal>
  );
}
