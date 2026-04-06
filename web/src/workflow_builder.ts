import type {
  WorkflowBuilderCatalog,
  WorkflowBuilderDocument,
  WorkflowBuilderStageDocument,
  WorkflowGlobalConfig,
  WorkflowStageDescriptor,
  WorkflowStageField,
} from './api';

export type BuilderStep = {
  id: string;
  name: string;
  stepType: string;
  fields: Record<string, unknown>;
};

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

export function defaultGlobals(): WorkflowGlobalConfig {
  return {
    resources: {
      repo: {
        repo_ref: '',
        git_ref: 'WORKTREE',
      },
    },
    capabilities: {
      inference: {},
      context_export: {
        save_path: '/tmp/repo_context.txt',
      },
      changeset_schema: {},
      'gateway_model/changeset': {},
      compile_commands: {},
      'sap/import': {},
      'sap/export': {},
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
