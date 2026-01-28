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

    pub fn create_conversation(&self, items: Vec<(String, String)>) -> Result<String> {
        // POST /v1/conversations
        // Body: { items: [ { role, content }, ... ] }
        // Response: { id: "conv_..." , ... }

        let url = format!("{}/v1/conversations", self.base_url.trim_end_matches('/'));

        let payload_items: Vec<serde_json::Value> = items
            .into_iter()
            .map(|(role, content)| serde_json::json!({"role": role, "content": content}))
            .collect();

        let body = serde_json::json!({
            "items": payload_items
        });

        let rb = self.http.post(url).json(&body);
        let rb = self.auth(rb)?;
        let resp = rb.send().context("OpenAI /v1/conversations request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_txt = resp.text().unwrap_or_default();
            return Err(anyhow!("OpenAI /v1/conversations returned {}: {}", status, body_txt));
        }

        let v: serde_json::Value = resp
            .json()
            .context("Failed to parse /v1/conversations JSON")?;

        let id = v
            .get("id")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow!("/v1/conversations response missing 'id'"))?;

        Ok(id.to_string())
    }

    /// Fetch conversation items and convert them into a simple (role, text) transcript.
    /// Uses: GET /v1/conversations/{conversation_id}/items
    pub fn list_conversation_messages(&self, conversation_id: &str) -> Result<Vec<(String, String)>> {
        let url = format!(
            "{}/v1/conversations/{}/items",
            self.base_url.trim_end_matches('/'),
            conversation_id
        );

        let rb = self.http.get(url);
        let rb = self.auth(rb)?;
        let resp = rb.send().context("OpenAI /v1/conversations/{id}/items request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_txt = resp.text().unwrap_or_default();
            return Err(anyhow!(
                "OpenAI /v1/conversations/{}/items returned {}: {}",
                conversation_id,
                status,
                body_txt
            ));
        }

        let v: serde_json::Value = resp
            .json()
            .context("Failed to parse /v1/conversations/{id}/items JSON")?;

        let mut out: Vec<(String, String)> = Vec::new();

        // Response shape is an item list: { data: [ ...items... ], ... }
        if let Some(data) = v.get("data").and_then(|d| d.as_array()) {
            for item in data {
                // We only care about message-like items with role + content.
                let role = item
                    .get("role")
                    .and_then(|r| r.as_str())
                    .unwrap_or("")
                    .to_string();

                if role.is_empty() {
                    continue;
                }

                let mut text = String::new();

                // Typical shape: content: [ { type: "input_text"|"output_text", text: "..." }, ... ]
                if let Some(content_arr) = item.get("content").and_then(|c| c.as_array()) {
                    for part in content_arr {
                        if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                text.push_str("\n");
                            }
                            text.push_str(t);
                        }
                    }
                }

                // Fallback: some message items may contain a direct text field.
                if text.is_empty() {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                        text = t.to_string();
                    }
                }

                if !text.is_empty() {
                    out.push((role, text));
                }
            }
        }

        Ok(out)
    }

    /// Generate text using /v1/responses attached to a persistent conversation.
    ///
    /// - If `conversation_id` is None, creates a new conversation using `seed_items_if_new`.
    /// - Sends only the `turn_items` as the input delta for this turn.
    /// - Returns (assistant_text, conversation_id).
    pub fn chat_in_conversation(
        &self,
        model: &str,
        conversation_id: Option<String>,
        seed_items_if_new: Vec<(String, String)>,
        turn_items: Vec<(String, String)>,
    ) -> Result<(String, String)> {
        let conv_id = match conversation_id {
            Some(id) => id,
            None => self.create_conversation(seed_items_if_new)?,
        };

        let url = format!("{}/v1/responses", self.base_url.trim_end_matches('/'));

        let input: Vec<serde_json::Value> = turn_items
            .into_iter()
            .map(|(role, content)| serde_json::json!({"role": role, "content": content}))
            .collect();

        let body = serde_json::json!({
            "model": model,
            "conversation": conv_id,
            "input": input
        });

        let rb = self.http.post(url).json(&body);
        let rb = self.auth(rb)?;
        let resp = rb.send().context("OpenAI /v1/responses request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_txt = resp.text().unwrap_or_default();
            return Err(anyhow!("OpenAI /v1/responses returned {}: {}", status, body_txt));
        }

        let v: serde_json::Value = resp.json().context("Failed to parse /v1/responses JSON")?;

        let mut out = String::new();
        if let Some(output_items) = v.get("output").and_then(|o| o.as_array()) {
            for item in output_items {
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

                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    if !out.is_empty() {
                        out.push_str("\n");
                    }
                    out.push_str(t);
                }
            }
        }

        Ok((out, conv_id))
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
