pub mod api;
pub mod browser;
pub mod panel;
pub mod prompting;
pub mod session;
pub mod stage_support;
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

pub(crate) fn resolve_inference_prompt(local_state: &Value) -> String {
    let composed_prompt = local_state
        .get("composed_prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();

    if !composed_prompt.is_empty() {
        return composed_prompt.to_string();
    }

    for key in ["model_input_blocks", "prompt_blocks", "composed_prompt_blocks"] {
        let prompt = local_state
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|block| {
                block
                    .get("content")
                    .or_else(|| block.get("text"))
                    .or_else(|| block.get("value"))
                    .and_then(Value::as_str)
            })
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");

        if !prompt.is_empty() {
            return prompt;
        }
    }

    local_state
        .get("transient_prompt_fragments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|fragment| {
            fragment.as_str().or_else(|| {
                fragment
                    .get("text")
                    .or_else(|| fragment.get("content"))
                    .or_else(|| fragment.get("value"))
                    .and_then(Value::as_str)
            })
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
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

    let resolved_session = session::resolve_inference_session(ctx).await?;
    let selected_transport = resolved_session.config.transport.clone();
    let selected_provider = resolved_session.config.provider.clone();
    let selected_model = resolved_session.config.model.clone();

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

    let response = match selected_transport {
        InferenceTransport::Browser => browser::execute(ctx, &model_input).await?,
        InferenceTransport::Api => api::execute(ctx, &model_input).await?,
    };

    ctx.state
        .orchestration_inputs
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

    local_state
        .get("transient_prompt_fragments")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    let object = item.as_object()?;
                    let key = object
                        .get("capability_key")
                        .or_else(|| object.get("capability"))
                        .or_else(|| object.get("key"))
                        .and_then(Value::as_str)
                        .unwrap_or("prompt_fragment");
                    let label = object
                        .get("label")
                        .or_else(|| object.get("title"))
                        .and_then(Value::as_str)
                        .unwrap_or(key);
                    let content = object
                        .get("content")
                        .or_else(|| object.get("text"))
                        .or_else(|| object.get("value"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let content_format = object
                        .get("content_format")
                        .or_else(|| object.get("format"))
                        .and_then(Value::as_str)
                        .unwrap_or("markdown");

                    Some(json!({
                        "index": index,
                        "capability_key": key,
                        "label": label,
                        "content_format": content_format,
                        "content": content
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
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
