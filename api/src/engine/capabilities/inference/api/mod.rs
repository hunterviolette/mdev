pub mod oai;

use anyhow::Result;
use serde_json::json;

use super::{prompting::ModelInput, session, persist_inference_config, InferenceResult, InferenceTransport};
use super::super::registry::CapabilityContext;

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    input: &ModelInput,
) -> Result<serde_json::Value> {

    let resolved_session = session::resolve_inference_session(ctx).await?;
    let mut inference_cfg = resolved_session.config;
    let current_process_session_id = ctx.state.process_session_id().to_string();
    let prior_conversation_id = session::runtime_string(&inference_cfg, "conversation_id")
        .or_else(|| inference_cfg.conversation_id.clone());

    let client = oai::OpenAIInferenceClient::from_env();
    let (text, conversation_id) = client
        .chat_in_conversation(
            &inference_cfg.model,
            prior_conversation_id,
            Vec::new(),
            vec![("user".to_string(), input.text.clone())],
        )
        .await?;

    inference_cfg.conversation_id = Some(conversation_id.clone());
    session::set_runtime_string(&mut inference_cfg, "conversation_id", Some(conversation_id.clone()));
    session::set_runtime_string(
        &mut inference_cfg,
        "process_session_id",
        Some(current_process_session_id.clone()),
    );
    persist_inference_config(ctx, &resolved_session.name, &inference_cfg).await?;

    let result = InferenceResult {
        transport: InferenceTransport::Api,
        text,
        conversation_id: Some(conversation_id),
        browser_session_id: None,
    };

    Ok(json!(result))
}
