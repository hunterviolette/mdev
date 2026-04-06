use axum::{extract::State, routing::{get, post}, Json, Router};
use serde_json::{json, Value};

use crate::{
    app_state::AppState,
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
        WorkflowGlobalConfig,
        WorkflowStageDescriptor,
        WorkflowStageField,
        WorkflowStageFieldGroup,
        WorkflowStageFieldUi,
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
    State(_state): State<AppState>,
    Json(req): Json<CompileWorkflowBuilderRequest>,
) -> Result<Json<CompileWorkflowBuilderResponse>, (axum::http::StatusCode, String)> {
    let catalog = default_builder_catalog();
    let compiled = compile_document(&catalog, req.document)?;
    Ok(Json(compiled))
}

fn compile_document(
    catalog: &WorkflowBuilderCatalog,
    document: WorkflowBuilderDocument,
) -> Result<CompileWorkflowBuilderResponse, (axum::http::StatusCode, String)> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut steps = Vec::with_capacity(document.stages.len());

    for stage in &document.stages {
        let Some(descriptor) = catalog.stage_descriptors.iter().find(|d| d.step_type == stage.step_type) else {
            errors.push(format!("Unknown stage type '{}'", stage.step_type));
            continue;
        };

        match compile_stage(descriptor, stage, &document.stages) {
            Ok(step) => steps.push(step),
            Err(err) => errors.push(err),
        }
    }

    if document.stages.is_empty() {
        warnings.push("Builder document has no stages.".to_string());
    }

    Ok(CompileWorkflowBuilderResponse {
        ok: errors.is_empty(),
        definition: WorkflowTemplateDefinition {
            version: 1,
            globals: if is_empty_object(&document.globals.resources) && is_empty_object(&document.globals.capabilities) {
                default_globals()
            } else {
                document.globals
            },
            steps,
        },
        warnings,
        errors,
    })
}

fn compile_stage(
    descriptor: &WorkflowStageDescriptor,
    stage: &WorkflowBuilderStageDocument,
    _all_stages: &[WorkflowBuilderStageDocument],
) -> Result<WorkflowStepDefinition, String> {
    let mut step_value = serde_json::to_value(&descriptor.definition_template).map_err(|err| err.to_string())?;
    set_path(&mut step_value, "id", Value::String(stage.id.clone()))?;
    set_path(&mut step_value, "name", Value::String(stage.name.clone()))?;

    for group in &descriptor.editable_fields {
        for field in &group.fields {
            let value = stage
                .field_values
                .get(&field.key)
                .cloned()
                .unwrap_or_else(|| field.default.clone());
            set_path(&mut step_value, &field.bind_to, value).map_err(|err| format!("{}: {}", stage.step_type, err))?;
        }
    }

    serde_json::from_value(step_value).map_err(|err| err.to_string())
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
            sap_export_descriptor(),
        ],
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
            "inference": {},
            "context_export": {
                "save_path": "/tmp/repo_context.txt"
            },
            "changeset_schema": {},
            "gateway_model/changeset": {},
            "compile_commands": {},
            "sap/import": {},
            "sap/export": {}
        }),
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
    template.execution_logic = json!({
        "kind": "design_stage_policy",
        "automation": {
            "inject_context": true
        }
    });
    template.execution_plan = vec![
        capability_node("context_export"),
        capability_node_after("inference", vec!["context_export"]),
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
                bool_field("automation.inject_context", "Inject context", "execution_logic.automation.inject_context", true),
            ],
        }],
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
            "enabled": true,
            "max_consecutive_failures": 1
        }),
        compile_checks: json!({}),
    };
    template.execution_logic = json!({
        "kind": "code_stage_policy",
        "automation": {
            "inject_context": true,
            "inject_changeset_schema": true,
            "include_apply_error": true,
            "include_compile_error": true,
            "auto_apply_changeset": true,
            "max_consecutive_apply_failures": 1
        }
    });
    template.execution_plan = vec![
        capability_node("context_export"),
        capability_node_after("inference", vec!["context_export"]),
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
                bool_field("automation.inject_context", "Inject context", "execution_logic.automation.inject_context", true),
                bool_field("automation.inject_changeset_schema", "Inject changeset schema", "execution_logic.automation.inject_changeset_schema", true),
                bool_field("automation.include_apply_error", "Include apply errors", "execution_logic.automation.include_apply_error", true),
                bool_field("automation.include_compile_error", "Include compile errors", "execution_logic.automation.include_compile_error", true),
                bool_field("automation.auto_apply_changeset", "Auto apply changeset", "execution_logic.automation.auto_apply_changeset", true),
                int_field("automation.max_consecutive_apply_failures", "Max consecutive apply failures", "execution_logic.automation.max_consecutive_apply_failures", 1),
            ],
        }],
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
        routes: default_routes("sap_export", "sap_import", "sap_import"),
    }
}

fn sap_export_descriptor() -> WorkflowStageDescriptor {
    let mut template = base_stage_template("sap_export", "SAP Export", AutomationMode::Automatic);
    template.execution_logic = json!({
        "kind": "sap_export_stage_policy"
    });
    template.execution_plan = vec![capability_node("sap/export")];

    WorkflowStageDescriptor {
        step_type: "sap_export".to_string(),
        label: "SAP Export".to_string(),
        category: "sap".to_string(),
        description: "Export SAP artifacts through backend-owned stage descriptors and compile flow.".to_string(),
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
        routes: default_routes("review", "sap_export", "sap_export"),
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
        ui: if multiline { field_ui("textarea") } else { field_ui("text") },
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
