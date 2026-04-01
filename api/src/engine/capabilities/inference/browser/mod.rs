pub mod adapter;

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use super::{persist_inference_config, BrowserConfig, BrowserProbeResult, InferenceConfig};
use super::super::{
    context_export,
    registry::{find_result, CapabilityContext, CapabilityResult},
};

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

fn dependency_upload_paths(ctx: &CapabilityContext<'_>, prior_results: &[CapabilityResult]) -> Result<Vec<PathBuf>> {
    let mut uploads = Vec::new();

    if let Some(result) = find_result(prior_results, "context_export") {
        if result.ok {
            if let Some(path) = result.payload.get("output_path").and_then(Value::as_str) {
                let trimmed = path.trim();
                if !trimmed.is_empty() {
                    uploads.push(PathBuf::from(trimmed));
                }
            }
        }
    }

    if uploads.is_empty() {
        if let Some(repo_context) = ctx.local_state.get("repo_context").cloned() {
            let rendered = context_export::render_context_export_text(repo_context)?;
            let mut temp_path = std::env::temp_dir();
            temp_path.push(format!("repo_context_{}.txt", ctx.run_id));
            std::fs::write(&temp_path, rendered.as_bytes())?;
            uploads.push(temp_path);
        }
    }

    Ok(uploads)
}

pub async fn execute(ctx: &CapabilityContext<'_>, prior_results: &[CapabilityResult]) -> Result<serde_json::Value> {
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

    let target_url = inference_cfg.browser.target_url.trim().to_string();
    let session_id = ensure_live_browser_session(&mut inference_cfg.browser)?;

    let current_probe = adapter::probe(&mut inference_cfg.browser).ok();
    let should_open_target = if target_url.is_empty() {
        false
    } else {
        match current_probe.as_ref() {
            Some(probe) => !probe.page_open || probe.url != target_url,
            None => true,
        }
    };

    if should_open_target {
        adapter::open_url(&mut inference_cfg.browser, &target_url)?;
    }

    let current_probe = adapter::probe(&mut inference_cfg.browser).ok();
    if let Some(probe) = current_probe.as_ref() {
        if !probe.ready {
            persist_inference_config(ctx, &inference_cfg).await?;
            return Ok(json!({
                "transport": "browser",
                "text": "",
                "conversation_id": Value::Null,
                "browser_session_id": inference_cfg.browser.session_id,
                "probe": probe,
                "ok": false,
                "message": format!(
                    "Browser page is not chat-ready; send was skipped (page_open={}, chat_input_found={}, chat_input_visible={}, url={})",
                    probe.page_open,
                    probe.chat_input_found,
                    probe.chat_input_visible,
                    probe.url
                )
            }));
        }
    }

    let upload_paths = dependency_upload_paths(ctx, prior_results)?;
    let mut uploaded_files = Vec::new();
    for upload_path in upload_paths.iter() {
        if upload_path.exists() {
            adapter::upload_file(&mut inference_cfg.browser, upload_path.as_path())?;
            uploaded_files.push(upload_path.to_string_lossy().to_string());
        }
    }

    let result = adapter::send_chat_and_wait(&mut inference_cfg.browser, &prompt)?;
    persist_inference_config(ctx, &inference_cfg).await?;

    let probe = match adapter::probe(&mut inference_cfg.browser) {
        Ok(probe) => probe,
        Err(_) => BrowserProbeResult {
            session_id,
            browser_connected: true,
            page_open: !target_url.is_empty(),
            url: target_url,
            profile: inference_cfg.browser.profile.clone(),
            chat_input_found: false,
            chat_input_visible: false,
            chat_submit_found: false,
            ready: false,
        },
    };

    let bridge_result = serde_json::from_str::<Value>(&result.text).unwrap_or_else(|_| json!({
        "ok": true,
        "text": result.text,
        "send": Value::Null,
        "read": Value::Null,
    }));
    let send_sent = bridge_result
        .get("send")
        .and_then(|v| v.get("sent"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !send_sent {
        return Err(anyhow!("Browser bridge did not send chat successfully"));
    }

    Ok(json!({
        "ok": bridge_result.get("ok").and_then(Value::as_bool).unwrap_or(true),
        "transport": result.transport,
        "text": bridge_result.get("text").and_then(Value::as_str).unwrap_or(""),
        "conversation_id": result.conversation_id,
        "browser_session_id": result.browser_session_id,
        "probe": probe,
        "uploaded_files": uploaded_files,
        "send": bridge_result.get("send").cloned().unwrap_or(Value::Null),
        "read": bridge_result.get("read").cloned().unwrap_or(Value::Null)
    }))
}
