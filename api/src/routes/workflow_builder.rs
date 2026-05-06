use axum::{extract::State, routing::{get, post}, Json, Router};
use serde_json::{json, Value};

use crate::{
    app_state::AppState,
    engine::capabilities::planner,
    engine::capabilities::inference::stage_support::{
        build_inference_execution_plan,
        InferenceStageSettings,
    },
    models::{
        AutomationMode,
        CompileWorkflowBuilderRequest,
        CompileWorkflowBuilderResponse,
        StageExecutionNode,
        StageExecutionNodeKind,
        WorkflowBuilderCatalog,
        WorkflowBuilderDocument,
        WorkflowBuilderStageDocument,
        WorkflowCapabilityBinding,
        WorkflowCapabilitySummaryItem,
        WorkflowGlobalConfig,
        WorkflowGovernancePolicyDescriptor,
        WorkflowStageDescriptor,
        WorkflowStageField,
        WorkflowStageFieldGroup,
        WorkflowStageFieldOption,
        WorkflowStageFieldUi,
        WorkflowStageFieldVisibility,
        WorkflowStageRoute,
        WorkflowStepAdvancementConfig,
        WorkflowStepDefinition,
        WorkflowStepExecutionConfig,
        WorkflowStepPromptConfig,
        WorkflowTemplateDefinition,
        WorkflowTransition,
    },
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/workflow-builder-catalog", get(get_workflow_builder_catalog))
        .route("/api/workflow-builder/compile", post(compile_workflow_builder))
}

async fn get_workflow_builder_catalog(
    State(_state): State<AppState>,
) -> Result<Json<WorkflowBuilderCatalog>, (axum::http::StatusCode, String)> {
    Ok(Json(default_builder_catalog()))
}

async fn compile_workflow_builder(
    State(state): State<AppState>,
    Json(req): Json<CompileWorkflowBuilderRequest>,
) -> Result<Json<CompileWorkflowBuilderResponse>, (axum::http::StatusCode, String)> {
    let catalog = default_builder_catalog();
    let compiled = compile_document(&state, &catalog, req.document).await?;
    Ok(Json(compiled))
}

async fn compile_document(
    state: &AppState,
    catalog: &WorkflowBuilderCatalog,
    document: WorkflowBuilderDocument,
) -> Result<CompileWorkflowBuilderResponse, (axum::http::StatusCode, String)> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let mut globals = if is_empty_object(&document.globals.resources)
        && is_empty_object(&document.globals.capabilities)
    {
        default_globals()
    } else {
        document.globals
    };

    normalize_global_planner_fragment(state, &mut globals).await.map_err(internal)?;

    let global_state = serde_json::to_value(&globals).map_err(internal)?;
    let repo_ref = globals
        .resources
        .get("repo")
        .and_then(|value| value.get("repo_ref"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let mut steps = Vec::with_capacity(document.stages.len());

    for stage in &document.stages {
        let normalized_stage_type = stage.step_type.trim().to_lowercase();
        let Some(descriptor) = catalog.stage_descriptors.iter().find(|d| d.step_type == stage.step_type || d.step_type == normalized_stage_type) else {
            errors.push(format!("Unknown stage type '{}'", stage.step_type));
            continue;
        };

        match compile_stage(descriptor, stage, &document.stages, &global_state, &repo_ref) {
            Ok(step) => steps.push(step),
            Err(err) => errors.push(err),
        }
    }

    if document.stages.is_empty() {
        warnings.push("Builder document has no stages.".to_string());
    }

    let capability_summary = compile_workflow_capability_summary(&globals, &steps).map_err(internal)?;

    Ok(CompileWorkflowBuilderResponse {
        ok: errors.is_empty(),
        definition: WorkflowTemplateDefinition {
            version: 1,
            globals,
            governance: compile_governance(&catalog, &document.governance)
                .map_err(|err| (axum::http::StatusCode::BAD_REQUEST, err))?,
            steps,
        },
        capability_summary,
        warnings,
        errors,
    })
}

fn field_visibility_value(
    descriptor: &WorkflowStageDescriptor,
    stage: &WorkflowBuilderStageDocument,
    path: &str,
) -> Option<Value> {
    if let Some(value) = stage.field_values.get(path) {
        return Some(value.clone());
    }

    for group in &descriptor.editable_fields {
        for field in &group.fields {
            if field.key == path || field.bind_to == path {
                return stage
                    .field_values
                    .get(&field.key)
                    .cloned()
                    .or_else(|| Some(field.default.clone()));
            }
        }
    }

    None
}

fn field_is_visible(
    descriptor: &WorkflowStageDescriptor,
    stage: &WorkflowBuilderStageDocument,
    field: &WorkflowStageField,
) -> bool {
    field.visible_when.iter().all(|condition| {
        field_visibility_value(descriptor, stage, &condition.path)
            .map(|value| value == condition.equals)
            .unwrap_or(false)
    })
}

fn compile_stage(
    descriptor: &WorkflowStageDescriptor,
    stage: &WorkflowBuilderStageDocument,
    _all_stages: &[WorkflowBuilderStageDocument],
    global_state: &Value,
    repo_ref: &str,
) -> Result<WorkflowStepDefinition, String> {
    let mut step_value = serde_json::to_value(&descriptor.definition_template).map_err(|err| err.to_string())?;
    set_path(&mut step_value, "id", Value::String(stage.id.clone()))?;
    set_path(&mut step_value, "name", Value::String(stage.name.clone()))?;

    for group in &descriptor.editable_fields {
        for field in &group.fields {
            if !field_is_visible(descriptor, stage, field) {
                continue;
            }

            let value = stage
                .field_values
                .get(&field.key)
                .cloned()
                .unwrap_or_else(|| field.default.clone());
            set_path(&mut step_value, &field.bind_to, value).map_err(|err| format!("{}: {}", stage.step_type, err))?;
        }
    }

    let mut step: WorkflowStepDefinition = serde_json::from_value(step_value).map_err(|err| err.to_string())?;
    planner::normalize_planner_features(&mut step, global_state, repo_ref);
    normalize_compile_commands_from_text(&mut step);
    Ok(step)
}

async fn normalize_global_planner_fragment(
    state: &AppState,
    globals: &mut WorkflowGlobalConfig,
) -> Result<(), String> {
    let repo_ref = globals
        .resources
        .get("repo")
        .and_then(|value| value.get("repo_ref"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let mut global_state = serde_json::to_value(&*globals).map_err(|err| err.to_string())?;
    planner::apply_repo_planner_capability(&state.db, &mut global_state, &repo_ref)
        .await
        .map_err(|err| err.to_string())?;

    if let Some(capabilities) = global_state.get("capabilities").cloned() {
        globals.capabilities = capabilities;
    }

    Ok(())
}

fn normalize_compile_commands_from_text(step: &mut WorkflowStepDefinition) {
    if step.step_type != "compile" {
        return;
    }

    let Some(commands_text) = step
        .execution
        .compile_checks
        .get("commands_text")
        .and_then(Value::as_str)
    else {
        return;
    };

    let commands = commands_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|command| Value::String(command.to_string()))
        .collect::<Vec<_>>();

    if commands.is_empty() {
        return;
    }

    if let Some(obj) = step.execution.compile_checks.as_object_mut() {
        obj.insert("commands".to_string(), Value::Array(commands));
    }
}

fn compile_workflow_capability_summary(
    globals: &WorkflowGlobalConfig,
    steps: &[WorkflowStepDefinition],
) -> anyhow::Result<Vec<WorkflowCapabilitySummaryItem>> {
    let global_state = serde_json::to_value(globals)?;
    let repo_ref = globals
        .resources
        .get("repo")
        .and_then(|value| value.get("repo_ref"))
        .and_then(Value::as_str)
        .unwrap_or("");

    let mut by_key: std::collections::BTreeMap<String, WorkflowCapabilitySummaryItem> = std::collections::BTreeMap::new();

    for step in steps {
        let local_state = materialize_builder_stage_state(step);
        let plan = resolve_builder_effective_execution_plan(&global_state, repo_ref, step, &local_state)?;
        let mut stage_keys = plan
            .into_iter()
            .filter(|node| node.enabled && node.kind == StageExecutionNodeKind::Capability)
            .map(|node| node.key)
            .collect::<Vec<_>>();

        stage_keys.sort();
        stage_keys.dedup();

        if builder_stage_uses_inference(step) && !stage_keys.iter().any(|key| key == "context_export") {
            stage_keys.push("context_export".to_string());
            stage_keys.sort();
            stage_keys.dedup();
        }

        for key in stage_keys {
            let entry = by_key.entry(key.clone()).or_insert_with(|| WorkflowCapabilitySummaryItem {
                key: key.clone(),
                stage_ids: Vec::new(),
                stage_types: Vec::new(),
            });

            if !entry.stage_ids.iter().any(|item| item == &step.id) {
                entry.stage_ids.push(step.id.clone());
            }
            if !entry.stage_types.iter().any(|item| item == &step.step_type) {
                entry.stage_types.push(step.step_type.clone());
            }
        }
    }

    Ok(by_key.into_values().collect())
}

fn builder_stage_uses_inference(step: &WorkflowStepDefinition) -> bool {
    step.step_type == "design"
        || step.step_type == "code"
        || step.execution_plan.iter().any(|node| node.key == "inference")
        || step
            .execution_logic
            .get("connections")
            .and_then(|v| v.get("inference"))
            .is_some()
}

fn materialize_builder_stage_state(step: &WorkflowStepDefinition) -> Value {
    json!({
        "execution": step.execution,
        "execution_logic": step.execution_logic,
        "prompt": step.prompt,
        "config": step.config,
    })
}

fn resolve_builder_effective_execution_plan(
    global_state: &Value,
    repo_ref: &str,
    step: &WorkflowStepDefinition,
    local_state: &Value,
) -> anyhow::Result<Vec<StageExecutionNode>> {
    match step.step_type.as_str() {
        "code" => build_inference_execution_plan(
            repo_ref,
            global_state,
            step,
            local_state,
            InferenceStageSettings {
                include_changeset_schema: step.prompt.include_changeset_schema,
            },
        ),
        "design" => build_inference_execution_plan(
            repo_ref,
            global_state,
            step,
            local_state,
            InferenceStageSettings {
                include_changeset_schema: false,
            },
        ),
        "compile" => Ok(vec![StageExecutionNode {
            kind: StageExecutionNodeKind::Capability,
            key: "compile_commands".to_string(),
            enabled: true,
            config: json!({}),
            input_mapping: json!({}),
            output_mapping: json!({}),
            run_after: vec![],
            condition: Value::Null,
        }]),
        _ => {
            if !step.execution_plan.is_empty() {
                Ok(step.execution_plan.clone())
            } else {
                Ok(synthesize_execution_plan(&step.capabilities))
            }
        }
    }
}

fn synthesize_execution_plan(bindings: &[WorkflowCapabilityBinding]) -> Vec<StageExecutionNode> {
    bindings
        .iter()
        .filter(|binding| binding.enabled)
        .map(|binding| StageExecutionNode {
            kind: StageExecutionNodeKind::Capability,
            key: binding.capability.clone(),
            enabled: true,
            config: binding.config.clone(),
            input_mapping: binding.input_mapping.clone(),
            output_mapping: binding.output_mapping.clone(),
            run_after: Vec::new(),
            condition: Value::Null,
        })
        .collect()
}


fn capability_keys_for_stage_definition(step: &WorkflowStepDefinition) -> Vec<String> {
    let mut out: Vec<String> = step
        .execution_plan
        .iter()
        .filter(|node| node.enabled && node.kind == StageExecutionNodeKind::Capability)
        .map(|node| node.key.clone())
        .collect();

    if out.is_empty() {
        out = step
            .capabilities
            .iter()
            .filter(|binding| binding.enabled)
            .map(|binding| binding.capability.clone())
            .collect();
    }

    out.sort();
    out.dedup();
    out
}

fn applicable_governance_policies(
    descriptor: &WorkflowStageDescriptor,
    step: &WorkflowStepDefinition,
) -> Vec<WorkflowGovernancePolicyDescriptor> {
    let capabilities = capability_keys_for_stage_definition(step);
    descriptor
        .available_governance_policies
        .iter()
        .filter(|policy| {
            policy.required_capabilities.is_empty()
                || policy
                    .required_capabilities
                    .iter()
                    .all(|required| capabilities.iter().any(|item| item == required))
        })
        .cloned()
        .collect()
}

fn compile_governance(catalog: &WorkflowBuilderCatalog, governance: &Value) -> Result<Value, String> {
    let mut available = std::collections::BTreeMap::new();
    for descriptor in &catalog.stage_descriptors {
        for policy in &descriptor.available_governance_policies {
            available.entry(policy.key.clone()).or_insert_with(|| policy.clone());
        }
    }

    let Some(governance_obj) = governance.as_object() else {
        return Ok(json!({}));
    };

    let mut compiled = json!({});
    for (policy_key, selected_config) in governance_obj {
        let Some(policy_descriptor) = available.get(policy_key) else {
            return Err(format!("governance policy '{}' is not available", policy_key));
        };

        let mut config = json!({});
        for field in &policy_descriptor.fields {
            let selected_value = selected_config
                .get(&field.key)
                .cloned()
                .unwrap_or_else(|| field.default.clone());
            set_path(&mut config, &field.key, selected_value)
                .map_err(|err| format!("governance policy '{}': {}", policy_key, err))?;
        }
        set_path(&mut compiled, policy_key, config)?;
    }

    Ok(compiled)
}

fn default_builder_catalog() -> WorkflowBuilderCatalog {
    WorkflowBuilderCatalog {
        version: 2,
        stage_descriptors: vec![
            design_descriptor(),
            code_descriptor(),
            compile_descriptor(),
            review_descriptor(),
            sap_import_descriptor(),
            sap_syntax_descriptor(),
            sap_export_descriptor(),
        ],
    }
}

fn changeset_governance_policy_descriptor() -> WorkflowGovernancePolicyDescriptor {
    WorkflowGovernancePolicyDescriptor {
        key: "changeset_file_failures".to_string(),
        label: "Changeset file failure guardrail".to_string(),
        description: "Inject targeted file context after repeated changeset failures, escalate to broad context if failures continue, and pause after too many consecutive failures for the same file.".to_string(),
        capability: "gateway_model/changeset".to_string(),
        required_capabilities: vec!["gateway_model/changeset".to_string()],
        fields: vec![
            WorkflowStageField {
                key: "inject_context_after_consecutive_failures".to_string(),
                label: "Inject file context after failures".to_string(),
                field_type: "integer".to_string(),
                bind_to: "inject_context_after_consecutive_failures".to_string(),
                default: json!(4),
                description: "Number of consecutive failures for the same file before generating and uploading a targeted context_export for that file.".to_string(),
                required: false,
                options: Vec::new(),
                visible_when: Vec::new(),
                ui: field_ui("number"),
            },
            WorkflowStageField {
                key: "inject_broad_context_after_consecutive_failures".to_string(),
                label: "Inject broad context after failures".to_string(),
                field_type: "integer".to_string(),
                bind_to: "inject_broad_context_after_consecutive_failures".to_string(),
                default: json!(5),
                description: "Number of consecutive failures for the same file before escalating from targeted file context to a broader context_export.".to_string(),
                required: false,
                options: Vec::new(),
                visible_when: Vec::new(),
                ui: field_ui("number"),
            },
            WorkflowStageField {
                key: "pause_after_consecutive_failures".to_string(),
                label: "Pause after failures".to_string(),
                field_type: "integer".to_string(),
                bind_to: "pause_after_consecutive_failures".to_string(),
                default: json!(8),
                description: "Number of consecutive failures for the same file before pausing the workflow.".to_string(),
                required: false,
                options: Vec::new(),
                visible_when: Vec::new(),
                ui: field_ui("number"),
            },
        ],
    }
}

fn compile_governance_policy_descriptor() -> WorkflowGovernancePolicyDescriptor {
    WorkflowGovernancePolicyDescriptor {
        key: "compile_failures".to_string(),
        label: "Compile failure guardrail".to_string(),
        description: "Pause after repeated consecutive compile failures.".to_string(),
        capability: "compile_commands".to_string(),
        required_capabilities: vec!["compile_commands".to_string()],
        fields: vec![WorkflowStageField {
            key: "pause_after_consecutive_failures".to_string(),
            label: "Pause after failures".to_string(),
            field_type: "integer".to_string(),
            bind_to: "pause_after_consecutive_failures".to_string(),
            default: json!(5),
            description: "Number of consecutive compile failures before pausing the workflow.".to_string(),
            required: false,
            options: Vec::new(),
            visible_when: Vec::new(),
            ui: field_ui("number"),
        }],
    }
}

fn default_globals() -> WorkflowGlobalConfig {
    WorkflowGlobalConfig {
        resources: json!({
            "repo": {
                "repo_ref": "",
                "git_ref": "WORKTREE"
            }
        }),
        capabilities: json!({
            "inference": {
            },
            "context_export": {
                "enabled": false,
                "save_path": "/tmp/repo_context.txt"
            },
            "changeset_schema": {
                "enabled": false
            },
            "gateway_model/changeset": {},
            "compile_commands": {
                "commands": []
            },
            "planner": {
                "fragment_armed": false,
                "schema_armed": false,
                "auto_apply_armed": false,
                "selected_feature_id": null,
                "supervisor_run_id": null,
                "schema_id": "supervisor_feature_plan_item_v1",
                "preserve_rough_definition": true
            },
            "sap/import": {},
            "sap/export": {}
        }),
        automation: json!({}),
    }
}

fn base_stage_template(step_type: &str, label: &str, automation_mode: AutomationMode) -> WorkflowStepDefinition {
    WorkflowStepDefinition {
        id: step_type.to_string(),
        name: label.to_string(),
        step_type: step_type.to_string(),
        automation_mode: automation_mode.clone(),
        execution: WorkflowStepExecutionConfig::default(),
        prompt: WorkflowStepPromptConfig {
            include_repo_context: false,
            include_changeset_schema: false,
            include_user_context: true,
        },
        config: json!({}),
        capabilities: Vec::<WorkflowCapabilityBinding>::new(),
        execution_logic: json!({}),
        execution_plan: Vec::<StageExecutionNode>::new(),
        transitions: Vec::<WorkflowTransition>::new(),
        advancement: WorkflowStepAdvancementConfig {
            mode: Some(match automation_mode {
                AutomationMode::Manual => "manual".to_string(),
                AutomationMode::Assisted => "assisted".to_string(),
                AutomationMode::Automatic => "automatic".to_string(),
            }),
            auto_run_on_enter: matches!(automation_mode, AutomationMode::Automatic),
            auto_advance_on_success: matches!(automation_mode, AutomationMode::Automatic),
            auto_advance_on_error: false,
            auto_advance_on_paused: false,
        },
    }
}

fn design_descriptor() -> WorkflowStageDescriptor {
    let mut template = base_stage_template("design", "Design", AutomationMode::Manual);
    template.prompt = WorkflowStepPromptConfig {
        include_repo_context: true,
        include_changeset_schema: false,
        include_user_context: true,
    };
    template.config = json!({});
    template.execution_logic = json!({
        "kind": "design_stage_policy",
        "connection_bundles": ["design_code_inference_default"],
        "connections": {
            "inference": {
                "repo_context": {}
            }
        },
        "structured_output": {
            "fine_feature_format_armed": false,
            "auto_normalize_and_apply_to_planner": false,
            "preserve_rough_definition": true,
            "schema_id": "supervisor_feature_plan_item_v1",
            "apply_handler": "supervisor_planner_item"
        }
    });
    template.execution_plan = vec![
        capability_node("context_export"),
        capability_node_after("inference", vec!["context_export"]),
        StageExecutionNode {
            kind: StageExecutionNodeKind::Capability,
            key: "gateway_model/changeset".to_string(),
            enabled: true,
            config: json!({}),
            input_mapping: json!({}),
            output_mapping: json!({}),
            run_after: vec!["inference".to_string()],
            condition: json!({
                "path": "execution_logic.automation.auto_apply_changeset",
                "equals": true
            }),
        },
    ];

    WorkflowStageDescriptor {
        step_type: "design".to_string(),
        label: "Design".to_string(),
        category: "core".to_string(),
        description: "Design stage driven by backend stage descriptor defaults.".to_string(),
        definition_template: template,
        editable_fields: vec![WorkflowStageFieldGroup {
            key: "design".to_string(),
            label: "Design".to_string(),
            fields: vec![
                text_field("prompt.user_input", "User input", "prompt.user_input", ""),
            ],
        }],
        available_governance_policies: vec![],
        routes: default_routes("code", "design", "design"),
    }
}

fn code_descriptor() -> WorkflowStageDescriptor {
    let mut template = base_stage_template("code", "Code", AutomationMode::Automatic);
    template.prompt = WorkflowStepPromptConfig {
        include_repo_context: true,
        include_changeset_schema: true,
        include_user_context: true,
    };
    template.execution = WorkflowStepExecutionConfig {
        changeset_apply: json!({
            "enabled": true
        }),
        compile_checks: json!({}),
    };
    template.execution_logic = json!({
        "kind": "code_stage_policy",
        "connection_bundles": ["design_code_inference_default"],
        "connections": {
            "inference": {
                "repo_context": {},
                "changeset_schema": {}
            }
        },
        "automation": {
            "auto_apply_changeset": true
        }
    });
    template.execution_plan = vec![
        capability_node("context_export"),
        capability_node_after("inference", vec!["context_export"]),
        StageExecutionNode {
            kind: StageExecutionNodeKind::Capability,
            key: "gateway_model/changeset".to_string(),
            enabled: true,
            config: json!({}),
            input_mapping: json!({}),
            output_mapping: json!({}),
            run_after: vec!["inference".to_string()],
            condition: json!({
                "path": "execution_logic.automation.auto_apply_changeset",
                "equals": true
            }),
        },
    ];

    WorkflowStageDescriptor {
        step_type: "code".to_string(),
        label: "Code".to_string(),
        category: "core".to_string(),
        description: "Code stage with backend-owned execution plan and automation defaults.".to_string(),
        definition_template: template,
        editable_fields: vec![WorkflowStageFieldGroup {
            key: "code".to_string(),
            label: "Code".to_string(),
            fields: vec![
                text_field("prompt.user_input", "User input", "prompt.user_input", ""),
                bool_field("automation.auto_apply_changeset", "Auto apply changeset", "execution_logic.automation.auto_apply_changeset", true),
            ],
        }],
        available_governance_policies: vec![
            changeset_governance_policy_descriptor(),
        ],
        routes: default_routes("compile", "code", "code"),
    }
}

fn compile_descriptor() -> WorkflowStageDescriptor {
    let mut template = base_stage_template("compile", "Compile", AutomationMode::Automatic);
    template.execution = WorkflowStepExecutionConfig {
        changeset_apply: json!({}),
        compile_checks: json!({
            "commands": []
        }),
    };
    template.execution_logic = json!({
        "kind": "compile_stage_policy",
        "automation": {
            "run_compile_checks": true
        }
    });
    template.execution_plan = vec![capability_node("compile_commands")];

    WorkflowStageDescriptor {
        step_type: "compile".to_string(),
        label: "Compile".to_string(),
        category: "core".to_string(),
        description: "Compile stage with backend-defined compile execution behavior.".to_string(),
        definition_template: template,
        editable_fields: vec![WorkflowStageFieldGroup {
            key: "compile".to_string(),
            label: "Compile".to_string(),
            fields: vec![
                text_field("execution.compile_checks.commands_text", "Compile commands", "execution.compile_checks.commands_text", ""),
            ],
        }],
        available_governance_policies: vec![compile_governance_policy_descriptor()],
        routes: default_routes("review", "compile", "compile"),
    }
}

fn review_descriptor() -> WorkflowStageDescriptor {
    let mut template = base_stage_template("review", "Review", AutomationMode::Manual);
    template.execution_logic = json!({
        "kind": "review_stage_policy",
        "require_manual_approval": true
    });
    template.execution_plan = vec![];

    WorkflowStageDescriptor {
        step_type: "review".to_string(),
        label: "Review".to_string(),
        category: "core".to_string(),
        description: "Review stage with backend-owned approval policy.".to_string(),
        definition_template: template,
        editable_fields: vec![WorkflowStageFieldGroup {
            key: "review".to_string(),
            label: "Review".to_string(),
            fields: vec![
                bool_field("execution_logic.require_manual_approval", "Require manual approval", "execution_logic.require_manual_approval", true),
            ],
        }],
        available_governance_policies: vec![],
        routes: default_routes("", "design", "review"),
    }
}

fn sap_import_descriptor() -> WorkflowStageDescriptor {
    let mut template = base_stage_template("sap_import", "SAP Import", AutomationMode::Automatic);
    template.execution_logic = json!({
        "kind": "sap_import_stage_policy"
    });
    template.execution_plan = vec![capability_node("sap/import")];

    WorkflowStageDescriptor {
        step_type: "sap_import".to_string(),
        label: "SAP Import".to_string(),
        category: "sap".to_string(),
        description: "Import SAP content through backend-owned stage descriptors and compile flow.".to_string(),
        definition_template: template,
        editable_fields: vec![WorkflowStageFieldGroup {
            key: "sap_import".to_string(),
            label: "SAP Import".to_string(),
            fields: vec![
                text_field("package_name", "Package name", "config.sap_import.package_name", ""),
                bool_field("include_subpackages", "Include subpackages", "config.sap_import.include_subpackages", true),
                bool_field("include_xml_artifacts", "Include XML artifacts", "config.sap_import.include_xml_artifacts", false),
                text_field("object_uris_text", "Object URIs", "config.sap_import.object_uris_text", ""),
                text_field("connection.base_url", "ADT base URL", "config.sap_import.connection.base_url", ""),
                text_field("connection.client", "SAP client", "config.sap_import.connection.client", ""),
                text_field("connection.auth_type", "Auth type", "config.sap_import.connection.auth_type", "basic"),
                text_field("connection.username", "Username", "config.sap_import.connection.username", ""),
                text_field("connection.password", "Password", "config.sap_import.connection.password", ""),
                text_field("connection.authorization", "Authorization header", "config.sap_import.connection.authorization", ""),
                text_field("connection.cookie_header", "Cookie header", "config.sap_import.connection.cookie_header", ""),
                text_field("connection.bridge_dir", "ADT bridge dir", "config.sap_import.connection.bridge_dir", "adt-bridge"),
            ],
        }],
        available_governance_policies: vec![],
        routes: default_routes("sap_export", "sap_import", "sap_import"),
    }
}

fn sap_syntax_descriptor() -> WorkflowStageDescriptor {
    let mut template = base_stage_template("sap_syntax", "SAP Syntax", AutomationMode::Automatic);
    template.execution_logic = json!({
        "kind": "sap_syntax_stage_policy"
    });
    template.execution_plan = vec![capability_node_with_config("sap/export", json!({ "mode": "syntax" }))];

    WorkflowStageDescriptor {
        step_type: "sap_syntax".to_string(),
        label: "SAP Syntax".to_string(),
        category: "sap".to_string(),
        description: "Push inactive SAP artifacts and run backend-owned syntax validation against unstaged worktree changes.".to_string(),
        definition_template: template,
        editable_fields: vec![WorkflowStageFieldGroup {
            key: "sap_syntax".to_string(),
            label: "SAP Syntax".to_string(),
            fields: vec![
                text_field("manifest_paths_text", "Manifest paths", "config.sap_syntax.manifest_paths_text", ""),
                text_field("corr_nr", "Transport request", "config.sap_syntax.corr_nr", ""),
                text_field("connection.base_url", "ADT base URL", "config.sap_syntax.connection.base_url", ""),
                text_field("connection.client", "SAP client", "config.sap_syntax.connection.client", ""),
                text_field("connection.auth_type", "Auth type", "config.sap_syntax.connection.auth_type", "basic"),
                text_field("connection.username", "Username", "config.sap_syntax.connection.username", ""),
                text_field("connection.password", "Password", "config.sap_syntax.connection.password", ""),
                text_field("connection.authorization", "Authorization header", "config.sap_syntax.connection.authorization", ""),
                text_field("connection.cookie_header", "Cookie header", "config.sap_syntax.connection.cookie_header", ""),
                text_field("connection.bridge_dir", "ADT bridge dir", "config.sap_syntax.connection.bridge_dir", "adt-bridge"),
            ],
        }],
        available_governance_policies: vec![],
        routes: default_routes("review", "sap_syntax", "sap_syntax"),
    }
}

fn sap_export_descriptor() -> WorkflowStageDescriptor {
    let mut template = base_stage_template("sap_export", "SAP Export", AutomationMode::Automatic);
    template.execution_logic = json!({
        "kind": "sap_export_stage_policy"
    });
    template.execution_plan = vec![capability_node_with_config("sap/export", json!({ "mode": "export" }))];

    WorkflowStageDescriptor {
        step_type: "sap_export".to_string(),
        label: "SAP Export".to_string(),
        category: "sap".to_string(),
        description: "Activate SAP artifacts through backend-owned stage descriptors using unstaged worktree changes.".to_string(),
        definition_template: template,
        editable_fields: vec![WorkflowStageFieldGroup {
            key: "sap_export".to_string(),
            label: "SAP Export".to_string(),
            fields: vec![
                text_field("manifest_paths_text", "Manifest paths", "config.sap_export.manifest_paths_text", ""),
                bool_field("auto_activate", "Auto activate", "config.sap_export.auto_activate", true),
                text_field("corr_nr", "Transport request", "config.sap_export.corr_nr", ""),
                text_field("connection.base_url", "ADT base URL", "config.sap_export.connection.base_url", ""),
                text_field("connection.client", "SAP client", "config.sap_export.connection.client", ""),
                text_field("connection.auth_type", "Auth type", "config.sap_export.connection.auth_type", "basic"),
                text_field("connection.username", "Username", "config.sap_export.connection.username", ""),
                text_field("connection.password", "Password", "config.sap_export.connection.password", ""),
                text_field("connection.authorization", "Authorization header", "config.sap_export.connection.authorization", ""),
                text_field("connection.cookie_header", "Cookie header", "config.sap_export.connection.cookie_header", ""),
                text_field("connection.bridge_dir", "ADT bridge dir", "config.sap_export.connection.bridge_dir", "adt-bridge"),
            ],
        }],
        available_governance_policies: vec![],
        routes: default_routes("", "sap_export", "sap_export"),
    }
}

fn capability_node(key: &str) -> StageExecutionNode {
    StageExecutionNode {
        kind: StageExecutionNodeKind::Capability,
        key: key.to_string(),
        enabled: true,
        config: json!({}),
        input_mapping: json!({}),
        output_mapping: json!({}),
        run_after: Vec::new(),
        condition: Value::Null,
    }
}

fn capability_node_with_config(key: &str, config: Value) -> StageExecutionNode {
    StageExecutionNode {
        kind: StageExecutionNodeKind::Capability,
        key: key.to_string(),
        enabled: true,
        config,
        input_mapping: json!({}),
        output_mapping: json!({}),
        run_after: Vec::new(),
        condition: Value::Null,
    }
}

fn capability_node_after(key: &str, run_after: Vec<&str>) -> StageExecutionNode {
    StageExecutionNode {
        kind: StageExecutionNodeKind::Capability,
        key: key.to_string(),
        enabled: true,
        config: json!({}),
        input_mapping: json!({}),
        output_mapping: json!({}),
        run_after: run_after.into_iter().map(|v| v.to_string()).collect(),
        condition: Value::Null,
    }
}

fn default_routes(on_success: &str, on_error: &str, on_paused: &str) -> Vec<WorkflowStageRoute> {
    vec![
        WorkflowStageRoute {
            key: "on_success".to_string(),
            label: "On success".to_string(),
            description: String::new(),
            target: on_success.to_string(),
            target_required: false,
            allow_terminate: true,
        },
        WorkflowStageRoute {
            key: "on_error".to_string(),
            label: "On error".to_string(),
            description: String::new(),
            target: on_error.to_string(),
            target_required: false,
            allow_terminate: true,
        },
        WorkflowStageRoute {
            key: "on_paused".to_string(),
            label: "On paused".to_string(),
            description: String::new(),
            target: on_paused.to_string(),
            target_required: false,
            allow_terminate: true,
        },
    ]
}

fn field_ui(control: &str) -> WorkflowStageFieldUi {
    WorkflowStageFieldUi {
        control: control.to_string(),
        placeholder: String::new(),
        min_rows: if control == "textarea" { 4 } else { 0 },
        format: String::new(),
    }
}

fn visible_when(mut field: WorkflowStageField, path: &str, equals: Value) -> WorkflowStageField {
    field.visible_when.push(WorkflowStageFieldVisibility {
        path: path.to_string(),
        equals,
    });
    field
}

fn bool_field(key: &str, label: &str, bind_to: &str, default: bool) -> WorkflowStageField {
    WorkflowStageField {
        key: key.to_string(),
        label: label.to_string(),
        field_type: "boolean".to_string(),
        bind_to: bind_to.to_string(),
        default: Value::Bool(default),
        description: String::new(),
        required: false,
        options: Vec::new(),
        visible_when: Vec::new(),
        ui: field_ui("switch"),
    }
}

fn int_field(key: &str, label: &str, bind_to: &str, default: i64) -> WorkflowStageField {
    WorkflowStageField {
        key: key.to_string(),
        label: label.to_string(),
        field_type: "integer".to_string(),
        bind_to: bind_to.to_string(),
        default: Value::Number(default.into()),
        description: String::new(),
        required: false,
        options: Vec::new(),
        visible_when: Vec::new(),
        ui: field_ui("number"),
    }
}

fn text_field(key: &str, label: &str, bind_to: &str, default: &str) -> WorkflowStageField {
    let multiline = key.ends_with("commands_text") || key.ends_with("manifest_paths_text");
    WorkflowStageField {
        key: key.to_string(),
        label: label.to_string(),
        field_type: if multiline {
            "multiline_text".to_string()
        } else {
            "text".to_string()
        },
        bind_to: bind_to.to_string(),
        default: Value::String(default.to_string()),
        description: String::new(),
        required: false,
        options: Vec::new(),
        visible_when: Vec::new(),
        ui: if multiline { field_ui("textarea") } else { field_ui("text") },
    }
}

fn select_field(key: &str, label: &str, bind_to: &str, default: &str, options: Vec<(&str, &str)>) -> WorkflowStageField {
    WorkflowStageField {
        key: key.to_string(),
        label: label.to_string(),
        field_type: "text".to_string(),
        bind_to: bind_to.to_string(),
        default: Value::String(default.to_string()),
        description: String::new(),
        required: true,
        options: options.into_iter().map(|(value, label)| WorkflowStageFieldOption {
            value: value.to_string(),
            label: label.to_string(),
        }).collect(),
        visible_when: Vec::new(),
        ui: field_ui("select"),
    }
}

fn set_path(root: &mut Value, path: &str, value: Value) -> Result<(), String> {
    let parts: Vec<&str> = path.split('.').filter(|part| !part.trim().is_empty()).collect();
    if parts.is_empty() {
        return Err("path cannot be empty".to_string());
    }

    let mut cursor = root;
    for part in &parts[..parts.len() - 1] {
        if !cursor.is_object() {
            *cursor = json!({});
        }
        let obj = cursor.as_object_mut().ok_or_else(|| format!("{} is not an object", part))?;
        cursor = obj.entry((*part).to_string()).or_insert_with(|| json!({}));
    }

    if !cursor.is_object() {
        *cursor = json!({});
    }
    let obj = cursor.as_object_mut().ok_or_else(|| "target is not an object".to_string())?;
    obj.insert(parts[parts.len() - 1].to_string(), value);
    Ok(())
}

fn is_empty_object(value: &Value) -> bool {
    value.as_object().map(|obj| obj.is_empty()).unwrap_or(true)
}

fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
