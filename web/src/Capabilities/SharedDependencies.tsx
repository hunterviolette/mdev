import {
  Button,
  Card,
  Group,
  Modal,
  Select,
  Stack,
  Switch,
  Text,
  TextInput,
} from '@mantine/core';
import { IconPlus, IconTrash } from '@tabler/icons-react';

import type {
  DependencyProviderSpec,
  SharedDependenciesConfig,
} from '../api';

export type SharedDependenciesProps = {
  opened: boolean;
  value: SharedDependenciesConfig;
  onClose: () => void;
  onChange: (value: SharedDependenciesConfig) => void;
};

function newProvider(index: number): DependencyProviderSpec {
  return {
    id: `provider-${index}`,
    label: `Dependency provider ${index}`,
    ecosystem: 'node',
    root: '.',
    manifests: ['package-lock.json'],
    trusted_artifact: {
      kind: 'node_modules',
      path: 'node_modules',
      read_only: true,
    },
    isolated: {
      storage_path: `.mdev/dependencies/provider-${index}`,
      seed_from_trusted: true,
      install: {
        commands: [],
        stop_on_failure: true,
      },
    },
    mismatch: {
      disposition: 'operator_checkpoint',
      allowed_dispositions: [
        'create_isolated_dependencies',
        'continue_trusted_with_warning',
        'skip_stage',
      ],
    },
  };
}

export function SharedDependencies({
  opened,
  value,
  onClose,
  onChange,
}: SharedDependenciesProps) {
  function updateProvider(
    index: number,
    update: Partial<DependencyProviderSpec>
  ) {
    onChange({
      ...value,
      providers: value.providers.map((provider, providerIndex) =>
        providerIndex === index ? { ...provider, ...update } : provider
      ),
    });
  }

  function updateEcosystem(
    index: number,
    ecosystem: DependencyProviderSpec['ecosystem']
  ) {
    const provider = value.providers[index];
    const node = ecosystem === 'node';

    updateProvider(index, {
      ecosystem,
      manifests: node ? ['package-lock.json'] : ['Cargo.lock'],
      trusted_artifact: node
        ? {
            kind: 'node_modules',
            path: 'node_modules',
            read_only: true,
          }
        : {
            kind: 'cargo',
            cargo_home: '.cargo',
            target_directory: 'target',
            share_target: true,
            read_only: true,
          },
      isolated: {
        ...provider.isolated,
        storage_path: `.mdev/dependencies/${provider.id}`,
      },
    });
  }

  return (
    <Modal
      opened={opened}
      onClose={onClose}
      title="Shared dependencies"
      size="xl"
      centered
    >
      <Stack gap="md">
        <Group justify="space-between" align="flex-start">
          <div>
            <Text fw={700}>Dependency providers</Text>
            <Text size="sm" c="dimmed">
              Compile and DeployQA stages may optionally select these providers.
            </Text>
          </div>

          <Switch
            label="Enabled"
            checked={value.enabled}
            onChange={(event) =>
              onChange({
                ...value,
                enabled: event.currentTarget.checked,
              })
            }
          />
        </Group>

        {value.providers.map((provider, index) => (
          <Card key={`${provider.id}-${index}`} withBorder padding="sm">
            <Stack gap="sm">
              <Group justify="space-between">
                <Text fw={600}>{provider.label || provider.id}</Text>
                <Button
                  size="compact-xs"
                  variant="subtle"
                  color="red"
                  leftSection={<IconTrash size={14} />}
                  onClick={() =>
                    onChange({
                      ...value,
                      providers: value.providers.filter(
                        (_, providerIndex) => providerIndex !== index
                      ),
                    })
                  }
                >
                  Remove
                </Button>
              </Group>

              <Group grow align="flex-start">
                <TextInput
                  label="Provider ID"
                  value={provider.id}
                  onChange={(event) =>
                    updateProvider(index, { id: event.currentTarget.value })
                  }
                />

                <TextInput
                  label="Label"
                  value={provider.label}
                  onChange={(event) =>
                    updateProvider(index, { label: event.currentTarget.value })
                  }
                />

                <Select
                  label="Ecosystem"
                  value={provider.ecosystem}
                  data={[
                    { value: 'node', label: 'Node' },
                    { value: 'cargo', label: 'Cargo' },
                  ]}
                  onChange={(next) =>
                    updateEcosystem(
                      index,
                      next === 'cargo' ? 'cargo' : 'node'
                    )
                  }
                />
              </Group>

              <TextInput
                label="Workspace root"
                description="Path relative to the workflow repository."
                value={provider.root}
                onChange={(event) =>
                  updateProvider(index, { root: event.currentTarget.value })
                }
              />

              <TextInput
                label="Manifest files"
                description="Comma-separated lockfiles or manifests used to verify compatibility."
                value={provider.manifests.join(', ')}
                onChange={(event) =>
                  updateProvider(index, {
                    manifests: event.currentTarget.value
                      .split(',')
                      .map((item) => item.trim())
                      .filter(Boolean),
                  })
                }
              />

              <TextInput
                label="Isolated storage path"
                value={provider.isolated.storage_path}
                onChange={(event) =>
                  updateProvider(index, {
                    isolated: {
                      ...provider.isolated,
                      storage_path: event.currentTarget.value,
                    },
                  })
                }
              />

              <Switch
                label="Seed isolated dependencies from trusted artifacts"
                checked={provider.isolated.seed_from_trusted}
                onChange={(event) =>
                  updateProvider(index, {
                    isolated: {
                      ...provider.isolated,
                      seed_from_trusted: event.currentTarget.checked,
                    },
                  })
                }
              />
            </Stack>
          </Card>
        ))}

        <Group justify="space-between">
          <Button
            variant="default"
            leftSection={<IconPlus size={16} />}
            onClick={() =>
              onChange({
                ...value,
                providers: [
                  ...value.providers,
                  newProvider(value.providers.length + 1),
                ],
              })
            }
          >
            Add provider
          </Button>

          <Button onClick={onClose}>Done</Button>
        </Group>
      </Stack>
    </Modal>
  );
}
