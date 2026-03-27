use std::collections::HashMap;
use std::path::PathBuf;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use reqwest::Url;
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

pub fn resolve_browser_bridge_dir_with_override(explicit: &str) -> String {
    let trimmed = explicit.trim();
    let normalized = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed)
        .to_string();

    if !normalized.is_empty() && std::path::Path::new(&normalized).exists() {
        return normalized;
    }

    if !normalized.is_empty() {
        let browser_dir = PathBuf::from(&normalized);
        if let Some(parent) = browser_dir.parent() {
            let candidate = parent.join("bridge");
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

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

pub fn resolve_browser_executable(explicit: &str) -> (String, String) {
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

    for candidate in [
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/usr/bin/microsoft-edge",
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ] {
        if std::path::Path::new(candidate).exists() {
            let lower = candidate.to_ascii_lowercase();
            let channel = if lower.contains("edge") {
                "msedge"
            } else if lower.contains("chrome") {
                "chrome"
            } else {
                "chromium"
            };
            return (candidate.to_string(), channel.to_string());
        }
    }

    ("msedge".to_string(), "msedge".to_string())
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
    pub dom_poll_ms: u64,
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

fn sap_discovery_to_flp_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let parsed = Url::parse(trimmed).ok()?;
    let path = parsed.path().to_ascii_lowercase();

    let looks_like_discovery = path.contains("/sap/bc/adt/discovery")
        || (path.contains("/sap/bc/adt") && path.ends_with("/discovery"));

    if !looks_like_discovery {
        return None;
    }

    let mut flp = parsed;
    flp.set_path("/sap/bc/ui5_ui5/ui2/ushell/shells/abap/FioriLaunchpad.html");
    flp.set_fragment(Some("Shell-home"));

    let keep_keys = ["sap-client", "sap-language"];
    let kept: Vec<(String, String)> = flp
        .query_pairs()
        .filter(|(k, _)| keep_keys.iter().any(|wanted| k.eq_ignore_ascii_case(wanted)))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    flp.query_pairs_mut().clear().extend_pairs(kept.iter().map(|(k, v)| (&**k, &**v)));

    Some(flp.to_string())
}

fn normalize_browser_url_for_launch(raw: &str) -> String {
    let normalized = sap_discovery_to_flp_url(raw).unwrap_or_else(|| raw.to_string());

    if normalized != raw {
        eprintln!(
            "[sap_adt] rewriting discovery URL for browser launch/open: {} -> {}",
            raw,
            normalized
        );
    }

    normalized
}

fn format_error_chain(err: &anyhow::Error) -> String {
    let mut parts = Vec::new();
    parts.push(err.to_string());
    for cause in err.chain().skip(1) {
        parts.push(cause.to_string());
    }
    parts.join(" | caused by: ")
}

fn summarize_cookie_header(cookie_header: &str) -> String {
    cookie_header
        .split(';')
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.split('=').next())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

fn log_discovery_request_debug(url: &str, cookie_header: &str) {
    eprintln!(
        "[sap_adt] discovery request debug url={} cookie_names=[{}] cookie_len={} url_details={} proxy_env={}",
        url,
        summarize_cookie_header(cookie_header),
        cookie_header.len(),
        describe_discovery_url(url),
        summarize_proxy_env()
    );
}

fn log_discovery_response_debug(status: reqwest::StatusCode, headers: &reqwest::header::HeaderMap, body: &str) {
    eprintln!("[sap_adt] discovery response status={}", status);
    for (name, value) in headers.iter() {
        let rendered = value.to_str().unwrap_or("<non-utf8>");
        eprintln!("[sap_adt] discovery response header {}={}", name.as_str(), rendered);
    }
    eprintln!("[sap_adt] discovery response body_len={}", body.len());
    eprintln!("[sap_adt] discovery response body={}", body);
}

fn log_discovery_error_debug(url: &str, cookie_header: &str, err: &anyhow::Error) {
    let reqwest_details = find_reqwest_error(err)
        .map(classify_reqwest_error)
        .unwrap_or_else(|| "kind=[non-reqwest] url=<none>".to_string());

    eprintln!(
        "[sap_adt] discovery request transport failed url={} cookie_names=[{}] cookie_len={} reqwest_details={} error_chain={}",
        url,
        summarize_cookie_header(cookie_header),
        cookie_header.len(),
        reqwest_details,
        format_error_chain(err)
    );
}

fn summarize_proxy_env() -> String {
    let keys = [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ];

    keys.iter()
        .filter_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| format!("{}={}", key, value))
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn describe_discovery_url(url: &str) -> String {
    match Url::parse(url) {
        Ok(parsed) => format!(
            "scheme={} host={} port={} path={} query={} fragment={}",
            parsed.scheme(),
            parsed.host_str().unwrap_or("<none>"),
            parsed
                .port_or_known_default()
                .map(|p| p.to_string())
                .unwrap_or_else(|| "<none>".to_string()),
            parsed.path(),
            parsed.query().unwrap_or("<none>"),
            parsed.fragment().unwrap_or("<none>")
        ),
        Err(err) => format!("unparseable_url error={}", err),
    }
}

fn classify_reqwest_error(err: &reqwest::Error) -> String {
    let mut flags = Vec::new();

    if err.is_builder() {
        flags.push("builder");
    }
    if err.is_redirect() {
        flags.push("redirect");
    }
    if err.is_status() {
        flags.push("status");
    }
    if err.is_timeout() {
        flags.push("timeout");
    }
    if err.is_request() {
        flags.push("request");
    }
    if err.is_connect() {
        flags.push("connect");
    }
    if err.is_body() {
        flags.push("body");
    }
    if err.is_decode() {
        flags.push("decode");
    }

    if flags.is_empty() {
        flags.push("unknown");
    }

    let url = err
        .url()
        .map(|u| u.as_str().to_string())
        .unwrap_or_else(|| "<none>".to_string());

    format!("kind=[{}] url={}", flags.join(","), url)
}

fn find_reqwest_error(err: &anyhow::Error) -> Option<&reqwest::Error> {
    for cause in err.chain() {
        if let Some(reqwest_err) = cause.downcast_ref::<reqwest::Error>() {
            return Some(reqwest_err);
        }
    }
    None
}

fn build_discovery_http_client(accept_invalid_certs: bool) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .danger_accept_invalid_certs(accept_invalid_certs)
        .build()
        .context("Failed to build discovery debug HTTP client")
}

fn try_discovery_transport_probe(
    discovery_url: &str,
    cookie_header: &str,
    accept_invalid_certs: bool,
) -> Result<String> {
    let client = build_discovery_http_client(accept_invalid_certs)?;

    let resp = client
        .get(discovery_url)
        .header(reqwest::header::COOKIE, cookie_header)
        .header(reqwest::header::ACCEPT, "application/xml, text/xml, */*")
        .header(reqwest::header::USER_AGENT, "mdev-sap-adt-debug/1.0")
        .send()
        .with_context(|| {
            format!(
                "Discovery request send failed for {} (accept_invalid_certs={})",
                discovery_url, accept_invalid_certs
            )
        })?;

    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp.text().unwrap_or_default();
    log_discovery_response_debug(status, &headers, &body);

    if !status.is_success() {
        return Err(anyhow!(
            "Discovery returned {} (accept_invalid_certs={}): {}",
            status,
            accept_invalid_certs,
            body
        ));
    }

    Ok(body)
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
    let normalized_url = normalize_browser_url_for_launch(url);

    client.send_json(json!({
        "cmd": "open_page",
        "session_id": session_id,
        "url": normalized_url,
        "timeout_ms": bridge_timeout_ms(cfg)
    }))?;

    Ok(())
}

pub fn launch_browser(cfg: &mut BrowserTurnConfig, url: Option<&str>) -> Result<String> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;

    client.ensure_started(&cfg.bridge_dir)?;

    let requested_profile = if cfg.profile.is_empty() { "auto" } else { &cfg.profile };
    let browser_channel_value = match requested_profile {
        "chrome" | "msedge" | "chromium" => Value::String(requested_profile.to_string()),
        _ => Value::Null,
    };
    let normalized_url = url.map(normalize_browser_url_for_launch);

    let value = client.send_json(json!({
        "cmd": "start_session",
        "session_id": Value::Null,
        "profile": requested_profile,
        "url": normalized_url,
        "headed": true,
        "user_data_dir": cfg.user_data_dir,
        "browser_channel": browser_channel_value,
        "timeout_ms": bridge_timeout_ms(cfg)
    }))?;

    let session_id = value
        .get("session_id")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("data").and_then(|v| v.get("session_id")).and_then(|v| v.as_str()))
        .ok_or_else(|| anyhow!("Browser bridge start_session response missing session_id"))?
        .to_string();

    cfg.session_id = Some(session_id.clone());
    Ok(session_id)
}

pub fn get_session_cookies(cfg: &mut BrowserTurnConfig, urls: &[String]) -> Result<String> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;

    client.ensure_started(&cfg.bridge_dir)?;

    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing before get_cookies"))?;
    let started = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis((bridge_timeout_ms(cfg) as u64).max(5_000));
    let poll = std::time::Duration::from_millis(1_000);
    let url_list = urls.join(", ");
    let mut attempt: u32 = 0;
    let mut last_header: Option<String> = None;

    loop {
        attempt += 1;

        let value = client.send_json(json!({
            "cmd": "get_cookies",
            "session_id": session_id,
            "urls": urls
        }))?;

        let data = value.get("data").cloned().unwrap_or(value);
        let cookie_header = data
            .get("cookie_header")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                data.get("cookies")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|c| {
                                let name = c.get("name").and_then(|v| v.as_str())?;
                                let value = c.get("value").and_then(|v| v.as_str())?;
                                Some(format!("{}={}", name, value))
                            })
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                    .filter(|s| !s.trim().is_empty())
            })
            .unwrap_or_default();

        let cookie_count = data
            .get("cookies")
            .and_then(|v| v.as_array())
            .map(|v| v.len())
            .unwrap_or(0);
        let cookie_name_list = data
            .get("cookie_names")
            .and_then(|v| v.as_array())
            .map(|names| {
                names
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .or_else(|| {
                data.get("cookies")
                    .and_then(|v| v.as_array())
                    .map(|cookies| {
                        cookies
                            .iter()
                            .filter_map(|c| c.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
                            .collect::<Vec<_>>()
                    })
            })
            .unwrap_or_default();
        let cookie_names = cookie_name_list.join(",");
        let has_required_cookies = {
            let has_mysapsso2 = cookie_name_list.iter().any(|name| name.eq_ignore_ascii_case("MYSAPSSO2"));
            let has_session = cookie_name_list.iter().any(|name| name.eq_ignore_ascii_case("SAP_SESSIONID_S4D_010"));
            let has_usercontext = cookie_name_list.iter().any(|name| name.eq_ignore_ascii_case("sap-usercontext"));
            has_mysapsso2 && has_session && has_usercontext
        };

        eprintln!(
            "[sap_adt] cookie harvest attempt={} session_id={} urls=[{}] cookie_count={} cookie_names=[{}] cookie_header={}",
            attempt,
            session_id,
            url_list,
            cookie_count,
            cookie_names,
            cookie_header
        );

        if has_required_cookies && !cookie_header.trim().is_empty() {
            return Ok(cookie_header);
        }

        if !cookie_header.trim().is_empty() {
            eprintln!(
                "[sap_adt] harvested cookies ready but waiting for full required set session_id={} cookie_names=[{}] cookie_len={}",
                session_id,
                cookie_names,
                cookie_header.len()
            );
        }

        last_header = Some(cookie_header);

        if started.elapsed() >= timeout {
            let last_header = last_header.unwrap_or_default();
            eprintln!(
                "[sap_adt] cookie harvest timeout session_id={} elapsed_ms={} last_cookie_header={}",
                session_id,
                started.elapsed().as_millis(),
                last_header
            );
            return Ok(last_header);
        }

        std::thread::sleep(poll);
    }
}

pub fn debug_discovery_request(discovery_url: &str, cookie_header: &str) -> Result<String> {
    log_discovery_request_debug(discovery_url, cookie_header);

    match try_discovery_transport_probe(discovery_url, cookie_header, false) {
        Ok(body) => Ok(body),
        Err(primary_err) => {
            log_discovery_error_debug(discovery_url, cookie_header, &primary_err);

            eprintln!(
                "[sap_adt] discovery request diagnostic retry starting url={} accept_invalid_certs=true",
                discovery_url
            );

            match try_discovery_transport_probe(discovery_url, cookie_header, true) {
                Ok(body) => {
                    eprintln!(
                        "[sap_adt] discovery diagnostic result: insecure retry succeeded; this strongly suggests TLS trust/certificate validation is the blocking issue for the normal request"
                    );
                    Ok(body)
                }
                Err(insecure_err) => {
                    log_discovery_error_debug(discovery_url, cookie_header, &insecure_err);
                    Err(primary_err.context(format!(
                        "Diagnostic insecure retry also failed for {}",
                        discovery_url
                    )))
                }
            }
        }
    }
}


pub fn close_session(cfg: &mut BrowserTurnConfig) -> Result<()> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("Browser bridge mutex poisoned"))?;

    client.ensure_started(&cfg.bridge_dir)?;

    let session_id = cfg.session_id.clone().ok_or_else(|| anyhow!("Browser session missing before close_session"))?;

    client.send_json(json!({
        "cmd": "close_session",
        "session_id": session_id
    }))?;

    cfg.session_id = None;
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
    remaining_ms.min(cfg.response_poll_ms.max(250))
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
    close_session(cfg)
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
            client.send_json(json!({
                "cmd": "set_poll_config",
                "session_id": session_id,
                "response_poll_ms": cfg.response_poll_ms.max(250),
                "dom_poll_ms": cfg.dom_poll_ms.max(250)
            }))?;
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
