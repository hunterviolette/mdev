pub mod api;
pub mod browser;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::registry::{
    CapabilityContext,
    CapabilityInvocation,
    CapabilityInvocationRequest,
    CapabilityResult,
    StageCapabilityPolicy,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelInferenceAction {
    Configure,
    LaunchBrowser,
    OpenUrl,
    ProbeBrowser,
    ConnectBrowserSession,
    DisconnectBrowserSession,
    GetConnectionStatus,
    SendPrompt,
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

pub fn apply_configuration(cfg: &mut InferenceConfig, payload: &Value) -> Result<()> {
    if let Some(transport) = payload.get("transport").and_then(|v| v.as_str()) {
        cfg.transport = match transport {
            "browser" => InferenceTransport::Browser,
            _ => InferenceTransport::Api,
        };
    }

    if let Some(model) = payload.get("model").and_then(|v| v.as_str()) {
        cfg.model = model.to_string();
    }

    if let Some(browser_cfg) = payload.get("browser") {
        cfg.browser = serde_json::from_value(browser_cfg.clone())?;
    }

    Ok(())
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

fn default_true() -> bool { true }
fn default_model() -> String { "gpt-4.1".to_string() }
fn default_profile() -> String { "auto".to_string() }
fn default_cdp_url() -> String { "http://127.0.0.1:9222".to_string() }
fn default_response_timeout_ms() -> u64 { 120_000 }
fn default_response_poll_ms() -> u64 { 1_000 }
fn default_dom_poll_ms() -> u64 { 1_000 }

fn default_bridge_dir() -> String {
    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors() {
            let candidate = dir.join("bridge");
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        for dir in cwd.ancestors() {
            let candidate = dir.join("bridge");
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    "bridge".to_string()
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    policy: &StageCapabilityPolicy,
    _config: Value,
) -> Result<CapabilityResult> {
    let mut follow_ups: Vec<CapabilityInvocation> = Vec::new();

    let include_repo_context = ctx
        .local_state
        .get("prompt_fragment_enabled")
        .and_then(Value::as_object)
        .and_then(|m| m.get("repo_context"))
        .and_then(Value::as_bool)
        .unwrap_or(ctx.step.prompt.include_repo_context);

    let include_changeset_schema = ctx
        .local_state
        .get("prompt_fragment_enabled")
        .and_then(Value::as_object)
        .and_then(|m| m.get("changeset_schema"))
        .and_then(Value::as_bool)
        .unwrap_or(ctx.step.prompt.include_changeset_schema);

    if include_repo_context && policy.allowed_invocations.iter().any(|item| *item == "context_export") {
        follow_ups.push(CapabilityInvocation {
            capability: "context_export".to_string(),
            config: json!({}),
        });
    }

    if include_changeset_schema && policy.allowed_invocations.iter().any(|item| *item == "changeset_schema") {
        follow_ups.push(CapabilityInvocation {
            capability: "changeset_schema".to_string(),
            config: json!({}),
        });
    }

    if ctx.step.step_type == "code" && policy.allowed_invocations.iter().any(|item| *item == "apply_changeset") {
        follow_ups.push(CapabilityInvocation {
            capability: "apply_changeset".to_string(),
            config: json!({}),
        });
    }

    let runtime_transport = ctx
        .local_state
        .get("inference")
        .cloned()
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
        InferenceTransport::Browser => browser::execute(ctx).await?,
        InferenceTransport::Api => api::execute(ctx).await?,
    };

    let sent_prompt = ctx
        .local_state
        .get("composed_prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    Ok(CapabilityResult {
        ok: true,
        capability: "inference".to_string(),
        payload: json!({
            "message": "Inference capability executed.",
            "prompt": sent_prompt,
            "result": response,
        }),
        follow_ups: if follow_ups.is_empty() {
            CapabilityInvocationRequest::None
        } else {
            CapabilityInvocationRequest::Many(follow_ups)
        },
    })
}
