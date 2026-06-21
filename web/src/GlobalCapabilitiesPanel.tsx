import type { ReactNode } from 'react';
import { Badge, Button, Card, Group, SimpleGrid, Stack, Text, Title } from '@mantine/core';

type GlobalCapabilitiesPanelProps = {
  onOpenInference: () => void;
  onOpenRepoFragment: () => void;
  onOpenChangesetSchema: () => void;
  onOpenPlanner: () => void;
  onOpenApplyChangeset: () => void;
  onOpenGitPatchPayload: () => void;
  repoContextArmed: boolean;
  changesetSchemaArmed: boolean;
  plannerArmed: boolean;
};

type CapabilityCardProps = {
  title: string;
  eyebrow: string;
  description: string;
  buttonLabel: string;
  onClick: () => void;
  badge?: ReactNode;
};

function CapabilityCard(props: CapabilityCardProps) {
  const { title, eyebrow, description, buttonLabel, onClick, badge } = props;

  return (
    <Card withBorder radius="md" p="md" style={{ height: '100%' }}>
      <Stack gap="sm" h="100%">
        <Group justify="space-between" align="flex-start" wrap="nowrap">
          <Stack gap={2} style={{ minWidth: 0 }}>
            <Text size="xs" c="dimmed" tt="uppercase" fw={700}>{eyebrow}</Text>
            <Title order={5}>{title}</Title>
          </Stack>
          {badge}
        </Group>
        <Text size="sm" c="dimmed" style={{ flex: 1 }}>{description}</Text>
        <Button variant="light" fullWidth onClick={onClick}>{buttonLabel}</Button>
      </Stack>
    </Card>
  );
}

function ArmedBadge(props: { armed: boolean }) {
  return (
    <Badge color={props.armed ? 'green' : 'gray'} variant="light">
      {props.armed ? 'Armed' : 'Not armed'}
    </Badge>
  );
}

export function GlobalCapabilitiesPanel(props: GlobalCapabilitiesPanelProps) {
  const {
    onOpenInference,
    onOpenRepoFragment,
    onOpenChangesetSchema,
    onOpenPlanner,
    onOpenApplyChangeset,
    onOpenGitPatchPayload,
    repoContextArmed,
    changesetSchemaArmed,
    plannerArmed,
  } = props;

  return (
    <Stack gap="md">
      <Group justify="space-between" align="flex-end" wrap="wrap">
        <Stack gap={2}>
          <Title order={4}>Capability cockpit</Title>
          <Text size="sm" c="dimmed">Configure reusable workflow capabilities, shared payloads, and handoff tools.</Text>
        </Stack>
        <Group gap="xs">
          <Badge variant="light" color="blue">Workflow-global</Badge>
          <Badge variant="light" color="gray">Manual tools</Badge>
        </Group>
      </Group>

      <SimpleGrid cols={{ base: 1, sm: 2, lg: 3 }} spacing="md">
        <CapabilityCard
          eyebrow="Inference"
          title="Inference sessions"
          description="Manage reusable named inference sessions and map inference-enabled workflow stages to those sessions."
          buttonLabel="Manage sessions"
          onClick={onOpenInference}
          badge={<Badge color="blue" variant="light">Core</Badge>}
        />
        <CapabilityCard
          eyebrow="Context"
          title="Repo fragment"
          description="Choose repository files and fragments that can be injected into model-backed stages."
          buttonLabel="Configure fragment"
          onClick={onOpenRepoFragment}
          badge={<ArmedBadge armed={repoContextArmed} />}
        />
        <CapabilityCard
          eyebrow="Schema"
          title="Changeset schema"
          description="Expose the canonical changeset contract so code stages can produce apply-ready patches."
          buttonLabel="Patch schema"
          onClick={onOpenChangesetSchema}
          badge={<ArmedBadge armed={changesetSchemaArmed} />}
        />
        <CapabilityCard
          eyebrow="Planner"
          title="Repo planner"
          description="Create or edit the repo-level supervisor planner shared by every workflow using this repo root."
          buttonLabel="Open planner"
          onClick={onOpenPlanner}
          badge={<ArmedBadge armed={plannerArmed} />}
        />
        <CapabilityCard
          eyebrow="Apply"
          title="Apply changeset"
          description="Paste and apply a changeset payload directly against the current workflow repository."
          buttonLabel="Open applier"
          onClick={onOpenApplyChangeset}
          badge={<Badge color="orange" variant="light">Manual</Badge>}
        />
        <CapabilityCard
          eyebrow="Git handoff"
          title="Git patch payload"
          description="Generate a portable git patch payload for another repo, or apply one received from elsewhere."
          buttonLabel="Generate or apply"
          onClick={onOpenGitPatchPayload}
          badge={<Badge color="violet" variant="light">Portable</Badge>}
        />
      </SimpleGrid>
    </Stack>
  );
}
