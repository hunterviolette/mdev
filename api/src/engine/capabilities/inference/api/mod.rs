pub mod anthropic;
pub mod openai;

use std::fs;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde_json::{json, Value};

use super::prompting::{ModelAttachment, ModelAttachmentKind, ModelInput};
use super::session::{InferenceSession, ResolvedInferenceRoute};
use super::transport::{InferenceTransportAdapter, InferenceTransportExecution};
use super::super::registry::CapabilityContext;
use super::ApiConfig;

pub struct ApiProviderExecution {
    pub text: String,
    pub state: Value,
    pub metadata: Value,
}

#[async_trait]
pub trait ApiProviderAdapter: Send + Sync {
    fn name(&self) -> &'static str;

    async fn create_session(
        &self,
        config: &ApiConfig,
        input: &ModelInput,
    ) -> Result<ApiProviderExecution>;

    async fn continue_session(
        &self,
        config: &ApiConfig,
        state: &Value,
        input: &ModelInput,
    ) -> Result<ApiProviderExecution>;
}

pub struct EncodedAttachment {
    pub filename: String,
    pub media_type: String,
    pub kind: ModelAttachmentKind,
    pub bytes: Vec<u8>,
    pub base64: String,
}

pub fn model_input_text(input: &ModelInput) -> String {
    let mut text = input.text.clone();

    if let Some(context) = input
        .pasted_context_text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !text.trim().is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(context);
    }

    text
}

pub fn encode_attachment(attachment: &ModelAttachment) -> Result<EncodedAttachment> {
    let bytes = fs::read(&attachment.path).with_context(|| {
        format!(
            "failed to read inference attachment '{}' from {}",
            attachment.filename,
            attachment.path.display()
        )
    })?;

    Ok(EncodedAttachment {
        filename: attachment.filename.clone(),
        media_type: attachment.media_type.clone(),
        kind: attachment.kind.clone(),
        base64: BASE64_STANDARD.encode(&bytes),
        bytes,
    })
}

pub fn data_url(attachment: &EncodedAttachment) -> String {
    format!(
        "data:{};base64,{}",
        attachment.media_type,
        attachment.base64
    )
}

pub fn provider_state(provider: &str, state: Value) -> Value {
    json!({
        "provider": provider,
        "state": state
    })
}

pub fn nested_provider_state<'a>(state: &'a Value, provider: &str) -> &'a Value {
    if state
        .get("provider")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case(provider))
    {
        state.get("state").unwrap_or(&Value::Null)
    } else {
        &Value::Null
    }
}

pub struct ApiInferenceTransport;

pub static API_INFERENCE_TRANSPORT: ApiInferenceTransport = ApiInferenceTransport;

#[async_trait]
impl InferenceTransportAdapter for ApiInferenceTransport {
    async fn session_available(
        &self,
        _state: &crate::app_state::AppState,
        _route: &ResolvedInferenceRoute,
        _session: &InferenceSession,
    ) -> Result<bool> {
        Ok(true)
    }

    async fn create_session(
        &self,
        ctx: &CapabilityContext<'_>,
        route: &ResolvedInferenceRoute,
        input: &ModelInput,
    ) -> Result<InferenceTransportExecution> {
        execute(ctx, route, None, input).await
    }

    async fn continue_session(
        &self,
        ctx: &CapabilityContext<'_>,
        route: &ResolvedInferenceRoute,
        session: &InferenceSession,
        input: &ModelInput,
    ) -> Result<InferenceTransportExecution> {
        execute(ctx, route, Some(session), input).await
    }
}

async fn execute(
    _ctx: &CapabilityContext<'_>,
    route: &ResolvedInferenceRoute,
    session: Option<&InferenceSession>,
    input: &ModelInput,
) -> Result<InferenceTransportExecution> {
    let config = &route.config.api;
    let provider_name = config.provider.trim().to_ascii_lowercase();
    let adapter = provider_adapter(provider_name.as_str())?;

    if let Some(session) = session {
        if let Some(existing_provider) = session
            .transport_state
            .get("provider")
            .and_then(Value::as_str)
        {
            if !existing_provider.eq_ignore_ascii_case(adapter.name()) {
                bail!(
                    "inference session '{}' belongs to API provider '{}' but route '{}' uses '{}'",
                    session.id,
                    existing_provider,
                    route.name,
                    adapter.name()
                );
            }
        }
    }

    let execution = match session {
        Some(session) => {
            let state = nested_provider_state(&session.transport_state, adapter.name());
            adapter.continue_session(config, state, input).await?
        }
        None => adapter.create_session(config, input).await?,
    };

    Ok(InferenceTransportExecution {
        text: execution.text,
        metadata: json!({
            "ok": true,
            "transport": "api",
            "provider": adapter.name(),
            "model": config.model,
            "provider_options": config.provider_options,
            "provider_metadata": execution.metadata
        }),
        transport_state: provider_state(adapter.name(), execution.state),
    })
}

fn provider_adapter(provider: &str) -> Result<&'static dyn ApiProviderAdapter> {
    match provider {
        "openai" => Ok(&openai::OPENAI_PROVIDER),
        "anthropic" => Ok(&anthropic::ANTHROPIC_PROVIDER),
        other => bail!("unsupported API inference provider '{}'", other),
    }
}
