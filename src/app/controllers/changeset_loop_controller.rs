
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::app::actions::{Action, ComponentId};
use crate::app::browser_bridge::{browser_model_options, close_session_page, launch_and_attach, open_url_in_session, probe_session, send_chat_and_wait, set_runtime_timeout_secs, timeout_runtime_now, BrowserTurnConfig};
use crate::app::state::{AppState, BrowserBridgeStatus, ExecuteLoopMessage, ExecuteLoopMode, ExecuteLoopTransport};

fn browser_runtime_dir(state: &AppState) -> PathBuf {
    let mut dir = state
        .platform
        .app_data_dir("DescribeRepo")
        .unwrap_or_else(|_| std::env::temp_dir());
    dir.push("browser");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn resolve_browser_bridge_dir() -> String {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join("bridge");
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("bridge");
        if candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }

    "bridge".to_string()
}

fn resolve_browser_executable(explicit: &str) -> (String, String) {
    let explicit = explicit.trim();
    if !explicit.is_empty() {
        let lower = explicit.to_ascii_lowercase();
        let channel = if lower.contains("edge") || lower.contains("msedge") {
            "msedge"
        } else if lower.contains("chrome") {
            "chrome"
        } else {
            "chromium"
        };
        return (explicit.to_string(), channel.to_string());
    }

    if cfg!(target_os = "windows") {
        for key in ["PROGRAMFILES(X86)", "PROGRAMFILES"] {
            if let Ok(root) = std::env::var(key) {
                let edge = PathBuf::from(&root).join("Microsoft/Edge/Application/msedge.exe");
                if edge.exists() {
                    return (edge.to_string_lossy().into_owned(), "msedge".to_string());
                }
                let chrome = PathBuf::from(&root).join("Google/Chrome/Application/chrome.exe");
                if chrome.exists() {
                    return (chrome.to_string_lossy().into_owned(), "chrome".to_string());
                }
                let chromium = PathBuf::from(&root).join("Chromium/Application/chrome.exe");
                if chromium.exists() {
                    return (chromium.to_string_lossy().into_owned(), "chromium".to_string());
                }
            }
        }
        return ("msedge.exe".to_string(), "msedge".to_string());
    }

    if cfg!(target_os = "macos") {
        let edge = PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge");
        if edge.exists() {
            return (edge.to_string_lossy().into_owned(), "msedge".to_string());
        }
        let chrome = PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
        if chrome.exists() {
            return (chrome.to_string_lossy().into_owned(), "chrome".to_string());
        }
        let chromium = PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium");
        if chromium.exists() {
            return (chromium.to_string_lossy().into_owned(), "chromium".to_string());
        }
        return ("Microsoft Edge".to_string(), "msedge".to_string());
    }

    for candidate in [
        ("microsoft-edge", "msedge"),
        ("microsoft-edge-stable", "msedge"),
        ("msedge", "msedge"),
        ("google-chrome", "chrome"),
        ("google-chrome-stable", "chrome"),
        ("chromium-browser", "chromium"),
        ("chromium", "chromium")
    ] {
        if std::process::Command::new("which")
            .arg(candidate.0)
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
        {
            return (candidate.0.to_string(), candidate.1.to_string());
        }
    }

    ("microsoft-edge".to_string(), "msedge".to_string())
}

fn resolve_user_data_dir(state: &AppState, explicit: &str, browser_family: &str) -> String {
    let explicit = explicit.trim();
    if !explicit.is_empty() {
        let path = PathBuf::from(explicit);
        let _ = std::fs::create_dir_all(&path);
        return path.to_string_lossy().into_owned();
    }

    let mut dir = browser_runtime_dir(state);
    dir.push("profiles");
    dir.push(browser_family);
    let _ = std::fs::create_dir_all(&dir);
    dir.to_string_lossy().into_owned()
}

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {

        Action::ExecuteLoopBrowserLaunchAndAttach { loop_id } => {
            let Some(st) = state.execute_loops.get(loop_id) else {
                return false;
            };

            if st.transport != ExecuteLoopTransport::BrowserBridge {
                return false;
            }

            let edge_executable_override = st.browser_edge_executable.clone();
            let user_data_dir_override = st.browser_user_data_dir.clone();
            let cdp_url = st.browser_cdp_url.clone();
            let page_url_contains = st.browser_page_url_contains.clone();
            let auto_launch_edge = st.browser_auto_launch_edge;
            let bridge_dir = resolve_browser_bridge_dir();
            let (edge_executable, browser_profile) = resolve_browser_executable(&edge_executable_override);
            let user_data_dir = resolve_user_data_dir(state, &user_data_dir_override, &browser_profile);

            let mut cfg = BrowserTurnConfig {
                bridge_dir,
                edge_executable,
                user_data_dir,
                cdp_url,
                page_url_contains: String::new(),
                profile: browser_profile.clone(),
                session_id: None,
                auto_launch_edge,
                runtime_key: String::new(),
                response_timeout_ms: st.browser_response_timeout_ms,
                response_poll_ms: st.browser_response_poll_ms,
            };

            match launch_and_attach(&mut cfg) {
                Ok(session_id) => {
                    if let Some(stm) = state.execute_loops.get_mut(loop_id) {
                        stm.browser_session_id = Some(session_id.clone());
                        stm.browser_status = BrowserBridgeStatus::Attached;
                        stm.browser_attached = true;
                        stm.browser_last_probe = None;
                        stm.browser_probe_pending = false;
                        stm.browser_probe_error = None;
                        stm.browser_profile = browser_profile.clone();
                        stm.model = "browser-web".to_string();
                        stm.model_options = browser_model_options();
                        stm.last_status = Some(format!("Browser attached: {}", session_id));
                    }
                }
                Err(e) => {
                    if let Some(stm) = state.execute_loops.get_mut(loop_id) {
                        stm.browser_status = BrowserBridgeStatus::Detached;
                        stm.browser_attached = false;
                        stm.browser_session_id = None;
                        stm.last_status = Some(format!("Browser attach failed: {:#}", e));
                    }
                }
            }
            true
        }

        Action::ExecuteLoopBrowserOpenUrl { loop_id } => {
            let Some(st) = state.execute_loops.get(loop_id) else {
                return false;
            };

            if st.transport != ExecuteLoopTransport::BrowserBridge {
                return false;
            }

            let Some(session_id) = st.browser_session_id.clone() else {
                if let Some(stm) = state.execute_loops.get_mut(loop_id) {
                    stm.last_status = Some("Attach a browser session before opening a URL.".to_string());
                }
                return true;
            };

            let target_url = st.browser_target_url.trim().to_string();
            if target_url.is_empty() {
                if let Some(stm) = state.execute_loops.get_mut(loop_id) {
                    stm.last_status = Some("Enter a URL before opening a new tab.".to_string());
                }
                return true;
            };

            let bridge_dir = if st.browser_bridge_dir.trim().is_empty() {
                resolve_browser_bridge_dir()
            } else {
                st.browser_bridge_dir.clone()
            };
            let (edge_executable, browser_profile) = resolve_browser_executable(&st.browser_edge_executable);
            let user_data_dir = resolve_user_data_dir(state, &st.browser_user_data_dir, &browser_profile);

            let mut cfg = BrowserTurnConfig {
                bridge_dir,
                edge_executable,
                user_data_dir,
                cdp_url: st.browser_cdp_url.clone(),
                page_url_contains: target_url.clone(),
                profile: browser_profile.clone(),
                session_id: Some(session_id),
                auto_launch_edge: false,
                runtime_key: String::new(),
                response_timeout_ms: st.browser_response_timeout_ms,
                response_poll_ms: st.browser_response_poll_ms,
            };

            match open_url_in_session(&mut cfg, &target_url) {
                Ok(()) => {
                    match probe_session(&mut cfg) {
                        Ok(probe) => {
                            if let Some(stm) = state.execute_loops.get_mut(loop_id) {
                                stm.browser_attached = true;
                                stm.browser_last_probe = Some(probe.clone());
                                stm.browser_probe_pending = false;
                                stm.browser_probe_error = None;
                                stm.browser_page_url_contains = probe.url.clone();
                                stm.browser_profile = browser_profile.clone();
                                stm.model_options = browser_model_options();
                                stm.model = "browser-web".to_string();
                                stm.browser_status = if probe.ready {
                                    BrowserBridgeStatus::Ready
                                } else {
                                    BrowserBridgeStatus::Attached
                                };
                                stm.last_status = Some(if probe.ready {
                                    format!("Opened {} and browser page is chat-ready.", target_url)
                                } else {
                                    format!("Opened {} in a new browser tab.", target_url)
                                });
                            }
                        }
                        Err(e) => {
                            if let Some(stm) = state.execute_loops.get_mut(loop_id) {
                                stm.browser_status = BrowserBridgeStatus::Attached;
                                stm.browser_attached = true;
                                stm.browser_page_url_contains = target_url.clone();
                                stm.browser_profile = browser_profile.clone();
                                stm.model_options = browser_model_options();
                                stm.model = "browser-web".to_string();
                                stm.browser_last_probe = None;
                                stm.browser_probe_pending = false;
                                stm.browser_probe_error = Some(format!("{:#}", e));
                                stm.last_status = Some(format!("Opened {} in a new browser tab.", target_url));
                            }
                        }
                    }
                }
                Err(e) => {
                    if let Some(stm) = state.execute_loops.get_mut(loop_id) {
                        stm.last_status = Some(format!("Open URL failed: {:#}", e));
                    }
                }
            }
            true
        }

        Action::ExecuteLoopBrowserProbe { loop_id } => {
            let Some(st) = state.execute_loops.get(loop_id) else {
                return false;
            };

            if st.transport != ExecuteLoopTransport::BrowserBridge {
                return false;
            }

            let Some(session_id) = st.browser_session_id.clone() else {
                if let Some(stm) = state.execute_loops.get_mut(loop_id) {
                    stm.browser_status = BrowserBridgeStatus::Detached;
                    stm.browser_attached = false;
                    stm.browser_last_probe = None;
                    stm.browser_probe_pending = false;
                    stm.browser_probe_error = Some("No browser session attached.".to_string());
                    stm.last_status = Some("No browser session attached.".to_string());
                }
                return true;
            };

            let bridge_dir = if st.browser_bridge_dir.trim().is_empty() {
                resolve_browser_bridge_dir()
            } else {
                st.browser_bridge_dir.clone()
            };
            let (edge_executable, browser_profile) = resolve_browser_executable(&st.browser_edge_executable);
            let user_data_dir = resolve_user_data_dir(state, &st.browser_user_data_dir, &browser_profile);

            let mut cfg = BrowserTurnConfig {
                bridge_dir,
                edge_executable,
                user_data_dir,
                cdp_url: st.browser_cdp_url.clone(),
                page_url_contains: st.browser_page_url_contains.clone(),
                profile: browser_profile.clone(),
                session_id: Some(session_id),
                auto_launch_edge: false,
                runtime_key: String::new(),
                response_timeout_ms: st.browser_response_timeout_ms,
                response_poll_ms: st.browser_response_poll_ms,
            };

            match probe_session(&mut cfg) {
                Ok(probe) => {
                    if let Some(stm) = state.execute_loops.get_mut(loop_id) {
                        let was_ready = stm.browser_status == BrowserBridgeStatus::Ready;
                        stm.browser_attached = true;
                        stm.browser_last_probe = Some(probe.clone());
                        stm.browser_probe_pending = false;
                        stm.browser_probe_error = None;
                        stm.browser_profile = browser_profile.clone();
                        stm.browser_page_url_contains = probe.url.clone();
                        stm.browser_status = if probe.ready {
                            BrowserBridgeStatus::Ready
                        } else if !probe.page_open {
                            BrowserBridgeStatus::Attached
                        } else if was_ready && probe.chat_input_found {
                            BrowserBridgeStatus::Ready
                        } else {
                            BrowserBridgeStatus::Attached
                        };
                        stm.last_status = Some(if probe.ready {
                            "Browser page is chat-ready.".to_string()
                        } else if !probe.page_open {
                            "Browser is still attached, but no active tab is open.".to_string()
                        } else if was_ready && probe.chat_input_found {
                            "Browser page remains ready while the chat input settles.".to_string()
                        } else {
                            "Browser page attached but not chat-ready.".to_string()
                        });
                    }
                }
                Err(e) => {
                    if let Some(stm) = state.execute_loops.get_mut(loop_id) {
                        let msg = format!("{:#}", e);
                        let stale = msg.to_ascii_lowercase().contains("unknown session_id") || msg.to_ascii_lowercase().contains("disconnected");
                        if stale {
                            stm.browser_session_id = None;
                            stm.browser_status = BrowserBridgeStatus::Detached;
                            stm.browser_attached = false;
                        } else {
                            stm.browser_status = BrowserBridgeStatus::Attached;
                            stm.browser_attached = true;
                        }
                        stm.browser_last_probe = None;
                        stm.browser_probe_pending = false;
                        stm.browser_probe_error = Some(msg.clone());
                        stm.last_status = Some(format!("Browser probe failed: {}", msg));
                    }
                }
            }
            true
        }

        Action::ExecuteLoopBrowserDetach { loop_id } => {
            let Some(st) = state.execute_loops.get(loop_id) else {
                return false;
            };

            let Some(session_id) = st.browser_session_id.clone() else {
                if let Some(stm) = state.execute_loops.get_mut(loop_id) {
                    stm.browser_status = BrowserBridgeStatus::Detached;
                    stm.browser_attached = false;
                    stm.browser_page_url_contains.clear();
                    stm.browser_last_probe = None;
                    stm.browser_probe_pending = false;
                    stm.browser_probe_error = None;
                    stm.last_status = Some("Browser tab detached".to_string());
                }
                return true;
            };

            let bridge_dir = if st.browser_bridge_dir.trim().is_empty() {
                resolve_browser_bridge_dir()
            } else {
                st.browser_bridge_dir.clone()
            };
            let (edge_executable, browser_profile) = resolve_browser_executable(&st.browser_edge_executable);
            let user_data_dir = resolve_user_data_dir(state, &st.browser_user_data_dir, &browser_profile);

            let mut cfg = BrowserTurnConfig {
                bridge_dir,
                edge_executable,
                user_data_dir,
                cdp_url: st.browser_cdp_url.clone(),
                page_url_contains: st.browser_page_url_contains.clone(),
                profile: browser_profile.clone(),
                session_id: Some(session_id),
                auto_launch_edge: false,
                runtime_key: String::new(),
                response_timeout_ms: st.browser_response_timeout_ms,
                response_poll_ms: st.browser_response_poll_ms,
            };

            let close_result = close_session_page(&mut cfg);

            if let Some(stm) = state.execute_loops.get_mut(loop_id) {
                stm.browser_status = BrowserBridgeStatus::Attached;
                stm.browser_attached = true;
                stm.browser_page_url_contains.clear();
                stm.browser_last_probe = None;
                stm.browser_probe_pending = false;
                stm.browser_probe_error = close_result.as_ref().err().map(|e| format!("{:#}", e));
                stm.last_status = match close_result {
                    Ok(()) => Some("Browser tab detached".to_string()),
                    Err(e) => Some(format!("Browser tab detach failed: {:#}", e)),
                };
            }
            true
        }

        Action::ExecuteLoopSend { loop_id } => {
            let blocked = state
                .execute_loops
                .get(loop_id)
                .map(|st| st.transport == ExecuteLoopTransport::BrowserBridge && st.browser_status != BrowserBridgeStatus::Ready)
                .unwrap_or(false);
            if blocked {
                if let Some(st) = state.execute_loops.get_mut(loop_id) {
                    st.last_status = Some("Browser page is not chat-ready. Probe or open a valid chat page before sending.".to_string());
                }
                return true;
            }
            send_turn(state, *loop_id);
            true
        }

        Action::ExecuteLoopRunPostprocess { loop_id } => {
            start_postprocess(state, *loop_id);
            true
        }

        Action::ExecuteLoopSetMode { loop_id, mode } => {
            if let Some(st) = state.execute_loops.get_mut(loop_id) {
                st.mode = *mode;
                st.awaiting_review = false;
                st.last_status = Some(match mode {
                    ExecuteLoopMode::Conversation => "Mode: Conversation".to_string(),
                    ExecuteLoopMode::ChangeSet => "Mode: ChangeSet".to_string(),
                });

                if *mode == ExecuteLoopMode::ChangeSet {
                    st.include_context_next = true;
                }
            }
            true
        }

        Action::ExecuteLoopInjectContext { loop_id } => {

            let ctx_text = match state.generate_current_context_text() {
                Ok(t) => t,
                Err(e) => {
                    if let Some(st) = state.execute_loops.get_mut(loop_id) {
                        st.last_status = Some(format!("Context generation failed: {:#}", e));
                    }
                    return true;
                }
            };

            if let Some(st) = state.execute_loops.get_mut(loop_id) {
                if st.draft.trim().is_empty() {
                    st.draft = format!("CONTEXT UPDATE:\n{}\n", ctx_text);
                } else {
                    st.draft = format!("CONTEXT UPDATE:\n{}\n\n{}", ctx_text, st.draft);
                }
                st.last_status = Some("Context prepared in draft (user message).".to_string());
            }
            true
        }

        Action::ExecuteLoopClearChat { loop_id } => {
            use std::time::{SystemTime, UNIX_EPOCH};

            let now_ms: u64 = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            let sys_content = state
                .execute_loops
                .get(loop_id)
                .map(|st| st.instruction.clone())
                .unwrap_or_default();

            let cleared_messages = vec![ExecuteLoopMessage {
                role: "system".to_string(),
                content: sys_content,
            }];

            if let Some(st) = state.execute_loops.get_mut(loop_id) {
                st.messages = cleared_messages.clone();
                st.draft.clear();
                st.awaiting_review = false;
                st.pending = false;
                st.pending_rx = None;
                st.active_browser_runtime_key = None;

                st.conversation_id = None;

                st.postprocess_pending = false;
                st.postprocess_rx = None;

                st.last_auto_applier_id = None;
                st.last_auto_applier_status = None;
                st.awaiting_apply_result = false;

                st.last_status = Some("Chat cleared.".to_string());
            }

            let bound_task_id = state
                .tasks
                .iter()
                .find_map(|(tid, t)| (t.bound_execute_loop == Some(*loop_id)).then_some(*tid));

            if let Some(tid) = bound_task_id {
                let active_cid = state.tasks.get(&tid).and_then(|t| t.active_conversation);
                if let Some(cid) = active_cid {
                    if let Some(t) = state.tasks.get_mut(&tid) {
                        if let Some(snap) = t.conversations.get_mut(&cid) {
                            snap.messages = cleared_messages;
                            snap.conversation_id = None;
                            snap.updated_at_ms = now_ms;
                        }
                    }

                    state.task_store_dirty = true;
                    state.save_repo_task_store();
                }
            }

            true
        }

        Action::ExecuteLoopMarkReviewed { loop_id } => {
            if let Some(st) = state.execute_loops.get_mut(loop_id) {
                st.awaiting_review = false;
                st.last_status = Some("Reviewed. Ready.".to_string());
            }
            true
        }

        Action::ExecuteLoopClear { loop_id } => {
            if let Some(st) = state.execute_loops.get_mut(loop_id) {
                st.iterations.clear();
                st.last_status = Some("Iterations cleared.".to_string());
            }
            true
        }

        _ => false,
    }
}

fn send_turn(state: &mut AppState, loop_id: ComponentId) {
    let busy = state
        .execute_loops
        .get(&loop_id)
        .map(|st| st.pending || st.postprocess_pending)
        .unwrap_or(false);
    if busy {
        if let Some(st) = state.execute_loops.get_mut(&loop_id) {
            st.last_status = Some("Already waiting…".to_string());
        }
        return;
    }

    let (model, mode, instruction, draft, _existing_msgs, include_context_next, conversation_id) = {
        let Some(st) = state.execute_loops.get(&loop_id) else {
            return;
        };
        (
            st.model.clone(),
            st.mode,
            st.instruction.clone(),
            st.draft.clone(),
            st.messages.clone(),
            st.include_context_next,
            st.conversation_id.clone(),
        )
    };

    if draft.trim().is_empty() {
        if let Some(st) = state.execute_loops.get_mut(&loop_id) {
            st.last_status = Some("Nothing to send (draft is empty).".to_string());
        }
        return;
    }

    let transport = state
        .execute_loops
        .get(&loop_id)
        .map(|ls| ls.transport)
        .unwrap_or(ExecuteLoopTransport::Api);

    let fetched_models = {
        let need_fetch = state
            .execute_loops
            .get(&loop_id)
            .map(|ls| ls.model_options.is_empty())
            .unwrap_or(false);
        if need_fetch {
            match transport {
                ExecuteLoopTransport::Api => state.openai.list_models().ok(),
                ExecuteLoopTransport::BrowserBridge => state
                    .execute_loops
                    .get(&loop_id)
                    .map(|_| browser_model_options()),
            }
        } else {
            None
        }
    };

    let is_new_conversation = conversation_id.is_none();

    let ctx_text_opt = if is_new_conversation && include_context_next {
        match state.generate_current_context_text() {
            Ok(t) => Some(t),
            Err(e) => {
                if let Some(st) = state.execute_loops.get_mut(&loop_id) {
                    st.last_status = Some(format!("Context generation failed: {:#}", e));
                }
                None
            }
        }
    } else {
        None
    };

    let seed_items_if_new: Vec<(String, String)> = if is_new_conversation {
        let schema = crate::app::ui::changeset_applier::CHANGESET_SCHEMA_EXAMPLE;
        let mut sys = String::new();
        if !instruction.trim().is_empty() {
            sys.push_str(&instruction);
        }

        if let Some(ctx_text) = &ctx_text_opt {
            sys.push_str("\n\nREPO CONTEXT (generated):\n");
            sys.push_str(ctx_text);
        }

        sys.push_str("\n\nCHANGESET MODE CONTRACT:\n");
        sys.push_str("- When the user sends MODE: changeset, return ONLY ONE valid JSON object matching the ChangeSet schema.\n");
        sys.push_str("\nSCHEMA EXAMPLE (copy this structure exactly):\n");
        sys.push_str(schema);

        vec![
          (
            "system".to_string(),
            sys
          )
        ]
    } else {
        Vec::new()
    };

    let mode_header = match mode {
        ExecuteLoopMode::Conversation => "Conversation mode: please discuss coding design and do not provide any changeset payloads",
        ExecuteLoopMode::ChangeSet => "Changeset mode: please provide only strict JSON changeset format and do not waste any token's inserting comments into the code",
    };

    let user_payload = format!("{}\n\n{}", mode_header, draft.trim());

    let mut turn_items: Vec<(String, String)> = Vec::new();
    turn_items.push(("user".to_string(), user_payload));

    let seed_items_for_api = seed_items_if_new.clone();

    let mut active_browser_runtime_key: Option<String> = None;

    let (tx, rx) = mpsc::channel::<Result<crate::app::state::ExecuteLoopTurnResult, String>>();

    match transport {
        ExecuteLoopTransport::Api => {
            let openai = state.openai.clone();
            std::thread::spawn(move || {
                let res = openai
                    .chat_in_conversation(&model, conversation_id, seed_items_for_api, turn_items)
                    .map(|(text, conv_id)| crate::app::state::ExecuteLoopTurnResult {
                        text,
                        conversation_id: Some(conv_id),
                        browser_session_id: None,
                    })
                    .map_err(|e| format!("{:#}", e));
                let _ = tx.send(res);
            });
        }
        ExecuteLoopTransport::BrowserBridge => {
            let ready = state
                .execute_loops
                .get(&loop_id)
                .map(|st| st.browser_status == BrowserBridgeStatus::Ready)
                .unwrap_or(false);
            if !ready {
                if let Some(st) = state.execute_loops.get_mut(&loop_id) {
                    st.last_status = Some("Browser page is not chat-ready. Probe or open a valid chat page before sending.".to_string());
                }
                return;
            }
            let (cdp_url, page_match, edge_exe_override, user_data_dir_override, session_id, response_timeout_ms, response_poll_ms) = {
                let Some(st) = state.execute_loops.get(&loop_id) else {
                    return;
                };
                (
                    st.browser_cdp_url.clone(),
                    st.browser_page_url_contains.clone(),
                    st.browser_edge_executable.clone(),
                    st.browser_user_data_dir.clone(),
                    st.browser_session_id.clone(),
                    st.browser_response_timeout_ms,
                    st.browser_response_poll_ms,
                )
            };
            let bridge_dir = resolve_browser_bridge_dir();
            let (edge_exe, browser_channel) = resolve_browser_executable(&edge_exe_override);
            let user_data_dir = resolve_user_data_dir(state, &user_data_dir_override, &browser_channel);

            let mut prompt = String::new();
            if is_new_conversation {
                for (role, content) in seed_items_if_new.iter() {
                    prompt.push_str(&role.to_uppercase());
                    prompt.push_str(":\n");
                    prompt.push_str(content);
                    prompt.push_str("\n\n");
                }
            }
            for (role, content) in turn_items.iter() {
                prompt.push_str(&role.to_uppercase());
                prompt.push_str(":\n");
                prompt.push_str(content);
                prompt.push_str("\n\n");
            }

            let runtime_nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let runtime_key = format!("execute-loop-{}-{}", loop_id, runtime_nonce);
            active_browser_runtime_key = Some(runtime_key.clone());
            set_runtime_timeout_secs(&runtime_key, (response_timeout_ms.max(1000) + 999) / 1000);
            std::thread::spawn(move || {
                let mut cfg = BrowserTurnConfig {
                    bridge_dir,
                    edge_executable: edge_exe,
                    user_data_dir,
                    cdp_url,
                    page_url_contains: page_match.clone(),
                    profile: browser_channel,
                    session_id,
                    auto_launch_edge: false,
                    runtime_key: runtime_key.clone(),
                    response_timeout_ms,
                    response_poll_ms,
                };
                let res = send_chat_and_wait(&mut cfg, &prompt).map_err(|e| format!("{:#}", e));
                timeout_runtime_now(&runtime_key);
                let _ = tx.send(res);
            });
        }
    }

    if is_new_conversation {
        if let Some(st) = state.execute_loops.get_mut(&loop_id) {
            if let Some((_, sys_content)) = seed_items_if_new.first() {
                if st.messages.is_empty() {
                    st.messages.push(ExecuteLoopMessage {
                        role: "system".to_string(),
                        content: sys_content.clone(),
                    });
                } else {
                    if st.messages[0].role == "system" {
                        st.messages[0].content = sys_content.clone();
                    } else {
                        st.messages.insert(
                            0,
                            ExecuteLoopMessage {
                                role: "system".to_string(),
                                content: sys_content.clone(),
                            },
                        );
                    }
                }
            }
            st.include_context_next = false;
        }
    }

    let mut _did_mutate = false;


    {
        let Some(st) = state.execute_loops.get_mut(&loop_id) else {
            return;
        };

        if let Some(mut ms) = fetched_models {
            ms.sort();
            ms.dedup();
            st.model_options = ms;
            if !st.model_options.is_empty() && !st.model_options.iter().any(|m| m == &st.model) {
                st.model = st.model_options[0].clone();
            }
        }
        if st.transport == ExecuteLoopTransport::BrowserBridge {
            st.last_status = Some("Waiting for browser bridge response via browser bridge...".to_string());
        }

        st.messages.push(ExecuteLoopMessage {
            role: "user".to_string(),
            content: st.draft.trim().to_string(),
        });
        st.draft.clear();

        st.include_context_next = false;

        st.pending = true;
        st.pending_rx = Some(rx);
        st.active_browser_runtime_key = active_browser_runtime_key;

        st.last_status = Some("Waiting for response…".to_string());
        _did_mutate = true;
    }

    if _did_mutate {
        state.task_store_dirty = true;
        state.save_repo_task_store();
    }
}

fn start_postprocess(state: &mut AppState, loop_id: ComponentId) {
    let (cmd, cwd, already_pending) = {
        let Some(st) = state.execute_loops.get(&loop_id) else {
            return;
        };
        let pending = st.postprocess_pending;
        let cmd = st.postprocess_cmd.trim().to_string();
        let cwd = state
            .inputs
            .local_repo
            .clone()
            .or_else(|| state.inputs.repo.clone())
            .unwrap_or_else(|| PathBuf::from("."));
        (cmd, cwd, pending)
    };

    if already_pending {
        if let Some(st) = state.execute_loops.get_mut(&loop_id) {
            st.last_status = Some("Postprocess already running…".to_string());
        }
        return;
    }

    if cmd.is_empty() {
        if let Some(st) = state.execute_loops.get_mut(&loop_id) {
            st.last_status = Some("Postprocess command is empty.".to_string());
        }
        return;
    }

    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    let cmd_for_thread = cmd.clone();

    std::thread::spawn(move || {
        let out = run_command_best_effort(&cmd_for_thread, &cwd);
        let _ = tx.send(out);
    });

    if let Some(st) = state.execute_loops.get_mut(&loop_id) {
        st.postprocess_pending = true;
        st.postprocess_rx = Some(rx);
        st.last_status = Some(format!("Postprocess running: {}", cmd));
    }
}

fn run_command_best_effort(cmd: &str, cwd: &PathBuf) -> Result<String, String> {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd")
            .arg("/C")
            .arg(cmd)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("Failed to spawn cmd: {:#}", e))?;

        let mut s = String::new();
        s.push_str(&String::from_utf8_lossy(&output.stdout));
        if !output.stderr.is_empty() {
            if !s.is_empty() {
                s.push_str("\n");
            }
            s.push_str(&String::from_utf8_lossy(&output.stderr));
        }

        if output.status.success() {
            Ok(s)
        } else {
            Err(s)
        }
    }

    #[cfg(not(windows))]
    {
        let output = std::process::Command::new("sh")
            .arg("-lc")
            .arg(cmd)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("Failed to spawn sh: {:#}", e))?;

        let mut s = String::new();
        s.push_str(&String::from_utf8_lossy(&output.stdout));
        if !output.stderr.is_empty() {
            if !s.is_empty() {
                s.push_str("\n");
            }
            s.push_str(&String::from_utf8_lossy(&output.stderr));
        }

        if output.status.success() {
            Ok(s)
        } else {
            Err(s)
        }
    }
}
