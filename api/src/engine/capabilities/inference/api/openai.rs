use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use super::{
    data_url,
    encode_attachment,
    model_input_text,
    ApiProviderAdapter,
    ApiProviderExecution,
};
use super::super::{prompting::{ModelAttachmentKind, ModelInput}, ApiConfig};

pub struct OpenAiProvider;

pub static OPENAI_PROVIDER: OpenAiProvider = OpenAiProvider;

#[derive(Clone)]
struct OpenAiClient {
    http: Client,
    base_url: String,
    api_key: Option<String>,
}

impl OpenAiClient {
    fn new() -> Self {
        Self {
            http: Client::new(),
            base_url: "https://api.openai.com".to_string(),
            api_key: std::env::var("OPENAI_API_KEY").ok(),
        }
    }

    fn auth(&self, request: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
        let key = self
            .api_key
            .as_ref()
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("OPENAI_API_KEY is not set"))?;
        Ok(request.bearer_auth(key))
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let response = self
            .auth(self.http.get(format!("{}/v1/models", self.base_url)))?
            .send()
            .await
            .context("OpenAI /v1/models request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "OpenAI /v1/models returned {}: {}",
                status,
                body
            ));
        }

        let value: Value = response
            .json()
            .await
            .context("failed to parse OpenAI models response")?;

        Ok(value
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect())
    }

    async fn create_conversation(&self) -> Result<String> {
        let response = self
            .auth(
                self.http
                    .post(format!("{}/v1/conversations", self.base_url))
                    .json(&json!({})),
            )?
            .send()
            .await
            .context("OpenAI /v1/conversations request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "OpenAI /v1/conversations returned {}: {}",
                status,
                body
            ));
        }

        let value: Value = response
            .json()
            .await
            .context("failed to parse OpenAI conversation response")?;

        value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("OpenAI conversation response missing id"))
    }

    async fn respond(
        &self,
        model: &str,
        conversation_id: &str,
        content: Vec<Value>,
        options: &Value,
    ) -> Result<(String, Value)> {
        let mut payload = json!({
            "model": model,
            "conversation": conversation_id,
            "input": [{
                "role": "user",
                "content": content
            }]
        });

        if let Some(options) = options.as_object() {
            if let Some(payload_object) = payload.as_object_mut() {
                for (key, value) in options {
                    if !matches!(key.as_str(), "model" | "conversation" | "input") {
                        payload_object.insert(key.clone(), value.clone());
                    }
                }
            }
        }

        let response = self
            .auth(
                self.http
                    .post(format!("{}/v1/responses", self.base_url))
                    .json(&payload),
            )?
            .send()
            .await
            .context("OpenAI /v1/responses request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "OpenAI /v1/responses returned {}: {}",
                status,
                body
            ));
        }

        let value: Value = response
            .json()
            .await
            .context("failed to parse OpenAI response")?;

        Ok((response_text(&value), value))
    }
}

pub async fn list_models() -> Result<Vec<String>> {
    OpenAiClient::new().list_models().await
}

#[async_trait]
impl ApiProviderAdapter for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn create_session(
        &self,
        config: &ApiConfig,
        input: &ModelInput,
    ) -> Result<ApiProviderExecution> {
        let client = OpenAiClient::new();
        let conversation_id = client.create_conversation().await?;
        execute_turn(&client, config, conversation_id, input).await
    }

    async fn continue_session(
        &self,
        config: &ApiConfig,
        state: &Value,
        input: &ModelInput,
    ) -> Result<ApiProviderExecution> {
        let client = OpenAiClient::new();
        let conversation_id = state
            .get("conversation_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("OpenAI inference session is missing conversation_id"))?;

        execute_turn(&client, config, conversation_id, input).await
    }
}

async fn execute_turn(
    client: &OpenAiClient,
    config: &ApiConfig,
    conversation_id: String,
    input: &ModelInput,
) -> Result<ApiProviderExecution> {
    let content = build_input_content(input)?;
    let (output, response) = client
        .respond(
            config.model.as_str(),
            conversation_id.as_str(),
            content,
            &config.provider_options,
        )
        .await?;

    Ok(ApiProviderExecution {
        text: output,
        state: json!({
            "conversation_id": conversation_id
        }),
        metadata: json!({
            "conversation_id": conversation_id,
            "response_id": response.get("id").cloned().unwrap_or(Value::Null),
            "attachment_count": input.attachments.len()
        }),
    })
}

fn build_input_content(input: &ModelInput) -> Result<Vec<Value>> {
    let mut content = Vec::new();
    let text = model_input_text(input);

    if !text.trim().is_empty() {
        content.push(json!({
            "type": "input_text",
            "text": text
        }));
    }

    for attachment in &input.attachments {
        let encoded = encode_attachment(attachment)?;

        match encoded.kind {
            ModelAttachmentKind::Image => {
                content.push(json!({
                    "type": "input_image",
                    "image_url": data_url(&encoded)
                }));
            }
            ModelAttachmentKind::Text
            | ModelAttachmentKind::Document
            | ModelAttachmentKind::Binary => {
                content.push(json!({
                    "type": "input_file",
                    "filename": encoded.filename,
                    "file_data": data_url(&encoded)
                }));
            }
        }
    }

    if content.is_empty() {
        content.push(json!({
            "type": "input_text",
            "text": ""
        }));
    }

    Ok(content)
}

fn response_text(value: &Value) -> String {
    if let Some(text) = value
        .get("output_text")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return text.to_string();
    }

    let mut output = Vec::new();

    if let Some(items) = value.get("output").and_then(Value::as_array) {
        for item in items {
            if let Some(content) = item.get("content").and_then(Value::as_array) {
                for part in content {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            output.push(text.to_string());
                        }
                    }
                }
            }
        }
    }

    output.join("\n")
}
