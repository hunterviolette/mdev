import { useEffect, useState } from 'react';
import { Alert, Button, Card, Checkbox, Group, Stack, Switch, Text, Title } from '@mantine/core';

export type AutomationProfile = {
  enabled: boolean;
  new_session: string[];
};

type AutomationProps = {
  value?: Partial<AutomationProfile> | null;
  busy?: boolean;
  status?: string | null;
  onSave: (profile: AutomationProfile) => Promise<void> | void;
  onCancel: () => void;
};

const CAPABILITIES = [
  ['repo_context', 'Repo fragment'],
  ['changeset_schema', 'Changeset schema'],
  ['planner_fragment', 'Planner fragment'],
  ['planner_schema', 'Planner schema'],
  ['planner_apply', 'Planner apply'],
] as const;

const DEFAULT_PROFILE: AutomationProfile = {
  enabled: true,
  new_session: ['repo_context', 'changeset_schema', 'planner_fragment'],
};

function normalizeProfile(value?: Partial<AutomationProfile> | null): AutomationProfile {
  return {
    enabled: value?.enabled ?? DEFAULT_PROFILE.enabled,
    new_session: Array.isArray(value?.new_session)
      ? value.new_session.filter((item): item is string => typeof item === 'string')
      : DEFAULT_PROFILE.new_session,
  };
}

function toggleCapability(values: string[], capability: string, enabled: boolean): string[] {
  if (enabled) {
    return values.includes(capability) ? values : [...values, capability];
  }

  return values.filter((item) => item !== capability);
}

export function Automation(props: AutomationProps) {
  const { value, busy = false, status = null, onSave, onCancel } = props;
  const [profile, setProfile] = useState<AutomationProfile>(() => normalizeProfile(value));

  useEffect(() => {
    setProfile(normalizeProfile(value));
  }, [value]);

  return (
    <Stack gap="md">
      <Group justify="space-between" align="flex-start" wrap="wrap">
        <Stack gap={2}>
          <Title order={4}>Automation</Title>
          <Text size="sm" c="dimmed">
            Configure which global single-use capabilities are armed when an inference session restarts.
          </Text>
        </Stack>
        <Switch
          label="Enabled"
          checked={profile.enabled}
          onChange={(event) => {
            const checked = event.currentTarget.checked;
            setProfile((current) => ({
              ...current,
              enabled: checked,
            }));
          }}
        />
      </Group>

      <Card withBorder>
        <Stack gap="sm">
          <Title order={5}>New inference session</Title>
          <Text size="sm" c="dimmed">
            Armed capabilities remain active until a stage that declares and includes them consumes them. Planner inputs are armed only when a planner feature is selected.
          </Text>
          {CAPABILITIES.map(([capability, label]) => (
            <Checkbox
              key={`new-session-${capability}`}
              label={label}
              checked={profile.new_session.includes(capability)}
              onChange={(event) => {
                const checked = event.currentTarget.checked;
                setProfile((current) => ({
                  ...current,
                  new_session: toggleCapability(
                    current.new_session,
                    capability,
                    checked,
                  ),
                }));
              }}
            />
          ))}
        </Stack>
      </Card>

      {status ? (
        <Alert color={status.toLowerCase().includes('saved') ? 'green' : 'red'}>
          {status}
        </Alert>
      ) : null}

      <Group justify="flex-end">
        <Button size="xs" variant="default" onClick={onCancel}>Cancel</Button>
        <Button size="xs" loading={busy} onClick={() => void onSave(profile)}>Save</Button>
      </Group>
    </Stack>
  );
}
