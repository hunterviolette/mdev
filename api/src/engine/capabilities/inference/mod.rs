pub mod api;
pub mod browser;
pub mod panel;
pub mod session;
pub mod stage_support;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    #[serde(default = "default_profile")]
    pub profile: String,
    #[serde(default = "default_cdp_url")]
    pub cdp_url: String,
    #[serde(default)]
    pub page_url_contains: String,
    #[serde(default)]
    pub target_url: String,
    #[serde(default)]
    pub edge_executable: String,
    #[serde(default)]
    pub user_data_dir: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default = "default_true")]
    pub auto_launch_edge: bool,
    #[serde(default = "default_response_timeout_ms")]
    pub response_timeout_ms: u64,
    #[serde(default = "default_response_poll_ms")]
    pub response_poll_ms: u64,
    #[serde(default = "default_dom_poll_ms")]
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
pub struct InferenceConfig {
    pub transport: InferenceTransport,
    #[serde(default)]
    pub provider: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub runtime: Value,
    #[serde(default)]
    pub browser: BrowserConfig,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            transport: InferenceTransport::Api,
            provider: "openai".to_string(),
            model: default_model(),
            endpoint: String::new(),
            conversation_id: None,
            runtime: json!({}),
            browser: BrowserConfig::default(),
        }
    }
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

pub use session::persist_inference_config;

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    prior_results: &[CapabilityResult],
    _config: Value,
) -> Result<CapabilityResult> {
    let policy = super::registry::stage_capability_policy(ctx.step)?;
    let consumed_capabilities = consumed_inference_capabilities(ctx.local_state);

    let mut follow_ups = Vec::new();
    if ctx.step.step_type == "code" && policy.allowed_invocations.iter().any(|item| item == "changeset") {
        follow_ups.push(CapabilityInvocation {
            capability: "changeset".to_string(),
            config: json!({}),
        });
    }

    let resolved_session = session::resolve_inference_session(ctx).await?;
    let selected_transport = resolved_session.config.transport.clone();

    let sent_prompt = ctx
        .local_state
        .get("composed_prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if selected_transport == InferenceTransport::Browser && sent_prompt.trim().is_empty() {
        return Ok(CapabilityResult {
            ok: false,
            capability: "inference".to_string(),
            payload: json!({
                "message": "Browser inference prompt is empty before send_chat",
                "prompt": sent_prompt,
                "result": {
                    "ok": false,
                    "message": "Browser inference prompt is empty before send_chat"
                }
            }),
            follow_ups: CapabilityInvocationRequest::None,
        });
    }

    let response = match selected_transport {
        InferenceTransport::Browser => browser::execute(ctx, prior_results).await?,
        InferenceTransport::Api => api::execute(ctx).await?,
    };

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

    Ok(CapabilityResult {
        ok: capability_ok,
        capability: "inference".to_string(),
        payload: json!({
            "message": message,
            "prompt": sent_prompt,
            "result": response,
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

    if local_state
        .get("transient_prompt_fragments")
        .and_then(Value::as_array)
        .map(|items| !items.is_empty())
        .unwrap_or(false)
    {
        consumed.push("prompt_fragments".to_string());
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
