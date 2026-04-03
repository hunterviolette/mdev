pub mod api;
pub mod browser;
pub mod stage_support;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
    #[serde(default = "default_bridge_dir")]
    pub bridge_dir: String,
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
            bridge_dir: default_bridge_dir(),
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
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub browser: BrowserConfig,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            transport: InferenceTransport::Api,
            model: default_model(),
            conversation_id: None,
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

pub async fn persist_inference_config(ctx: &CapabilityContext<'_>, cfg: &InferenceConfig) -> Result<()> {
    let mut run = crate::engine::load_run(ctx.state, ctx.run_id).await?;
    let root = crate::engine::ensure_engine_root(&mut run.context);
    let global_state = ensure_object_slot(root, "global_state");
    let capabilities = ensure_object_slot(global_state, "capabilities");
    capabilities.insert("inference".to_string(), serde_json::to_value(cfg)?);
    crate::engine::persist_context(ctx.state, ctx.run_id, &run.context).await
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    prior_results: &[CapabilityResult],
    _config: Value,
) -> Result<CapabilityResult> {
    let policy = super::registry::stage_capability_policy(ctx.step)?;
    let include_changeset_schema = ctx
        .local_state
        .get("prompt_fragment_enabled")
        .and_then(|v| v.get("changeset_schema"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut follow_ups = Vec::new();
    if include_changeset_schema && policy.allowed_invocations.iter().any(|item| *item == "changeset_schema") {
        follow_ups.push(CapabilityInvocation {
            capability: "changeset_schema".to_string(),
            config: json!({}),
        });
    }

    if ctx.step.step_type == "code" && policy.allowed_invocations.iter().any(|item| item == "gateway_model/changeset") {
        follow_ups.push(CapabilityInvocation {
            capability: "gateway_model/changeset".to_string(),
            config: json!({}),
        });
    }

    let runtime_transport = ctx
        .local_state
        .get("capabilities")
        .and_then(|v| v.get("inference"))
        .cloned()
        .or_else(|| ctx.local_state.get("inference").cloned())
        .and_then(|v| serde_json::from_value::<InferenceConfig>(v).ok())
        .map(|cfg| cfg.transport);

    let configured_transport = ctx
        .step
        .config
        .get("inference_transport")
        .and_then(Value::as_str)
        .and_then(|value| match value {
            "browser" => Some(InferenceTransport::Browser),
            "api" => Some(InferenceTransport::Api),
            _ => None,
        });

    let selected_transport = runtime_transport
        .or(configured_transport)
        .unwrap_or(InferenceTransport::Api);

    let response = match selected_transport {
        InferenceTransport::Browser => browser::execute(ctx, prior_results).await?,
        InferenceTransport::Api => api::execute(ctx).await?,
    };

    let sent_prompt = ctx
        .local_state
        .get("composed_prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let response_ok = response
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let response_text = response
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let capability_ok = response_ok && (!sent_prompt.trim().is_empty() || response_text.trim().is_empty() || selected_transport != InferenceTransport::Browser);

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

fn default_profile() -> String {
    "default".to_string()
}

fn default_bridge_dir() -> String {
    "bridge".to_string()
}

fn default_cdp_url() -> String {
    "http://127.0.0.1:9222".to_string()
}

fn default_model() -> String {
    "gpt-4.1".to_string()
}

fn default_true() -> bool {
    true
}

fn default_response_timeout_ms() -> u64 {
    120_000
}

fn default_response_poll_ms() -> u64 {
    1_000
}

fn default_dom_poll_ms() -> u64 {
    1_000
}
