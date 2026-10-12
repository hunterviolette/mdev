import type {
  WorkflowBuilderCatalog,
  WorkflowBuilderDocument,
  WorkflowBuilderStageDocument,
  WorkflowGlobalConfig,
  SharedDependenciesConfig,
  WorkflowStageDescriptor,
  WorkflowStageField,
  WorkflowTemplateDefinition,
} from './api';

export type BuilderStep = {
  id: string;
  name: string;
  stepType: string;
  fields: Record<string, unknown>;
};

export function capabilityDisplayLabel(capabilityKey: string): string {
  switch (capabilityKey) {
    case 'context_export':
      return 'Context export';
    case 'inference':
      return 'Inference';
    case 'changeset':
      return 'ChangeSet apply';
    case 'shared_dependencies':
      return 'Shared dependencies';
    case 'terminal_runtime':
      return 'Terminal runtime';
    case 'deploy_qa':
      return 'DeployQA';
    case 'qa_environment':
      return 'DeployQA';
    case 'compile_commands':
      return 'Compile commands';
    case 'sap/import':
      return 'SAP import';
    case 'sap/export':
      return 'SAP export';
    default:
      return capabilityKey;
  }
}

export function flattenStageFields(descriptor: WorkflowStageDescriptor): WorkflowStageField[] {
  return descriptor.editable_fields.flatMap((group) => group.fields);
}

export function builderStepFromDescriptor(descriptor: WorkflowStageDescriptor, id?: string): BuilderStep {
  return {
    id: id ?? `${descriptor.step_type}-${crypto.randomUUID()}`,
    name: descriptor.label,
    stepType: descriptor.step_type,
    fields: Object.fromEntries(flattenStageFields(descriptor).map((field) => [field.key, field.default])),
  };
}

export function buildStageDocument(step: BuilderStep): WorkflowBuilderStageDocument {
  return {
    id: step.id,
    name: step.name,
    step_type: step.stepType,
    field_values: step.fields,
  };
}

function defaultSharedDependencies(): SharedDependenciesConfig {
  return {
    enabled: true,
    providers: [
      {
        id: 'node-root',
        label: 'Node root',
        ecosystem: 'node',
        root: '.',
        manifests: ['package.json', 'package-lock.json'],
        isolated: {
          storage_path: 'node_modules',
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
      },
      {
        id: 'cargo-root',
        label: 'Cargo root',
        ecosystem: 'cargo',
        root: 'api',
        manifests: ['Cargo.lock'],
      },
    ],
  };
}

export function defaultGlobals(): WorkflowGlobalConfig {
  return {
    resources: {
      repo: {
        repo_ref: '',
        git_ref: 'WORKTREE',
      },
    },
    capabilities: {
      inference: {
        stage_sessions: {},
        sessions: {},
      },
      shared_dependencies: defaultSharedDependencies(),
    },
    automation: {
    },
  };
}

export function inferenceRouteNames(globals: WorkflowGlobalConfig): string[] {
  const inference = (globals.capabilities?.inference ?? {}) as Record<string, unknown>;
  const sessions = (inference.sessions ?? {}) as Record<string, unknown>;
  return Object.keys(sessions);
}

export function buildBuilderDocument(steps: BuilderStep[], globals?: WorkflowGlobalConfig): WorkflowBuilderDocument {
  return {
    version: 1,
    globals: globals ?? defaultGlobals(),
    stages: steps.map((step) => buildStageDocument(step)),
  };
}

export function descriptorMap(catalog: WorkflowBuilderCatalog): Record<string, WorkflowStageDescriptor> {
  const out: Record<string, WorkflowStageDescriptor> = {};
  for (const descriptor of catalog.stage_descriptors) {
    out[descriptor.step_type] = descriptor;
    out[descriptor.step_type.trim().toLowerCase()] = descriptor;
    out[descriptor.label] = descriptor;
    out[descriptor.label.trim().toLowerCase()] = descriptor;
  }
  return out;
}

export function builderStepsFromDefinition(
  definition: WorkflowTemplateDefinition | null | undefined,
  catalog: WorkflowBuilderCatalog
): BuilderStep[] {
  if (!definition) {
    return [];
  }

  const descriptors = descriptorMap(catalog);

  return definition.steps.flatMap((step) => {
    const descriptor = descriptors[step.step_type];
    if (!descriptor) {
      return [];
    }

    const defaults = Object.fromEntries(
      flattenStageFields(descriptor).map((field) => [field.key, field.default])
    );

    const fieldValues = Object.fromEntries(
      flattenStageFields(descriptor).map((field) => [field.key, readPath(step as Record<string, unknown>, field.bind_to, field.default)])
    );

    return [{
      id: step.id,
      name: step.name,
      stepType: step.step_type,
      fields: {
        ...defaults,
        ...fieldValues,
      },
    }];
  });
}

function readPath(root: Record<string, unknown>, path: string, fallback: unknown): unknown {
  const parts = path.split('.').filter(Boolean);
  let current: unknown = root;

  for (const part of parts) {
    if (!current || typeof current !== 'object' || !(part in (current as Record<string, unknown>))) {
      return fallback;
    }
    current = (current as Record<string, unknown>)[part];
  }

  return current ?? fallback;
}
