pub mod adapter;
pub mod ports;

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use sqlx::Row;

use super::{prompting::ModelInput, BrowserConfig, BrowserProbeResult, InferenceConfig};
use super::session::{InferenceSession, ResolvedInferenceRoute};
use super::transport::{InferenceTransportAdapter, InferenceTransportExecution};
use super::super::registry::{find_result, CapabilityContext, CapabilityResult};
use crate::engine::{
    capabilities::{capability_enabled, context_export},
    stages::stage_supports_capability,
};

pub struct BrowserInferenceTransport;

pub static BROWSER_INFERENCE_TRANSPORT: BrowserInferenceTransport = BrowserInferenceTransport;

#[async_trait::async_trait]
impl InferenceTransportAdapter for BrowserInferenceTransport {
    async fn session_available(
        &self,
        _state: &crate::app_state::AppState,
        route: &ResolvedInferenceRoute,
        session: &InferenceSession,
    ) -> Result<bool> {
        if matches!(
            route.config.lifecycle,
            super::InferenceSessionLifecycle::Persistent
        ) {
            return Ok(true);
        }

        let session_id = session
            .transport_state
            .get("bridge_session_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        let Some(session_id) = session_id else {
            return Ok(false);
        };

        if !adapter::list_session_ids()?
            .iter()
            .any(|live_session_id| live_session_id == &session_id)
        {
            return Ok(false);
        }

        let mut browser = route.config.browser.clone();
        browser.session_id = Some(session_id);

        if let Some(cdp_url) = session
            .transport_state
            .get("cdp_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            browser.cdp_url = cdp_url.to_string();
        }

        if let Some(profile) = session
            .transport_state
            .get("profile")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            browser.profile = profile.to_string();
        }

        if let Some(conversation_url) = session
            .transport_state
            .get("conversation_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            browser.target_url = conversation_url.to_string();
        }

        match tokio::task::spawn_blocking(move || adapter::probe(&mut browser)).await {
            Ok(Ok(probe)) => Ok(probe.browser_connected && probe.page_open),
            Ok(Err(error)) if is_stale_session_error(&error) => Ok(false),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(anyhow!("Browser session probe task failed: {}", error)),
        }
    }

    fn is_session_unavailable_error(&self, error: &anyhow::Error) -> bool {
        is_stale_session_error(error)
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

fn ensure_live_browser_session(browser: &mut BrowserConfig) -> Result<String> {
    let existing = browser.session_id.clone().unwrap_or_default();

    if !existing.trim().is_empty() {
        let live_session_ids = adapter::list_session_ids()?;
        if live_session_ids
            .iter()
            .any(|session_id| session_id == existing.trim())
        {
            tracing::info!(session_id = %existing, target_url = %browser.target_url, "reusing existing browser bridge session");
            return Ok(existing);
        }

        tracing::warn!(session_id = %existing, target_url = %browser.target_url, "stored browser bridge session is absent from the bridge registry; attempting cdp recovery");
        browser.session_id = None;
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


fn repo_context_upload_enabled(ctx: &CapabilityContext<'_>) -> bool {
    stage_supports_capability(ctx.step, "repo_context")
        && capability_enabled(ctx.local_state, "repo_context", false)
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

fn repo_context_inline_prompt_enabled(ctx: &CapabilityContext<'_>) -> bool {
    if !repo_context_upload_enabled(ctx) {
        return false;
    }

    ctx.local_state
        .get("repo_context")
        .and_then(|v| v.get("inline_repo_context_in_prompt"))
        .and_then(Value::as_bool)
        .or_else(|| {
            ctx.local_state
                .get("capabilities")
                .and_then(|v| v.get("context_export"))
                .and_then(|v| v.get("inline_repo_context_in_prompt"))
                .and_then(Value::as_bool)
        })
        .unwrap_or(false)
}

fn repo_context_payload(ctx: &CapabilityContext<'_>) -> Option<Value> {
    ctx.local_state.get("repo_context").cloned().or_else(|| {
        ctx.local_state
            .get("capabilities")
            .and_then(|v| v.get("context_export"))
            .cloned()
    })
}

fn append_repo_context_prompt(prompt: &mut String, context_text: &str) {
    prompt.push_str("\n\n### REPO CONTEXT\n");
    prompt.push_str("Repository context is included inline below.\n\n");
    prompt.push_str("```text\n");
    prompt.push_str(context_text);
    if !context_text.ends_with('\n') {
        prompt.push('\n');
    }
    prompt.push_str("```");
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

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    route: &ResolvedInferenceRoute,
    logical_session: Option<&InferenceSession>,
    input: &ModelInput,
) -> Result<InferenceTransportExecution> {
    let mut inference_cfg = route.config.clone();
    let recover_stale_session = matches!(
        route.config.lifecycle,
        super::InferenceSessionLifecycle::Persistent
    );

    let app_settings = load_app_settings_value(ctx).await.unwrap_or_else(|_| json!({}));
    apply_app_browser_defaults(&mut inference_cfg, &app_settings);
    if inference_cfg.browser.profile.trim().is_empty() || inference_cfg.browser.profile.trim() == "default" || inference_cfg.browser.profile.trim() == "auto" {
        inference_cfg.browser.profile = format!("inference-{}", route.name);
    }
    inference_cfg.browser.cdp_url = ports::allocate_cdp_url_for_session(ctx, &route.name, &inference_cfg.browser.cdp_url).await?;

    let persisted_state = logical_session
        .map(|session| session.transport_state.clone())
        .unwrap_or_else(|| json!({}));

    inference_cfg.browser.session_id = persisted_state
        .get("bridge_session_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let configured_target_url = inference_cfg.browser.target_url.trim().to_string();
    let persisted_conversation_url = persisted_state
        .get("conversation_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("")
        .to_string();

    let target_url = if logical_session.is_some() && !persisted_conversation_url.is_empty() {
        persisted_conversation_url
    } else {
        configured_target_url
    };
    inference_cfg.browser.target_url = target_url.clone();

    let (browser, session_id) = tokio::task::spawn_blocking({
        let mut browser = inference_cfg.browser.clone();
        move || {
            let session_id = ensure_live_browser_session(&mut browser)?;
            Ok::<_, anyhow::Error>((browser, session_id))
        }
    })
    .await
    .map_err(|error| anyhow!("Browser session attachment task failed: {}", error))??;
    inference_cfg.browser = browser;

    let inline_repo_context = repo_context_inline_prompt_enabled(ctx);
    let mut pasted_context_sections = Vec::new();

    if inline_repo_context {
        if let Some(payload) = repo_context_payload(ctx) {
            let context = context_export::render_context_export_text(payload)?;
            if !context.trim().is_empty() {
                pasted_context_sections.push(context);
            }
        }
    }

    if let Some(compile_context) = input.pasted_context_text.as_deref() {
        if !compile_context.trim().is_empty() {
            pasted_context_sections.push(compile_context.trim().to_string());
        }
    }

    let pasted_context_text = if pasted_context_sections.is_empty() {
        None
    } else {
        Some(pasted_context_sections.join("\n\n"))
    };
    let attachment_paths = input
        .attachments
        .iter()
        .map(|attachment| attachment.path.clone())
        .collect::<Vec<_>>();
    let input_text = input.text.clone();
    let blocking_target_url = target_url.clone();
    let blocking_session_id = session_id.clone();

    let blocking_result = tokio::task::spawn_blocking(move || -> Result<_> {
        let current_probe = adapter::probe(&mut inference_cfg.browser).ok();
        let should_open_target = if blocking_target_url.is_empty() {
            false
        } else {
            match current_probe.as_ref() {
                Some(probe) => !probe.page_open,
                None => true,
            }
        };

        if should_open_target {
            if let Err(err) = adapter::open_url(&mut inference_cfg.browser, &blocking_target_url) {
                if !is_stale_session_error(&err) || !recover_stale_session {
                    return Err(err);
                }

                inference_cfg.browser.session_id = None;
                ensure_live_browser_session(&mut inference_cfg.browser)?;
                adapter::open_url(&mut inference_cfg.browser, &blocking_target_url)?;
            }
        }

        let readiness_target_url = current_probe
            .as_ref()
            .filter(|probe| probe.page_open)
            .map(|probe| probe.url.trim().to_string())
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| inference_cfg.browser.target_url.trim().to_string());
        let readiness_probe = match wait_for_browser_chat_ready(
            &mut inference_cfg.browser,
            &readiness_target_url,
        ) {
            Ok(probe) => probe,
            Err(err) if is_stale_session_error(&err) => {
                if !recover_stale_session {
                    return Err(err);
                }
                if !recover_stale_session {
                    return Err(err);
                }

                inference_cfg.browser.session_id = None;
                ensure_live_browser_session(&mut inference_cfg.browser)?;

                if !readiness_target_url.is_empty() {
                    adapter::open_url(
                        &mut inference_cfg.browser,
                        &readiness_target_url,
                    )?;
                }

                wait_for_browser_chat_ready(
                    &mut inference_cfg.browser,
                    &readiness_target_url,
                )?
            }
            Err(err) => return Err(err),
        };

        if !readiness_probe.ready
            || !browser_probe_url_matches(&readiness_probe.url, &readiness_target_url)
        {
            return Ok((
                inference_cfg,
                Some(readiness_probe),
                None,
                Vec::<String>::new(),
            ));
        }

        let pasted_context_enabled = pasted_context_text.is_some();
        let mut uploaded_files = Vec::new();

        if !pasted_context_enabled {
            for attachment_path in &attachment_paths {
                if attachment_path.exists() {
                    adapter::upload_file(
                        &mut inference_cfg.browser,
                        attachment_path.as_path(),
                    )?;
                    uploaded_files.push(attachment_path.to_string_lossy().to_string());
                }
            }
        }

        let result = match adapter::send_chat_and_wait_with_pasted_context(
            &mut inference_cfg.browser,
            input_text.as_str(),
            pasted_context_text.as_deref(),
        ) {
            Ok(result) => result,
            Err(err) if is_stale_session_error(&err) => {
                if !recover_stale_session {
                    return Err(err);
                }
                if !recover_stale_session {
                    return Err(err);
                }

                inference_cfg.browser.session_id = None;
                ensure_live_browser_session(&mut inference_cfg.browser)?;

                if !blocking_target_url.is_empty() {
                    adapter::open_url(
                        &mut inference_cfg.browser,
                        &blocking_target_url,
                    )?;
                }

                wait_for_browser_chat_ready(
                    &mut inference_cfg.browser,
                    &blocking_target_url,
                )?;

                if !pasted_context_enabled {
                    for attachment_path in &attachment_paths {
                        if attachment_path.exists() {
                            adapter::upload_file(
                                &mut inference_cfg.browser,
                                attachment_path.as_path(),
                            )?;
                        }
                    }
                }

                adapter::send_chat_and_wait_with_pasted_context(
                    &mut inference_cfg.browser,
                    input_text.as_str(),
                    pasted_context_text.as_deref(),
                )?
            }
            Err(err) => return Err(err),
        };

        let probe = match adapter::probe(&mut inference_cfg.browser) {
            Ok(probe) => probe,
            Err(_) => BrowserProbeResult {
                session_id: blocking_session_id,
                browser_connected: true,
                page_open: !blocking_target_url.is_empty(),
                url: blocking_target_url,
                profile: inference_cfg.browser.profile.clone(),
                chat_input_found: false,
                chat_input_visible: false,
                chat_submit_found: false,
                ready: false,
            },
        };

        Ok((inference_cfg, None, Some((result, probe)), uploaded_files))
    })
    .await
    .map_err(|error| anyhow!("Browser bridge blocking task failed: {}", error))??;

    let (inference_cfg, readiness_failure, completed, uploaded_files) = blocking_result;
    let final_session_id = inference_cfg.browser.session_id.clone().unwrap_or(session_id);
    let cdp_url = inference_cfg.browser.cdp_url.clone();
    let debug_port = cdp_url.rsplit(':').next().map(str::to_string);

    if let Some(readiness_probe) = readiness_failure {
        let readiness_target_url = inference_cfg.browser.target_url.trim().to_string();
        let transport_state = json!({
            "bridge_session_id": final_session_id,
            "conversation_url": readiness_probe.url,
            "cdp_url": cdp_url,
            "debug_port": debug_port,
            "profile": inference_cfg.browser.profile
        });
        return Ok(InferenceTransportExecution {
            text: String::new(),
            metadata: json!({
                "transport": "browser",
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
            }),
            transport_state,
        });
    }

    let (result, probe) = completed
        .ok_or_else(|| anyhow!("Browser bridge blocking task returned no result"))?;

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

    let conversation_url = if probe.url.trim().is_empty() {
        target_url
    } else {
        probe.url.clone()
    };

    Ok(InferenceTransportExecution {
        text: bridge_result
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        metadata: json!({
            "ok": bridge_result.get("ok").and_then(Value::as_bool).unwrap_or(true),
            "transport": result.transport,
            "conversation_id": result.conversation_id,
            "browser_session_id": result.browser_session_id,
            "probe": probe,
            "uploaded_files": uploaded_files,
            "repo_context_inline_prompt": inline_repo_context,
            "send": bridge_result.get("send").cloned().unwrap_or(Value::Null),
            "read": bridge_result.get("read").cloned().unwrap_or(Value::Null)
        }),
        transport_state: json!({
            "bridge_session_id": final_session_id,
            "conversation_url": conversation_url,
            "cdp_url": cdp_url,
            "debug_port": debug_port,
            "profile": inference_cfg.browser.profile
        }),
    })
}
