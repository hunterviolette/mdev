import { useState, type ReactNode } from 'react';
import { RepoSync } from './Capabilities/RepoSync';
import { Automation, type AutomationProfile } from './Capabilities/Automation';
import {
  getRun,
  getWorkflowBuilderCatalog,
  patchWorkflowGlobalState,
  type WorkflowAutomationControlDescriptor,
  type WorkflowTemplateDefinition,
} from './api';
import { Badge, Button, Card, Group, Modal, SimpleGrid, Stack, Text, Title } from '@mantine/core';

type GlobalCapabilitiesPanelProps = {
  onOpenInference: () => void;
  onOpenRepoFragment: () => void;
  onOpenChangesetSchema: () => void;
  onOpenPlanner: () => void;
  onOpenApplyChangeset: () => void;
  onOpenGitPatchPayload: () => void;
  onOpenSharedDependencies: () => void;
  onOpenDeployQA: () => void;
  repoContextArmed: boolean;
  changesetSchemaArmed: boolean;
  plannerArmed: boolean;
  sharedDependenciesEnabled: boolean;
  deployQAAvailable: boolean;
  repoSyncEnabled?: boolean;
  repoSyncPaired?: boolean;
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
    onOpenSharedDependencies,
    onOpenDeployQA,
    repoContextArmed,
    changesetSchemaArmed,
    plannerArmed,
    sharedDependenciesEnabled,
    deployQAAvailable,
    repoSyncEnabled = false,
    repoSyncPaired = false,
  } = props;

  const [repoSyncOpen, setRepoSyncOpen] = useState(false);
  const [automationOpen, setAutomationOpen] = useState(false);
  const [automationSaving, setAutomationSaving] = useState(false);
  const [automationStatus, setAutomationStatus] = useState<string | null>(null);
  const [automationValue, setAutomationValue] = useState<Partial<AutomationProfile>>({});
  const [automationControls, setAutomationControls] = useState<WorkflowAutomationControlDescriptor[]>([]);
  const [automationCapabilities, setAutomationCapabilities] = useState<string[]>([]);

  function currentWorkflowRunId(): string | null {
    const match = window.location.pathname.match(/^\/workflows\/([^/]+)\/capabilities\/?$/);
    if (!match?.[1]) return null;
    try {
      return decodeURIComponent(match[1]);
    } catch {
      return match[1];
    }
  }

  function invokedAutomationCapabilities(definition: WorkflowTemplateDefinition): string[] {
    const capabilities = new Set<string>();

    for (const step of definition.steps ?? []) {
      for (const node of step.execution_plan ?? []) {
        if (node.kind !== 'capability' || node.enabled === false) continue;

        const key = node.key.trim().toLowerCase();
        if (key === 'gateway_model/changeset' || key === 'changeset_apply') {
          capabilities.add('changeset');
        } else {
          capabilities.add(key);
        }
      }
    }

    return Array.from(capabilities);
  }

  async function openAutomation() {
    const runId = currentWorkflowRunId();
    setAutomationStatus(null);
    setAutomationOpen(true);

    if (!runId) {
      setAutomationStatus('Unable to resolve the current workflow run.');
      return;
    }

    try {
      const [run, catalog] = await Promise.all([
        getRun(runId),
        getWorkflowBuilderCatalog(),
      ]);
      const workflowEngine = (run.context?.workflow_engine ?? {}) as Record<string, unknown>;
      const globalState = (workflowEngine.global_state ?? {}) as Record<string, unknown>;
      const runtimeAutomation = globalState.automation;
      const definitionAutomation = run.definition?.globals?.automation;

      setAutomationValue(
        runtimeAutomation && typeof runtimeAutomation === 'object' && !Array.isArray(runtimeAutomation)
          ? runtimeAutomation as Partial<AutomationProfile>
          : definitionAutomation && typeof definitionAutomation === 'object' && !Array.isArray(definitionAutomation)
            ? definitionAutomation as Partial<AutomationProfile>
            : {}
      );
      setAutomationControls(catalog.automation_controls ?? []);
      setAutomationCapabilities(invokedAutomationCapabilities(run.definition));
    } catch (error) {
      setAutomationStatus(error instanceof Error ? error.message : String(error));
    }
  }

  async function saveAutomation(profile: AutomationProfile) {
    const runId = currentWorkflowRunId();
    if (!runId) {
      setAutomationStatus('Unable to resolve the current workflow run.');
      return;
    }

    setAutomationSaving(true);
    setAutomationStatus(null);

    try {
      const run = await getRun(runId);
      const workflowEngine = (run.context?.workflow_engine ?? {}) as Record<string, unknown>;
      const globalState = (workflowEngine.global_state ?? {}) as Record<string, unknown>;

      await patchWorkflowGlobalState(runId, {
        ...globalState,
        automation: structuredClone(profile),
      });

      setAutomationValue(structuredClone(profile));
      setAutomationStatus('Saved');
      setAutomationOpen(false);
    } catch (error) {
      setAutomationStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setAutomationSaving(false);
    }
  }

  const repoSyncBadge = repoSyncPaired
    ? { label: repoSyncEnabled ? 'Active' : 'Paired', color: repoSyncEnabled ? 'green' : 'blue' }
    : repoSyncEnabled
      ? { label: 'Configured', color: 'blue' }
      : { label: 'Unpaired', color: 'gray' };

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

      <Modal
        opened={automationOpen}
        onClose={() => {
          if (!automationSaving) {
            setAutomationOpen(false);
            setAutomationStatus(null);
          }
        }}
        title={
          <Group gap="sm" wrap="nowrap">
            <Text fw={600}>Automation</Text>
            <Text size="sm" c="dimmed" fw={400}>
              Choose which behaviors run automatically and when they should activate.
            </Text>
          </Group>
        }
        size="min(1120px, 94vw)"
        centered
        styles={{
          content: { maxHeight: '92dvh' },
          body: { overflow: 'hidden' },
        }}
      >
        <Automation
          value={automationValue}
          controls={automationControls}
          invokedCapabilities={automationCapabilities}
          busy={automationSaving}
          status={automationStatus}
          onCancel={() => {
            if (!automationSaving) {
              setAutomationOpen(false);
              setAutomationStatus(null);
            }
          }}
          onSave={saveAutomation}
        />
      </Modal>

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
          eyebrow="Lifecycle"
          title="Automation"
          description="Choose which workflow behaviors should run automatically and configure their thresholds."
          buttonLabel="Configure automation"
          onClick={() => void openAutomation()}
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
        <CapabilityCard
          eyebrow="Repository mirror"
          title="Repo Sync"
          description="Pair another computer over authenticated TLS and mirror successful ChangeSets between mapped repositories."
          buttonLabel="Configure sync"
          onClick={() => setRepoSyncOpen(true)}
          badge={
            <Badge color={repoSyncBadge.color} variant="light">
              {repoSyncBadge.label}
            </Badge>
          }
        />
        <CapabilityCard
          eyebrow="Dependencies"
          title="Shared dependencies"
          description="Configure reusable Node and Cargo dependency providers that Compile and DeployQA stages may select."
          buttonLabel="Configure dependencies"
          onClick={onOpenSharedDependencies}
          badge={
            <Badge color={sharedDependenciesEnabled ? 'green' : 'gray'} variant="light">
              {sharedDependenciesEnabled ? 'Enabled' : 'Disabled'}
            </Badge>
          }
        />
        <CapabilityCard
          eyebrow="QA deployment"
          title="DeployQA"
          description="Configure and monitor the selected workflow QA deployment, services, readiness, ports, routing, and output."
          buttonLabel={deployQAAvailable ? 'Configure deployment' : 'No QA stage'}
          onClick={onOpenDeployQA}
          badge={
            <Badge color={deployQAAvailable ? 'blue' : 'gray'} variant="light">
              {deployQAAvailable ? 'Available' : 'Unavailable'}
            </Badge>
          }
        />
      </SimpleGrid>

      <RepoSync
        opened={repoSyncOpen}
        onClose={() => setRepoSyncOpen(false)}
      />
    </Stack>
  );
}
