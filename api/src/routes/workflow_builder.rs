use axum::{extract::State, routing::{get, post}, Json, Router};
use serde_json::{json, Value};
use crate::engine::capabilities::inference::panel::{build_inference_config_panel, inference_config_from_panel, InferenceConfigPanel};

use crate::{
    app_state::AppState,
    engine::stages,
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
        .route("/api/workflow-builder/inference-panel", post(build_workflow_builder_inference_panel))
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

#[derive(Debug, Clone, serde::Deserialize)]
struct InferencePanelRequest {
    definition: WorkflowTemplateDefinition,
    #[serde(default)]
    globals: WorkflowGlobalConfig,
    #[serde(default)]
    panel: Option<InferenceConfigPanel>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct InferencePanelResponse {
    ok: bool,
    panel: InferenceConfigPanel,
    inference: Value,
}

async fn build_workflow_builder_inference_panel(
    Json(req): Json<InferencePanelRequest>,
) -> Result<Json<InferencePanelResponse>, (axum::http::StatusCode, String)> {
    let inference = req
        .panel
        .clone()
        .map(|panel| inference_config_from_panel(&req.globals, panel))
        .unwrap_or_else(|| req.globals.capabilities.get("inference").cloned().unwrap_or_else(|| json!({})));
    let panel = req
        .panel
        .unwrap_or_else(|| build_inference_config_panel(&req.definition, &req.globals));

    Ok(Json(InferencePanelResponse {
        ok: true,
        panel,
        inference,
    }))
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
    normalize_shared_dependencies(&mut globals);

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

    normalize_qa_environment(&mut globals, &steps);

    let capability_summary = compile_workflow_capability_summary(&globals, &steps).map_err(internal)?;

    Ok(CompileWorkflowBuilderResponse {
        ok: errors.is_empty(),
        definition: WorkflowTemplateDefinition {
            version: 1,
            globals,
            governance: compile_governance(&catalog, &steps, &document.governance)
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

    normalize_qa_readiness_discriminators(&mut step_value);

    let mut step: WorkflowStepDefinition = serde_json::from_value(step_value).map_err(|err| err.to_string())?;
    planner::normalize_planner_features(&mut step, global_state, repo_ref);
    normalize_compile_commands_from_text(&mut step);
    Ok(step)
}

fn normalize_qa_readiness_discriminators(step: &mut Value) {
    let Some(services) = step
        .get_mut("execution")
        .and_then(|value| value.get_mut("qa"))
        .and_then(|value| value.get_mut("environment"))
        .and_then(|value| value.get_mut("services"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };

    for service in services {
        let Some(readiness) = service
            .get_mut("readiness")
            .and_then(Value::as_object_mut)
        else {
            continue;
        };

        if !readiness.contains_key("kind") {
            if let Some(value) = readiness.remove("type") {
                readiness.insert("kind".to_string(), value);
            } else {
                readiness.insert("kind".to_string(), Value::String("http".to_string()));
            }
        } else {
            readiness.remove("type");
        }
    }
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

        if builder_stage_uses_automation(&global_state, step)
            && !stage_keys.iter().any(|key| key == "automation")
        {
            stage_keys.push("automation".to_string());
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

fn builder_stage_uses_automation(
    global_state: &Value,
    step: &WorkflowStepDefinition,
) -> bool {
    let profile = crate::engine::capabilities::automation::profile_from_global_state(global_state);

    profile.enabled
        && profile.new_session.iter().any(|capability| {
            crate::engine::stages::stage_supports_capability(step, capability)
        })
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
        "compile" => Ok(vec![
            StageExecutionNode {
                kind: StageExecutionNodeKind::Capability,
                key: "shared_dependencies".to_string(),
                enabled: true,
                config: json!({}),
                input_mapping: json!({}),
                output_mapping: json!({}),
                run_after: vec![],
                condition: Value::Null,
            },
            StageExecutionNode {
                kind: StageExecutionNodeKind::Capability,
                key: "compile_commands".to_string(),
                enabled: true,
                config: json!({}),
                input_mapping: json!({}),
                output_mapping: json!({}),
                run_after: vec!["shared_dependencies".to_string()],
                condition: Value::Null,
            },
        ]),
        "qa" => Ok(vec![
            StageExecutionNode {
                kind: StageExecutionNodeKind::Capability,
                key: "shared_dependencies".to_string(),
                enabled: true,
                config: json!({}),
                input_mapping: json!({}),
                output_mapping: json!({}),
                run_after: vec![],
                condition: Value::Null,
            },
            StageExecutionNode {
                kind: StageExecutionNodeKind::Capability,
                key: "qa_environment".to_string(),
                enabled: true,
                config: json!({}),
                input_mapping: json!({}),
                output_mapping: json!({}),
                run_after: vec!["shared_dependencies".to_string()],
                condition: Value::Null,
            },
        ]),
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


fn compile_governance(
    catalog: &WorkflowBuilderCatalog,
    steps: &[WorkflowStepDefinition],
    governance: &Value,
) -> Result<Value, String> {
    let mut available = std::collections::BTreeMap::new();
    for descriptor in &catalog.stage_descriptors {
        if !steps.iter().any(|step| step.step_type == descriptor.step_type) {
            continue;
        }

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

pub(crate) fn normalize_shared_dependencies(globals: &mut WorkflowGlobalConfig) {
    let capability_value = globals
        .capabilities
        .get("shared_dependencies")
        .cloned();

    let source = capability_value
        .or_else(|| serde_json::to_value(&globals.shared_dependencies).ok())
        .unwrap_or_else(|| json!({
            "enabled": false,
            "providers": []
        }));

    let enabled = source
        .get("enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    let providers = source
        .get("providers")
        .and_then(|value| value.as_array())
        .map(|providers| {
            providers
                .iter()
                .filter_map(|provider| {
                    let provider = provider.as_object()?;
                    let id = provider.get("id")?.as_str()?.trim();
                    let ecosystem = provider.get("ecosystem")?.as_str()?.trim();
                    let root = provider
                        .get("root")
                        .and_then(|value| value.as_str())
                        .unwrap_or(".")
                        .trim();

                    if id.is_empty() || ecosystem != "node" {
                        return None;
                    }

                    let label = provider
                        .get("label")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or(id);

                    let manifests = provider
                        .get("manifests")
                        .and_then(|value| value.as_array())
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(|value| value.as_str())
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                                .map(str::to_string)
                                .collect::<Vec<_>>()
                        })
                        .filter(|values| !values.is_empty())
                        .unwrap_or_else(|| vec!["package-lock.json".to_string()]);

                    let isolated = provider
                        .get("isolated")
                        .cloned()
                        .unwrap_or_else(|| json!({
                            "storage_path": "node_modules",
                            "seed_from_trusted": true,
                            "install": {
                                "commands": [],
                                "stop_on_failure": true
                            }
                        }));

                    let mismatch = provider
                        .get("mismatch")
                        .cloned()
                        .unwrap_or_else(|| json!({
                            "disposition": "operator_checkpoint",
                            "allowed_dispositions": [
                                "create_isolated_dependencies",
                                "continue_trusted_with_warning",
                                "skip_stage"
                            ]
                        }));

                    Some(json!({
                        "id": id,
                        "label": label,
                        "ecosystem": ecosystem,
                        "root": if root.is_empty() { "." } else { root },
                        "manifests": manifests,
                        "isolated": isolated,
                        "mismatch": mismatch
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if !globals.capabilities.is_object() {
        globals.capabilities = json!({});
    }

    if let Some(capabilities) = globals.capabilities.as_object_mut() {
        capabilities.insert(
            "shared_dependencies".to_string(),
            json!({
                "enabled": enabled,
                "providers": providers
            }),
        );
    }

    globals.shared_dependencies = Default::default();
}

pub(crate) fn normalize_qa_environment(
    globals: &mut WorkflowGlobalConfig,
    steps: &[WorkflowStepDefinition],
) {
    if !globals.capabilities.is_object() {
        globals.capabilities = json!({});
    }

    let source = globals
        .capabilities
        .get("qa_environment")
        .cloned()
        .or_else(|| {
            steps
                .iter()
                .find(|step| step.step_type == "qa")
                .and_then(|step| step.execution.qa.as_ref())
                .and_then(|qa| serde_json::to_value(qa).ok())
        });

    let Some(source) = source else {
        if let Some(capabilities) = globals.capabilities.as_object_mut() {
            capabilities.remove("qa_environment");
        }
        return;
    };

    let environment = source.get("environment").unwrap_or(&source);
    let port_range = environment
        .get("port_range")
        .cloned()
        .unwrap_or_else(|| json!({ "start": 24000, "end": 24999 }));
    let hostname_template = environment
        .get("hostname_template")
        .and_then(Value::as_str)
        .unwrap_or("{run}.qa.localhost");
    let shutdown_grace_seconds = environment
        .get("shutdown_grace_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(5);

    let services = environment
        .get("services")
        .and_then(Value::as_array)
        .map(|services| {
            services
                .iter()
                .filter_map(|service| {
                    let id = service.get("id")?.as_str()?;
                    let label = service
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or(id);
                    let command_value = service.get("command")?;
                    let command = command_value
                        .as_str()
                        .or_else(|| command_value.get("command").and_then(Value::as_str))?;
                    let working_directory = service
                        .get("working_directory")
                        .and_then(Value::as_str)
                        .or_else(|| {
                            command_value
                                .get("working_directory")
                                .and_then(Value::as_str)
                        })
                        .unwrap_or(".");
                    let command_environment = command_value
                        .get("environment")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    let service_environment = service
                        .get("environment")
                        .cloned()
                        .unwrap_or(command_environment);
                    let port = service.get("port").cloned().unwrap_or_else(|| json!({}));
                    let readiness = service
                        .get("readiness")
                        .cloned()
                        .unwrap_or_else(|| json!({
                            "kind": "http",
                            "path": "/",
                            "timeout_seconds": 60
                        }));

                    Some(json!({
                        "id": id,
                        "label": label,
                        "command": command,
                        "working_directory": working_directory,
                        "environment": service_environment,
                        "port_environment_variable": port
                            .get("environment_variable")
                            .and_then(Value::as_str)
                            .or_else(|| {
                                service
                                    .get("port_environment_variable")
                                    .and_then(Value::as_str)
                            })
                            .unwrap_or("PORT"),
                        "preferred_port": port
                            .get("preferred")
                            .cloned()
                            .or_else(|| service.get("preferred_port").cloned())
                            .unwrap_or(Value::Null),
                        "readiness": readiness,
                        "public": service
                            .get("public")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if let Some(capabilities) = globals.capabilities.as_object_mut() {
        capabilities.insert(
            "qa_environment".to_string(),
            json!({
                "enabled": true,
                "port_range": port_range,
                "hostname_template": hostname_template,
                "services": services,
                "shutdown_grace_seconds": shutdown_grace_seconds
            }),
        );
    }
}

fn governance_policy_descriptor(key: &str) -> Option<WorkflowGovernancePolicyDescriptor> {
    match key {
        "changeset_file_failures" => Some(changeset_governance_policy_descriptor()),
        "compile_failures" => Some(compile_governance_policy_descriptor()),
        _ => None,
    }
}

fn default_builder_catalog() -> WorkflowBuilderCatalog {
    let mut stage_descriptors = stages::registered_stage_descriptors();

    for descriptor in &mut stage_descriptors {
        descriptor.available_governance_policies = stages::automation_policy_keys_for_stage_type(&descriptor.step_type)
            .iter()
            .filter_map(|key| governance_policy_descriptor(key))
            .collect();
    }

    WorkflowBuilderCatalog {
        version: 2,
        stage_descriptors,
    }
}

fn changeset_governance_policy_descriptor() -> WorkflowGovernancePolicyDescriptor {
    WorkflowGovernancePolicyDescriptor {
        key: "changeset_file_failures".to_string(),
        label: "Changeset file failure guardrail".to_string(),
        description: "Inject targeted file context after repeated changeset failures, escalate to broad context if failures continue, and pause after too many consecutive failures for the same file.".to_string(),
        capability: "changeset".to_string(),
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
                "default_session": "coding",
                "stage_sessions": {
                    "design": "coding",
                    "code": "coding",
                    "review": "review"
                },
                "sessions": {
                    "coding": {
                        "provider": "openai",
                        "transport": "api",
                        "model": "gpt-4.1",
                        "runtime": {}
                    },
                    "review": {
                        "provider": "openai",
                        "transport": "api",
                        "model": "gpt-4.1",
                        "runtime": {}
                    }
                }
            },
            "context_export": {
                "enabled": false,
                "save_path": "broad_context_file.txt"
            },
            "changeset_schema": {
                "enabled": false
            },
            "gateway_model/changeset": {},
            "compile_commands": {
                "commands": []
            },
            "planner": {
                "planner_id": "",
                "feature_id": "",
                "fragment_armed": false,
                "schema_armed": false,
                "auto_apply_armed": false
            },
            "sap/import": {},
            "sap/export": {}
        }),
        automation: json!({}),
        shared_dependencies: crate::engine::runtime_tools::SharedDependenciesConfig::default(),
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

pub(crate) fn design_descriptor() -> WorkflowStageDescriptor {
    let mut template = base_stage_template("design", "Design", AutomationMode::Automatic);
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
        "automation": {
            "empty_user_input_default": ""
        },
        "structured_output": {
            "fine_feature_format_armed": false,
            "auto_normalize_and_apply_to_planner": false,
            "preserve_rough_definition": true,
            "schema_id": "planner_feature_refinement_v1",
            "apply_handler": "planner_apply"
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
                text_field("automation.empty_user_input_default", "Empty user input default", "execution_logic.automation.empty_user_input_default", ""),
            ],
        }],
        available_governance_policies: vec![],
        routes: default_routes("code", "design", "design"),
    }
}

pub(crate) fn code_descriptor() -> WorkflowStageDescriptor {
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
        ..WorkflowStepExecutionConfig::default()
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
            "auto_apply_changeset": true,
            "empty_user_input_default": ""
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
                text_field("automation.empty_user_input_default", "Empty user input default", "execution_logic.automation.empty_user_input_default", ""),
                bool_field("automation.auto_apply_changeset", "Auto apply changeset", "execution_logic.automation.auto_apply_changeset", true),
            ],
        }],
        available_governance_policies: vec![],
        routes: default_routes("compile", "code", "code"),
    }
}

pub(crate) fn compile_descriptor() -> WorkflowStageDescriptor {
    let mut template = base_stage_template("compile", "Compile", AutomationMode::Automatic);
    template.execution = WorkflowStepExecutionConfig {
        changeset_apply: json!({}),
        compile_checks: json!({
            "commands": []
        }),
        ..WorkflowStepExecutionConfig::default()
    };
    template.execution_logic = json!({
        "kind": "compile_stage_policy",
        "automation": {
            "run_compile_checks": true
        }
    });
    template.execution_plan = vec![
        capability_node("shared_dependencies"),
        capability_node_after("compile_commands", vec!["shared_dependencies"]),
    ];

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
        available_governance_policies: vec![],
        routes: default_routes("review", "compile", "compile"),
    }
}

pub(crate) fn qa_descriptor() -> WorkflowStageDescriptor {
    let template = WorkflowStepDefinition {
        id: "qa-preview".to_string(),
        name: "DeployQA".to_string(),
        step_type: "qa".to_string(),
        automation_mode: AutomationMode::Manual,
        execution: WorkflowStepExecutionConfig {
            qa: Some(crate::engine::runtime_tools::QaStageSpec {
                dependency_providers: Vec::new(),
                environment: crate::engine::runtime_tools::QaEnvironmentSpec {
                    port_range: crate::engine::runtime_tools::PortRangeSpec {
                        start: 24000,
                        end: 24999,
                    },
                    hostname_template: "{run}.qa.localhost".to_string(),
                    prepare: crate::engine::runtime_tools::TerminalSequenceSpec::default(),
                    services: vec![crate::engine::runtime_tools::QaServiceSpec {
                        id: "application".to_string(),
                        label: "Application".to_string(),
                        command: crate::engine::runtime_tools::TerminalCommandSpec {
                            id: "application-dev".to_string(),
                            label: "npm run dev".to_string(),
                            command: "npm run dev".to_string(),
                            arguments: Vec::new(),
                            working_directory: ".".to_string(),
                            environment: Default::default(),
                            shell: crate::engine::runtime_tools::TerminalShell::System,
                            mode: crate::engine::runtime_tools::TerminalCommandMode::Service,
                            timeout_seconds: None,
                            continue_on_error: false,
                        },
                        port: crate::engine::runtime_tools::QaServicePortSpec {
                            environment_variable: "PORT".to_string(),
                            preferred: None,
                        },
                        readiness: crate::engine::runtime_tools::QaReadinessSpec::Http {
                            path: "/".to_string(),
                            expected_status: Some(200),
                            timeout_seconds: 60,
                        },
                        public: true,
                    }],
                    shutdown_grace_seconds: 5,
                },
            }),
            ..WorkflowStepExecutionConfig::default()
        },
        prompt: WorkflowStepPromptConfig::default(),
        config: json!({}),
        capabilities: vec![WorkflowCapabilityBinding {
            capability: "qa_environment".to_string(),
            enabled: true,
            config: json!({}),
            input_mapping: json!({}),
            output_mapping: json!({}),
        }],
        execution_logic: json!({}),
        execution_plan: vec![
            capability_node("shared_dependencies"),
            capability_node_after("qa_environment", vec!["shared_dependencies"]),
        ],
        transitions: Vec::new(),
        advancement: WorkflowStepAdvancementConfig {
            mode: Some("manual".to_string()),
            auto_run_on_enter: false,
            auto_advance_on_success: false,
            auto_advance_on_error: false,
            auto_advance_on_paused: false,
        },
    };

    WorkflowStageDescriptor {
        step_type: "qa".to_string(),
        label: "DeployQA".to_string(),
        category: "validation".to_string(),
        description: "Deploy and manage a QA application with optional shared dependencies, allocated ports, readiness checks, and temporary routing.".to_string(),
        definition_template: template.clone(),
        editable_fields: vec![
            WorkflowStageFieldGroup {
                key: "dependencies".to_string(),
                label: "Dependencies".to_string(),
                fields: vec![WorkflowStageField {
                    key: "dependency_providers".to_string(),
                    label: "Dependency providers".to_string(),
                    field_type: "dependency_providers".to_string(),
                    bind_to: "execution.qa.dependency_providers".to_string(),
                    default: json!([]),
                    description: "Trusted dependency providers resolved before the QA environment starts.".to_string(),
                    required: false,
                    options: Vec::new(),
                    visible_when: Vec::new(),
                    ui: WorkflowStageFieldUi {
                        control: "dependency_providers".to_string(),
                        placeholder: "node-root, cargo-root".to_string(),
                        min_rows: 0,
                        format: String::new(),
                    },
                }],
            },
            WorkflowStageFieldGroup {
                key: "services".to_string(),
                label: "Services".to_string(),
                fields: vec![WorkflowStageField {
                    key: "services".to_string(),
                    label: "Deployment services".to_string(),
                    field_type: "qa_services".to_string(),
                    bind_to: "execution.qa.environment.services".to_string(),
                    default: serde_json::to_value(
                        template
                            .execution
                            .qa
                            .as_ref()
                            .map(|qa| qa.environment.services.clone())
                            .unwrap_or_default(),
                    )
                    .unwrap_or_else(|_| json!([])),
                    description: "Commands, environments, ports, readiness checks, and routing for every deployed service.".to_string(),
                    required: true,
                    options: Vec::new(),
                    visible_when: Vec::new(),
                    ui: WorkflowStageFieldUi {
                        control: "qa_services".to_string(),
                        placeholder: String::new(),
                        min_rows: 0,
                        format: "json".to_string(),
                    },
                }],
            },
            WorkflowStageFieldGroup {
                key: "routing".to_string(),
                label: "Routing".to_string(),
                fields: vec![
                    int_field("port_start", "Port range start", "execution.qa.environment.port_range.start", 24000),
                    int_field("port_end", "Port range end", "execution.qa.environment.port_range.end", 24999),
                    text_field("hostname_template", "Hostname template", "execution.qa.environment.hostname_template", "{run}.qa.localhost"),
                    int_field("shutdown_grace_seconds", "Shutdown grace seconds", "execution.qa.environment.shutdown_grace_seconds", 5),
                ],
            },
        ],
        available_governance_policies: Vec::new(),
        routes: default_routes("", "qa", "qa"),
    }
}

pub(crate) fn merge_patches_descriptor() -> WorkflowStageDescriptor {
    let mut template = base_stage_template("merge_patches", "Merge patches", AutomationMode::Automatic);
    template.prompt = WorkflowStepPromptConfig {
        include_repo_context: false,
        include_changeset_schema: false,
        include_user_context: true,
    };
    template.config = json!({});
    template.execution_logic = json!({
        "kind": "merge_patches_stage_policy",
        "automation": {
            "apply_patches": true
        }
    });
    template.execution_plan = Vec::<StageExecutionNode>::new();

    WorkflowStageDescriptor {
        step_type: "merge_patches".to_string(),
        label: "Merge patches".to_string(),
        category: "core".to_string(),
        description: "Apply supervisor child workflow patches in order against the integration worktree.".to_string(),
        definition_template: template,
        editable_fields: vec![],
        available_governance_policies: vec![],
        routes: default_routes("review", "merge_patches", "merge_patches"),
    }
}

pub(crate) fn review_descriptor() -> WorkflowStageDescriptor {
    let mut template = base_stage_template("review", "Review", AutomationMode::Manual);
    template.execution_logic = json!({
        "kind": "review_stage_policy",
        "require_manual_approval": true,
        "ai_review": {
            "enabled": false
        }
    });
    template.execution_plan = vec![];

    WorkflowStageDescriptor {
        step_type: "review".to_string(),
        label: "Review".to_string(),
        category: "core".to_string(),
        description: "Review stage with backend-owned validation and approval policy.".to_string(),
        definition_template: template,
        editable_fields: vec![WorkflowStageFieldGroup {
            key: "review".to_string(),
            label: "Review".to_string(),
            fields: vec![
                bool_field("execution_logic.require_manual_approval", "Manual approval", "execution_logic.require_manual_approval", true),
                bool_field("execution_logic.ai_review.enabled", "AI review", "execution_logic.ai_review.enabled", false),
            ],
        }],
        available_governance_policies: vec![],
        routes: default_routes("", "code", "review"),
    }
}

pub(crate) fn sap_import_descriptor() -> WorkflowStageDescriptor {
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

pub(crate) fn sap_syntax_descriptor() -> WorkflowStageDescriptor {
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

pub(crate) fn sap_export_descriptor() -> WorkflowStageDescriptor {
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
    let multiline = key.ends_with("commands_text")
        || key.ends_with("manifest_paths_text")
        || key.ends_with("empty_user_input_default");
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
    let parts: Vec<&str> = path
        .split('.')
        .filter(|part| !part.trim().is_empty())
        .collect();

    if parts.is_empty() {
        return Err("path cannot be empty".to_string());
    }

    fn assign(cursor: &mut Value, parts: &[&str], value: Value) -> Result<(), String> {
        let part = parts[0];
        let is_last = parts.len() == 1;

        if let Ok(index) = part.parse::<usize>() {
            if !cursor.is_array() {
                *cursor = Value::Array(Vec::new());
            }

            let array = cursor
                .as_array_mut()
                .ok_or_else(|| format!("{} is not an array", part))?;

            while array.len() <= index {
                array.push(Value::Null);
            }

            if is_last {
                array[index] = value;
                return Ok(());
            }

            let next_is_index = parts[1].parse::<usize>().is_ok();
            if array[index].is_null() {
                array[index] = if next_is_index {
                    Value::Array(Vec::new())
                } else {
                    json!({})
                };
            }

            return assign(&mut array[index], &parts[1..], value);
        }

        if !cursor.is_object() {
            *cursor = json!({});
        }

        let object = cursor
            .as_object_mut()
            .ok_or_else(|| format!("{} is not an object", part))?;

        if is_last {
            object.insert(part.to_string(), value);
            return Ok(());
        }

        let next_is_index = parts[1].parse::<usize>().is_ok();
        let child = object.entry(part.to_string()).or_insert_with(|| {
            if next_is_index {
                Value::Array(Vec::new())
            } else {
                json!({})
            }
        });

        if next_is_index && !child.is_array() {
            *child = Value::Array(Vec::new());
        } else if !next_is_index && !child.is_object() {
            *child = json!({});
        }

        assign(child, &parts[1..], value)
    }

    assign(root, &parts, value)
}

fn is_empty_object(value: &Value) -> bool {
    value.as_object().map(|obj| obj.is_empty()).unwrap_or(true)
}

fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
