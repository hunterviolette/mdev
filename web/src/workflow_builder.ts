import type {
  WorkflowBuilderCatalog,
  WorkflowBuilderDocument,
  WorkflowBuilderStageDocument,
  WorkflowGlobalConfig,
  WorkflowGovernancePolicyDescriptor,
  WorkflowStageDescriptor,
  WorkflowStageField,
  WorkflowStageGovernancePolicy,
  WorkflowTemplateDefinition,
} from './api';

export type BuilderStep = {
  id: string;
  name: string;
  stepType: string;
  fields: Record<string, unknown>;
  governancePolicies?: WorkflowStageGovernancePolicy[];
};

export function capabilityDisplayLabel(capabilityKey: string): string {
  switch (capabilityKey) {
    case 'context_export':
      return 'Context export';
    case 'inference':
      return 'Inference';
    case 'gateway_model/changeset':
      return 'ChangeSet apply';
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

export function governancePolicyMap(descriptor: WorkflowStageDescriptor): Record<string, WorkflowGovernancePolicyDescriptor> {
  return Object.fromEntries((descriptor.available_governance_policies ?? []).map((policy) => [policy.key, policy]));
}

export function ensureStageGovernancePolicies(
  descriptor: WorkflowStageDescriptor,
  selected: WorkflowStageGovernancePolicy[] | undefined,
): WorkflowStageGovernancePolicy[] {
  const byKey = new Map((selected ?? []).map((item) => [item.key, item]));
  return (descriptor.available_governance_policies ?? []).map((policy) => ({
    key: policy.key,
    enabled: byKey.get(policy.key)?.enabled ?? false,
    config: Object.fromEntries(
      policy.fields.map((field) => [
        field.key,
        (byKey.get(policy.key)?.config as Record<string, unknown> | undefined)?.[field.key] ?? field.default,
      ])
    ),
  }));
}

export function builderStepFromDescriptor(descriptor: WorkflowStageDescriptor, id?: string): BuilderStep {
  return {
    id: id ?? `${descriptor.step_type}-${crypto.randomUUID()}`,
    name: descriptor.label,
    stepType: descriptor.step_type,
    fields: Object.fromEntries(flattenStageFields(descriptor).map((field) => [field.key, field.default])),
    governancePolicies: ensureStageGovernancePolicies(descriptor, []),
  };
}

export function buildStageDocument(step: BuilderStep): WorkflowBuilderStageDocument {
  return {
    id: step.id,
    name: step.name,
    step_type: step.stepType,
    field_values: step.fields,
    governance_policies: step.governancePolicies ?? [],
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
    },
    automation: {
    },
  };
}

export function buildBuilderDocument(steps: BuilderStep[], globals?: WorkflowGlobalConfig): WorkflowBuilderDocument {
  return {
    version: 1,
    globals: globals ?? defaultGlobals(),
    stages: steps.map((step) => buildStageDocument(step)),
  };
}

export function descriptorMap(catalog: WorkflowBuilderCatalog): Record<string, WorkflowStageDescriptor> {
  return Object.fromEntries(catalog.stage_descriptors.map((descriptor) => [descriptor.step_type, descriptor]));
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
      governancePolicies: ensureStageGovernancePolicies(descriptor, step.governance?.policies),
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
