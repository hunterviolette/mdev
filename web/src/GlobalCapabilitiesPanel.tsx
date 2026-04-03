import { useEffect, useState } from 'react';
import { Alert, Button, Group, JsonInput, Stack, Text } from '@mantine/core';

type GlobalCapabilitiesPanelProps = {
  value: Record<string, unknown>;
  busy?: boolean;
  onSave: (payload: Record<string, unknown>) => Promise<void>;
};

export function GlobalCapabilitiesPanel(props: GlobalCapabilitiesPanelProps) {
  const { value, busy = false, onSave } = props;
  const [draft, setDraft] = useState(JSON.stringify(value, null, 2));
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    setDraft(JSON.stringify(value, null, 2));
    setError(null);
    setStatus(null);
  }, [value]);

  const handleSave = async () => {
    setError(null);
    setStatus(null);
    try {
      const parsed = JSON.parse(draft) as Record<string, unknown>;
      await onSave(parsed);
      setStatus('Global capabilities saved.');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <Stack>
      <Text size="sm">Edit workflow-global capability state. Stage panels remain for stage-local state and overrides.</Text>
      {error ? <Alert color="red">{error}</Alert> : null}
      {status ? <Alert color="green">{status}</Alert> : null}
      <JsonInput
        value={draft}
        onChange={setDraft}
        autosize
        minRows={16}
        validationError="Invalid JSON"
        formatOnBlur
      />
      <Group justify="flex-end">
        <Button onClick={() => void handleSave()} loading={busy}>Save global capabilities</Button>
      </Group>
    </Stack>
  );
}
