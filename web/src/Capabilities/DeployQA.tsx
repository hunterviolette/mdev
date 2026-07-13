import {
  Button,
  Card,
  Checkbox,
  Group,
  Modal,
  NumberInput,
  Select,
  Stack,
  Switch,
  Text,
  TextInput,
  Textarea,
} from '@mantine/core';
import { IconPlus, IconTrash } from '@tabler/icons-react';

import type {
  DependencyProviderSpec,
  QaServiceSpec,
  TerminalCommandSpec,
} from '../api';

export type DeployQAValues = {
  dependency_providers: string[];
  services: QaServiceSpec[];
  port_start: number;
  port_end: number;
  hostname_template: string;
  shutdown_grace_seconds: number;
};

export type DeployQAProps = {
  opened: boolean;
  disabled?: boolean;
  providers: DependencyProviderSpec[];
  values: DeployQAValues;
  onClose: () => void;
  onChange: <K extends keyof DeployQAValues>(key: K, value: DeployQAValues[K]) => void;
};

function serviceCommand(
  id: string,
  label: string,
  command: string,
  workingDirectory: string,
  environment: Record<string, string>
): TerminalCommandSpec {
  return {
    id,
    label,
    command,
    arguments: [],
    working_directory: workingDirectory,
    environment,
    shell: 'system',
    mode: 'service',
    timeout_seconds: null,
    continue_on_error: false,
  };
}

function mdevDefaultServices(): QaServiceSpec[] {
  return [
    {
      id: 'web',
      label: 'Web',
      command: serviceCommand(
        'deploy-qa-web',
        'npm run dev',
        'npm run dev -- --host 127.0.0.1 --port {port} --strictPort',
        'web',
        {
          VITE_API_BASE_URL: '{service.api.internal_url}/api',
        }
      ),
      port: {
        environment_variable: 'WORKFLOW_WEB_PORT',
        preferred: null,
      },
      readiness: {
        kind: 'http',
        path: '/',
        expected_status: 200,
        timeout_seconds: 60,
      },
      public: false,
    },
    {
      id: 'api',
      label: 'API',
      command: serviceCommand(
        'deploy-qa-api',
        'cargo run',
        'cargo run',
        'api',
        {
          WORKFLOW_API_HOST: '127.0.0.1',
        }
      ),
      port: {
        environment_variable: 'WORKFLOW_API_PORT',
        preferred: null,
      },
      readiness: {
        kind: 'http',
        path: '/api/health',
        expected_status: 200,
        timeout_seconds: 120,
      },
      public: false,
    },
  ];
}

function newService(index: number): QaServiceSpec {
  const id = `service-${index}`;
  return {
    id,
    label: `Service ${index}`,
    command: serviceCommand(
      `deploy-qa-${id}`,
      `Service ${index}`,
      'npm run dev',
      '.',
      {}
    ),
    port: {
      environment_variable: 'PORT',
      preferred: null,
    },
    readiness: {
      kind: 'http',
      path: '/',
      expected_status: 200,
      timeout_seconds: 60,
    },
    public: false,
  };
}

function environmentText(service: QaServiceSpec): string {
  return Object.entries(service.command.environment ?? {})
    .map(([key, value]) => `${key}=${value}`)
    .join('\n');
}

function parseEnvironment(value: string): Record<string, string> {
  const environment: Record<string, string> = {};

  for (const line of value.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) continue;

    const separator = trimmed.indexOf('=');
    if (separator <= 0) continue;

    environment[trimmed.slice(0, separator).trim()] = trimmed
      .slice(separator + 1)
      .trim();
  }

  return environment;
}

export const defaultDeployQAValues: DeployQAValues = {
  dependency_providers: [],
  services: mdevDefaultServices(),
  port_start: 24000,
  port_end: 24999,
  hostname_template: '{run}.qa.localhost',
  shutdown_grace_seconds: 5,
};

export function DeployQA({
  opened,
  disabled = false,
  providers,
  values,
  onClose,
  onChange,
}: DeployQAProps) {
  const sharedDependenciesEnabled = values.dependency_providers.length > 0;

  function updateService(index: number, next: QaServiceSpec) {
    onChange(
      'services',
      values.services.map((service, serviceIndex) =>
        serviceIndex === index
          ? { ...next, public: false }
          : { ...service, public: false }
      )
    );
  }

  return (
    <Modal
      opened={opened}
      onClose={onClose}
      title="DeployQA"
      size="xl"
      centered
    >
      <Stack gap="md">
        <Stack gap="xs">
          <Group justify="space-between" align="flex-start">
            <div>
              <Text fw={600}>Shared dependencies</Text>
              <Text size="sm" c="dimmed">
                Optionally reuse trusted dependency providers before starting the deployment.
              </Text>
            </div>
            <Switch
              disabled={disabled || providers.length === 0}
              checked={sharedDependenciesEnabled}
              onChange={(event) =>
                onChange(
                  'dependency_providers',
                  event.currentTarget.checked
                    ? providers.map((provider) => provider.id)
                    : []
                )
              }
            />
          </Group>

          {sharedDependenciesEnabled ? (
            <Checkbox.Group
              value={values.dependency_providers}
              onChange={(next) => onChange('dependency_providers', next)}
            >
              <Stack gap={4}>
                {providers.map((provider) => (
                  <Checkbox
                    key={provider.id}
                    value={provider.id}
                    label={`${provider.label} · ${provider.ecosystem}`}
                    disabled={disabled}
                  />
                ))}
              </Stack>
            </Checkbox.Group>
          ) : (
            <Text size="sm" c="dimmed">
              The deployment will run without shared dependency reuse.
            </Text>
          )}
        </Stack>

        <Group justify="space-between">
          <div>
            <Text fw={700}>Services</Text>
            <Text size="sm" c="dimmed">
              Ports are allocated before startup. Environment values may use {'{port}'}, {'{deployment}'}, {'{service.<id>.port}'}, and {'{service.<id>.url}'}.
            </Text>
          </div>
          <Button
            variant="default"
            leftSection={<IconPlus size={16} />}
            disabled={disabled}
            onClick={() =>
              onChange('services', [
                ...values.services,
                newService(values.services.length + 1),
              ])
            }
          >
            Add service
          </Button>
        </Group>

        {values.services.map((service, index) => {
          const readiness = service.readiness ?? {};
          const readinessType =
            typeof readiness.kind === 'string'
              ? readiness.kind
              : typeof readiness.type === 'string'
                ? readiness.type
                : 'http';
          const readinessPath =
            typeof readiness.path === 'string' ? readiness.path : '/';
          const readinessStatus =
            typeof readiness.expected_status === 'number'
              ? readiness.expected_status
              : 200;
          const readinessTimeout =
            typeof readiness.timeout_seconds === 'number'
              ? readiness.timeout_seconds
              : 60;

          return (
            <Card key={`${service.id}-${index}`} withBorder padding="md">
              <Stack gap="sm">
                <Group justify="space-between">
                  <Text fw={700}>{service.label || service.id}</Text>
                  <Button
                    size="compact-xs"
                    variant="subtle"
                    color="red"
                    leftSection={<IconTrash size={14} />}
                    disabled={disabled}
                    onClick={() =>
                      onChange(
                        'services',
                        values.services.filter(
                          (_, serviceIndex) => serviceIndex !== index
                        )
                      )
                    }
                  >
                    Remove
                  </Button>
                </Group>

                <Group grow align="flex-start">
                  <TextInput
                    label="Service ID"
                    value={service.id}
                    disabled={disabled}
                    onChange={(event) => {
                      const id = event.currentTarget.value;
                      updateService(index, {
                        ...service,
                        id,
                        command: {
                          ...service.command,
                          id: service.command.id || `deploy-qa-${id}`,
                        },
                      });
                    }}
                  />
                  <TextInput
                    label="Label"
                    value={service.label}
                    disabled={disabled}
                    onChange={(event) =>
                      updateService(index, {
                        ...service,
                        label: event.currentTarget.value,
                      })
                    }
                  />
                </Group>

                <TextInput
                  label="Command"
                  value={service.command.command}
                  disabled={disabled}
                  onChange={(event) =>
                    updateService(index, {
                      ...service,
                      command: {
                        ...service.command,
                        command: event.currentTarget.value,
                      },
                    })
                  }
                />

                <TextInput
                  label="Working directory"
                  value={service.command.working_directory}
                  disabled={disabled}
                  onChange={(event) =>
                    updateService(index, {
                      ...service,
                      command: {
                        ...service.command,
                        working_directory: event.currentTarget.value,
                      },
                    })
                  }
                />

                <Textarea
                  label="Environment"
                  description="One KEY=value entry per line."
                  minRows={3}
                  autosize
                  value={environmentText(service)}
                  disabled={disabled}
                  onChange={(event) =>
                    updateService(index, {
                      ...service,
                      command: {
                        ...service.command,
                        environment: parseEnvironment(event.currentTarget.value),
                      },
                    })
                  }
                />

                <Group grow align="flex-start">
                  <TextInput
                    label="Port environment variable"
                    value={service.port.environment_variable}
                    disabled={disabled}
                    onChange={(event) =>
                      updateService(index, {
                        ...service,
                        port: {
                          ...service.port,
                          environment_variable: event.currentTarget.value,
                        },
                      })
                    }
                  />
                  <NumberInput
                    label="Preferred port"
                    min={1}
                    max={65535}
                    value={service.port.preferred ?? ''}
                    disabled={disabled}
                    onChange={(value) =>
                      updateService(index, {
                        ...service,
                        port: {
                          ...service.port,
                          preferred:
                            typeof value === 'number' && Number.isFinite(value)
                              ? value
                              : null,
                        },
                      })
                    }
                  />
                </Group>

                <Group grow align="flex-start">
                  <Select
                    label="Readiness type"
                    value={readinessType}
                    data={[
                      { value: 'http', label: 'HTTP' },
                      { value: 'none', label: 'None' },
                    ]}
                    disabled={disabled}
                    onChange={(value) =>
                      updateService(index, {
                        ...service,
                        readiness:
                          value === 'none'
                            ? { kind: 'none' }
                            : {
                                kind: 'http',
                                path: readinessPath,
                                expected_status: readinessStatus,
                                timeout_seconds: readinessTimeout,
                              },
                      })
                    }
                  />
                  <TextInput
                    label="HTTP readiness path"
                    value={readinessPath}
                    disabled={disabled || readinessType === 'none'}
                    onChange={(event) =>
                      updateService(index, {
                        ...service,
                        readiness: {
                          ...readiness,
                          kind: 'http',
                          path: event.currentTarget.value,
                        },
                      })
                    }
                  />
                  <NumberInput
                    label="Readiness timeout seconds"
                    min={1}
                    value={readinessTimeout}
                    disabled={disabled || readinessType === 'none'}
                    onChange={(value) =>
                      updateService(index, {
                        ...service,
                        readiness: {
                          ...readiness,
                          kind: 'http',
                          timeout_seconds: Number(value) || 60,
                        },
                      })
                    }
                  />
                </Group>

              </Stack>
            </Card>
          );
        })}

        <Group grow align="flex-start">
          <NumberInput
            label="Port range start"
            min={1}
            max={65535}
            value={values.port_start}
            disabled={disabled}
            onChange={(value) =>
              onChange('port_start', Number(value) || 24000)
            }
          />
          <NumberInput
            label="Port range end"
            min={1}
            max={65535}
            value={values.port_end}
            disabled={disabled}
            onChange={(value) =>
              onChange('port_end', Number(value) || 24999)
            }
          />
        </Group>

        <TextInput
          label="Hostname template"
          description="The first public service uses this hostname. Additional public services receive a -<service-id> suffix unless {service} is present."
          value={values.hostname_template}
          disabled={disabled}
          onChange={(event) =>
            onChange('hostname_template', event.currentTarget.value)
          }
        />

        <NumberInput
          label="Shutdown grace seconds"
          min={0}
          value={values.shutdown_grace_seconds}
          disabled={disabled}
          onChange={(value) =>
            onChange('shutdown_grace_seconds', Number(value) || 0)
          }
        />

        <Group justify="space-between">
          <Button
            variant="default"
            disabled={disabled}
            onClick={() => {
              onChange('dependency_providers', []);
              onChange('services', mdevDefaultServices());
              onChange('port_start', 24000);
              onChange('port_end', 24999);
              onChange('hostname_template', '{run}.qa.localhost');
              onChange('shutdown_grace_seconds', 5);
            }}
          >
            Reset MDEV defaults
          </Button>
          <Button onClick={onClose}>Done</Button>
        </Group>
      </Stack>
    </Modal>
  );
}
