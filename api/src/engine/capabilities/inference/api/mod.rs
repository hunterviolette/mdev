pub mod oai;

use anyhow::Result;
use serde_json::json;

use super::{persist_inference_config, InferenceConfig, InferenceResult, InferenceTransport};
use super::super::registry::CapabilityContext;

pub async fn execute(ctx: &CapabilityContext<'_>) -> Result<serde_json::Value> {
    let prompt = ctx
        .local_state
        .get("composed_prompt")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();

    let mut inference_cfg: InferenceConfig = ctx
        .local_state
        .get("capabilities")
        .and_then(|v| v.get("inference"))
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    let client = oai::OpenAIInferenceClient::from_env();
    let (text, conversation_id) = client
        .chat_in_conversation(
            &inference_cfg.model,
            inference_cfg.conversation_id.clone(),
            Vec::new(),
            vec![("user".to_string(), prompt)],
        )
        .await?;

    inference_cfg.conversation_id = Some(conversation_id.clone());
    persist_inference_config(ctx, &inference_cfg).await?;

    let result = InferenceResult {
        transport: InferenceTransport::Api,
        text,
        conversation_id: Some(conversation_id),
        browser_session_id: None,
    };

    Ok(json!(result))
}
