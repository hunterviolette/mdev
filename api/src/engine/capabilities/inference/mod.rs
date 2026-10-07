pub mod api;
pub mod browser;
pub mod prompting;
pub mod session;
pub mod stage_support;
pub mod transport;
pub mod model_output;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::runtime_env::default_browser_cdp_url as runtime_default_browser_cdp_url;

use super::registry::{
    CapabilityContext,
    CapabilityInvocation,
    CapabilityInvocationRequest,
    CapabilityResult,
};

fn ensure_object_slot<'a>(parent: &'a mut serde_json::Map<String, Value>, key: &str) -> &'a mut serde_json::Map<String, Value> {
    let slot = parent
        .entry(key.to_string())
        .or_insert_with(|| json!({}));
    if !slot.is_object() {
        *slot = json!({});
    }
    slot.as_object_mut().expect("object slot must be object")
}


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InferenceTransport {
    Api,
    Browser,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InferenceSessionLifecycle {
    Persistent,
    NonPersistent,
}

impl Default for InferenceSessionLifecycle {
    fn default() -> Self {
        Self::Persistent
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    #[serde(default = "default_profile", skip_serializing)]
    pub profile: String,
    #[serde(default = "default_cdp_url", skip_serializing)]
    pub cdp_url: String,
    #[serde(default, skip_serializing)]
    pub page_url_contains: String,
    #[serde(default)]
    pub target_url: String,
    #[serde(default, skip_serializing)]
    pub edge_executable: String,
    #[serde(default, skip_serializing)]
    pub user_data_dir: String,
    #[serde(skip)]
    pub session_id: Option<String>,
    #[serde(default = "default_true", skip_serializing)]
    pub auto_launch_edge: bool,
    #[serde(default = "default_response_timeout_ms", skip_serializing)]
    pub response_timeout_ms: u64,
    #[serde(default = "default_response_poll_ms", skip_serializing)]
    pub response_poll_ms: u64,
    #[serde(default = "default_dom_poll_ms", skip_serializing)]
    pub dom_poll_ms: u64,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            profile: default_profile(),
            cdp_url: default_cdp_url(),
            page_url_contains: String::new(),
            target_url: String::new(),
            edge_executable: String::new(),
            user_data_dir: String::new(),
            session_id: None,
            auto_launch_edge: true,
            response_timeout_ms: default_response_timeout_ms(),
            response_poll_ms: default_response_poll_ms(),
            dom_poll_ms: default_dom_poll_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_api_provider")]
    pub provider: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub provider_options: Value,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            provider: default_api_provider(),
            model: default_model(),
            endpoint: String::new(),
            provider_options: json!({}),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct InferenceConfig {
    pub transport: InferenceTransport,
    #[serde(default)]
    pub lifecycle: InferenceSessionLifecycle,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub browser: BrowserConfig,
}

impl Serialize for InferenceConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("InferenceConfig", 3)?;
        state.serialize_field("transport", &self.transport)?;
        state.serialize_field("lifecycle", &self.lifecycle)?;
        match self.transport {
            InferenceTransport::Api => state.serialize_field("api", &self.api)?,
            InferenceTransport::Browser => state.serialize_field("browser", &self.browser)?,
        }
        state.end()
    }
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            transport: InferenceTransport::Api,
            lifecycle: InferenceSessionLifecycle::Persistent,
            api: ApiConfig::default(),
            browser: BrowserConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum InferenceSessionBindingMode {
    #[default]
    New,
    Existing,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InferenceStageSessionBinding {
    #[serde(default)]
    pub mode: InferenceSessionBindingMode,
    pub session: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum InferenceStageSessionBindingSpec {
    Binding(InferenceStageSessionBinding),
    Legacy(String),
}

impl InferenceStageSessionBindingSpec {
    pub fn session_name(&self) -> &str {
        match self {
            Self::Binding(binding) => binding.session.as_str(),
            Self::Legacy(session) => session.as_str(),
        }
    }

    pub fn existing_session_id(&self) -> Option<&str> {
        match self {
            Self::Binding(binding) if binding.mode == InferenceSessionBindingMode::Existing => {
                binding
                    .session_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InferenceCapabilityConfig {
    #[serde(default)]
    pub stage_sessions: std::collections::BTreeMap<String, InferenceStageSessionBindingSpec>,
    #[serde(default)]
    pub sessions: std::collections::BTreeMap<String, InferenceConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct InferenceCapabilitiesState {
    #[serde(default)]
    pub inference: Option<InferenceCapabilityConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct InferenceGlobalState {
    #[serde(default)]
    pub capabilities: InferenceCapabilitiesState,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct InferenceWorkflowEngineState {
    #[serde(default)]
    pub global_state: InferenceGlobalState,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct InferenceRunContext {
    #[serde(default)]
    pub workflow_engine: Option<InferenceWorkflowEngineState>,
    #[serde(default)]
    pub global_state: Option<InferenceGlobalState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceResult {
    pub transport: InferenceTransport,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub browser_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserProbeResult {
    pub session_id: String,
    pub browser_connected: bool,
    pub page_open: bool,
    pub url: String,
    pub profile: String,
    pub chat_input_found: bool,
    pub chat_input_visible: bool,
    pub chat_submit_found: bool,
    pub ready: bool,
}


pub async fn execute(
    ctx: &CapabilityContext<'_>,
    prior_results: &[CapabilityResult],
    _config: Value,
) -> Result<CapabilityResult> {
    let policy = super::registry::stage_capability_policy(ctx.step).ok();
    let consumed_capabilities = consumed_inference_capabilities(ctx.local_state);

    let mut follow_ups = Vec::new();
    let changeset_allowed = policy
        .as_ref()
        .map(|policy| policy.allowed_invocations.iter().any(|item| item == "changeset"))
        .unwrap_or(false);
    if ctx.step.step_type == "code" && changeset_allowed {
        follow_ups.push(CapabilityInvocation {
            capability: "changeset".to_string(),
            config: json!({}),
        });
    }

    let resolved_route = session::resolve_inference_route(ctx).await?;
    let selected_transport = resolved_route.config.transport.clone();
    let (selected_provider, selected_model) = match selected_transport {
        InferenceTransport::Api => (
            resolved_route.config.api.provider.clone(),
            resolved_route.config.api.model.clone(),
        ),
        InferenceTransport::Browser => ("browser".to_string(), String::new()),
    };

    let model_input = prompting::build_model_input(ctx, prior_results)?;
    let sent_prompt = model_input.text.clone();

    if sent_prompt.trim().is_empty() {
        return Ok(CapabilityResult {
            ok: false,
            capability: "inference".to_string(),
            payload: json!({
                "message": "Central prompting produced an empty model input.",
                "prompt": sent_prompt,
                "result": {
                    "ok": false,
                    "message": "Central prompting produced an empty model input."
                }
            }),
            follow_ups: CapabilityInvocationRequest::None,
        });
    }

    let inference = transport::Inference::new();
    let response = inference.execute(ctx, &resolved_route, &model_input).await?;

    ctx.state
        .prompt_inputs
        .acknowledge(ctx.run_id, &model_input.consumed_input_ids);

    let response_ok = response
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let response_text = response
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let capability_ok = response_ok
        && !sent_prompt.trim().is_empty()
        && (ctx.step.step_type != "code" || !response_text.trim().is_empty());

    let message = if capability_ok {
        "Inference capability executed.".to_string()
    } else {
        response
            .get("message")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("Inference capability failed.")
            .to_string()
    };

    let prompt_blocks = serde_json::to_value(&model_input.input_blocks)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();

    Ok(CapabilityResult {
        ok: capability_ok,
        capability: "inference".to_string(),
        payload: json!({
            "message": message,
            "prompt": sent_prompt,
            "result": response,
            "model_io": {
                "provider": selected_provider,
                "model": selected_model,
                "transport": selected_transport,
                "capability_key": "inference",
                "block_label": "Inference model call",
                "input": sent_prompt,
                "output": response_text,
                "content_format": "markdown",
                "status": if capability_ok { "completed" } else { "failed" },
                "step_id": ctx.step.id,
                "stage_type": ctx.step.step_type,
                "input_blocks": prompt_blocks,
                "output_blocks": []
            },
            "consumed_capabilities": consumed_capabilities,
        }),
        follow_ups: if capability_ok {
            if follow_ups.is_empty() {
                CapabilityInvocationRequest::None
            } else {
                CapabilityInvocationRequest::Many(follow_ups)
            }
        } else {
            CapabilityInvocationRequest::None
        },
    })
}

fn model_input_blocks(local_state: &Value) -> Vec<Value> {
    if let Some(items) = local_state
        .get("model_input_blocks")
        .and_then(Value::as_array)
    {
        return items.clone();
    }

    if let Some(items) = local_state
        .get("prompt_blocks")
        .and_then(Value::as_array)
    {
        return items.clone();
    }

    if let Some(items) = local_state
        .get("composed_prompt_blocks")
        .and_then(Value::as_array)
    {
        return items.clone();
    }

    Vec::new()
}

fn consumed_inference_capabilities(local_state: &Value) -> Vec<String> {
    let enabled = local_state
        .get("prompt_fragment_enabled")
        .and_then(Value::as_object);

    let mut consumed = Vec::new();

    if enabled
        .and_then(|items| items.get("repo_context"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        consumed.push("repo_context".to_string());
    }

    if enabled
        .and_then(|items| items.get("changeset_schema"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        consumed.push("changeset_schema".to_string());
    }

    if enabled
        .and_then(|items| items.get("planning_fragment"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        consumed.push("planner_fragment".to_string());
    }

    if enabled
        .and_then(|items| items.get("planner_schema"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        consumed.push("planner_schema".to_string());
    }

    consumed
}

fn default_profile() -> String {
    "default".to_string()
}

fn default_cdp_url() -> String {
    runtime_default_browser_cdp_url()
        .expect("WORKFLOW_BROWSER_CDP_HOST and WORKFLOW_BROWSER_CDP_PORT must be set")
}

fn default_api_provider() -> String {
    "openai".to_string()
}

fn default_model() -> String {
    "gpt-4.1".to_string()
}

fn default_true() -> bool {
    true
}

fn default_response_timeout_ms() -> u64 {
    600_000
}

fn default_response_poll_ms() -> u64 {
    1_000
}

fn default_dom_poll_ms() -> u64 {
    1_000
}
