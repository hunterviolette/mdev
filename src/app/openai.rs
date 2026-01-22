// src/app/openai.rs
// OpenAI client:
// - list_models(): used to populate dropdown
// - chat_completion_text(): convenience wrapper
// - chat_completion_messages(): implemented via Responses API (/v1/responses)

use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;

#[derive(Clone)]
pub struct OpenAIClient {
    http: Client,
    base_url: String,
    api_key: Option<String>,
}

impl OpenAIClient {
    pub fn from_env() -> Self {
        let api_key = std::env::var("OPENAI_API_KEY").ok();
        let base_url = std::env::var("OPENAI_BASE_URL")
            .ok()
            .unwrap_or_else(|| "https://api.openai.com".to_string());
        Self {
            http: Client::new(),
            base_url,
            api_key,
        }
    }

    fn auth(&self, rb: reqwest::blocking::RequestBuilder) -> Result<reqwest::blocking::RequestBuilder> {
        let key = self
            .api_key
            .as_ref()
            .ok_or_else(|| anyhow!("OPENAI_API_KEY is not set"))?;
        Ok(rb.bearer_auth(key))
    }

    pub fn list_models(&self) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct ModelsResp {
            data: Vec<ModelItem>,
        }
        #[derive(Deserialize)]
        struct ModelItem {
            id: String,
        }

        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));
        let rb = self.http.get(url);
        let rb = self.auth(rb)?;
        let resp = rb
            .send()
            .context("OpenAI /v1/models request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(anyhow!("OpenAI /v1/models returned {}: {}", status, body));
        }

        let parsed: ModelsResp = resp.json().context("Failed to parse /v1/models JSON")?;
        let mut ids: Vec<String> = parsed.data.into_iter().map(|m| m.id).collect();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    /// Convenience helper: system + user messages.
    pub fn chat_completion_text(&self, model: &str, system: &str, user: &str) -> Result<String> {
        self.chat_completion_messages(
            model,
            vec![
                ("system".to_string(), system.to_string()),
                ("user".to_string(), user.to_string()),
            ],
            0.2,
        )
    }

    /// Text generation using the Responses API.
    /// We keep the name for compatibility with the rest of the app.
    pub fn chat_completion_messages(
        &self,
        model: &str,
        messages: Vec<(String, String)>,
        temperature: f32,
    ) -> Result<String> {
        // Request: POST /v1/responses
        // Body: { model, input: [ {role, content}, ... ], temperature }
        // Content may be a string per the API reference.

        let url = format!("{}/v1/responses", self.base_url.trim_end_matches('/'));

        let input: Vec<serde_json::Value> = messages
            .into_iter()
            .map(|(role, content)| serde_json::json!({"role": role, "content": content}))
            .collect();

        // NOTE: Some models (e.g. certain GPT-5 variants) reject `temperature` on /v1/responses.
        // To stay compatible across models, we omit it entirely here.
        let _ = temperature; // keep signature stable
        let body = serde_json::json!({
            "model": model,
            "input": input
        });

        let rb = self.http.post(url).json(&body);
        let rb = self.auth(rb)?;
        let resp = rb
            .send()
            .context("OpenAI /v1/responses request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_txt = resp.text().unwrap_or_default();
            return Err(anyhow!("OpenAI /v1/responses returned {}: {}", status, body_txt));
        }

        // Parse a minimal subset and robustly extract output text.
        let v: serde_json::Value = resp.json().context("Failed to parse /v1/responses JSON")?;

        // Typical structure:
        // { output: [ { type: "message", role: "assistant", content: [ { type: "output_text", text: "..." } ] } ] }
        // We'll aggregate any content parts that have a "text" field.
        let mut out = String::new();

        if let Some(output_items) = v.get("output").and_then(|o| o.as_array()) {
            for item in output_items {
                // Only collect assistant message items when present.
                let role_ok = item
                    .get("role")
                    .and_then(|r| r.as_str())
                    .map(|r| r == "assistant")
                    .unwrap_or(true);

                if !role_ok {
                    continue;
                }

                if let Some(content_arr) = item.get("content").and_then(|c| c.as_array()) {
                    for part in content_arr {
                        if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                            if !out.is_empty() {
                                out.push_str("\n");
                            }
                            out.push_str(t);
                        }
                    }
                }

                // Some variants may embed text directly.
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    if !out.is_empty() {
                        out.push_str("\n");
                    }
                    out.push_str(t);
                }
            }
        }

        Ok(out)
    }
}
