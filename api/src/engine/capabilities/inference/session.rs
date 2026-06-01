use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{ensure_object_slot, InferenceConfig, InferenceTransport};
use super::super::registry::CapabilityContext;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedInferenceSession {
    pub name: String,
    pub config: InferenceConfig,
}

pub async fn resolve_inference_session(ctx: &CapabilityContext<'_>) -> Result<ResolvedInferenceSession> {
    let run = crate::engine::load_run(ctx.state, ctx.run_id).await?;
    let global_state = run
        .context
        .get("workflow_engine")
        .and_then(|v| v.get("global_state"))
        .or_else(|| run.context.get("engine").and_then(|v| v.get("global_state")))
        .or_else(|| run.context.get("global_state"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let inference = global_state
        .get("capabilities")
        .and_then(|v| v.get("inference"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let session_name = explicit_stage_session(ctx)
        .or_else(|| mapped_stage_session(&inference, ctx.step.step_type.as_str()))
        .or_else(|| inference.get("default_session").and_then(Value::as_str).map(str::to_string))
        .or_else(|| first_session_name(&inference))
        .ok_or_else(|| anyhow!("inference capability has no configured sessions"))?;

    let session_value = inference
        .get("sessions")
        .and_then(|v| v.get(session_name.as_str()))
        .cloned()
        .ok_or_else(|| anyhow!("inference session '{}' is not configured", session_name))?;

    let mut config = serde_json::from_value::<InferenceConfig>(session_value)
        .map_err(|err| anyhow!("failed to decode inference session '{}': {}", session_name, err))?;

    if config.provider.trim().is_empty() {
        config.provider = default_provider_for_transport(&config.transport).to_string();
    }

    Ok(ResolvedInferenceSession {
        name: session_name,
        config,
    })
}

pub async fn persist_inference_config(
    ctx: &CapabilityContext<'_>,
    session_name: &str,
    cfg: &InferenceConfig,
) -> Result<()> {
    let mut run = crate::engine::load_run(ctx.state, ctx.run_id).await?;
    let root = crate::engine::ensure_engine_root(&mut run.context);
    let global_state = ensure_object_slot(root, "global_state");
    let capabilities = ensure_object_slot(global_state, "capabilities");
    let inference = ensure_object_slot(capabilities, "inference");
    let sessions = ensure_object_slot(inference, "sessions");

    sessions.insert(session_name.to_string(), serde_json::to_value(cfg)?);
    crate::engine::persist_context(ctx.state, ctx.run_id, &run.context).await
}

pub fn runtime_string(cfg: &InferenceConfig, key: &str) -> Option<String> {
    cfg.runtime
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn set_runtime_string(cfg: &mut InferenceConfig, key: &str, value: Option<String>) {
    if !cfg.runtime.is_object() {
        cfg.runtime = json!({});
    }

    let obj = cfg.runtime.as_object_mut().expect("runtime must be object");
    match value {
        Some(value) if !value.trim().is_empty() => {
            obj.insert(key.to_string(), Value::String(value));
        }
        _ => {
            obj.remove(key);
        }
    }
}

pub fn selected_session_from_local_state(local_state: &Value) -> Option<String> {
    local_state
        .get("capabilities")
        .and_then(|v| v.get("inference"))
        .and_then(|v| v.get("selected_session"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn resolve_stage_session_name(inference: &Value, step_type: &str) -> Option<String> {
    mapped_stage_session(inference, step_type)
        .or_else(|| inference.get("default_session").and_then(Value::as_str).map(str::to_string))
        .or_else(|| first_session_name(inference))
}

fn explicit_stage_session(ctx: &CapabilityContext<'_>) -> Option<String> {
    ctx.step
        .execution_logic
        .get("connections")
        .and_then(|v| v.get("inference"))
        .and_then(|v| v.get("session"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| selected_session_from_local_state(ctx.local_state))
}

fn mapped_stage_session(inference: &Value, step_type: &str) -> Option<String> {
    inference
        .get("stage_sessions")
        .and_then(|v| v.get(step_type))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn first_session_name(inference: &Value) -> Option<String> {
    inference
        .get("sessions")
        .and_then(Value::as_object)
        .and_then(|sessions| sessions.keys().next().cloned())
}

fn default_provider_for_transport(transport: &InferenceTransport) -> &'static str {
    match transport {
        InferenceTransport::Api => "openai",
        InferenceTransport::Browser => "browser",
    }
}
