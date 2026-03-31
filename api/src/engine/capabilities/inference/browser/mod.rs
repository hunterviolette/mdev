pub mod adapter;

use anyhow::Result;
use serde_json::{json, Value};

use super::{BrowserConfig, BrowserProbeResult, InferenceConfig};
use super::super::registry::CapabilityContext;

fn is_stale_session_error(err: &anyhow::Error) -> bool {
    let msg = format!("{:#}", err).to_ascii_lowercase();
    msg.contains("unknown session id") || msg.contains("unknown session_id") || msg.contains("disconnected")
}

fn ensure_live_browser_session(browser: &mut BrowserConfig) -> Result<String> {
    let existing = browser.session_id.clone().unwrap_or_default();
    if !existing.trim().is_empty() {
        match adapter::probe(browser) {
            Ok(_) => return Ok(existing),
            Err(err) if is_stale_session_error(&err) => {
                browser.session_id = None;
            }
            Err(err) => return Err(err),
        }
    }

    let session_id = adapter::launch_and_attach(browser)?;
    browser.session_id = Some(session_id.clone());
    Ok(session_id)
}

pub async fn execute(ctx: &CapabilityContext<'_>) -> Result<serde_json::Value> {
    let prompt = ctx
        .local_state
        .get("composed_prompt")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();

    let mut inference_cfg: InferenceConfig = ctx
        .local_state
        .get("inference")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_else(|| InferenceConfig {
            browser: BrowserConfig::default(),
            ..InferenceConfig::default()
        });

    ensure_live_browser_session(&mut inference_cfg.browser)?;
    let result = adapter::send_chat_and_wait(&mut inference_cfg.browser, &prompt)?;
    Ok(json!(result))
}

pub fn connect_session(cfg: &mut InferenceConfig) -> Result<Value> {
    let target_url = cfg.browser.target_url.trim().to_string();
    let session_id = ensure_live_browser_session(&mut cfg.browser)?;

    let current_probe = adapter::probe(&mut cfg.browser).ok();
    let should_open_target = if target_url.is_empty() {
        false
    } else {
        match current_probe.as_ref() {
            Some(probe) => !probe.page_open || probe.url != target_url,
            None => true,
        }
    };

    if should_open_target {
        adapter::open_url(&mut cfg.browser, &target_url)?;
    }

    let probe = match adapter::probe(&mut cfg.browser) {
        Ok(probe) => probe,
        Err(_) => BrowserProbeResult {
            session_id: session_id.clone(),
            browser_connected: true,
            page_open: !target_url.is_empty(),
            url: target_url.clone(),
            profile: cfg.browser.profile.clone(),
            chat_input_found: false,
            chat_input_visible: false,
            chat_submit_found: false,
            ready: false,
        },
    };

    Ok(json!({
        "ok": true,
        "connected": probe.browser_connected && probe.page_open,
        "session_id": session_id,
        "target_url": if target_url.is_empty() { Value::Null } else { Value::String(target_url) },
        "ready": probe.ready,
        "probe": probe,
    }))
}

pub fn disconnect_session(cfg: &mut InferenceConfig) -> Result<Value> {
    cfg.browser.session_id = None;
    Ok(json!({ "ok": true }))
}

pub fn open_session_url(cfg: &mut InferenceConfig, url: &str) -> Result<Value> {
    ensure_live_browser_session(&mut cfg.browser)?;
    adapter::open_url(&mut cfg.browser, url)?;
    Ok(json!({ "ok": true, "url": url }))
}

pub fn probe_session(cfg: &mut InferenceConfig) -> Result<Value> {
    ensure_live_browser_session(&mut cfg.browser)?;
    let probe = adapter::probe(&mut cfg.browser)?;
    Ok(json!({ "ok": true, "probe": probe }))
}

pub fn connection_status(cfg: &mut InferenceConfig) -> Result<Value> {
    match cfg.transport {
        super::InferenceTransport::Api => Ok(json!({
            "ok": true,
            "transport": "api",
            "connected": !cfg.model.trim().is_empty(),
            "ready": !cfg.model.trim().is_empty(),
            "model": cfg.model,
        })),
        super::InferenceTransport::Browser => {
            let session_id = cfg.browser.session_id.clone().unwrap_or_default();
            if session_id.trim().is_empty() {
                return Ok(json!({
                    "ok": true,
                    "transport": "browser",
                    "connected": false,
                    "ready": false,
                    "session_id": Value::Null,
                    "probe": Value::Null,
                }));
            }

            match adapter::probe(&mut cfg.browser) {
                Ok(probe) => Ok(json!({
                    "ok": true,
                    "transport": "browser",
                    "connected": probe.browser_connected && probe.page_open,
                    "ready": probe.ready,
                    "session_id": session_id,
                    "probe": probe,
                })),
                Err(err) if is_stale_session_error(&err) => {
                    cfg.browser.session_id = None;
                    Ok(json!({
                        "ok": true,
                        "transport": "browser",
                        "connected": false,
                        "ready": false,
                        "session_id": Value::Null,
                        "probe": Value::Null,
                    }))
                }
                Err(err) => Ok(json!({
                    "ok": false,
                    "transport": "browser",
                    "connected": false,
                    "ready": false,
                    "session_id": session_id,
                    "error": format!("{:#}", err),
                }))
            }
        }
    }
}
