use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpStream, ToSocketAddrs},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use uuid::Uuid;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use crate::engine::capabilities::inference::{BrowserConfig, BrowserProbeResult, InferenceResult, InferenceTransport};

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

fn bridge_entrypoint(bridge_root: &std::path::Path) -> Result<(String, Vec<String>)> {
    let dist_entry = bridge_root.join("dist").join("index.js");
    if dist_entry.exists() {
        return Ok((
            "node".to_string(),
            vec![dist_entry.to_string_lossy().to_string()],
        ));
    }

    let package_json = bridge_root.join("package.json");
    if package_json.exists() {
        #[cfg(target_os = "windows")]
        {
            return Ok((
                "npm.cmd".to_string(),
                vec!["run".to_string(), "start".to_string()],
            ));
        }

        #[cfg(not(target_os = "windows"))]
        {
            return Ok((
                "npm".to_string(),
                vec!["run".to_string(), "start".to_string()],
            ));
        }
    }

    Err(anyhow!("bridge entrypoint not found under {}", bridge_root.display()))
}

fn ensure_bridge_built(bridge_root: &std::path::Path) -> Result<()> {
    let dist_entry = bridge_root.join("dist").join("index.js");
    if dist_entry.exists() {
        return Ok(());
    }

    let package_json = bridge_root.join("package.json");
    if !package_json.exists() {
        return Err(anyhow!("bridge/package.json not found under {}", bridge_root.display()));
    }

    #[cfg(target_os = "windows")]
    let npm = "npm.cmd";
    #[cfg(not(target_os = "windows"))]
    let npm = "npm";

    let install = Command::new(npm)
        .arg("install")
        .current_dir(bridge_root)
        .output()
        .with_context(|| format!("failed to run npm install in {}", bridge_root.display()))?;

    if !install.status.success() {
        return Err(anyhow!(
            "npm install failed: {}",
            String::from_utf8_lossy(&install.stderr)
        ));
    }

    let build = Command::new(npm)
        .args(["run", "build"])
        .current_dir(bridge_root)
        .output()
        .with_context(|| format!("failed to run npm run build in {}", bridge_root.display()))?;

    if !build.status.success() {
        return Err(anyhow!(
            "npm run build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        ));
    }

    if !dist_entry.exists() {
        return Err(anyhow!("bridge build completed but dist/index.js was not created"));
    }

    Ok(())
}

fn bridge_cmd(cmd: &str) -> Value {
    json!({
        "id": Uuid::new_v4().to_string(),
        "cmd": cmd,
    })
}

fn timeout_ms(cfg: &BrowserConfig) -> u64 {
    if cfg.dom_poll_ms == 0 { 1_000 } else { cfg.dom_poll_ms }
}

struct BridgeClient {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    launched_browser: Option<Child>,
}

impl BridgeClient {
    fn new() -> Self {
        Self {
            child: None,
            stdin: None,
            stdout: None,
            launched_browser: None,
        }
    }

    fn ensure_started(&mut self, bridge_dir: &str) -> Result<()> {
        if self.child.is_some() {
            debug!(bridge_dir, "browser bridge already running");
            return Ok(());
        }

        let bridge_root = resolve_bridge_dir(bridge_dir);
        ensure_bridge_built(&bridge_root)?;
        let (program, args) = bridge_entrypoint(&bridge_root)?;
        info!(bridge_dir, bridge_root = %bridge_root.display(), program = %program, args = ?args, "starting browser bridge process");

        let mut child = Command::new(&program)
            .args(&args)
            .current_dir(&bridge_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("failed to start bridge with {} {:?}", program, args))?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("bridge stdin unavailable"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("bridge stdout unavailable"))?;

        self.stdin = Some(stdin);
        self.stdout = Some(BufReader::new(stdout));
        self.child = Some(child);
        info!(bridge_dir, "browser bridge process started");
        Ok(())
    }

    fn send_json(&mut self, value: Value) -> Result<Value> {
        let stdin = self.stdin.as_mut().ok_or_else(|| anyhow!("bridge stdin unavailable"))?;
        let stdout = self.stdout.as_mut().ok_or_else(|| anyhow!("bridge stdout unavailable"))?;

        let cmd = value.get("cmd").and_then(Value::as_str).unwrap_or("unknown").to_string();
        let payload = serde_json::to_string(&value)?;
        debug!(cmd, payload = %payload, "sending command to browser bridge");
        stdin.write_all(payload.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;

        let mut line = String::new();
        stdout.read_line(&mut line)?;
        if line.trim().is_empty() {
            error!(cmd, "browser bridge returned empty response");
            return Err(anyhow!("bridge returned empty response"));
        }

        debug!(cmd, response = %line.trim(), "received response from browser bridge");
        let response: Value = serde_json::from_str(line.trim())?;
        if let Some(err) = response.get("error").and_then(|v| v.as_str()) {
            error!(cmd, error = %err, "browser bridge returned error response");
            return Err(anyhow!(err.to_string()));
        }
        Ok(response)
    }

    fn shutdown(&mut self) {
        self.stdin.take();
        self.stdout.take();

        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }

        if let Some(mut browser) = self.launched_browser.take() {
            let _ = browser.kill();
            let _ = browser.wait();
        }
    }
}

impl Drop for BridgeClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn shutdown_browser_bridge() {
    if let Ok(mut client) = bridge_client().lock() {
        client.shutdown();
    }
}

fn launch_edge(cfg: &BrowserConfig) -> Result<Child> {
    let executable = resolve_edge_executable(cfg);

    let user_data_dir = resolve_user_data_dir(&cfg.user_data_dir);
    let cdp_url = if cfg.cdp_url.trim().is_empty() {
        "http://127.0.0.1:9222".to_string()
    } else {
        cfg.cdp_url.clone()
    };
    let host_port = cdp_url
        .trim()
        .strip_prefix("http://")
        .or_else(|| cdp_url.trim().strip_prefix("https://"))
        .unwrap_or(cdp_url.trim())
        .split('/')
        .next()
        .unwrap_or("127.0.0.1:9222")
        .to_string();
    let port = host_port
        .split(':')
        .nth(1)
        .unwrap_or("9222")
        .parse::<u16>()
        .unwrap_or(9222);
    let launch_url = normalize_browser_url_for_launch(&cfg.target_url);

    info!(executable = %executable, user_data_dir = %user_data_dir.display(), port, launch_url = %launch_url, "launching Edge with remote debugging");

    let child = Command::new(&executable)
        .arg(format!("--remote-debugging-port={}", port))
        .arg(format!("--user-data-dir={}", user_data_dir.to_string_lossy()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg(launch_url)
        .spawn()
        .with_context(|| format!("failed to launch Edge via {}", executable))?;

    Ok(child)
}

fn bridge_client() -> &'static Mutex<BridgeClient> {
    static CLIENT: OnceLock<Mutex<BridgeClient>> = OnceLock::new();
    CLIENT.get_or_init(|| Mutex::new(BridgeClient::new()))
}

fn cdp_reachable(cdp_url: &str) -> bool {
    let raw = cdp_url.trim();
    if raw.is_empty() {
        return false;
    }

    let no_scheme = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))
        .unwrap_or(raw);
    let host_port = no_scheme.split('/').next().unwrap_or(no_scheme);

    match host_port.to_socket_addrs() {
        Ok(mut addrs) => addrs.any(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()),
        Err(_) => false,
    }
}

fn resolve_edge_executable(cfg: &BrowserConfig) -> String {
    let configured = cfg.edge_executable.trim();
    if !configured.is_empty() {
        return configured.to_string();
    }

    #[cfg(target_os = "windows")]
    {
        let candidates = [
            "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
            "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
        ];

        for candidate in candidates {
            if std::path::Path::new(candidate).exists() {
                return candidate.to_string();
            }
        }

        "msedge.exe".to_string()
    }

    #[cfg(not(target_os = "windows"))]
    {
        "msedge".to_string()
    }
}

fn normalize_browser_url_for_launch(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return "https://website.com/".to_string();
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return trimmed.to_string();
    }
    format!("https://{}", trimmed.trim_start_matches('/'))
}


pub fn launch_and_attach(cfg: &mut BrowserConfig) -> Result<String> {
    info!(cdp_url = %cfg.cdp_url, bridge_dir = %cfg.bridge_dir, target_url = %cfg.target_url, page_url_contains = %cfg.page_url_contains, auto_launch_edge = cfg.auto_launch_edge, "launch_and_attach starting");
    if !cdp_reachable(&cfg.cdp_url) {
        warn!(cdp_url = %cfg.cdp_url, "cdp endpoint not reachable before attach");
        if cfg.auto_launch_edge {
            info!(cdp_url = %cfg.cdp_url, "attempting to launch edge with remote debugging");
            let child = launch_edge(cfg)?;
            if let Ok(mut client) = bridge_client().lock() {
                client.launched_browser = Some(child);
            }
        }
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if cdp_reachable(&cfg.cdp_url) {
            info!(cdp_url = %cfg.cdp_url, "cdp endpoint became reachable");
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    let mut last_err: Option<anyhow::Error> = None;
    for attempt_index in 0..20 {
        let attempt = (|| -> Result<String> {
            debug!(attempt = attempt_index + 1, cdp_url = %cfg.cdp_url, "attempting browser bridge connect_over_cdp");
            let mut client = bridge_client().lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;
            client.ensure_started(&cfg.bridge_dir)?;
            let mut payload = bridge_cmd("connect_over_cdp");
            payload["cdp_url"] = Value::String(cfg.cdp_url.clone());
            payload["profile"] = Value::String(if cfg.profile.is_empty() { "auto".to_string() } else { cfg.profile.clone() });
            let page_url_contains = if !cfg.page_url_contains.trim().is_empty() {
                cfg.page_url_contains.trim().to_string()
            } else if !cfg.target_url.trim().is_empty() {
                normalize_browser_url_for_launch(&cfg.target_url)
            } else {
                String::new()
            };
            if !page_url_contains.is_empty() {
                payload["page_url_contains"] = Value::String(page_url_contains);
            }
            if !cfg.session_id.as_deref().unwrap_or("").is_empty() {
                payload["session_id"] = Value::String(cfg.session_id.clone().unwrap_or_default());
            }
            payload["timeout_ms"] = Value::Number(timeout_ms(cfg).into());
            let value = client.send_json(payload)?;
            let session_id = value
                .get("session_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("bridge response missing session_id"))?
                .to_string();
            Ok(session_id)
        })();

        match attempt {
            Ok(session_id) => {
                info!(session_id = %session_id, attempt = attempt_index + 1, "browser attached successfully");
                cfg.session_id = Some(session_id.clone());
                return Ok(session_id);
            }
            Err(err) => {
                error!(attempt = attempt_index + 1, error = %format!("{:#}", err), "browser attach attempt failed");
                last_err = Some(err);
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }

    let err = last_err.unwrap_or_else(|| anyhow!("Browser attach failed after retries"));
    error!(error = %format!("{:#}", err), "launch_and_attach exhausted retries");
    Err(err)
}

pub fn open_url(cfg: &mut BrowserConfig, url: &str) -> Result<()> {
    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing before open_url"))?;
    let normalized_url = normalize_browser_url_for_launch(url);
    info!(session_id = %session_id, url = %normalized_url, "opening browser url");
    let mut client = bridge_client().lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;
    client.ensure_started(&cfg.bridge_dir)?;
    let mut payload = bridge_cmd("open_page");
    payload["session_id"] = Value::String(session_id);
    payload["url"] = Value::String(normalized_url.clone());
    payload["timeout_ms"] = Value::Number(timeout_ms(cfg).into());
    client.send_json(payload)?;
    cfg.target_url = normalized_url;
    info!(url = %cfg.target_url, "browser url opened");
    Ok(())
}

pub fn probe(cfg: &mut BrowserConfig) -> Result<BrowserProbeResult> {
    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing before probe"))?;
    debug!(session_id = %session_id, "probing browser session");
    let mut client = bridge_client().lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;
    client.ensure_started(&cfg.bridge_dir)?;
    let mut payload = bridge_cmd("probe_page");
    payload["session_id"] = Value::String(session_id.clone());
    payload["profile"] = Value::String(if cfg.profile.is_empty() { "auto".to_string() } else { cfg.profile.clone() });
    payload["timeout_ms"] = Value::Number(timeout_ms(cfg).into());
    let value = client.send_json(payload)?;
    let data = value.get("data").cloned().unwrap_or(value);
    let probe: BrowserProbeResult = serde_json::from_value(data)?;
    info!(session_id = %session_id, ready = probe.ready, page_open = probe.page_open, chat_input_found = probe.chat_input_found, chat_submit_found = probe.chat_submit_found, url = %probe.url, "browser probe completed");
    Ok(probe)
}

pub fn upload_file(cfg: &mut BrowserConfig, file_path: &std::path::Path) -> Result<()> {
    let mut client = bridge_client().lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;
    client.ensure_started(&cfg.bridge_dir)?;

    if cfg.session_id.is_none() {
        let session_id = launch_and_attach(cfg)?;
        cfg.session_id = Some(session_id);
    }

    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing after connect"))?;

    let mut payload = bridge_cmd("upload_file");
    payload["session_id"] = Value::String(session_id);
    payload["file_path"] = Value::String(file_path.to_string_lossy().to_string());
    payload["timeout_ms"] = Value::Number(timeout_ms(cfg).into());
    let value = client.send_json(payload)?;
    let data = value.get("data").cloned().unwrap_or(value);
    let ready = data.get("ready").and_then(|v| v.as_bool()).unwrap_or(false);
    if !ready {
        let upload_name = data
            .get("upload_name")
            .and_then(|v| v.as_str())
            .or_else(|| file_path.file_name().and_then(|v| v.to_str()))
            .unwrap_or("unknown");
        return Err(anyhow!("Browser bridge upload did not become ready ({})", upload_name));
    }

    Ok(())
}

pub fn send_chat_and_wait(cfg: &mut BrowserConfig, text: &str) -> Result<InferenceResult> {
    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing before send_chat"))?;

    let probe = probe(cfg)?;
    if !probe.ready {
        return Err(anyhow!(
            "Browser page is not chat-ready before send_chat (page_open={}, chat_input_found={}, chat_input_visible={}, url={})",
            probe.page_open,
            probe.chat_input_found,
            probe.chat_input_visible,
            probe.url
        ));
    }

    let mut client = bridge_client().lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;
    client.ensure_started(&cfg.bridge_dir)?;

    let mut send_payload = bridge_cmd("send_chat");
    send_payload["session_id"] = Value::String(session_id.clone());
    send_payload["text"] = Value::String(text.to_string());
    send_payload["timeout_ms"] = Value::Number(timeout_ms(cfg).into());
    let send_value = client.send_json(send_payload)?;

    let send_data = send_value.get("data").cloned().unwrap_or(send_value);
    let sent = send_data
        .get("sent")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if !sent {
        let method = send_data
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        return Err(anyhow!(
            "Browser bridge did not send chat successfully (method={})",
            method
        ));
    }

    let mut read_payload = bridge_cmd("read_response");
    read_payload["session_id"] = Value::String(session_id);
    read_payload["response_selector"] = Value::Null;
    read_payload["timeout_ms"] = Value::Number(cfg.response_timeout_ms.into());
    let read_value = client.send_json(read_payload)?;

    let read_data = read_value.get("data").cloned().unwrap_or(read_value);
    let text = read_data
        .get("response")
        .and_then(|v| v.as_str())
        .or_else(|| read_data.get("text").and_then(|v| v.as_str()))
        .or_else(|| read_data.get("content").and_then(|v| v.as_str()))
        .or_else(|| read_data.get("message").and_then(|v| v.as_str()))
        .or_else(|| read_data.get("output_text").and_then(|v| v.as_str()))
        .or_else(|| {
            read_data.get("response")
                .and_then(|v| v.as_object())
                .and_then(|obj| obj.get("text"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            read_data.get("message")
                .and_then(|v| v.as_object())
                .and_then(|obj| obj.get("content"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .to_string();

    Ok(InferenceResult {
        transport: InferenceTransport::Browser,
        text: json!({
            "ok": true,
            "text": text,
            "send": send_data,
            "read": read_data,
        }).to_string(),
        conversation_id: None,
        browser_session_id: cfg.session_id.clone(),
    })
}
