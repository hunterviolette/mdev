use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::app::state::ExecuteLoopTurnResult;

static CLIENT: OnceLock<Mutex<BrowserBridgeClient>> = OnceLock::new();
static RUNTIME_TIMEOUTS: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
static RUNTIME_STARTED_AT: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

fn client() -> &'static Mutex<BrowserBridgeClient> {
    CLIENT.get_or_init(|| Mutex::new(BrowserBridgeClient::new()))
}

fn runtime_timeouts() -> &'static Mutex<HashMap<String, u64>> {
    RUNTIME_TIMEOUTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn runtime_started_at() -> &'static Mutex<HashMap<String, Instant>> {
    RUNTIME_STARTED_AT.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn begin_runtime_timeout(runtime_key: &str, timeout_secs: u64) {
    if runtime_key.trim().is_empty() {
        return;
    }
    if let Ok(mut map) = runtime_timeouts().lock() {
        map.insert(runtime_key.to_string(), timeout_secs.max(1));
    }
    if let Ok(mut map) = runtime_started_at().lock() {
        map.insert(runtime_key.to_string(), Instant::now());
    }
}

pub fn set_runtime_timeout_secs(runtime_key: &str, timeout_secs: u64) {
    if runtime_key.trim().is_empty() || timeout_secs == 0 {
        return;
    }
    if let Ok(mut map) = runtime_timeouts().lock() {
        map.insert(runtime_key.to_string(), timeout_secs.max(1));
    }
}

pub fn timeout_runtime_now(runtime_key: &str) {
    if runtime_key.trim().is_empty() {
        return;
    }
    if let Ok(mut map) = runtime_timeouts().lock() {
        map.insert(runtime_key.to_string(), 1);
    }
    if let Ok(mut map) = runtime_started_at().lock() {
        map.insert(runtime_key.to_string(), Instant::now() - Duration::from_secs(2));
    }
}

fn set_session_response_timeout_ms_with_client(
    client: &mut BrowserBridgeClient,
    session_id: &str,
    timeout_ms: u64,
) -> Result<()> {
    let timeout_ms = timeout_ms.max(1000);
    eprintln!(
        "[browser_bridge] -> set_response_timeout session_id={} timeout_ms={}",
        session_id,
        timeout_ms
    );

    let resp = client.send_json(json!({
        "cmd": "set_response_timeout",
        "session_id": session_id,
        "timeout_ms": timeout_ms
    }))?;

    let ack_timeout_ms = resp
        .get("data")
        .and_then(|v| v.get("timeout_ms"))
        .and_then(|v| v.as_u64())
        .unwrap_or(timeout_ms);

    eprintln!(
        "[browser_bridge] <- set_response_timeout session_id={} timeout_ms={}",
        session_id,
        ack_timeout_ms
    );

    Ok(())
}

pub fn set_session_response_timeout_ms(
    cfg: &BrowserTurnConfig,
    session_id: &str,
    timeout_ms: u64,
) -> Result<()> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;

    client.ensure_started(&cfg.bridge_dir)?;
    set_session_response_timeout_ms_with_client(&mut client, session_id, timeout_ms)
}

pub fn active_response_timeout_ms(runtime_key: &str, fallback_ms: u64) -> u64 {
    let timeout_secs = runtime_timeouts()
        .lock()
        .ok()
        .and_then(|map| map.get(runtime_key).copied())
        .unwrap_or((fallback_ms.max(1000) + 999) / 1000)
        .max(1);

    timeout_secs * 1000
}

pub fn runtime_timeout_remaining_secs(runtime_key: &str, fallback_secs: u64) -> u64 {
    let timeout_secs = runtime_timeouts()
        .lock()
        .ok()
        .and_then(|map| map.get(runtime_key).copied())
        .unwrap_or(fallback_secs)
        .max(1);

    let started_at = runtime_started_at()
        .lock()
        .ok()
        .and_then(|map| map.get(runtime_key).copied());

    match started_at {
        Some(started_at) => timeout_secs.saturating_sub(started_at.elapsed().as_secs()),
        None => timeout_secs,
    }
}

#[derive(Clone, Debug)]
pub struct BrowserTurnConfig {
    pub bridge_dir: String,
    pub edge_executable: String,
    pub user_data_dir: String,
    pub cdp_url: String,
    pub page_url_contains: String,
    pub profile: String,
    pub session_id: Option<String>,
    pub auto_launch_edge: bool,
    pub runtime_key: String,
    pub response_timeout_ms: u64,
    pub response_poll_ms: u64,
}
fn bridge_timeout_ms(cfg: &BrowserTurnConfig) -> u64 {
    cfg.response_timeout_ms.max(1000)
}


struct BrowserBridgeClient {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    next_id: u64,
}

impl BrowserBridgeClient {
    fn new() -> Self {
        Self {
            child: None,
            stdin: None,
            stdout: None,
            next_id: 1,
        }
    }

    fn command_id(&mut self) -> String {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id.to_string()
    }

    fn ensure_started(&mut self, bridge_dir: &str) -> Result<()> {
        if let Some(child) = self.child.as_mut() {
            let status = child.try_wait()?;
            eprintln!("[browser_bridge] ensure_started existing child status: {:?}", status);
            if status.is_none() {
                eprintln!("[browser_bridge] bridge already running");
                return Ok(());
            }
            eprintln!("[browser_bridge] bridge exited, restarting");
        } else {
            eprintln!("[browser_bridge] no bridge child, starting");
        }

        self.child = None;
        self.stdin = None;
        self.stdout = None;

        let npm = if cfg!(target_os = "windows") { "npm.cmd" } else { "npm" };
        eprintln!("[browser_bridge] spawning bridge in dir={}", bridge_dir);
        let mut child = Command::new(npm)
            .arg("start")
            .current_dir(bridge_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("Failed to start browser bridge from {}", bridge_dir))?;

        eprintln!("[browser_bridge] spawned pid={:?}", child.id());

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("Bridge stdin unavailable"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("Bridge stdout unavailable"))?;

        self.stdin = Some(stdin);
        self.stdout = Some(BufReader::new(stdout));
        self.child = Some(child);

        std::thread::sleep(Duration::from_millis(1200));
        Ok(())
    }

    fn send_json(&mut self, mut payload: Value) -> Result<Value> {
        let id = self.command_id();
        payload["id"] = Value::String(id.clone());
        let cmd_name = payload
            .get("cmd")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        eprintln!("[browser_bridge] -> {}", payload);

        let stdin = self.stdin.as_mut().ok_or_else(|| anyhow!("Bridge stdin not connected"))?;
        writeln!(stdin, "{}", payload.to_string()).context("Failed writing bridge command")?;
        stdin.flush().context("Failed flushing bridge stdin")?;

        let stdout = self.stdout.as_mut().ok_or_else(|| anyhow!("Bridge stdout not connected"))?;
        let mut line = String::new();
        loop {
            line.clear();
            let n = stdout.read_line(&mut line).context("Failed reading bridge response")?;
            if n == 0 {
                eprintln!("[browser_bridge] <- EOF");
                return Err(anyhow!("Bridge exited before sending a response"));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            eprintln!("[browser_bridge] <- raw {}", trimmed);
            let parsed: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if parsed.get("id").and_then(|v| v.as_str()) != Some(id.as_str()) {
                eprintln!("[browser_bridge] ignoring response for different id");
                continue;
            }
            let ok = parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
            if ok {
                eprintln!("[browser_bridge] <- ok {}", parsed);
                return Ok(parsed);
            }
            let err = parsed
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown browser bridge error");
            let err_lower = err.to_ascii_lowercase();
            let expected_read_timeout = cmd_name == "read_response"
                && err_lower.contains("timed out waiting for completed response");
            if !expected_read_timeout {
                eprintln!("[browser_bridge] <- error {}", err);
            }
            return Err(anyhow!(err.to_string()));
        }
    }

    fn maybe_launch_edge(&mut self, edge_executable: &str, user_data_dir: &str, cdp_url: &str) -> Result<()> {
        let port = parse_cdp_port(cdp_url).unwrap_or(9222);
        let _ = Command::new(edge_executable)
            .arg(format!("--remote-debugging-port={}", port))
            .arg(format!("--user-data-dir={}", user_data_dir))
            .spawn()
            .with_context(|| format!("Failed to start Edge at {}", edge_executable))?;
        std::thread::sleep(Duration::from_millis(1800));
        Ok(())
    }
}

fn parse_cdp_port(cdp_url: &str) -> Option<u16> {
    let tail = cdp_url.rsplit(':').next()?;
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u16>().ok()
}

pub fn browser_model_options() -> Vec<String> {
    vec!["browser-web".to_string()]
}

pub fn launch_and_attach(cfg: &mut BrowserTurnConfig) -> Result<String> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;

    client.ensure_started(&cfg.bridge_dir)?;

    if cfg.auto_launch_edge {
        client.maybe_launch_edge(&cfg.edge_executable, &cfg.user_data_dir, &cfg.cdp_url)?;
    }

    let connect_resp = client.send_json(json!({
        "cmd": "connect_over_cdp",
        "session_id": Value::Null,
        "profile": if cfg.profile.is_empty() { "auto" } else { &cfg.profile },
        "cdp_url": cfg.cdp_url,
        "page_url_contains": if cfg.page_url_contains.is_empty() { Value::Null } else { Value::String(cfg.page_url_contains.clone()) },
        "timeout_ms": bridge_timeout_ms(cfg)
    }))?;

    let session_id = connect_resp
        .get("session_id")
        .and_then(|v| v.as_str())
        .or_else(|| connect_resp.get("data").and_then(|v| v.get("session_id")).and_then(|v| v.as_str()))
        .ok_or_else(|| anyhow!("Browser bridge connect_over_cdp response missing session_id"))?
        .to_string();

    cfg.session_id = Some(session_id.clone());
    Ok(session_id)
}

pub fn open_url_in_session(cfg: &mut BrowserTurnConfig, url: &str) -> Result<()> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;

    client.ensure_started(&cfg.bridge_dir)?;

    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing before open_page"))?;

    client.send_json(json!({
        "cmd": "open_page",
        "session_id": session_id,
        "url": url,
        "timeout_ms": bridge_timeout_ms(cfg)
    }))?;

    Ok(())
}

pub fn upload_file(cfg: &mut BrowserTurnConfig, file_path: &std::path::Path) -> Result<()> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;

    client.ensure_started(&cfg.bridge_dir)?;

    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing before upload_file"))?;

    client.send_json(json!({
        "cmd": "upload_file",
        "session_id": session_id,
        "file_path": file_path.to_string_lossy().to_string(),
        "timeout_ms": bridge_timeout_ms(cfg)
    }))?;

    Ok(())
}

pub fn probe_session(cfg: &mut BrowserTurnConfig) -> Result<crate::app::state::BrowserProbeResult> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;

    client.ensure_started(&cfg.bridge_dir)?;

    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing before probe_page"))?;

    let value = client.send_json(json!({
        "cmd": "probe_page",
        "session_id": session_id,
        "profile": if cfg.profile.is_empty() { "auto" } else { &cfg.profile },
        "timeout_ms": bridge_timeout_ms(cfg)
    }))?;

    let data = value.get("data").cloned().unwrap_or(value);
    Ok(serde_json::from_value(data)?)
}


fn response_attempt_timeout_ms(cfg: &BrowserTurnConfig, remaining_ms: u64) -> u64 {
    remaining_ms.min(cfg.response_poll_ms.max(1000))
}

fn read_response_once(
    client: &mut BrowserBridgeClient,
    session_id: &str,
    timeout_ms: u64,
) -> Result<(usize, String)> {
    let value = client.send_json(json!({
        "cmd": "read_response",
        "session_id": session_id,
        "response_selector": Value::Null,
        "timeout_ms": timeout_ms
    }))?;

    let data = value.get("data").cloned().unwrap_or(value);
    let count = data
        .get("response_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let text = data
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok((count, text))
}

pub fn close_session_page(cfg: &mut BrowserTurnConfig) -> Result<()> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;

    client.ensure_started(&cfg.bridge_dir)?;

    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing before close_page"))?;

    client.send_json(json!({
        "cmd": "close_page",
        "session_id": session_id
    }))?;

    Ok(())
}

pub fn send_chat_and_wait(cfg: &mut BrowserTurnConfig, text: &str) -> Result<ExecuteLoopTurnResult> {
    {
        let mutex = client();
        let mut client = mutex.lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;

        client.ensure_started(&cfg.bridge_dir)?;

        if cfg.auto_launch_edge && cfg.session_id.is_none() {
            client.maybe_launch_edge(&cfg.edge_executable, &cfg.user_data_dir, &cfg.cdp_url)?;
        }

        if cfg.session_id.is_none() {
            let connect_resp = client.send_json(json!({
                "cmd": "connect_over_cdp",
                "session_id": Value::Null,
                "profile": if cfg.profile.is_empty() { "auto" } else { &cfg.profile },
                "cdp_url": cfg.cdp_url,
                "page_url_contains": cfg.page_url_contains,
                "timeout_ms": bridge_timeout_ms(cfg)
            }))?;

            let session_id = connect_resp
                .get("session_id")
                .and_then(|v| v.as_str())
                .or_else(|| connect_resp.get("data").and_then(|v| v.get("session_id")).and_then(|v| v.as_str()))
                .ok_or_else(|| anyhow!("Browser bridge connect_over_cdp response missing session_id"))?;

            cfg.session_id = Some(session_id.to_string());
        }

        let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing after connect"))?;

        client.send_json(json!({
            "cmd": "send_chat",
            "session_id": session_id,
            "text": text,
            "timeout_ms": bridge_timeout_ms(cfg)
        }))?;
    }

    begin_runtime_timeout(&cfg.runtime_key, (cfg.response_timeout_ms.max(1000) + 999) / 1000);
    let poll_ms = cfg.response_poll_ms.max(250);
    let fallback_timeout_secs = (cfg.response_timeout_ms.max(1000) + 999) / 1000;
    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing after connect"))?;

    let text = loop {
        let remaining_secs = runtime_timeout_remaining_secs(&cfg.runtime_key, fallback_timeout_secs);
        if remaining_secs == 0 {
            return Err(anyhow!("Timed out waiting for browser response"));
        }

        let remaining_ms = remaining_secs.saturating_mul(1000).max(1000);
        let patched_total_ms = active_response_timeout_ms(&cfg.runtime_key, cfg.response_timeout_ms.max(1000));
        let slice_ms = response_attempt_timeout_ms(cfg, remaining_ms);


        let read_result = {
            let mutex = client();
            let mut client = mutex.lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;
            client.ensure_started(&cfg.bridge_dir)?;
            set_session_response_timeout_ms_with_client(&mut client, &session_id, patched_total_ms)?;
            read_response_once(&mut client, &session_id, slice_ms)
        };

        match read_result {
            Ok((_count, text)) => {
                break text;
            }
            Err(err) => {
                let msg = format!("{:#}", err);
                let lower = msg.to_ascii_lowercase();
                if lower.contains("timed out waiting for completed response") || lower.contains("waitforselector") {
                    std::thread::sleep(Duration::from_millis(poll_ms));
                    continue;
                }
                return Err(err);
            }
        }
    };

    Ok(ExecuteLoopTurnResult {
        text,
        conversation_id: None,
        browser_session_id: cfg.session_id.clone(),
    })
}
