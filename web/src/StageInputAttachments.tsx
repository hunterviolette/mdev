import { useRef, useState } from 'react';
import { ActionIcon, Button, Group, Stack, Text, Textarea } from '@mantine/core';
import { IconPaperclip, IconTrash } from '@tabler/icons-react';

export const MAX_ATTACHMENT_BYTES = 8 * 1024 * 1024;
export const MAX_TOTAL_ATTACHMENT_BYTES = 8 * 1024 * 1024;

const attachmentsByStage = new Map<string, File[]>();

export function getStageInputAttachments(runId: string, stepId: string): File[] {
  return [...(attachmentsByStage.get(`${runId}:${stepId}`) ?? [])];
}

export function clearStageInputAttachments(runId: string, stepId: string): void {
  attachmentsByStage.delete(`${runId}:${stepId}`);
}

export async function serializeStageInputAttachments(files: File[]): Promise<Array<{ filename: string; media_type: string; data: string }>> {
  return Promise.all(files.map(async (file) => {
    const bytes = new Uint8Array(await file.arrayBuffer());
    let binary = '';
    for (let offset = 0; offset < bytes.length; offset += 8192) {
      binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
    }
    return {
      filename: file.name,
      media_type: file.type || 'application/octet-stream',
      data: btoa(binary)
    };
  }));
}

export function StageInputAttachments(props: {
  runId: string | null;
  stepId: string | null;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  label?: string;
  description?: string;
  minRows?: number;
  defaultInputControl?: React.ReactNode;
  defaultInputEditor?: React.ReactNode;
}) {
  const { runId, stepId, value, onChange, placeholder, disabled, label = 'User input', description, minRows = 2, defaultInputControl, defaultInputEditor } = props;
  const inputRef = useRef<HTMLInputElement>(null);
  const identity = `${runId ?? ''}:${stepId ?? ''}`;
  const [attachments, setAttachments] = useState<File[]>(() => attachmentsByStage.get(identity) ?? []);
  const [error, setError] = useState<string | null>(null);

  function updateAttachments(next: File[]) {
    attachmentsByStage.set(identity, next);
    setAttachments(next);
  }

  function addFiles(files: File[]) {
    if (!runId || !stepId || disabled) return;
    const accepted: File[] = [];
    let total = attachments.reduce((sum, file) => sum + file.size, 0);
    let nextError: string | null = null;
    for (const file of files) {
      if (file.size > MAX_ATTACHMENT_BYTES) {
        nextError = `${file.name} exceeds the 8 MB attachment limit.`;
        continue;
      }
      if (total + file.size > MAX_TOTAL_ATTACHMENT_BYTES) {
        nextError = 'Combined attachments cannot exceed 8 MB.';
        continue;
      }
      accepted.push(file);
      total += file.size;
    }
    setError(nextError);
    if (accepted.length) updateAttachments([...attachments, ...accepted]);
  }

  return (
    <Stack gap="xs">
      <Textarea
        label={label}
        description={description}
        value={value}
        onChange={(event) => onChange(event.currentTarget.value)}
        placeholder={placeholder}
        onPaste={(event) => {
          const files = Array.from(event.clipboardData.files);
          if (files.length) {
            event.preventDefault();
            addFiles(files);
          }
        }}
        disabled={disabled}
        autosize={false}
        rows={Math.max(minRows, 5)}
        styles={{ input: { overflowY: 'auto', resize: 'none' } }}
      />
      <input
        ref={inputRef}
        type="file"
        multiple
        hidden
        accept="image/*,.pdf,.txt,.md,.json,.csv,.xml,.yaml,.yml,.log,.html,.css,.js,.jsx,.ts,.tsx,.rs,.py,.sql"
        onChange={(event) => {
          addFiles(Array.from(event.currentTarget.files ?? []));
          event.currentTarget.value = '';
        }}
      />
      <Group gap="xs">
        {defaultInputControl}
        <Button
          size="xs"
          variant="light"
          leftSection={<IconPaperclip size={14} />}
          disabled={disabled || !runId || !stepId}
          onClick={() => inputRef.current?.click()}
        >
          Attach files
        </Button>
        <Text size="xs" c="dimmed">Paste images with Ctrl+V</Text>
      </Group>
      {defaultInputEditor}
      {attachments.map((file, index) => (
        <Group key={`${identity}:${index}:${file.name}:${file.size}`} justify="space-between" gap="xs">
          <Text size="xs" truncate>{file.name} ({(file.size / 1024).toFixed(1)} KB)</Text>
          <ActionIcon
            size="sm"
            variant="subtle"
            color="red"
            disabled={disabled}
            aria-label={`Remove ${file.name}`}
            onClick={() => updateAttachments(attachments.filter((_, current) => current !== index))}
          >
            <IconTrash size={14} />
          </ActionIcon>
        </Group>
      ))}
      {error ? <Text size="xs" c="red">{error}</Text> : null}
    </Stack>
  );
}
