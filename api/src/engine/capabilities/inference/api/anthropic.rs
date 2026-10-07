use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use super::{
    encode_attachment,
    model_input_text,
    ApiProviderAdapter,
    ApiProviderExecution,
    EncodedAttachment,
};
use super::super::{prompting::{ModelAttachmentKind, ModelInput}, ApiConfig};

pub struct AnthropicProvider;

pub static ANTHROPIC_PROVIDER: AnthropicProvider = AnthropicProvider;

#[derive(Clone)]
struct AnthropicClient {
    http: Client,
    base_url: String,
    api_key: Option<String>,
}

impl AnthropicClient {
    fn new(config: &ApiConfig) -> Self {
        let configured_endpoint = config.endpoint.trim();
        let base_url = if configured_endpoint.is_empty() {
            std::env::var("ANTHROPIC_BASE_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "https://api.anthropic.com".to_string())
        } else {
            configured_endpoint.to_string()
        };

        Self {
            http: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
        }
    }

    async fn message(
        &self,
        config: &ApiConfig,
        messages: &[Value],
    ) -> Result<Value> {
        let api_key = self
            .api_key
            .as_ref()
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("ANTHROPIC_API_KEY is not set"))?;

        let mut payload = json!({
            "model": config.model,
            "max_tokens": config
                .provider_options
                .get("max_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(4096),
            "messages": messages
        });

        if let Some(options) = config.provider_options.as_object() {
            if let Some(payload_object) = payload.as_object_mut() {
                for (key, value) in options {
                    if !matches!(key.as_str(), "model" | "messages" | "max_tokens") {
                        payload_object.insert(key.clone(), value.clone());
                    }
                }
            }
        }

        let response = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&payload)
            .send()
            .await
            .context("Anthropic /v1/messages request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Anthropic /v1/messages returned {}: {}",
                status,
                body
            ));
        }

        response
            .json()
            .await
            .context("failed to parse Anthropic message response")
    }
}

#[async_trait]
impl ApiProviderAdapter for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn create_session(
        &self,
        config: &ApiConfig,
        input: &ModelInput,
    ) -> Result<ApiProviderExecution> {
        execute_turn(config, Vec::new(), input).await
    }

    async fn continue_session(
        &self,
        config: &ApiConfig,
        state: &Value,
        input: &ModelInput,
    ) -> Result<ApiProviderExecution> {
        let history = state
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        execute_turn(config, history, input).await
    }
}

async fn execute_turn(
    config: &ApiConfig,
    mut history: Vec<Value>,
    input: &ModelInput,
) -> Result<ApiProviderExecution> {
    let user_content = build_user_content(input)?;
    history.push(json!({
        "role": "user",
        "content": user_content
    }));

    let client = AnthropicClient::new(config);
    let response = client.message(config, &history).await?;
    let text = response_text(&response);

    history.push(json!({
        "role": "assistant",
        "content": [{
            "type": "text",
            "text": text
        }]
    }));

    Ok(ApiProviderExecution {
        text,
        state: json!({
            "messages": history
        }),
        metadata: json!({
            "message_id": response.get("id").cloned().unwrap_or(Value::Null),
            "stop_reason": response.get("stop_reason").cloned().unwrap_or(Value::Null),
            "attachment_count": input.attachments.len()
        }),
    })
}

fn build_user_content(input: &ModelInput) -> Result<Vec<Value>> {
    let mut content = Vec::new();
    let text = model_input_text(input);

    if !text.trim().is_empty() {
        content.push(json!({
            "type": "text",
            "text": text
        }));
    }

    for attachment in &input.attachments {
        let encoded = encode_attachment(attachment)?;
        content.push(anthropic_attachment_block(&encoded)?);
    }

    if content.is_empty() {
        content.push(json!({
            "type": "text",
            "text": ""
        }));
    }

    Ok(content)
}

fn anthropic_attachment_block(attachment: &EncodedAttachment) -> Result<Value> {
    match attachment.kind {
        ModelAttachmentKind::Image => Ok(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": attachment.media_type,
                "data": attachment.base64
            }
        })),
        ModelAttachmentKind::Document => Ok(json!({
            "type": "document",
            "source": {
                "type": "base64",
                "media_type": attachment.media_type,
                "data": attachment.base64
            },
            "title": attachment.filename
        })),
        ModelAttachmentKind::Text => {
            let text = String::from_utf8(attachment.bytes.clone()).with_context(|| {
                format!(
                    "Anthropic text attachment '{}' is not valid UTF-8",
                    attachment.filename
                )
            })?;

            Ok(json!({
                "type": "text",
                "text": format!(
                    "<attachment filename=\"{}\">\n{}\n</attachment>",
                    attachment.filename,
                    text
                )
            }))
        }
        ModelAttachmentKind::Binary => bail!(
            "Anthropic API provider cannot encode binary attachment '{}' with media type '{}'",
            attachment.filename,
            attachment.media_type
        ),
    }
}

fn response_text(value: &Value) -> String {
    value
        .get("content")
        .and_then(Value::as_array)
        .map(|content| {
            content
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
