use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde_json::{json, Value};

#[derive(Clone)]
pub struct OpenAIInferenceClient {
    http: Client,
    base_url: String,
    api_key: Option<String>,
}

impl OpenAIInferenceClient {
    pub fn from_env() -> Self {
        Self {
            http: Client::new(),
            base_url: std::env::var("OPENAI_BASE_URL")
                .ok()
                .unwrap_or_else(|| "https://api.openai.com".to_string()),
            api_key: std::env::var("OPENAI_API_KEY").ok(),
        }
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
        let key = self
            .api_key
            .as_ref()
            .ok_or_else(|| anyhow!("OPENAI_API_KEY is not set"))?;
        Ok(rb.bearer_auth(key))
    }

    async fn create_conversation(&self, items: Vec<(String, String)>) -> Result<String> {
        let url = format!("{}/v1/conversations", self.base_url.trim_end_matches('/'));
        let payload_items: Vec<Value> = items
            .into_iter()
            .map(|(role, content)| json!({ "role": role, "content": content }))
            .collect();

        let rb = self.http.post(url).json(&json!({ "items": payload_items }));
        let resp = self.auth(rb)?.send().await.context("OpenAI /v1/conversations request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_txt = resp.text().await.unwrap_or_default();
            return Err(anyhow!("OpenAI /v1/conversations returned {}: {}", status, body_txt));
        }

        let v: Value = resp.json().await.context("Failed to parse /v1/conversations JSON")?;
        let id = v.get("id").and_then(|x| x.as_str()).ok_or_else(|| anyhow!("/v1/conversations response missing 'id'"))?;
        Ok(id.to_string())
    }

    pub async fn chat_in_conversation(
        &self,
        model: &str,
        conversation_id: Option<String>,
        prior_items: Vec<(String, String)>,
        turn_items: Vec<(String, String)>,
    ) -> Result<(String, String)> {
        let conv_id = match conversation_id {
            Some(id) if !id.trim().is_empty() => id,
            _ => self.create_conversation(prior_items).await?,
        };

        let url = format!("{}/v1/responses", self.base_url.trim_end_matches('/'));
        let input: Vec<Value> = turn_items
            .into_iter()
            .map(|(role, content)| json!({ "role": role, "content": content }))
            .collect();

        let rb = self.http.post(url).json(&json!({
            "model": model,
            "conversation": conv_id,
            "input": input
        }));

        let resp = self.auth(rb)?.send().await.context("OpenAI /v1/responses request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_txt = resp.text().await.unwrap_or_default();
            return Err(anyhow!("OpenAI /v1/responses returned {}: {}", status, body_txt));
        }

        let v: Value = resp.json().await.context("Failed to parse /v1/responses JSON")?;
        let mut out = String::new();

        if let Some(output_items) = v.get("output").and_then(|o| o.as_array()) {
            for item in output_items {
                if item.get("role").and_then(|r| r.as_str()) != Some("assistant") {
                    continue;
                }
                if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                    for part in content {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            if !out.is_empty() {
                                out.push('\n');
                            }
                            out.push_str(text);
                        }
                    }
                }
            }
        }

        Ok((out, conv_id))
    }
}
