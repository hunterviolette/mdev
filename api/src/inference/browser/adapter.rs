use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpStream, ToSocketAddrs},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::inference::{BrowserConfig, BrowserProbeResult, InferenceResult, InferenceTransport};

fn resolve_bridge_dir(raw: &str) -> PathBuf {
    let input = raw.trim();
    let candidate = PathBuf::from(if input.is_empty() { "bridge" } else { input });

    if candidate.is_absolute() && candidate.exists() {
        return candidate;
    }

    if let Ok(cwd) = std::env::current_dir() {
        let joined = cwd.join(&candidate);
        if joined.exists() {
            return joined;
        }

        for dir in cwd.ancestors() {
            let ancestor_joined = dir.join(&candidate);
            if ancestor_joined.exists() {
                return ancestor_joined;
            }
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors() {
            let ancestor_joined = dir.join(&candidate);
            if ancestor_joined.exists() {
                return ancestor_joined;
            }
        }
    }

    candidate
}

fn resolve_user_data_dir(raw: &str) -> PathBuf {
    let input = raw.trim();
    if !input.is_empty() {
        return PathBuf::from(input);
    }

    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join(".data").join("browser-profile");
        let _ = std::fs::create_dir_all(&candidate);
        return candidate;
    }

    std::env::temp_dir().join("workflow-api-browser-profile")
}

fn cdp_tcp_ready(cdp_url: &str) -> bool {
    let tail = cdp_url
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');

    let host_port = tail.split('/').next().unwrap_or(tail);
    let mut addrs = match host_port.to_socket_addrs() {
        Ok(v) => v,
        Err(_) => return false,
    };

    let addr = match addrs.next() {
        Some(v) => v,
        None => return false,
    };

    TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
}

fn wait_for_cdp(cdp_url: &str, timeout_ms: u64) -> Result<()> {
    let started = Instant::now();
    let budget = Duration::from_millis(timeout_ms.max(1000));
    while started.elapsed() < budget {
        if cdp_tcp_ready(cdp_url) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(anyhow!("CDP endpoint did not become ready at {} within {} ms", cdp_url, timeout_ms.max(1000)))
}

struct BridgeClient {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    next_id: u64,
}

impl BridgeClient {
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
            if child.try_wait()?.is_none() {
                return Ok(());
            }
        }

        self.child = None;
        self.stdin = None;
        self.stdout = None;

        let resolved_bridge_dir = resolve_bridge_dir(bridge_dir);
        let npm = if cfg!(target_os = "windows") { "npm.cmd" } else { "npm" };
        let mut child = Command::new(npm)
            .arg("start")
            .current_dir(&resolved_bridge_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("Failed to start browser bridge from {}", resolved_bridge_dir.display()))?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("Bridge stdin unavailable"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("Bridge stdout unavailable"))?;

        self.stdin = Some(stdin);
        self.stdout = Some(BufReader::new(stdout));
        self.child = Some(child);
        std::thread::sleep(std::time::Duration::from_millis(1200));
        Ok(())
    }

    fn send_json(&mut self, mut payload: Value) -> Result<Value> {
        let id = self.command_id();
        payload["id"] = Value::String(id.clone());

        let stdin = self.stdin.as_mut().ok_or_else(|| anyhow!("Bridge stdin not connected"))?;
        writeln!(stdin, "{}", payload)?;
        stdin.flush()?;

        let stdout = self.stdout.as_mut().ok_or_else(|| anyhow!("Bridge stdout not connected"))?;
        let mut line = String::new();

        loop {
            line.clear();
            let n = stdout.read_line(&mut line)?;
            if n == 0 {
                return Err(anyhow!("Bridge exited before sending a response"));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parsed: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if parsed.get("id").and_then(|v| v.as_str()) != Some(id.as_str()) {
                continue;
            }
            if parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Ok(parsed);
            }
            let err = parsed.get("error").and_then(|v| v.as_str()).unwrap_or("Unknown browser bridge error");
            return Err(anyhow!(err.to_string()));
        }
    }

    fn maybe_launch_edge(&mut self, edge_executable: &str, user_data_dir: &str, cdp_url: &str) -> Result<()> {
        let port = cdp_url
            .rsplit(':')
            .next()
            .and_then(|tail| {
                let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
                digits.parse::<u16>().ok()
            })
            .unwrap_or(9222);

        let resolved_user_data_dir = resolve_user_data_dir(user_data_dir);
        std::fs::create_dir_all(&resolved_user_data_dir)
            .with_context(|| format!("Failed to create browser profile dir {}", resolved_user_data_dir.display()))?;

        let _ = Command::new(edge_executable)
            .arg(format!("--remote-debugging-port={}", port))
            .arg(format!("--user-data-dir={}", resolved_user_data_dir.display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .spawn()
            .with_context(|| format!("Failed to start Edge at {}", edge_executable))?;

        wait_for_cdp(cdp_url, 15000)?;
        Ok(())
    }
}

fn bridge_client() -> &'static Mutex<BridgeClient> {
    static CLIENT: OnceLock<Mutex<BridgeClient>> = OnceLock::new();
    CLIENT.get_or_init(|| Mutex::new(BridgeClient::new()))
}

fn resolve_browser_executable(explicit: &str) -> String {
    let explicit = explicit.trim();
    if !explicit.is_empty() {
        return explicit.to_string();
    }

    if cfg!(target_os = "windows") {
        for key in ["PROGRAMFILES(X86)", "PROGRAMFILES"] {
            if let Ok(root) = std::env::var(key) {
                let edge = PathBuf::from(&root).join("Microsoft/Edge/Application/msedge.exe");
                if edge.exists() {
                    return edge.to_string_lossy().into_owned();
                }
            }
        }
        return "msedge.exe".to_string();
    }

    "msedge".to_string()
}

fn timeout_ms(cfg: &BrowserConfig) -> u64 {
    cfg.response_timeout_ms.max(1000)
}

pub fn launch_and_attach(cfg: &mut BrowserConfig) -> Result<String> {
    let mut client = bridge_client().lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;
    client.ensure_started(&cfg.bridge_dir)?;

    if cfg.auto_launch_edge {
        let edge_executable = resolve_browser_executable(&cfg.edge_executable);
        client.maybe_launch_edge(&edge_executable, &cfg.user_data_dir, &cfg.cdp_url)?;
    } else {
        wait_for_cdp(&cfg.cdp_url, 5000)?;
    }

    let mut last_err: Option<anyhow::Error> = None;
    for _attempt in 0..5 {
        match client.send_json(json!({
            "cmd": "connect_over_cdp",
            "session_id": Value::Null,
            "profile": if cfg.profile.is_empty() { "auto" } else { &cfg.profile },
            "cdp_url": cfg.cdp_url,
            "page_url_contains": Value::Null,
            "timeout_ms": timeout_ms(cfg)
        })) {
            Ok(value) => {
                let session_id = value
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .or_else(|| value.get("data").and_then(|v| v.get("session_id")).and_then(|v| v.as_str()))
                    .ok_or_else(|| anyhow!("Browser bridge connect_over_cdp response missing session_id"))?
                    .to_string();

                cfg.session_id = Some(session_id.clone());
                return Ok(session_id);
            }
            Err(err) => {
                last_err = Some(err);
                std::thread::sleep(std::time::Duration::from_millis(750));
            }
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow!("Browser attach failed after retries")))
}

pub fn open_url(cfg: &mut BrowserConfig, url: &str) -> Result<()> {
    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing before open_url"))?;
    let mut client = bridge_client().lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;
    client.ensure_started(&cfg.bridge_dir)?;
    client.send_json(json!({
        "cmd": "open_page",
        "session_id": session_id,
        "url": url,
        "timeout_ms": timeout_ms(cfg)
    }))?;
    cfg.target_url = url.to_string();
    Ok(())
}

pub fn probe(cfg: &mut BrowserConfig) -> Result<BrowserProbeResult> {
    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing before probe"))?;
    let mut client = bridge_client().lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;
    client.ensure_started(&cfg.bridge_dir)?;
    let value = client.send_json(json!({
        "cmd": "probe_page",
        "session_id": session_id,
        "profile": if cfg.profile.is_empty() { "auto" } else { &cfg.profile },
        "timeout_ms": timeout_ms(cfg)
    }))?;
    let data = value.get("data").cloned().unwrap_or(value);
    Ok(serde_json::from_value(data)?)
}

pub fn upload_file(cfg: &mut BrowserConfig, file_path: &std::path::Path) -> Result<()> {
    let mut client = bridge_client().lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;
    client.ensure_started(&cfg.bridge_dir)?;

    if cfg.session_id.is_none() {
        let session_id = launch_and_attach(cfg)?;
        cfg.session_id = Some(session_id);
    }

    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing after connect"))?;

    client.send_json(json!({
        "cmd": "upload_file",
        "session_id": session_id,
        "file_path": file_path.to_string_lossy().to_string(),
        "timeout_ms": timeout_ms(cfg)
    }))?;

    Ok(())
}

pub fn send_chat_and_wait(cfg: &mut BrowserConfig, text: &str) -> Result<InferenceResult> {
    let mut client = bridge_client().lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;
    client.ensure_started(&cfg.bridge_dir)?;

    if cfg.session_id.is_none() {
        let session_id = launch_and_attach(cfg)?;
        cfg.session_id = Some(session_id);
    }

    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing after connect"))?;

    client.send_json(json!({
        "cmd": "send_chat",
        "session_id": session_id,
        "text": text,
        "timeout_ms": timeout_ms(cfg)
    }))?;

    let read_value = client.send_json(json!({
        "cmd": "read_response",
        "session_id": session_id,
        "response_selector": Value::Null,
        "timeout_ms": cfg.response_timeout_ms
    }))?;

    let data = read_value.get("data").cloned().unwrap_or(read_value);
    let text = data.get("response").and_then(|v| v.as_str()).unwrap_or("").to_string();

    Ok(InferenceResult {
        transport: InferenceTransport::Browser,
        text,
        conversation_id: None,
        browser_session_id: cfg.session_id.clone(),
    })
}
