pub mod adapter;

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use sqlx::Row;

use super::{ensure_object_slot, persist_inference_config, BrowserConfig, BrowserProbeResult, InferenceConfig, InferenceTransport};
use super::super::registry::{find_result, CapabilityContext, CapabilityResult};
use crate::engine::capabilities::binding_specs;

fn is_stale_session_error(err: &anyhow::Error) -> bool {
    let msg = format!("{:#}", err).to_ascii_lowercase();
    msg.contains("unknown session id")
        || msg.contains("unknown session_id")
        || msg.contains("disconnected")
        || msg.contains("target page, context or browser has been closed")
        || msg.contains("target closed")
        || msg.contains("browser has been closed")
        || msg.contains("context has been closed")
        || msg.contains("page has been closed")
}

pub async fn mark_session_rearm_needed_if_browser_session_is_stale(
    state: &crate::app_state::AppState,
    run_id: uuid::Uuid,
) -> Result<bool> {
    let mut run = crate::engine::load_run(state, run_id).await?;
    let root = crate::engine::ensure_engine_root(&mut run.context);
    let global_state = ensure_object_slot(root, "global_state");
    let capabilities = ensure_object_slot(global_state, "capabilities");
    let inference_value = capabilities
        .get("inference")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let mut inference_cfg: InferenceConfig = serde_json::from_value(inference_value).unwrap_or_else(|_| InferenceConfig {
        browser: BrowserConfig::default(),
        ..InferenceConfig::default()
    });

    if !matches!(inference_cfg.transport, InferenceTransport::Browser) {
        return Ok(false);
    }

    let existing = inference_cfg.browser.session_id.clone().unwrap_or_default();
    if existing.trim().is_empty() {
        crate::engine::on_browser_session_changed(state, run_id, "missing", "").await?;
        return Ok(true);
    }

    let stale = match adapter::probe(&mut inference_cfg.browser) {
        Ok(probe) => !probe.page_open || (!probe.ready && probe.url.trim().is_empty()),
        Err(err) if is_stale_session_error(&err) => true,
        Err(err) => return Err(err),
    };

    if stale {
        crate::engine::on_browser_session_changed(state, run_id, &existing, "").await?;
    }

    Ok(stale)
}

fn ensure_live_browser_session(browser: &mut BrowserConfig) -> Result<String> {
    let existing = browser.session_id.clone().unwrap_or_default();
    if !existing.trim().is_empty() {
        match adapter::probe(browser) {
            Ok(probe) => {
                let stale = !probe.page_open || (!probe.ready && probe.url.trim().is_empty());
                if !stale {
                    tracing::info!(session_id = %existing, target_url = %browser.target_url, "reusing existing browser bridge session");
                    return Ok(existing);
                }

                tracing::warn!(
                    session_id = %existing,
                    target_url = %browser.target_url,
                    page_open = probe.page_open,
                    ready = probe.ready,
                    url = %probe.url,
                    "stored browser bridge session probe indicates a stale session; attempting cdp recovery"
                );
                browser.session_id = None;
            }
            Err(err) if is_stale_session_error(&err) => {
                tracing::warn!(session_id = %existing, error = %format!("{:#}", err), target_url = %browser.target_url, "stored browser bridge session is stale; attempting cdp recovery");
                browser.session_id = None;
            }
            Err(err) => return Err(err),
        }
    }

    let session_id = adapter::launch_and_attach(browser)?;
    tracing::info!(session_id = %session_id, previous_session_id = %existing, target_url = %browser.target_url, "browser bridge session attached");
    browser.session_id = Some(session_id.clone());
    Ok(session_id)
}

fn browser_probe_url_matches(probe_url: &str, target_url: &str) -> bool {
    let expected = target_url.trim().trim_end_matches('/');
    if expected.is_empty() {
        return true;
    }
    let actual = probe_url.trim().trim_end_matches('/');
    actual == expected
}

fn wait_for_browser_chat_ready(browser: &mut BrowserConfig, target_url: &str) -> Result<BrowserProbeResult> {
    let started = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(browser.response_timeout_ms.max(15_000));
    let poll = std::time::Duration::from_millis(browser.dom_poll_ms.clamp(250, 2_000));
    let mut last_probe: Option<BrowserProbeResult> = None;

    loop {
        match adapter::probe(browser) {
            Ok(probe) => {
                if probe.ready && browser_probe_url_matches(&probe.url, target_url) {
                    return Ok(probe);
                }
                last_probe = Some(probe);
            }
            Err(err) if is_stale_session_error(&err) => return Err(err),
            Err(err) => {
                if started.elapsed() >= timeout {
                    return Err(err);
                }
            }
        }

        if started.elapsed() >= timeout {
            if let Some(probe) = last_probe {
                return Ok(probe);
            }
            return adapter::probe(browser);
        }

        std::thread::sleep(poll);
    }
}

async fn clear_session_scoped_inference_runtime_on_new_browser_session(
    ctx: &CapabilityContext<'_>,
    previous_session_id: &str,
    next_session_id: &str,
) -> Result<()> {
    crate::engine::on_browser_session_changed(ctx.state, ctx.run_id, previous_session_id, next_session_id).await
}

async fn reset_session_scoped_shared_capability_lifecycle_on_new_browser_session(
    ctx: &CapabilityContext<'_>,
) -> Result<()> {
    let mut run = crate::engine::load_run(ctx.state, ctx.run_id).await?;
    crate::engine::shared_capability_lifecycle::reset_session_scoped_shared_capability_lifecycle(&mut run);
    crate::engine::refresh_inference_arm_state(&mut run, Some(ctx.step));
    crate::engine::persist_context(ctx.state, ctx.run_id, &run.context).await
}

fn repo_context_upload_enabled(ctx: &CapabilityContext<'_>) -> bool {
    if !binding_specs::stage_supports_shared_capability(ctx.step, "repo_context") {
        return false;
    }

    binding_specs::shared_capability_enabled(ctx.local_state, "repo_context", false)
}

fn dependency_upload_paths(ctx: &CapabilityContext<'_>, prior_results: &[CapabilityResult]) -> Result<Vec<PathBuf>> {
    if !repo_context_upload_enabled(ctx) {
        return Ok(Vec::new());
    }

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

    Ok(uploads)
}

async fn load_app_settings_value(ctx: &CapabilityContext<'_>) -> Result<Value> {
    let row = sqlx::query("SELECT settings_json FROM app_settings WHERE id = ?")
        .bind("global")
        .fetch_optional(&ctx.state.db)
        .await?;

    let value = match row {
        Some(row) => serde_json::from_str::<Value>(row.get::<String, _>("settings_json").as_str())
            .unwrap_or_else(|_| json!({})),
        None => json!({}),
    };

    Ok(value)
}

fn apply_app_browser_defaults(inference_cfg: &mut InferenceConfig, app_settings: &Value) {
    let browser_defaults = app_settings
        .get("browser")
        .and_then(Value::as_object);

    let Some(browser_defaults) = browser_defaults else {
        return;
    };

    if inference_cfg.browser.edge_executable.trim().is_empty() {
        if let Some(value) = browser_defaults.get("edge_executable_path").and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                inference_cfg.browser.edge_executable = trimmed.to_string();
            }
        }
    }

    if inference_cfg.browser.cdp_url.trim().is_empty() {
        if let Some(value) = browser_defaults.get("default_cdp_url").and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                inference_cfg.browser.cdp_url = trimmed.to_string();
            }
        }
    }

    if inference_cfg.browser.target_url.trim().is_empty() {
        if let Some(value) = browser_defaults.get("default_inference_browser_url").and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                inference_cfg.browser.target_url = trimmed.to_string();
            }
        }
    }

    if let Some(value) = browser_defaults.get("launch_on_connect").and_then(Value::as_bool) {
        inference_cfg.browser.auto_launch_edge = value;
    }
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
        .get("capabilities")
        .and_then(|v| v.get("inference"))
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_else(|| InferenceConfig {
            browser: BrowserConfig::default(),
            ..InferenceConfig::default()
        });

    let app_settings = load_app_settings_value(ctx).await.unwrap_or_else(|_| json!({}));
    apply_app_browser_defaults(&mut inference_cfg, &app_settings);

    let target_url = inference_cfg.browser.target_url.trim().to_string();
    let previous_session_id = inference_cfg.browser.session_id.clone().unwrap_or_default();
    let session_id = ensure_live_browser_session(&mut inference_cfg.browser)?;
    persist_inference_config(ctx, &inference_cfg).await?;
    clear_session_scoped_inference_runtime_on_new_browser_session(ctx, &previous_session_id, &session_id).await?;
    if previous_session_id.trim() != session_id.trim() {
        reset_session_scoped_shared_capability_lifecycle_on_new_browser_session(ctx).await?;
    }

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

    let readiness_target_url = inference_cfg.browser.target_url.trim().to_string();
    let readiness_probe = wait_for_browser_chat_ready(&mut inference_cfg.browser, &readiness_target_url)?;
    if !readiness_probe.ready || !browser_probe_url_matches(&readiness_probe.url, &readiness_target_url) {
        persist_inference_config(ctx, &inference_cfg).await?;
        return Ok(json!({
            "transport": "browser",
            "text": "",
            "conversation_id": Value::Null,
            "browser_session_id": inference_cfg.browser.session_id,
            "probe": readiness_probe,
            "ok": false,
            "message": format!(
                "Browser page is not chat-ready on the expected target URL after readiness wait; send was skipped (page_open={}, chat_input_found={}, chat_input_visible={}, url={}, expected_url={})",
                readiness_probe.page_open,
                readiness_probe.chat_input_found,
                readiness_probe.chat_input_visible,
                readiness_probe.url,
                readiness_target_url
            )
        }));
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
