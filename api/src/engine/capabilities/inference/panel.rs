use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::models::{StageExecutionNodeKind, WorkflowGlobalConfig, WorkflowTemplateDefinition};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InferenceConfigPanel {
    #[serde(default)]
    pub sessions: Vec<InferenceConfigPanelSession>,
    #[serde(default)]
    pub stage_mappings: Vec<InferenceConfigPanelStageMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceConfigPanelSession {
    pub name: String,
    pub transport: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub browser_url: Option<String>,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceConfigPanelStageMapping {
    pub stage_type: String,
    pub session: String,
}

pub fn build_inference_config_panel(
    definition: &WorkflowTemplateDefinition,
    globals: &WorkflowGlobalConfig,
) -> InferenceConfigPanel {
    let inference = globals
        .capabilities
        .get("inference")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let inference_obj = inference.as_object();
    let sessions_obj = inference_obj
        .and_then(|value| value.get("sessions"))
        .and_then(Value::as_object);

    let mut sessions = Vec::new();
    let mut session_names = BTreeSet::new();

    if let Some(sessions_obj) = sessions_obj {
        let default_session = inference_obj
            .and_then(|value| value.get("default_session"))
            .and_then(Value::as_str)
            .unwrap_or("");

        for (name, value) in sessions_obj {
            let session = value.as_object();
            let transport = session
                .and_then(|item| item.get("transport"))
                .and_then(Value::as_str)
                .unwrap_or("api")
                .to_string();

            let item = if transport == "browser" {
                let browser = session
                    .and_then(|item| item.get("browser"))
                    .and_then(Value::as_object);
                InferenceConfigPanelSession {
                    name: name.to_string(),
                    transport: "browser".to_string(),
                    provider: None,
                    model: None,
                    endpoint: None,
                    browser_url: browser
                        .and_then(|browser| browser.get("target_url"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    is_default: name == default_session,
                }
            } else {
                InferenceConfigPanelSession {
                    name: name.to_string(),
                    transport: "api".to_string(),
                    provider: session
                        .and_then(|item| item.get("provider"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| Some("openai".to_string())),
                    model: session
                        .and_then(|item| item.get("model"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| Some("gpt-4.1".to_string())),
                    endpoint: session
                        .and_then(|item| item.get("endpoint"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    browser_url: None,
                    is_default: name == default_session,
                }
            };

            session_names.insert(name.to_string());
            sessions.push(item);
        }
    }

    if sessions.is_empty() {
        sessions.push(InferenceConfigPanelSession {
            name: "coding".to_string(),
            transport: "api".to_string(),
            provider: Some("openai".to_string()),
            model: Some("gpt-4.1".to_string()),
            endpoint: None,
            browser_url: None,
            is_default: true,
        });
        session_names.insert("coding".to_string());
    }

    if !sessions.iter().any(|session| session.is_default) {
        if let Some(first) = sessions.first_mut() {
            first.is_default = true;
        }
    }

    let default_session = sessions
        .iter()
        .find(|session| session.is_default)
        .map(|session| session.name.clone())
        .unwrap_or_else(|| sessions[0].name.clone());

    let configured_stage_sessions = inference_obj
        .and_then(|value| value.get("stage_sessions"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut stage_mappings = Vec::new();
    for stage_type in inference_stage_types(definition) {
        let mapped = configured_stage_sessions
            .get(stage_type.as_str())
            .and_then(Value::as_str)
            .filter(|name| session_names.contains(*name))
            .unwrap_or(default_session.as_str());

        stage_mappings.push(InferenceConfigPanelStageMapping {
            stage_type,
            session: mapped.to_string(),
        });
    }

    InferenceConfigPanel {
        sessions,
        stage_mappings,
    }
}

pub fn inference_config_from_panel(
    existing_globals: &WorkflowGlobalConfig,
    panel: InferenceConfigPanel,
) -> Value {
    let existing_inference = existing_globals
        .capabilities
        .get("inference")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let existing_sessions = existing_inference
        .get("sessions")
        .and_then(Value::as_object);

    let mut sessions = serde_json::Map::new();
    let mut default_session = None;

    for session in panel.sessions {
        let name = session.name.trim();
        if name.is_empty() {
            continue;
        }

        if session.is_default || default_session.is_none() {
            default_session = Some(name.to_string());
        }

        let existing_runtime = existing_sessions
            .and_then(|items| items.get(name))
            .and_then(|item| item.get("runtime"))
            .cloned()
            .unwrap_or_else(|| json!({}));

        let value = if session.transport == "browser" {
            json!({
                "transport": "browser",
                "browser": {
                    "target_url": session.browser_url.unwrap_or_default()
                },
                "runtime": existing_runtime
            })
        } else {
            json!({
                "provider": session.provider.unwrap_or_else(|| "openai".to_string()),
                "transport": "api",
                "model": session.model.unwrap_or_else(|| "gpt-4.1".to_string()),
                "endpoint": session.endpoint.unwrap_or_default(),
                "runtime": existing_runtime
            })
        };

        sessions.insert(name.to_string(), value);
    }

    if sessions.is_empty() {
        sessions.insert(
            "coding".to_string(),
            json!({
                "provider": "openai",
                "transport": "api",
                "model": "gpt-4.1",
                "runtime": {}
            }),
        );
        default_session = Some("coding".to_string());
    }

    let default_session = default_session.unwrap_or_else(|| {
        sessions
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "coding".to_string())
    });

    let mut stage_sessions = BTreeMap::new();
    for mapping in panel.stage_mappings {
        if sessions.contains_key(mapping.session.as_str()) {
            stage_sessions.insert(mapping.stage_type, Value::String(mapping.session));
        }
    }

    json!({
        "default_session": default_session,
        "stage_sessions": stage_sessions,
        "sessions": sessions
    })
}

fn inference_stage_types(definition: &WorkflowTemplateDefinition) -> Vec<String> {
    let mut seen = BTreeSet::new();

    for step in &definition.steps {
        if stage_uses_inference(step) {
            seen.insert(step.step_type.clone());
        }
    }

    seen.into_iter().collect()
}

fn stage_uses_inference(step: &crate::models::WorkflowStepDefinition) -> bool {
    step.capabilities
        .iter()
        .any(|capability| capability.enabled && capability.capability == "inference")
        || step.execution_plan.iter().any(|node| {
            node.enabled
                && node.kind == StageExecutionNodeKind::Capability
                && node.key == "inference"
        })
}
