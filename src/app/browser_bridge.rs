use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::app::state::ExecuteLoopTurnResult;

static CLIENT: OnceLock<Mutex<BrowserBridgeClient>> = OnceLock::new();

fn client() -> &'static Mutex<BrowserBridgeClient> {
    CLIENT.get_or_init(|| Mutex::new(BrowserBridgeClient::new()))
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
            eprintln!("[browser_bridge] <- error {}", err);
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
        "timeout_ms": 60000
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
        "timeout_ms": 60000
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
        "timeout_ms": 5000
    }))?;

    let data = value.get("data").cloned().unwrap_or(value);
    Ok(serde_json::from_value(data)?)
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
            "timeout_ms": 60000
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
        "timeout_ms": 30000
    }))?;

    let read_resp = client.send_json(json!({
        "cmd": "read_response",
        "session_id": session_id,
        "timeout_ms": 180000,
        "idle_ms": 2000
    }))?;

    let text = read_resp
        .get("data")
        .and_then(|v| v.get("response"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Browser bridge read_response missing response text"))?
        .to_string();

    Ok(ExecuteLoopTurnResult {
        text,
        conversation_id: None,
        browser_session_id: cfg.session_id.clone(),
    })
}
