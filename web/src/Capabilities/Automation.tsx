import { Fragment, useEffect, useMemo, useState } from 'react';
import { Alert, Button, Checkbox, Group, NumberInput, ScrollArea, Stack, Switch, Table, Text } from '@mantine/core';
import type { WorkflowAutomationControlDescriptor } from '../api';

export type AutomationProfile = {
  new_session: string[];
  selected: string[];
  [key: string]: boolean | number | string[] | undefined;
};

type AutomationProps = {
  value?: Partial<AutomationProfile> | null;
  controls?: WorkflowAutomationControlDescriptor[];
  invokedCapabilities?: string[];
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

const DEFAULT_NEW_SESSION = ['repo_context', 'changeset_schema', 'planner_fragment'];

function normalizeControlValue(
  value: unknown,
  descriptor: WorkflowAutomationControlDescriptor,
): number | boolean {
  if (descriptor.field_type === 'boolean') {
    return typeof value === 'boolean'
      ? value
      : typeof descriptor.default === 'boolean'
        ? descriptor.default
        : false;
  }

  const fallback = typeof descriptor.default === 'number' ? descriptor.default : 1;
  return typeof value === 'number' && Number.isFinite(value)
    ? Math.max(1, Math.floor(value))
    : fallback;
}

function normalizeProfile(
  value: Partial<AutomationProfile> | null | undefined,
  controls: WorkflowAutomationControlDescriptor[],
): AutomationProfile {
  const profile: AutomationProfile = {
    new_session: Array.isArray(value?.new_session)
      ? value.new_session.filter((item): item is string => typeof item === 'string')
      : DEFAULT_NEW_SESSION,
    selected: Array.isArray(value?.selected)
      ? value.selected.filter((item): item is string => typeof item === 'string')
      : controls.map((descriptor) => descriptor.key),
  };

  for (const descriptor of controls) {
    profile[descriptor.key] = normalizeControlValue(value?.[descriptor.key], descriptor);
  }

  return profile;
}

function toggleValue(values: string[], key: string, enabled: boolean): string[] {
  if (enabled) {
    return values.includes(key) ? values : [...values, key];
  }

  return values.filter((item) => item !== key);
}

function thresholdUnit(descriptor: WorkflowAutomationControlDescriptor): string {
  return descriptor.key.includes('errors') ? 'errors' : 'failures';
}

function SectionRow(props: { title: string }) {
  return (
    <Table.Tr>
      <Table.Td colSpan={3} pt="md" pb="xs" px="sm">
        <Group gap="sm" wrap="nowrap">
          <Text size="xs" fw={700} tt="uppercase" c="dimmed" style={{ letterSpacing: '0.06em' }}>
            {props.title}
          </Text>
          <div
            style={{
              height: 1,
              flex: 1,
              background: 'var(--mantine-color-default-border)',
            }}
          />
        </Group>
      </Table.Td>
    </Table.Tr>
  );
}

function automationRowStyle(enabled: boolean, available = true) {
  return {
    opacity: available ? (enabled ? 1 : 0.52) : 0.34,
    boxShadow: enabled && available
      ? 'inset 3px 0 0 var(--mantine-primary-color-filled)'
      : 'none',
    transition: 'opacity 120ms ease, box-shadow 120ms ease',
  };
}

export function Automation(props: AutomationProps) {
  const {
    value,
    controls = [],
    invokedCapabilities = [],
    busy = false,
    status = null,
    onSave,
    onCancel,
  } = props;
  const [profile, setProfile] = useState<AutomationProfile>(() => normalizeProfile(value, controls));

  useEffect(() => {
    setProfile(normalizeProfile(value, controls));
  }, [value, controls]);

  const invoked = useMemo(() => new Set(invokedCapabilities), [invokedCapabilities]);
  const sections = useMemo(() => {
    const grouped = new Map<string, WorkflowAutomationControlDescriptor[]>();
    for (const descriptor of controls) {
      const items = grouped.get(descriptor.section) ?? [];
      items.push(descriptor);
      grouped.set(descriptor.section, items);
    }
    return Array.from(grouped.entries());
  }, [controls]);

  const controlAvailable = (descriptor: WorkflowAutomationControlDescriptor) =>
    descriptor.required_capabilities.every((capability) => invoked.has(capability));

  return (
    <Stack gap="lg" h="100%">
      <ScrollArea.Autosize mah="calc(82dvh - 110px)" offsetScrollbars type="auto">
        <Table
          verticalSpacing={0}
          horizontalSpacing="md"
          withRowBorders={false}
          style={{ tableLayout: 'fixed' }}
        >
          <Table.Thead>
            <Table.Tr>
              <Table.Th w={110} pb={6}>
                <Text
                  size="xs"
                  fw={700}
                  tt="uppercase"
                  style={{ letterSpacing: '0.04em' }}
                >
                  Enabled
                </Text>
              </Table.Th>
              <Table.Th pb={6}>
                <Text
                  size="xs"
                  fw={700}
                  tt="uppercase"
                  style={{ letterSpacing: '0.04em' }}
                >
                  Automation
                </Text>
              </Table.Th>
              <Table.Th w={260} pb={6}>
                <Text
                  size="xs"
                  fw={700}
                  tt="uppercase"
                  style={{ letterSpacing: '0.04em' }}
                >
                  Trigger
                </Text>
              </Table.Th>
            </Table.Tr>
          </Table.Thead>

          <Table.Tbody>
            <SectionRow title="New inference session" />

            {CAPABILITIES.map(([capability, label]) => {
              const selected = profile.new_session.includes(capability);

              return (
                <Table.Tr
                  key={`new-session-${capability}`}
                  style={automationRowStyle(selected)}
                >
                  <Table.Td py="sm" pl="md">
                    <Checkbox
                      aria-label={`Enable ${label}`}
                      checked={selected}
                      onChange={(event) => {
                        const checked = event.currentTarget.checked;
                        setProfile((current) => ({
                          ...current,
                          new_session: toggleValue(current.new_session, capability, checked),
                        }));
                      }}
                    />
                  </Table.Td>
                  <Table.Td py="sm">
                    <Text size="sm" fw={selected ? 600 : 400}>{label}</Text>
                  </Table.Td>
                  <Table.Td py="sm">
                    <Text size="sm" c="dimmed">On session start</Text>
                  </Table.Td>
                </Table.Tr>
              );
            })}

            {sections.map(([section, descriptors]) => (
              <Fragment key={section}>
                <SectionRow title={section} />

                {descriptors.map((descriptor) => {
                  const available = controlAvailable(descriptor);
                  const selected = profile.selected.includes(descriptor.key);
                  const currentValue = profile[descriptor.key];

                  return (
                    <Table.Tr
                      key={descriptor.key}
                      style={automationRowStyle(selected, available)}
                    >
                      <Table.Td py="md" pl="md" style={{ verticalAlign: 'top' }}>
                        <Checkbox
                          mt={2}
                          aria-label={`Enable ${descriptor.label}`}
                          disabled={!available}
                          checked={selected}
                          onChange={(event) => {
                            const checked = event.currentTarget.checked;
                            setProfile((current) => ({
                              ...current,
                              selected: toggleValue(current.selected, descriptor.key, checked),
                            }));
                          }}
                        />
                      </Table.Td>

                      <Table.Td py="md">
                        <Stack gap={3}>
                          <Text size="sm" fw={selected && available ? 600 : 400}>
                            {descriptor.label}
                          </Text>
                          <Text size="xs" c="dimmed" maw={560}>
                            {available
                              ? descriptor.description
                              : 'Required capability is not invoked by this workflow.'}
                          </Text>
                        </Stack>
                      </Table.Td>

                      <Table.Td py="md" style={{ verticalAlign: 'top' }}>
                        {descriptor.field_type === 'boolean' ? (
                          <Switch
                            mt={1}
                            aria-label={`${descriptor.label} value`}
                            disabled={!available || !selected}
                            checked={typeof currentValue === 'boolean' ? currentValue : false}
                            onChange={(event) => {
                              const checked = event.currentTarget.checked;
                              setProfile((current) => ({
                                ...current,
                                [descriptor.key]: checked,
                              }));
                            }}
                          />
                        ) : (
                          <Group gap="xs" wrap="nowrap">
                            <Text size="sm" c="dimmed">after</Text>
                            <NumberInput
                              aria-label={`${descriptor.label} threshold`}
                              min={1}
                              step={1}
                              w={76}
                              size="sm"
                              disabled={!available || !selected}
                              value={typeof currentValue === 'number'
                                ? currentValue
                                : normalizeControlValue(undefined, descriptor) as number}
                              onChange={(nextValue) => {
                                setProfile((current) => ({
                                  ...current,
                                  [descriptor.key]: normalizeControlValue(nextValue, descriptor),
                                }));
                              }}
                            />
                            <Text size="sm" c="dimmed">
                              {thresholdUnit(descriptor)}
                            </Text>
                          </Group>
                        )}
                      </Table.Td>
                    </Table.Tr>
                  );
                })}
              </Fragment>
            ))}
          </Table.Tbody>
        </Table>
      </ScrollArea.Autosize>

      {status ? (
        <Alert color={status.toLowerCase().includes('saved') ? 'green' : 'red'}>
          {status}
        </Alert>
      ) : null}

      <Group
        justify="flex-end"
        pt="md"
        style={{ borderTop: '1px solid var(--mantine-color-default-border)' }}
      >
        <Button variant="default" onClick={onCancel}>Cancel</Button>
        <Button loading={busy} onClick={() => void onSave(profile)}>Save</Button>
      </Group>
    </Stack>
  );
}
