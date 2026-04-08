use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
        OnceLock,
    },
};
 use std::collections::HashSet;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::header::{ACCEPT, CONTENT_TYPE, COOKIE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use super::browser_bridge;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SapAdtObjectManifest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub metadata_uri: String,
    #[serde(default)]
    pub object_uri: Option<String>,
    #[serde(default)]
    pub object_name: Option<String>,
    #[serde(default)]
    pub object_type: Option<String>,
    #[serde(default)]
    pub package_name: Option<String>,
    #[serde(skip_serializing, default)]
    pub etag: Option<String>,
    #[serde(skip_serializing, default)]
    pub metadata_xml: String,
    #[serde(default)]
    pub resources: Vec<SapAdtManifestResource>,
    #[serde(skip_serializing, default)]
    pub documents: Vec<SapAdtManifestDocument>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SapAdtManifestResource {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub rel: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(skip_serializing, default)]
    pub etag: Option<String>,
    #[serde(skip_serializing, default)]
    pub lock_handle: Option<String>,
    #[serde(skip_serializing, default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub readable: bool,
    #[serde(default)]
    pub editable: bool,
    #[serde(default)]
    pub activatable: bool,
    #[serde(default)]
    pub role: String,
    #[serde(skip_serializing, default)]
    pub body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SapAdtManifestDocument {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(skip_serializing, default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub path: String,
    #[serde(skip_serializing, default)]
    pub body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SapPackageObjectSummary {
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub object_type: String,
    #[serde(default)]
    pub package_name: Option<String>,
    #[serde(default)]
    pub source_uri: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SapConnectionConfig {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub auth_type: Option<String>,
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub authorization: Option<String>,
    #[serde(default)]
    pub cookie_header: Option<String>,
    #[serde(default)]
    pub negotiate_command: Option<String>,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub bridge_dir: Option<String>,
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn first_non_empty(values: &[Option<String>]) -> Option<String> {
    values
        .iter()
        .flatten()
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
}

fn sap_cookie_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sap_cookie_cache_key(config: &SapConnectionConfig) -> String {
    format!(
        "{}|{}",
        config.base_url.trim(),
        config.client.as_deref().unwrap_or("").trim()
    )
}

fn load_cached_cookie_header(config: &SapConnectionConfig) -> Option<String> {
    let key = sap_cookie_cache_key(config);
    sap_cookie_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&key).cloned())
        .filter(|value| !value.trim().is_empty())
}

fn store_cached_cookie_header(config: &SapConnectionConfig, cookie_header: &str) {
    let trimmed = cookie_header.trim();
    if trimmed.is_empty() {
        return;
    }
    if let Ok(mut cache) = sap_cookie_cache().lock() {
        cache.insert(sap_cookie_cache_key(config), trimmed.to_string());
    }
}

fn clear_cached_cookie_header(config: &SapConnectionConfig) {
    if let Ok(mut cache) = sap_cookie_cache().lock() {
        cache.remove(&sap_cookie_cache_key(config));
    }
}

pub fn harvest_cookie_header_for_runtime(base_url: &str, client: Option<String>) -> Result<String> {
    let config = SapConnectionConfig {
        base_url: base_url.trim().trim_end_matches('/').to_string(),
        auth_type: Some("cookie".to_string()),
        transport: Some("fetch".to_string()),
        username: None,
        password: None,
        authorization: None,
        cookie_header: None,
        negotiate_command: None,
        client,
        timeout_ms: Some(60_000),
        bridge_dir: None,
    };

    if let Some(cookie_header) = load_cached_cookie_header(&config) {
        return Ok(cookie_header);
    }

    let cookie_header = harvest_cookie_header_via_browser(&config)?;
    store_cached_cookie_header(&config, &cookie_header);
    Ok(cookie_header)
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SapImportObjectResult {
    pub object_uri: String,
    pub object_name: String,
    pub object_type: String,
    pub package_name: Option<String>,
    pub manifest_path: String,
    pub manifest_dir: String,
    pub resource_count: usize,
    pub document_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SapExportObjectResult {
    pub manifest_path: String,
    pub object_name: String,
    pub object_type: String,
    pub package_name: Option<String>,
    pub pushed_resource_uris: Vec<String>,
    pub syntax_ok: bool,
    pub syntax_details: String,
    pub activation_ok: bool,
    pub activation_details: String,
    pub workflow: Value,
}

#[derive(Debug, Deserialize)]
struct BridgeResponse {
    #[allow(dead_code)]
    id: u64,
    ok: bool,
    #[allow(dead_code)]
    cmd: String,
    #[allow(dead_code)]
    session_id: Option<String>,
    data: Value,
    #[serde(default)]
    error: Option<String>,
}

pub struct AdtBridgeProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: AtomicU64,
    session_id: Option<String>,
}

impl AdtBridgeProcess {
    pub fn start(bridge_dir: &Path) -> Result<Self> {
        let entry = bridge_dir.join("dist").join("index.js");
        if !entry.exists() {
            bail!("ADT bridge entrypoint does not exist: {}", entry.display());
        }

        let mut child = Command::new("node")
            .arg(entry)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to start ADT bridge")?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("ADT bridge stdin unavailable"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("ADT bridge stdout unavailable"))?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: AtomicU64::new(1),
            session_id: None,
        })
    }

    pub fn connect(&mut self, config: &SapConnectionConfig) -> Result<()> {
        let id = self.next_id();
        let payload = json!({
            "id": id,
            "cmd": "connect",
            "base_url": config.base_url,
            "auth": {
                "authType": config.auth_type,
                "transport": config.transport,
                "username": config.username,
                "password": config.password,
                "authorization": config.authorization,
                "cookieHeader": config.cookie_header,
                "negotiateCommand": config.negotiate_command,
                "client": config.client,
                "timeoutMs": config.timeout_ms,
                "sessionKey": format!("workflow-api-{}", id)
            }
        });
        let response = self.send(payload)?;
        let session_id = response
            .data
            .get("session_id")
            .and_then(Value::as_str)
            .or_else(|| response.session_id.as_deref())
            .ok_or_else(|| anyhow!("ADT bridge connect response missing session_id"))?;
        self.session_id = Some(session_id.to_string());
        Ok(())
    }

    pub fn list_package_objects(&mut self, package_name: &str, include_subpackages: bool) -> Result<Vec<SapPackageObjectSummary>> {
        let response = self.send_with_session(json!({
            "cmd": "list_package_objects",
            "package_name": package_name,
            "include_subpackages": include_subpackages
        }))?;

        let xml = response
            .data
            .get("body")
            .and_then(Value::as_str)
            .or_else(|| response.data.get("xml").and_then(Value::as_str))
            .unwrap_or_default();

        parse_package_tree_xml(xml)
    }

    pub fn read_object(&mut self, object_uri: &str, accept: Option<&str>) -> Result<BridgeReadResult> {
        let response = self.send_with_session(json!({
            "cmd": "read_object",
            "object_uri": object_uri,
            "accept": accept.unwrap_or("text/plain, application/xml, */*")
        }))?;
        parse_read_result(&response.data)
    }

    pub fn call_endpoint(
        &mut self,
        method: &str,
        uri: &str,
        body: Option<&str>,
        accept: Option<&str>,
        content_type: Option<&str>,
    ) -> Result<BridgeReadResult> {
        let response = self.send_with_session(json!({
            "cmd": "call_endpoint",
            "method": method,
            "uri": uri,
            "body": body,
            "accept": accept.unwrap_or("application/xml, text/xml, */*"),
            "content_type": content_type
        }))?;
        parse_read_result(&response.data)
    }

    pub fn lock_object(&mut self, object_uri: &str) -> Result<BridgeLockResult> {
        let response = self.send_with_session(json!({
            "cmd": "lock_object",
            "object_uri": object_uri
        }))?;
        let body = response.data.get("body").and_then(Value::as_str).unwrap_or_default().to_string();
        let headers = parse_headers(response.data.get("headers"));
        let lock_handle = header_lookup(&headers, "lock_handle")
            .or_else(|| header_lookup(&headers, "lock-handle"))
            .or_else(|| header_lookup(&headers, "x-lock-handle"))
            .or_else(|| extract_lock_handle(&body))
            .ok_or_else(|| anyhow!("lock response missing lock handle for {}", object_uri))?;
        Ok(BridgeLockResult { lock_handle, headers, body })
    }

    pub fn unlock_object(&mut self, object_uri: &str, lock_handle: &str) -> Result<()> {
        let _ = self.send_with_session(json!({
            "cmd": "unlock_object",
            "object_uri": object_uri,
            "lock_handle": lock_handle
        }))?;
        Ok(())
    }

    pub fn update_object(&mut self, object_uri: &str, source: &str, content_type: &str, lock_handle: Option<&str>, corr_nr: Option<&str>) -> Result<BridgeUpdateResult> {
        let response = self.send_with_session(json!({
            "cmd": "update_object",
            "object_uri": object_uri,
            "source": source,
            "content_type": content_type,
            "lock_handle": lock_handle,
            "corr_nr": corr_nr
        }))?;
        Ok(BridgeUpdateResult {
            status: response.data.get("status").and_then(Value::as_u64).map(|v| v as u16),
            headers: parse_headers(response.data.get("headers")),
            body: response.data.get("body").and_then(Value::as_str).unwrap_or_default().to_string(),
        })
    }

    pub fn syntax_check(&mut self, object_uri: &str) -> Result<BridgeSimpleResult> {
        let response = self.send_with_session(json!({
            "cmd": "syntax_check",
            "object_uri": object_uri
        }))?;
        Ok(BridgeSimpleResult {
            status: response.data.get("status").and_then(Value::as_u64).map(|v| v as u16),
            headers: parse_headers(response.data.get("headers")),
            body: response.data.get("body").and_then(Value::as_str).unwrap_or_default().to_string(),
        })
    }

    pub fn activate_object(&mut self, object_uri: &str) -> Result<BridgeSimpleResult> {
        let response = self.send_with_session(json!({
            "cmd": "activate_object",
            "object_uri": object_uri
        }))?;
        Ok(BridgeSimpleResult {
            status: response.data.get("status").and_then(Value::as_u64).map(|v| v as u16),
            headers: parse_headers(response.data.get("headers")),
            body: response.data.get("body").and_then(Value::as_str).unwrap_or_default().to_string(),
        })
    }

    pub fn get_problems(&mut self, result_uri: &str) -> Result<BridgeSimpleResult> {
        let response = self.send_with_session(json!({
            "cmd": "get_problems",
            "result_uri": result_uri
        }))?;
        Ok(BridgeSimpleResult {
            status: response.data.get("status").and_then(Value::as_u64).map(|v| v as u16),
            headers: parse_headers(response.data.get("headers")),
            body: response.data.get("body").and_then(Value::as_str).unwrap_or_default().to_string(),
        })
    }

    pub fn run_checkruns(&mut self, object_uri: &str) -> Result<BridgeSimpleResult> {
        let response = self.send_with_session(json!({
            "cmd": "call_endpoint",
            "method": "POST",
            "uri": "/sap/bc/adt/checkruns?reporters=abapCheckRun",
            "body": format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><chkrun:checkObjectList xmlns:chkrun=\"http://www.sap.com/adt/checkrun\" xmlns:adtcore=\"http://www.sap.com/adt/core\"><chkrun:checkObject adtcore:uri=\"{}\" chkrun:version=\"inactive\"/></chkrun:checkObjectList>",
                escape_xml_attr(object_uri)
            ),
            "content_type": "application/vnd.sap.adt.checkobjects+xml",
            "accept": "application/vnd.sap.adt.checkmessages+xml"
        }))?;
        Ok(BridgeSimpleResult {
            status: response.data.get("status").and_then(Value::as_u64).map(|v| v as u16),
            headers: parse_headers(response.data.get("headers")),
            body: response.data.get("body").and_then(Value::as_str).unwrap_or_default().to_string(),
        })
    }

    fn send_with_session(&mut self, mut payload: Value) -> Result<BridgeResponse> {
        let session_id = self.session_id.clone().ok_or_else(|| anyhow!("ADT bridge not connected"))?;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("id".to_string(), Value::from(self.next_id()));
            obj.insert("session_id".to_string(), Value::String(session_id));
        }
        self.send(payload)
    }

    fn send(&mut self, payload: Value) -> Result<BridgeResponse> {
        let line = serde_json::to_string(&payload).context("failed to serialize ADT bridge command")?;
        self.stdin.write_all(line.as_bytes()).context("failed writing ADT bridge command")?;
        self.stdin.write_all(b"\n").context("failed writing ADT bridge newline")?;
        self.stdin.flush().context("failed flushing ADT bridge stdin")?;

        let mut response_line = String::new();
        let read = self.stdout.read_line(&mut response_line).context("failed reading ADT bridge response")?;
        if read == 0 {
            bail!("ADT bridge closed unexpectedly")
        }

        let response: BridgeResponse = serde_json::from_str(response_line.trim()).context("failed parsing ADT bridge response")?;
        if !response.ok {
            bail!(response.error.unwrap_or_else(|| "ADT bridge command failed".to_string()));
        }
        Ok(response)
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }
}

impl Drop for AdtBridgeProcess {
    fn drop(&mut self) {
        if let Some(session_id) = self.session_id.clone() {
            let _ = self.send(json!({
                "id": self.next_id(),
                "cmd": "close_session",
                "session_id": session_id
            }));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn browser_bridge_entrypoint(bridge_root: &Path) -> Result<(String, Vec<String>)> {
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

    Err(anyhow!("browser bridge entrypoint not found under {}", bridge_root.display()))
}

fn ensure_browser_bridge_built(bridge_root: &Path) -> Result<()> {
    let dist_entry = bridge_root.join("dist").join("index.js");
    if dist_entry.exists() {
        return Ok(());
    }

    let package_json = bridge_root.join("package.json");
    if !package_json.exists() {
        return Err(anyhow!("browser bridge package.json not found under {}", bridge_root.display()));
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
        return Err(anyhow!("browser bridge build completed but dist/index.js was not created"));
    }

    Ok(())
}

struct BrowserBridgeProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: AtomicU64,
    session_id: Option<String>,
}

struct BrowserBridgeRuntime {
    browser: BrowserBridgeProcess,
    user_data_dir: PathBuf,
    discovery_url: String,
}

fn browser_bridge_runtime() -> &'static Mutex<Option<BrowserBridgeRuntime>> {
    static RUNTIME: OnceLock<Mutex<Option<BrowserBridgeRuntime>>> = OnceLock::new();
    RUNTIME.get_or_init(|| Mutex::new(None))
}

impl BrowserBridgeProcess {

    fn start(bridge_dir: &Path) -> Result<Self> {
        ensure_browser_bridge_built(bridge_dir)?;
        let (program, args) = browser_bridge_entrypoint(bridge_dir)?;

        tracing::info!(
            target: "workflow_api::sap",
            bridge_dir = %bridge_dir.display(),
            program = %program,
            args = ?args,
            "starting browser bridge process"
        );

        let mut child = Command::new(&program)
            .args(&args)
            .current_dir(bridge_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("failed to start browser bridge with {} {:?} from {}", program, args, bridge_dir.display()))?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("browser bridge stdin unavailable"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("browser bridge stdout unavailable"))?;

        std::thread::sleep(std::time::Duration::from_millis(1200));

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: AtomicU64::new(1),
            session_id: None,
        })
    }

    fn next_id(&self) -> String {
        self.next_id.fetch_add(1, Ordering::SeqCst).to_string()
    }

    fn send(&mut self, payload: Value) -> Result<Value> {
        let line = serde_json::to_string(&payload).context("failed to serialize browser bridge command")?;
        self.stdin.write_all(line.as_bytes()).context("failed writing browser bridge command")?;
        self.stdin.write_all(b"\n").context("failed writing browser bridge newline")?;
        self.stdin.flush().context("failed flushing browser bridge stdin")?;

        loop {
            let mut response_line = String::new();
            let read = self.stdout.read_line(&mut response_line).context("failed reading browser bridge response")?;
            if read == 0 {
                bail!("browser bridge closed unexpectedly")
            }

            let trimmed = response_line.trim();
            if trimmed.is_empty() {
                continue;
            }

            match serde_json::from_str::<Value>(trimmed) {
                Ok(response) => {
                    if !response.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                        let err = response.get("error").and_then(Value::as_str).unwrap_or("browser bridge command failed");
                        bail!(err.to_string())
                    }
                    return Ok(response);
                }
                Err(_) => {
                    tracing::warn!(
                        target: "workflow_api::sap",
                        line = %trimmed,
                        "ignoring non-JSON browser bridge stdout line"
                    );
                    continue;
                }
            }
        }
    }

    fn start_session(&mut self, user_data_dir: &str, profile: &str, url: &str, timeout_ms: u64) -> Result<String> {
        let requested_profile = if profile.trim().is_empty() { "msedge" } else { profile.trim() };
        let response = self.send(json!({
            "id": self.next_id(),
            "cmd": "start_session",
            "session_id": Value::Null,
            "profile": requested_profile,
            "url": url,
            "headed": true,
            "user_data_dir": user_data_dir,
            "browser_channel": browser_channel_value(requested_profile),
            "timeout_ms": timeout_ms.max(1000)
        }))?;

        let session_id = response
            .get("session_id")
            .and_then(Value::as_str)
            .or_else(|| response.get("data").and_then(|v| v.get("session_id")).and_then(Value::as_str))
            .ok_or_else(|| anyhow!("browser bridge start_session response missing session_id"))?
            .to_string();

        self.session_id = Some(session_id.clone());
        Ok(session_id)
    }

    fn connect_over_cdp(&mut self, cdp_url: &str, profile: &str, page_url_contains: &str, timeout_ms: u64) -> Result<String> {
        let requested_profile = if profile.trim().is_empty() { "auto" } else { profile.trim() };
        let response = self.send(json!({
            "id": self.next_id(),
            "cmd": "connect_over_cdp",
            "session_id": Value::Null,
            "profile": requested_profile,
            "cdp_url": cdp_url,
            "page_url_contains": if page_url_contains.trim().is_empty() { Value::Null } else { Value::String(page_url_contains.to_string()) },
            "timeout_ms": timeout_ms.max(1000)
        }))?;

        let session_id = response
            .get("session_id")
            .and_then(Value::as_str)
            .or_else(|| response.get("data").and_then(|v| v.get("session_id")).and_then(Value::as_str))
            .ok_or_else(|| anyhow!("browser bridge connect_over_cdp response missing session_id"))?
            .to_string();

        self.session_id = Some(session_id.clone());
        Ok(session_id)
    }

    fn open_page(&mut self, url: &str, timeout_ms: u64) -> Result<()> {
        let session_id = self.session_id.clone().ok_or_else(|| anyhow!("browser bridge session missing before open_page"))?;
        self.send(json!({
            "id": self.next_id(),
            "cmd": "open_page",
            "session_id": session_id,
            "url": url,
            "timeout_ms": timeout_ms.max(1000)
        }))?;
        Ok(())
    }

    fn get_cookies(&mut self, urls: &[String]) -> Result<Value> {
        let session_id = self.session_id.clone().ok_or_else(|| anyhow!("browser bridge session missing before get_cookies"))?;
        let response = self.send(json!({
            "id": self.next_id(),
            "cmd": "get_cookies",
            "session_id": session_id,
            "urls": urls
        }))?;
        Ok(response.get("data").cloned().unwrap_or(response))
    }
}

impl Drop for BrowserBridgeProcess {
    fn drop(&mut self) {
        if let Some(session_id) = self.session_id.clone() {
            let _ = self.send(json!({
                "id": self.next_id(),
                "cmd": "close_session",
                "session_id": session_id
            }));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn resolve_browser_bridge_dir_for_api() -> Result<PathBuf> {
    for key in ["MDEV_SAP_BROWSER_BRIDGE_DIR", "SAP_ADT_BROWSER_BRIDGE_DIR", "BROWSER_BRIDGE_DIR"] {
        if let Some(value) = env_var(key) {
            let path = PathBuf::from(value);
            if path.exists() {
                return Ok(path);
            }
        }
    }

    let cwd = std::env::current_dir().context("failed to resolve current_dir for browser bridge")?;
    for dir in cwd.ancestors() {
        let candidate = dir.join("bridge");
        if candidate.join("package.json").exists() {
            return Ok(candidate);
        }
    }

    bail!("could not resolve browser bridge directory")
}

fn resolve_browser_user_data_dir_for_api() -> Result<PathBuf> {
    if let Some(value) = env_var("MDEV_SAP_BROWSER_USER_DATA_DIR").or_else(|| env_var("SAP_ADT_BROWSER_USER_DATA_DIR")) {
        let path = PathBuf::from(value);
        std::fs::create_dir_all(&path)
            .with_context(|| format!("failed to create browser user data dir {}", path.display()))?;
        return Ok(path);
    }

    let base = env_var("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| env_var("APPDATA").map(PathBuf::from))
        .or_else(|| env_var("HOME").map(|home| PathBuf::from(home).join(".local").join("share")))
        .unwrap_or_else(std::env::temp_dir);
    let path = base.join("mdev").join("sap-adt-browser");
    std::fs::create_dir_all(&path)
        .with_context(|| format!("failed to create browser user data dir {}", path.display()))?;
    Ok(path)
}

fn resolve_browser_profile_for_api() -> String {
    env_var("MDEV_SAP_BROWSER_CHANNEL")
        .or_else(|| env_var("SAP_ADT_BROWSER_CHANNEL"))
        .or_else(|| env_var("BROWSER_CHANNEL"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "msedge".to_string())
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
        Ok(mut addrs) => addrs.any(|addr| TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(250)).is_ok()),
        Err(_) => false,
    }
}

fn resolve_edge_executable_for_api() -> String {
    if let Some(value) = env_var("MDEV_BROWSER_EDGE_EXECUTABLE")
        .or_else(|| env_var("SAP_ADT_BROWSER_EDGE_EXECUTABLE"))
        .or_else(|| env_var("BROWSER_EDGE_EXECUTABLE"))
    {
        return value;
    }

    #[cfg(target_os = "windows")]
    {
        let candidates = [
            "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
            "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
        ];

        for candidate in candidates {
            if Path::new(candidate).exists() {
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

fn maybe_launch_edge_for_api(user_data_dir: &Path, cdp_url: &str, target_url: &str) -> Result<()> {
    if cdp_reachable(cdp_url) {
        return Ok(());
    }

    let executable = resolve_edge_executable_for_api();
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

    tracing::info!(
        target: "workflow_api::sap",
        executable = %executable,
        user_data_dir = %user_data_dir.display(),
        port,
        target_url = %target_url,
        "launching Edge with remote debugging for SAP cookie harvest"
    );

    let _child = Command::new(&executable)
        .arg(format!("--remote-debugging-port={}", port))
        .arg(format!("--user-data-dir={}", user_data_dir.to_string_lossy()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg(target_url)
        .spawn()
        .with_context(|| format!("failed to launch Edge via {}", executable))?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if cdp_reachable(cdp_url) {
            tracing::info!(target: "workflow_api::sap", cdp_url = %cdp_url, "cdp endpoint became reachable after launching Edge");
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    bail!("launched Edge but CDP endpoint did not become reachable at {}", cdp_url)
}

fn browser_channel_value(profile: &str) -> Value {
    match profile.trim().to_ascii_lowercase().as_str() {
        "chrome" => Value::String("chrome".to_string()),
        "msedge" => Value::String("msedge".to_string()),
        "chromium" => Value::String("chromium".to_string()),
        _ => Value::Null,
    }
}

fn sap_discovery_to_flp_url(raw: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(raw).ok()?;
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

fn browser_launch_url_for_connection(config: &SapConnectionConfig) -> Result<String> {
    let discovery_url = discovery_url_for_connection(config)?;
    Ok(sap_discovery_to_flp_url(&discovery_url).unwrap_or(discovery_url))
}

fn discovery_url_for_connection(config: &SapConnectionConfig) -> Result<String> {
    let mut url = if config.base_url.to_ascii_lowercase().contains("/sap/bc/adt/discovery") {
        reqwest::Url::parse(&config.base_url)
            .with_context(|| format!("invalid SAP ADT discovery URL: {}", config.base_url))?
    } else {
        reqwest::Url::parse(&format!("{}/sap/bc/adt/discovery", config.base_url.trim_end_matches('/')))
            .with_context(|| format!("invalid SAP ADT base URL: {}", config.base_url))?
    };

    if let Some(client) = config.client.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
        url.query_pairs_mut().append_pair("sap-client", client);
    }

    Ok(url.to_string())
}

fn cookie_urls_for_discovery(discovery_url: &str) -> Vec<String> {
    let mut urls = vec![discovery_url.to_string()];

    if let Some(flp_url) = sap_discovery_to_flp_url(discovery_url) {
        if !urls.iter().any(|value| value == &flp_url) {
            urls.push(flp_url);
        }
    }

    if let Ok(url) = reqwest::Url::parse(discovery_url) {
        if let Some(host) = url.host_str() {
            let mut origin = format!("{}://{}", url.scheme(), host);
            if let Some(port) = url.port() {
                origin.push(':');
                origin.push_str(&port.to_string());
            }
            if !urls.iter().any(|value| value == &origin) {
                urls.push(origin);
            }
        }
    }
    urls
}

fn shared_browser_cfg_for_sap(config: &SapConnectionConfig) -> Result<browser_bridge::BrowserTurnConfig> {
    let bridge_dir = resolve_browser_bridge_dir_for_api()?;
    let user_data_dir = resolve_browser_user_data_dir_for_api()?;
    let browser_profile = resolve_browser_profile_for_api();
    let cdp_url = env_var("MDEV_BROWSER_CDP_URL")
        .or_else(|| env_var("SAP_ADT_BROWSER_CDP_URL"))
        .or_else(|| env_var("BROWSER_CDP_URL"))
        .unwrap_or_else(|| "http://127.0.0.1:9222".to_string());

    Ok(browser_bridge::BrowserTurnConfig {
        bridge_dir: bridge_dir.to_string_lossy().to_string(),
        edge_executable: resolve_edge_executable_for_api(),
        user_data_dir: user_data_dir.to_string_lossy().to_string(),
        cdp_url,
        page_url_contains: String::new(),
        profile: browser_profile,
        session_id: None,
        auto_launch_edge: true,
        runtime_key: "sap_cookie_harvest".to_string(),
        response_timeout_ms: config.timeout_ms.unwrap_or(60_000).max(10_000),
        response_poll_ms: 1_000,
        dom_poll_ms: 1_000,
    })
}

fn harvest_cookie_header_via_browser(config: &SapConnectionConfig) -> Result<String> {
    let discovery_url = discovery_url_for_connection(config)?;
    let browser_launch_url = browser_launch_url_for_connection(config)?;
    let cookie_urls = cookie_urls_for_discovery(&discovery_url);
    let mut browser = shared_browser_cfg_for_sap(config)?;

    tracing::info!(
        target: "workflow_api::sap",
        discovery_url = %discovery_url,
        browser_launch_url = %browser_launch_url,
        bridge_dir = %browser.bridge_dir,
        user_data_dir = %browser.user_data_dir,
        browser_profile = %browser.profile,
        cdp_url = %browser.cdp_url,
        "using browser bridge attach path for SAP cookie harvest"
    );

    browser_bridge::launch_and_attach(&mut browser)
        .context("Failed to attach browser bridge session for SAP ADT")?;
    browser_bridge::open_url_in_session(&mut browser, &browser_launch_url)
        .context("Failed to open SAP browser bootstrap URL in browser bridge session")?;

    let cookie_header = browser_bridge::get_session_cookies(&mut browser, &cookie_urls)
        .context("Failed to harvest SAP ADT cookies from browser bridge")?;

    Ok(cookie_header)
}

async fn direct_http_get_with_cookies(config: &SapConnectionConfig, url_or_path: &str, accept: &str) -> Result<BridgeReadResult> {
    let timeout = std::time::Duration::from_millis(config.timeout_ms.unwrap_or(60_000).max(1_000));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .danger_accept_invalid_certs(true)
        .build()
        .context("failed to build reqwest client for SAP direct HTTP request")?;

    let url = if url_or_path.starts_with("http://") || url_or_path.starts_with("https://") {
        url_or_path.to_string()
    } else {
        format!(
            "{}{}",
            config.base_url.trim_end_matches('/'),
            if url_or_path.starts_with('/') { url_or_path.to_string() } else { format!("/{}", url_or_path) }
        )
    };

    let mut cookie_header = load_cached_cookie_header(config)
        .or_else(|| {
            config
                .cookie_header
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| v.to_string())
        })
        .ok_or_else(|| anyhow!("SAP direct HTTP request requires a non-empty cookie header"))?;

    let mut resp = client
        .get(&url)
        .header(ACCEPT, accept)
        .header(COOKIE, cookie_header.as_str())
        .send()
        .await
        .with_context(|| format!("failed direct SAP HTTP GET to {}", url))?;

    let mut status = resp.status();
    let mut content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());
    let mut headers = resp
        .headers()
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|vv| (k.as_str().to_string(), vv.to_string())))
        .collect::<Vec<_>>();
    let mut body = resp.text().await.context("failed to read SAP direct HTTP response body")?;

    let unauthorized = status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN;
    let html_login = looks_like_non_adt_html(&body);

    if unauthorized || html_login {
        clear_cached_cookie_header(config);
        cookie_header = harvest_cookie_header_for_runtime(&config.base_url, config.client.clone())?;
        store_cached_cookie_header(config, &cookie_header);

        resp = client
            .get(&url)
            .header(ACCEPT, accept)
            .header(COOKIE, cookie_header.as_str())
            .send()
            .await
            .with_context(|| format!("failed direct SAP HTTP GET retry to {}", url))?;

        status = resp.status();
        content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());
        headers = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|vv| (k.as_str().to_string(), vv.to_string())))
            .collect::<Vec<_>>();
        body = resp.text().await.context("failed to read SAP direct HTTP retry response body")?;
    }

    tracing::info!(
        target: "workflow_api::sap",
        uri = %url,
        status = status.as_u16(),
        content_type = ?content_type,
        header_count = headers.len(),
        body_len = body.len(),
        looks_like_html = looks_like_non_adt_html(&body),
        looks_like_adt = looks_like_valid_adt_package_payload(&body),
        body_preview = %preview_xml(&body, 400),
        "direct SAP HTTP GET completed"
    );

    Ok(BridgeReadResult {
        object_uri: url,
        content_type,
        headers,
        body,
    })
}

async fn load_package_tree_xml_direct(config: &SapConnectionConfig, package_name: &str, include_subpackages: bool) -> Result<String> {
    let attempts = if include_subpackages {
        vec![
            format!("/sap/bc/adt/packages/$tree?packagename={}&type=all", encode_component(package_name)),
            format!("/sap/bc/adt/packages/$tree?packagename={}", encode_component(package_name)),
            format!("/sap/bc/adt/packages/$tree?packagename={}&type=package", encode_component(package_name)),
            format!("/sap/bc/adt/packages/$tree?packagename={}&type=flat", encode_component(package_name)),
        ]
    } else {
        vec![
            format!("/sap/bc/adt/packages/$tree?packagename={}&type=package", encode_component(package_name)),
            format!("/sap/bc/adt/packages/$tree?packagename={}", encode_component(package_name)),
            format!("/sap/bc/adt/packages/$tree?packagename={}&type=flat", encode_component(package_name)),
        ]
    };

    let mut last_xml = String::new();

    for uri in attempts {
        let resp = direct_http_get_with_cookies(config, &uri, "application/xml, text/xml, */*").await?;
        let xml = resp.body;
        last_xml = xml.clone();

        tracing::info!(
            target: "workflow_api::sap",
            package_name = %package_name,
            include_subpackages,
            uri = %uri,
            content_type = ?resp.content_type,
            header_count = resp.headers.len(),
            xml_len = xml.len(),
            object_ref_count = xml.matches("objectReference").count(),
            package_node_markers = xml.matches("packageTree").count(),
            uri_attr_count = xml.matches("uri=").count(),
            empty_tree = looks_like_empty_package_tree(&xml),
            has_error_marker = looks_like_non_adt_html(&xml) || looks_like_adt_error_payload(&xml),
            xml_preview = %preview_xml(&xml, 400),
            "package tree attempt completed"
        );

        if looks_like_non_adt_html(&xml) {
            continue;
        }
        if !looks_like_empty_package_tree(&xml) {
            return Ok(xml);
        }
    }

    if !last_xml.trim().is_empty() {
        bail!("ADT package tree returned non-ADT content for {}", package_name);
    }
    bail!("ADT package tree returned no usable payload for {}", package_name)
}

fn log_direct_adt_payload(
    stage: &str,
    package_name: &str,
    uri: &str,
    content_type: Option<&str>,
    headers: &[(String, String)],
    xml: &str,
) {
    tracing::info!(
        target: "workflow_api::sap",
        stage = stage,
        package_name = %package_name,
        uri = %uri,
        content_type = ?content_type,
        header_count = headers.len(),
        xml_len = xml.len(),
        object_ref_count = xml.matches("objectReference").count(),
        package_node_markers = xml.matches("packageTree").count(),
        uri_attr_count = xml.matches("uri=").count(),
        looks_like_html = looks_like_non_adt_html(xml),
        looks_like_adt_error = looks_like_adt_error_payload(xml),
        looks_like_valid_adt = looks_like_valid_adt_package_payload(xml),
        empty_tree = looks_like_empty_package_tree(xml),
        xml_preview = %preview_xml(xml, 400),
        "direct ADT payload received"
    );
}

async fn load_package_metadata_summary_direct(config: &SapConnectionConfig, package_name: &str) -> Result<(String, String)> {
    let uri = format!("/sap/bc/adt/packages/{}", encode_component(package_name));
    let resp = direct_http_get_with_cookies(config, &uri, "application/xml, text/xml, */*").await?;
    let content_type = resp.content_type.clone();
    let headers = resp.headers.clone();
    let xml = resp.body;

    log_direct_adt_payload(
        "package_metadata",
        package_name,
        &uri,
        content_type.as_deref(),
        &headers,
        &xml,
    );

    if looks_like_non_adt_html(&xml) {
        bail!("package metadata returned HTML/login page for {}", uri);
    }

    if looks_like_adt_error_payload(&xml) && !looks_like_valid_adt_package_payload(&xml) {
        bail!("package metadata returned exception payload for {}", uri);
    }

    let package_uri = extract_xml_attr(&xml, &["adtcore:uri", "uri"])
        .or_else(|| Some(uri.clone()))
        .ok_or_else(|| anyhow!("package metadata response missing uri for {}", package_name))?;
    let package_type = extract_xml_attr(&xml, &["adtcore:type", "type"])
        .or_else(|| Some("DEVC/K".to_string()))
        .ok_or_else(|| anyhow!("package metadata response missing type for {}", package_name))?;

    tracing::info!(
        target: "workflow_api::sap",
        package_name = %package_name,
        metadata_uri = %package_uri,
        package_type = %package_type,
        "resolved package metadata summary via direct HTTP"
    );

    Ok((package_uri, package_type))
}

async fn fetch_csrf_token_direct(config: &SapConnectionConfig) -> Result<String> {
    let cookie_header = load_cached_cookie_header(config)
        .or_else(|| {
            config
                .cookie_header
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| v.to_string())
        })
        .ok_or_else(|| anyhow!("SAP direct HTTP request requires a non-empty cookie header"))?;

    let timeout = std::time::Duration::from_millis(config.timeout_ms.unwrap_or(60_000).max(1_000));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .danger_accept_invalid_certs(true)
        .build()
        .context("failed to build reqwest client for SAP CSRF fetch")?;

    let mut url = format!("{}/sap/bc/adt/discovery", config.base_url.trim_end_matches('/'));
    if let Some(client_id) = config.client.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        url.push_str("?sap-client=");
        url.push_str(&encode_component(client_id));
    }

    let resp = client
        .get(&url)
        .header(ACCEPT, "application/xml, text/xml, */*")
        .header(COOKIE, cookie_header.as_str())
        .header("X-CSRF-Token", "Fetch")
        .send()
        .await
        .with_context(|| format!("failed direct SAP CSRF fetch to {}", url))?;

    let token = resp
        .headers()
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("SAP CSRF fetch did not return x-csrf-token"))?;

    Ok(token.to_string())
}

async fn load_nodestructure_xml_direct(
    config: &SapConnectionConfig,
    package_name: &str,
    package_uri: &str,
    package_type: &str,
) -> Result<String> {
    let uri = format!(
        "/sap/bc/adt/repository/nodestructure?parent_type={}&parent_name={}",
        encode_component(package_type),
        encode_component(package_name)
    );
    let body = format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
            "<adtcore:objectReferences xmlns:adtcore=\"http://www.sap.com/adt/core\">",
            "<adtcore:objectReference ",
            "uri=\"{}\" ",
            "name=\"{}\" ",
            "type=\"{}\" ",
            "parentUri=\"{}\" />",
            "</adtcore:objectReferences>"
        ),
        package_uri,
        package_name,
        package_type,
        package_uri
    );

    let cookie_header = config
        .cookie_header
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .or_else(|| load_cached_cookie_header(config))
        .ok_or_else(|| anyhow!("SAP direct HTTP request requires a non-empty cookie header"))?;

    let csrf_token = fetch_csrf_token_direct(config).await?;

    let timeout = std::time::Duration::from_millis(config.timeout_ms.unwrap_or(60_000).max(1_000));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .danger_accept_invalid_certs(true)
        .build()
        .context("failed to build reqwest client for SAP nodestructure request")?;

    let url = format!("{}{}", config.base_url.trim_end_matches('/'), &uri);

    let resp = client
        .post(&url)
        .header(
            ACCEPT,
            "application/vnd.sap.adt.repository.nodestructure.v2+xml, application/xml, */*",
        )
        .header(
            CONTENT_TYPE,
            "application/vnd.sap.adt.repository.nodestructure.v2+xml",
        )
        .header(COOKIE, cookie_header.as_str())
        .header("X-CSRF-Token", csrf_token.as_str())
        .body(body)
        .send()
        .await
        .with_context(|| format!("failed direct SAP nodestructure POST to {}", url))?;

    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());

    let headers = resp
        .headers()
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|vv| (k.as_str().to_string(), vv.to_string())))
        .collect::<Vec<_>>();

    let xml = resp
        .text()
        .await
        .context("failed to read SAP nodestructure response body")?;

    log_direct_adt_payload(
        "nodestructure",
        package_name,
        &uri,
        content_type.as_deref(),
        &headers,
        &xml,
    );

    if looks_like_non_adt_html(&xml) {
        bail!("nodestructure returned HTML/login page for {}", uri);
    }

    if xml.trim().eq_ignore_ascii_case("CSRF token validation failed")
        || xml.to_ascii_lowercase().contains("csrf token validation failed")
    {
        bail!("nodestructure returned CSRF validation failure for {}", uri);
    }

    if looks_like_adt_error_payload(&xml) && !looks_like_valid_adt_package_payload(&xml) {
        bail!("nodestructure returned exception payload for {}", uri);
    }

    Ok(xml)
}

async fn search_repository_objects_xml_direct(config: &SapConnectionConfig, package_name: &str) -> Result<String> {
    let uri = format!(
        "/sap/bc/adt/repository/informationsystem/search?query={}&maxResults=500",
        encode_component(package_name)
    );
    let resp = direct_http_get_with_cookies(config, &uri, "application/xml, text/xml, */*").await?;
    let content_type = resp.content_type.clone();
    let headers = resp.headers.clone();
    let xml = resp.body;

    log_direct_adt_payload(
        "repository_search",
        package_name,
        &uri,
        content_type.as_deref(),
        &headers,
        &xml,
    );

    if looks_like_non_adt_html(&xml) {
        bail!("repository search returned HTML/login page for {}", uri);
    }

    if looks_like_adt_error_payload(&xml) && !looks_like_valid_adt_package_payload(&xml) {
        bail!("repository search returned exception payload for {}", uri);
    }

    Ok(xml)
}

async fn validate_adt_session(_bridge: &mut AdtBridgeProcess, config: &SapConnectionConfig) -> Result<()> {
    let discovery_url = discovery_url_for_connection(config)?;
    let cookie_header = config
        .cookie_header
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("SAP cookie auth validation requires a non-empty cookie header"))?;

    let timeout = std::time::Duration::from_millis(config.timeout_ms.unwrap_or(60_000).max(1_000));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .danger_accept_invalid_certs(true)
        .build()
        .context("failed to build reqwest client for SAP ADT validation")?;

    let resp = client
        .get(&discovery_url)
        .header(ACCEPT, "application/xml, text/xml, */*")
        .header(COOKIE, cookie_header)
        .send()
        .await
        .with_context(|| format!("failed to send SAP ADT discovery request to {}", discovery_url))?;

    let status = resp.status();
    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = resp
        .text()
        .await
        .context("failed to read SAP ADT discovery response body")?;
    let lower = body.trim().to_ascii_lowercase();

    let looks_like_html = lower.starts_with("<html")
        || lower.contains("<!doctype html")
        || lower.contains("document.forms[0].submit()")
        || lower.contains("<body");

    let looks_like_adt = lower.contains("<servicedefinition")
        || lower.contains(":servicedefinition")
        || lower.contains("<adtcore:")
        || lower.contains("/sap/bc/adt");

    tracing::info!(
        target: "workflow_api::sap",
        discovery_url = %discovery_url,
        status = %status,
        content_type = %content_type,
        body_len = body.len(),
        looks_like_html,
        looks_like_adt,
        body_preview = %preview_xml(&body, 300),
        "validated ADT discovery via direct HTTP using harvested cookies"
    );

    if !status.is_success() {
        bail!("SAP ADT discovery validation failed with HTTP status {}", status);
    }

    if looks_like_html || !looks_like_adt {
        bail!("SAP ADT discovery returned non-ADT content after cookie harvest; cookies/session are not valid for backend ADT access")
    }

    Ok(())
}

pub async fn ensure_adt_bridge_connected(bridge: &mut AdtBridgeProcess, config: &SapConnectionConfig) -> Result<SapConnectionConfig> {
    let auth_type = config.auth_type.clone().unwrap_or_else(|| "cookie".to_string()).to_ascii_lowercase();
    if auth_type != "cookie" {
        bridge.connect(config)?;
        validate_adt_session(bridge, config).await?;
        return Ok(config.clone());
    }

    let mut effective = config.clone();

    if effective.cookie_header.as_deref().unwrap_or("").trim().is_empty() {
        effective.cookie_header = load_cached_cookie_header(&effective);
    }

    if effective.cookie_header.as_deref().unwrap_or("").trim().is_empty() {
        tracing::info!(target: "workflow_api::sap", base_url = %effective.base_url, "no cached SAP cookie header present; harvesting via browser bridge");
        let harvested = harvest_cookie_header_via_browser(&effective)?;
        store_cached_cookie_header(&effective, &harvested);
        effective.cookie_header = Some(harvested);
    }

    match bridge.connect(&effective) {
        Ok(_) => {
            match validate_adt_session(bridge, &effective).await {
                Ok(_) => {
                    if let Some(cookie_header) = effective.cookie_header.as_deref() {
                        store_cached_cookie_header(&effective, cookie_header);
                    }
                    Ok(effective)
                }
                Err(first_err) => {
                    tracing::warn!(
                        target: "workflow_api::sap",
                        base_url = %effective.base_url,
                        error = %first_err,
                        "SAP ADT session validation failed with current cookies; refreshing browser cookies and retrying once"
                    );

                    let refreshed = harvest_cookie_header_via_browser(&effective)?;
                    store_cached_cookie_header(&effective, &refreshed);
                    effective.cookie_header = Some(refreshed);
                    bridge.connect(&effective)?;
                    validate_adt_session(bridge, &effective)
                        .await
                        .with_context(|| format!("SAP ADT session validation failed after cookie refresh: {:#}", first_err))?;
                    Ok(effective)
                }
            }
        }
        Err(first_err) => {
            tracing::warn!(
                target: "workflow_api::sap",
                base_url = %effective.base_url,
                error = %first_err,
                "SAP ADT bridge connect failed with current cookies; refreshing browser cookies and retrying once"
            );

            let refreshed = harvest_cookie_header_via_browser(&effective)?;
            store_cached_cookie_header(&effective, &refreshed);
            effective.cookie_header = Some(refreshed);
            bridge.connect(&effective).with_context(|| format!("SAP ADT bridge connect failed after cookie refresh: {:#}", first_err))?;
            validate_adt_session(bridge, &effective).await?;
            Ok(effective)
        }
    }
}

#[derive(Clone, Debug)]
pub struct SapResolvedObject {
    pub uri: String,
    pub source_uri: Option<String>,
    pub name: String,
    pub object_type: String,
    pub package_name: Option<String>,
}

fn build_package_search_queries(package_name: &str) -> Vec<String> {
    let raw = package_name.trim().to_uppercase();
    if raw.is_empty() {
        return Vec::new();
    }

    let mut out = vec![
        raw.clone(),
        format!("devclass:{}", raw),
        format!("package:{}", raw),
        format!("devclass:{}*", raw),
        format!("{}*", raw),
    ];
    out.sort();
    out.dedup();
    out
}

fn encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push_str("%20"),
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
}

fn extract_xml_attr_value(xml: &str, attr_name: &str) -> Option<String> {
    let needle = format!("{}=\"", attr_name);
    let start = xml.find(&needle)? + needle.len();
    let rest = &xml[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn repository_parent_type(object_type: &str) -> &str {
    object_type.split('/').next().unwrap_or(object_type)
}

fn empty_object_references_xml() -> String {
    "<?xml version=\"1.0\" encoding=\"utf-8\"?><adtcore:objectReferences xmlns:adtcore=\"http://www.sap.com/adt/core\"/>".to_string()
}

fn looks_like_empty_package_tree(xml: &str) -> bool {
    let trimmed = xml.trim();
    let lower = trimmed.to_ascii_lowercase();
    let has_package_tree = lower.contains("<packagetree") || lower.contains(":packagetree");
    let has_object_reference = lower.contains("<objectreference") || lower.contains(":objectreference");
    let has_uri_attr = lower.contains(" uri=") || lower.contains(" adtcore:uri=") || lower.contains("\nuri=") || lower.contains("\nadtcore:uri=");
    has_package_tree && !has_object_reference && !has_uri_attr
}

fn looks_like_non_adt_html(xml: &str) -> bool {
    let lower = xml.trim().to_ascii_lowercase();
    lower.starts_with("<html")
        || lower.contains("<!doctype html")
        || lower.contains("<body")
        || lower.contains("document.forms[0].submit()")
}

fn looks_like_adt_error_payload(xml: &str) -> bool {
    let lower = xml.trim().to_ascii_lowercase();
    lower.contains("<exception")
        || lower.contains(":exception")
        || lower.contains("<message")
}

fn looks_like_valid_adt_package_payload(xml: &str) -> bool {
    let lower = xml.trim().to_ascii_lowercase();
    lower.contains("<adtcore:objectreferences")
        || lower.contains("<objectreferences")
        || lower.contains("<packagetree")
        || lower.contains(":packagetree")
        || lower.contains("<asx:abap")
        || lower.contains("<abap")
}


fn looks_like_global_package_catalog(
    xml: &str,
    package_uri: &str,
    package_type: &str,
) -> bool {
    let parsed = match parse_package_tree_xml(xml) {
        Ok(items) => items,
        Err(_) => return false,
    };

    if parsed.is_empty() {
        return false;
    }

    let all_package_nodes = parsed.iter().all(|item| {
        item.object_type == package_type && item.uri.starts_with("/sap/bc/adt/packages/")
    });

    let has_foreign_package_nodes = parsed.iter().any(|item| {
        item.uri.starts_with("/sap/bc/adt/packages/")
            && !item.uri.eq_ignore_ascii_case(package_uri)
    });

    let has_non_package_nodes = parsed.iter().any(|item| {
        item.object_type != package_type || !item.uri.starts_with("/sap/bc/adt/packages/")
    });

    all_package_nodes && has_foreign_package_nodes && !has_non_package_nodes
}



fn load_package_tree_xml(
    bridge: &mut AdtBridgeProcess,
    connection: &SapConnectionConfig,
    package_name: &str,
    include_subpackages: bool,
) -> Result<String> {
    let mut attempts: Vec<Vec<(&str, String)>> = Vec::new();

    if include_subpackages {
        attempts.push(vec![
            ("packagename", package_name.to_string()),
            ("type", "all".to_string()),
        ]);
    } else {
        attempts.push(vec![
            ("packagename", package_name.to_string()),
            ("type", "package".to_string()),
        ]);
    }

    attempts.push(vec![
        ("packagename", package_name.to_string()),
    ]);
    attempts.push(vec![
        ("packagename", package_name.to_string()),
        ("type", "all".to_string()),
    ]);
    attempts.push(vec![
        ("packagename", package_name.to_string()),
        ("type", "package".to_string()),
    ]);
    attempts.push(vec![
        ("packagename", package_name.to_string()),
        ("type", "flat".to_string()),
    ]);

    let mut last_xml = String::new();

    for pairs in attempts {
        let mut query: Vec<String> = Vec::new();
        for (key, value) in pairs {
            query.push(format!("{}={}", key, encode_component(&value)));
        }
        if let Some(client) = connection.client.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
            query.push(format!("sap-client={}", encode_component(client)));
        }

        let uri = format!("/sap/bc/adt/packages/$tree?{}", query.join("&"));
        let resp = bridge.call_endpoint(
            "GET",
            &uri,
            None,
            Some("application/xml, text/xml, */*"),
            None,
        )?;

        let content_type = resp.content_type.clone();
        let header_count = resp.headers.len();
        let xml = resp.body;

        let object_ref_count = xml.matches("objectReference").count();
        let uri_attr_count = xml.matches("adtcore:uri=").count() + xml.matches(" uri=").count();
        let package_node_markers = xml.matches("package").count() + xml.matches("treeNode").count();
        let empty_tree = looks_like_empty_package_tree(&xml);
        let has_error_marker = xml.contains("<exception") || xml.contains(":exception") || xml.contains("<message") || xml.contains("<html") || xml.contains("<!DOCTYPE html");

        tracing::info!(
            target: "workflow_api::sap",
            package_name = %package_name,
            include_subpackages,
            uri = %uri,
            content_type = ?content_type,
            header_count,
            xml_len = xml.len(),
            object_ref_count,
            package_node_markers,
            uri_attr_count,
            empty_tree,
            has_error_marker,
            xml_preview = %preview_xml(&xml, 400),
            "package tree attempt completed"
        );

        last_xml = xml.clone();

        if looks_like_non_adt_html(&xml) {
            bail!("ADT package tree returned HTML/login page for {}", uri);
        }

        if looks_like_adt_error_payload(&xml) && !looks_like_valid_adt_package_payload(&xml) {
            bail!("ADT package tree returned exception payload for {}", uri);
        }

        if !looks_like_empty_package_tree(&xml) {
            return Ok(xml);
        }
    }

    Ok(last_xml)
}

fn load_package_metadata_summary(
    bridge: &mut AdtBridgeProcess,
    package_name: &str,
) -> Result<(String, String)> {
    let package_uri = format!("/sap/bc/adt/packages/{}", package_name.trim().to_ascii_lowercase());

    let resp = bridge.call_endpoint(
        "GET",
        &package_uri,
        None,
        Some("application/xml, text/xml, */*"),
        None,
    )?;

    let xml = resp.body;
    let package_type = extract_xml_attr_value(&xml, "adtcore:type")
        .or_else(|| extract_xml_attr_value(&xml, "type"))
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| anyhow!("ADT package metadata did not expose a package type"))?;

    Ok((package_uri, package_type))
}

fn load_nodestructure_xml(
    bridge: &mut AdtBridgeProcess,
    package_name: &str,
    package_uri: &str,
    package_type: &str,
) -> Result<String> {
    let body = format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
            "<adtcore:objectReferences xmlns:adtcore=\"http://www.sap.com/adt/core\">",
            "<adtcore:objectReference ",
            "uri=\"{}\" ",
            "name=\"{}\" ",
            "type=\"{}\" ",
            "parentUri=\"{}\" />",
            "</adtcore:objectReferences>"
        ),
        package_uri,
        package_name,
        package_type,
        package_uri
    );

    let nodestructure_uri = format!(
        "/sap/bc/adt/repository/nodestructure?parent_type={}&parent_name={}",
        encode_component(package_type),
        encode_component(package_name)
    );

    let resp = bridge.call_endpoint(
        "POST",
        &nodestructure_uri,
        Some(&body),
        Some("application/vnd.sap.adt.repository.nodestructure.v2+xml, application/xml, */*"),
        Some("application/vnd.sap.adt.repository.nodestructure.v2+xml"),
    )?;

    Ok(resp.body)
}

fn search_repository_objects_xml(
    bridge: &mut AdtBridgeProcess,
    connection: &SapConnectionConfig,
    package_name: &str,
) -> Result<String> {
    let queries = build_package_search_queries(package_name);
    let mut last_xml = String::new();

    for query in queries {
        let mut search_url = reqwest::Url::parse(&format!(
            "{}/sap/bc/adt/repository/informationsystem/search",
            connection.base_url.trim_end_matches('/')
        ))
        .with_context(|| format!("Invalid SAP ADT repository search base URL: {}", connection.base_url))?;

        search_url
            .query_pairs_mut()
            .append_pair("operation", "quickSearch")
            .append_pair("query", &query)
            .append_pair("maxResults", "100");

        if let Some(client) = connection.client.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
            search_url.query_pairs_mut().append_pair("sap-client", client);
        }

        let uri = if let Some(q) = search_url.query() {
            format!("{}?{}", search_url.path(), q)
        } else {
            search_url.path().to_string()
        };

        let resp = bridge.call_endpoint(
            "GET",
            &uri,
            None,
            Some("application/xml, text/xml, */*"),
            None,
        )?;

        let xml = resp.body;
        last_xml = xml.clone();

        if xml.contains("adtcore:objectReference") || xml.contains("objectReference") {
            return Ok(xml);
        }
    }

    Ok(last_xml)
}


fn xml_attr(line: &str, attr: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let needle = format!("{}={}", attr, quote);
        let start = line.find(&needle)? + needle.len();
        let rest = &line[start..];
        let end = rest.find(quote)?;
        let value = rest[..end].trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn parse_repository_search_objects(xml: &str, package_name: &str) -> Vec<SapResolvedObject> {
    let expected = package_name.trim().to_uppercase();
    let mut out = Vec::new();

    for raw_line in xml.lines() {
        let line = raw_line.trim();
        if !(line.contains("adtcore:objectReference") || line.contains("objectReference")) {
            continue;
        }

        let uri = xml_attr(line, "adtcore:uri").or_else(|| xml_attr(line, "uri"));
        let Some(uri) = uri else { continue; };

        let source_uri = xml_attr(line, "adtcore:sourceUri").or_else(|| xml_attr(line, "sourceUri"));
        let name = xml_attr(line, "adtcore:name")
            .or_else(|| xml_attr(line, "name"))
            .unwrap_or_else(|| uri.rsplit('/').next().unwrap_or(uri.as_str()).to_string());
        let object_type = xml_attr(line, "adtcore:type")
            .or_else(|| xml_attr(line, "type"))
            .or_else(|| xml_attr(line, "objectType"))
            .unwrap_or_else(|| "OBJECT".to_string());
        let object_package = xml_attr(line, "adtcore:packageName")
            .or_else(|| xml_attr(line, "packageName"));

        let package_matches = object_package
            .as_deref()
            .map(|value| value.trim().eq_ignore_ascii_case(&expected))
            .unwrap_or(false);
        let name_matches = name.trim().eq_ignore_ascii_case(&expected);

        if package_matches || name_matches {
            out.push(SapResolvedObject {
                uri,
                source_uri,
                name,
                object_type,
                package_name: object_package,
            });
        }
    }

    out
}

pub async fn resolve_package_objects(
    _bridge: &mut AdtBridgeProcess,
    connection: &SapConnectionConfig,
    package_name: &str,
    _include_subpackages: bool,
) -> Result<Vec<SapResolvedObject>> {
    let package_name = package_name.trim();
    if package_name.is_empty() {
        bail!("Package name is required")
    }

    let xml = if let Ok((package_uri, package_type)) =
        load_package_metadata_summary_direct(connection, package_name).await
    {
        tracing::info!(
            target: "workflow_api::sap",
            package_name = %package_name,
            package_uri = %package_uri,
            package_type = %package_type,
            "resolving package objects via egui metadata plus nodestructure flow"
        );

        match load_nodestructure_xml_direct(connection, package_name, &package_uri, &package_type).await {
            Ok(xml) if xml.trim().is_empty() || looks_like_empty_package_tree(&xml) => {
                tracing::info!(
                    target: "workflow_api::sap",
                    package_name = %package_name,
                    package_uri = %package_uri,
                    package_type = %package_type,
                    "nodestructure returned empty tree"
                );
                empty_object_references_xml()
            }
            Ok(xml) if looks_like_global_package_catalog(&xml, &package_uri, &package_type) => {
                tracing::info!(
                    target: "workflow_api::sap",
                    package_name = %package_name,
                    package_uri = %package_uri,
                    package_type = %package_type,
                    "nodestructure returned global package catalog; suppressing package-object fallback"
                );
                empty_object_references_xml()
            }
            Ok(xml) => {
                tracing::info!(
                    target: "workflow_api::sap",
                    package_name = %package_name,
                    package_uri = %package_uri,
                    package_type = %package_type,
                    xml_len = xml.len(),
                    "nodestructure returned candidate package-object payload"
                );
                xml
            }
            Err(err) => {
                tracing::warn!(
                    target: "workflow_api::sap",
                    package_name = %package_name,
                    package_uri = %package_uri,
                    package_type = %package_type,
                    error = %err,
                    "nodestructure failed after metadata resolution; suppressing package-object fallback"
                );
                empty_object_references_xml()
            }
        }
    } else {
        tracing::info!(
            target: "workflow_api::sap",
            package_name = %package_name,
            "package metadata lookup failed; treating query as a non-package object search"
        );
        tracing::info!(
            target: "workflow_api::sap",
            package_name = %package_name,
            "falling back to repository search"
        );
        search_repository_objects_xml_direct(connection, package_name).await?
    };

    let parsed = parse_package_tree_xml(&xml)?;
    if parsed.is_empty() && (looks_like_non_adt_html(&xml) || !looks_like_valid_adt_package_payload(&xml)) {
        tracing::error!(
            target: "workflow_api::sap",
            package_name = %package_name,
            xml_len = xml.len(),
            looks_like_html = looks_like_non_adt_html(&xml),
            looks_like_adt_error = looks_like_adt_error_payload(&xml),
            looks_like_valid_adt = looks_like_valid_adt_package_payload(&xml),
            xml_preview = %preview_xml(&xml, 400),
            "package resolution produced a non-ADT payload after fallback chain"
        );
        bail!("package resolution returned a non-ADT payload instead of package objects");
    }

    let objects = parsed
        .into_iter()
        .map(|item| SapResolvedObject {
            uri: item.uri,
            source_uri: item.source_uri,
            name: item.name,
            object_type: item.object_type,
            package_name: item.package_name,
        })
        .collect::<Vec<_>>();

    tracing::info!(
        target: "workflow_api::sap",
        package_name = %package_name,
        count = objects.len(),
        "resolved package objects via egui-compatible package loader"
    );

    Ok(objects)
}

#[derive(Clone, Debug)]
pub struct BridgeReadResult {
    pub object_uri: String,
    pub content_type: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct BridgeLockResult {
    pub lock_handle: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct BridgeUpdateResult {
    pub status: Option<u16>,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct BridgeSimpleResult {
    pub status: Option<u16>,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

fn default_schema_version() -> u32 {
    1
}

fn parse_read_result(data: &Value) -> Result<BridgeReadResult> {
    let object_uri = data
        .get("object_uri")
        .and_then(Value::as_str)
        .or_else(|| data.get("uri").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    let content_type = data
        .get("content_type")
        .and_then(Value::as_str)
        .map(|v| v.to_string())
        .or_else(|| header_lookup(&parse_headers(data.get("headers")), "content-type"));
    let headers = parse_headers(data.get("headers"));
    let body = data.get("body").and_then(Value::as_str).unwrap_or_default().to_string();
    Ok(BridgeReadResult { object_uri, content_type, headers, body })
}

pub fn resolve_repo_path(repo_ref: &str) -> Result<PathBuf> {
    let path = PathBuf::from(repo_ref);
    if path.exists() {
        Ok(path)
    } else {
        bail!("repo_ref does not resolve to an existing path: {}", repo_ref)
    }
}

pub fn parse_connection(payload: &Value) -> Result<SapConnectionConfig> {
    let connection = payload
        .get("connection")
        .cloned()
        .or_else(|| payload.get("sap_connection").cloned())
        .unwrap_or_else(|| json!({}));

    let base_url = connection
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_default();

    let mut config: SapConnectionConfig = serde_json::from_value(json!({
        "base_url": base_url,
        "auth_type": connection.get("auth_type").cloned().unwrap_or(Value::Null),
        "transport": connection.get("transport").cloned().unwrap_or(Value::Null),
        "username": connection.get("username").cloned().unwrap_or(Value::Null),
        "password": connection.get("password").cloned().unwrap_or(Value::Null),
        "authorization": connection.get("authorization").cloned().unwrap_or(Value::Null),
        "cookie_header": connection.get("cookie_header").cloned().unwrap_or(Value::Null),
        "negotiate_command": connection.get("negotiate_command").cloned().unwrap_or(Value::Null),
        "client": connection.get("client").cloned().unwrap_or(Value::Null),
        "timeout_ms": connection.get("timeout_ms").cloned().unwrap_or(Value::Null),
        "bridge_dir": connection.get("bridge_dir").cloned().unwrap_or(Value::Null)
    }))
    .context("invalid SAP connection config payload")?;

    config.base_url = first_non_empty(&[
        env_var("ADT_HOST_URL"),
        env_var("MDEV_SAP_ADT_BASE_URL"),
        env_var("SAP_ADT_BASE_URL"),
        Some(config.base_url.clone()),
    ]).unwrap_or_default();

    config.client = first_non_empty(&[
        env_var("MDEV_SAP_CLIENT"),
        env_var("SAP_CLIENT"),
        config.client.clone(),
    ]);

    config.bridge_dir = first_non_empty(&[
        env_var("MDEV_SAP_ADT_BRIDGE_DIR"),
        env_var("SAP_ADT_BRIDGE_DIR"),
        config.bridge_dir.clone(),
        Some("adt-bridge".to_string()),
    ]);

    config.auth_type = first_non_empty(&[
        env_var("MDEV_SAP_ADT_AUTH_TYPE"),
        env_var("SAP_ADT_AUTH_TYPE"),
        config.auth_type.clone(),
        Some("cookie".to_string()),
    ]);

    config.transport = first_non_empty(&[
        env_var("MDEV_SAP_ADT_TRANSPORT"),
        env_var("SAP_ADT_TRANSPORT"),
        config.transport.clone(),
    ]);

    if config.auth_type.as_deref().map(|v| v.eq_ignore_ascii_case("cookie")).unwrap_or(true)
        && config.transport.as_deref().map(str::trim).unwrap_or("").is_empty()
    {
        config.transport = Some("fetch".to_string());
    }

    config.username = first_non_empty(&[
        env_var("MDEV_SAP_ADT_USERNAME"),
        env_var("SAP_ADT_USERNAME"),
        config.username.clone(),
    ]);

    config.password = first_non_empty(&[
        env_var("MDEV_SAP_ADT_PASSWORD"),
        env_var("SAP_ADT_PASSWORD"),
        config.password.clone(),
    ]);

    config.authorization = first_non_empty(&[
        env_var("MDEV_SAP_ADT_AUTHORIZATION"),
        env_var("SAP_ADT_AUTHORIZATION"),
        config.authorization.clone(),
    ]);

    config.cookie_header = first_non_empty(&[
        env_var("MDEV_SAP_ADT_COOKIE_HEADER"),
        env_var("SAP_ADT_COOKIE_HEADER"),
        config.cookie_header.clone(),
    ]);

    config.negotiate_command = first_non_empty(&[
        env_var("MDEV_SAP_ADT_NEGOTIATE_COMMAND"),
        env_var("SAP_ADT_NEGOTIATE_COMMAND"),
        config.negotiate_command.clone(),
    ]);

    config.timeout_ms = config.timeout_ms.or_else(|| {
        env_var("MDEV_SAP_ADT_TIMEOUT_MS")
            .or_else(|| env_var("SAP_ADT_TIMEOUT_MS"))
            .and_then(|value| value.parse::<u64>().ok())
    }).or(Some(60000));

    if config.base_url.trim().is_empty() {
        tracing::error!(
            target: "workflow_api::sap",
            "sap connection resolution failed: no ADT host configured"
        );
        bail!("SAP backend connection is not configured; set ADT_HOST_URL or MDEV_SAP_ADT_BASE_URL")
    }

    let auth_type = config.auth_type.clone().unwrap_or_default().to_ascii_lowercase();
    if auth_type == "header" && config.authorization.as_deref().unwrap_or("").trim().is_empty() {
        tracing::error!(
            target: "workflow_api::sap",
            base_url = %config.base_url,
            "sap connection resolution failed: header auth selected but no authorization header configured"
        );
        bail!("SAP backend header auth is not configured; set MDEV_SAP_ADT_AUTHORIZATION or SAP_ADT_AUTHORIZATION")
    }

    tracing::info!(
        target: "workflow_api::sap",
        base_url = %config.base_url,
        auth_type = %config.auth_type.clone().unwrap_or_default(),
        client = %config.client.clone().unwrap_or_default(),
        bridge_dir = %config.bridge_dir.clone().unwrap_or_default(),
        "resolved backend-owned SAP connection config"
    );

    Ok(config)
}

pub fn resolve_bridge_dir(config: &SapConnectionConfig) -> Result<PathBuf> {
    let raw = config.bridge_dir.as_deref().unwrap_or("adt-bridge").trim();
    let candidate = PathBuf::from(if raw.is_empty() { "adt-bridge" } else { raw });

    if candidate.is_absolute() && candidate.exists() {
        tracing::info!(target: "workflow_api::sap", path = %candidate.display(), "resolved ADT bridge directory from absolute path");
        return Ok(candidate);
    }

    let cwd = std::env::current_dir().context("failed to resolve current_dir for ADT bridge")?;

    let joined = cwd.join(&candidate);
    if joined.exists() {
        tracing::info!(target: "workflow_api::sap", path = %joined.display(), "resolved ADT bridge directory from current_dir");
        return Ok(joined);
    }

    for dir in cwd.ancestors() {
        let ancestor_joined = dir.join(&candidate);
        if ancestor_joined.exists() {
            tracing::info!(target: "workflow_api::sap", path = %ancestor_joined.display(), "resolved ADT bridge directory from current_dir ancestor");
            return Ok(ancestor_joined);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors() {
            let ancestor_joined = dir.join(&candidate);
            if ancestor_joined.exists() {
                tracing::info!(target: "workflow_api::sap", path = %ancestor_joined.display(), "resolved ADT bridge directory from current_exe ancestor");
                return Ok(ancestor_joined);
            }
        }
    }

    tracing::error!(
        target: "workflow_api::sap",
        requested = %candidate.display(),
        cwd = %cwd.display(),
        "could not resolve ADT bridge directory"
    );
    bail!("could not resolve ADT bridge directory")
}

pub fn split_multiline_items(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect()
}

pub fn normalize_repo_relative_path(path: &str) -> String {
    path.trim().replace('\\', "/").trim_end_matches('/').to_string()
}

pub fn manifest_dir_from_manifest_path(manifest_path: &str) -> String {
    manifest_path
        .strip_suffix("/manifest.adt.json")
        .unwrap_or("")
        .trim_matches('/')
        .to_string()
}

pub fn join_manifest_relative_path(manifest_dir: &str, relative_path: &str) -> String {
    let relative_path = relative_path.trim().trim_start_matches('/');
    if manifest_dir.is_empty() {
        relative_path.to_string()
    } else if relative_path.is_empty() {
        manifest_dir.to_string()
    } else {
        format!("{}/{}", manifest_dir, relative_path)
    }
}

pub fn manifest_directory_name(object_name: Option<&str>, object_type: Option<&str>, package_name: Option<&str>) -> String {
    let package = package_name.unwrap_or("UNKNOWNPKG").trim().replace('/', "_");
    let object = object_name.unwrap_or("OBJECT").trim().replace('/', "_");
    let kind = object_type.unwrap_or("TYPE").trim().replace('/', "_");
    format!("sap_adt/{}__{}__{}", package, kind, object)
}

fn manifest_body_looks_like_non_adt_html(body: &str) -> bool {
    let lower = body.trim().to_ascii_lowercase();
    lower.starts_with("<html")
        || lower.contains("<head")
        || lower.contains("<body")
        || lower.contains("login.microsoftonline.com")
        || lower.contains("samlrequest")
        || lower.contains("relaystate")
}

fn manifest_contains_usable_adt_content(manifest: &crate::engine::capabilities::sap::migration::sap_adt_manifest::SapAdtObjectManifest) -> bool {
    let metadata = manifest.metadata_xml.trim();
    if metadata.is_empty() {
        return !manifest.resources.is_empty() || !manifest.documents.is_empty();
    }

    if manifest_body_looks_like_non_adt_html(metadata) {
        return false;
    }

    metadata.contains("adtcore:")
        || metadata.contains("abapsource:")
        || metadata.contains("http://www.sap.com/adt/")
        || metadata.contains("<atom:link")
        || !manifest.resources.is_empty()
        || !manifest.documents.is_empty()
}

pub fn validate_imported_manifest(object_uri: &str, manifest: &crate::engine::capabilities::sap::migration::sap_adt_manifest::SapAdtObjectManifest) -> Result<()> {
    if manifest_body_looks_like_non_adt_html(&manifest.metadata_xml) {
        bail!(
            "SAP import for {} returned HTML/login content instead of ADT metadata",
            object_uri
        );
    }

    if !manifest_contains_usable_adt_content(manifest) {
        bail!(
            "SAP import for {} returned no usable ADT metadata/resources",
            object_uri
        );
    }

    Ok(())
}

fn fallback_name_is_placeholder(value: Option<&str>) -> bool {
    let Some(value) = value.map(str::trim) else {
        return true;
    };
    if value.is_empty() {
        return true;
    }

    value == "==============================CP"
        || value == "OBJECT"
        || value == "unnamed"
        || value.chars().all(|ch| ch == '=')
}

fn fallback_package_is_placeholder(value: Option<&str>) -> bool {
    let Some(value) = value.map(str::trim) else {
        return true;
    };
    value.is_empty() || value.eq_ignore_ascii_case("UNKNOWNPKG") || value.eq_ignore_ascii_case("package")
}

pub fn should_persist_sap_adt_resource(resource: &crate::engine::capabilities::sap::migration::sap_adt_manifest::SapAdtManifestResource, include_xml_artifacts: bool) -> bool {
    if include_xml_artifacts {
        return true;
    }

    let content_type = resource.content_type.clone().unwrap_or_default().to_ascii_lowercase();
    let path = resource.path.to_ascii_lowercase();
    !content_type.contains("xml") && !path.ends_with(".xml")
}

pub fn should_persist_sap_adt_document(document: &crate::engine::capabilities::sap::migration::sap_adt_manifest::SapAdtManifestDocument, include_xml_artifacts: bool) -> bool {
    if include_xml_artifacts {
        return true;
    }

    let content_type = document.content_type.clone().unwrap_or_default().to_ascii_lowercase();
    let path = document.path.to_ascii_lowercase();
    !content_type.contains("xml") && !path.ends_with(".xml")
}

pub fn import_object_to_worktree(
    _bridge: &mut AdtBridgeProcess,
    _repo: &Path,
    object_uri: &str,
    _object_name: Option<&str>,
    _object_type: Option<&str>,
    _package_name: Option<&str>,
    _clone_target_path: Option<&str>,
    _include_xml_artifacts: bool,
) -> Result<SapImportObjectResult> {
    bail!("manifest crawl import has been removed from common.rs for {}", object_uri)
}

pub fn read_manifest(repo: &Path, manifest_path: &str) -> Result<crate::engine::capabilities::sap::migration::sap_adt_manifest::SapAdtObjectManifest> {
    let full_path = repo.join(manifest_path);
    let bytes = fs::read(&full_path).with_context(|| format!("failed to read {}", manifest_path))?;
    serde_json::from_slice(&bytes).with_context(|| format!("invalid manifest JSON in {}", manifest_path))
}

pub fn write_manifest_tree(repo: &Path, manifest_dir: &str, manifest: &crate::engine::capabilities::sap::migration::sap_adt_manifest::SapAdtObjectManifest) -> Result<()> {
    fs::create_dir_all(repo.join("sap_adt")).context("failed to ensure sap_adt directory")?;

    for doc in &manifest.documents {
        let rel = doc.path.trim().trim_start_matches('/');
        if rel.is_empty() {
            continue;
        }
        let path = repo.join(manifest_dir).join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("failed to create parent dir for {}", path.display()))?;
        }
        fs::write(&path, doc.body.as_bytes()).with_context(|| format!("failed to write {}", path.display()))?;
    }

    for resource in &manifest.resources {
        let rel = resource.path.trim().trim_start_matches('/');
        if rel.is_empty() {
            continue;
        }
        let path = repo.join(manifest_dir).join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("failed to create parent dir for {}", path.display()))?;
        }
        fs::write(&path, resource.body.as_bytes()).with_context(|| format!("failed to write {}", path.display()))?;
    }

    let manifest_bytes = serde_json::to_vec_pretty(manifest).context("failed to serialize manifest JSON")?;
    let manifest_path = repo.join(manifest_dir).join("manifest.adt.json");
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create parent dir for {}", manifest_path.display()))?;
    }
    fs::write(&manifest_path, manifest_bytes).with_context(|| format!("failed to write {}", manifest_path.display()))?;
    Ok(())
}

fn xml_tag_text(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = block.find(&open)? + open.len();
    let rest = &block[start..];
    let end = rest.find(&close)?;
    let value = xml_unescape(rest[..end].trim());
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn looks_like_generated_oo_name(name: &str) -> bool {
    let trimmed = name.trim();
    trimmed.starts_with("LCL_") || trimmed.starts_with("LTCL_") || trimmed.starts_with("%")
}

fn choose_asxml_node_name(
    object_type: &str,
    uri: &str,
    object_name: &str,
    tech_name: &str,
    description: &str,
) -> String {
    let object_name = object_name.trim();
    let tech_name = tech_name.trim();
    let description = description.trim();

    if object_type.starts_with("DEVC/") {
        if !description.is_empty() {
            return description.to_string();
        }
        if !object_name.is_empty() {
            return object_name.to_string();
        }
    }

    if !description.is_empty() && !looks_like_generated_oo_name(description) {
        return description.to_string();
    }
    if !tech_name.is_empty() {
        return tech_name.to_string();
    }
    if !object_name.is_empty() {
        return object_name.to_string();
    }

    uri.rsplit('/').next().unwrap_or(uri).to_string()
}

fn parse_asxml_repository_nodes(xml: &str) -> Vec<SapPackageObjectSummary> {
    let mut out = Vec::new();
    let open = "<SEU_ADT_REPOSITORY_OBJ_NODE>";
    let close = "</SEU_ADT_REPOSITORY_OBJ_NODE>";
    let mut cursor = 0usize;

    while let Some(start_rel) = xml[cursor..].find(open) {
        let start = cursor + start_rel + open.len();
        let rest = &xml[start..];
        let Some(end_rel) = rest.find(close) else {
            break;
        };
        let block = &rest[..end_rel];

        let object_type = xml_tag_text(block, "OBJECT_TYPE")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "OBJECT".to_string());

        let uri = xml_tag_text(block, "OBJECT_URI")
            .or_else(|| xml_tag_text(block, "OBJECT_VIT_URI"))
            .unwrap_or_default();

        if !uri.is_empty() {
            let object_name = xml_tag_text(block, "OBJECT_NAME").unwrap_or_default();
            let tech_name = xml_tag_text(block, "TECH_NAME").unwrap_or_default();
            let raw_description = xml_tag_text(block, "DESCRIPTION").unwrap_or_default();

            let name = choose_asxml_node_name(
                &object_type,
                &uri,
                &object_name,
                &tech_name,
                &raw_description
            );

            let is_structural_node = object_type.starts_with("DEVC/")
                && !uri.starts_with("/sap/bc/adt/programs/")
                && !uri.starts_with("/sap/bc/adt/oo/")
                && !uri.starts_with("/sap/bc/adt/ddic/")
                && !uri.starts_with("/sap/bc/adt/ddls/")
                && !uri.starts_with("/sap/bc/adt/cds/")
                && !uri.starts_with("/sap/bc/adt/classes/")
                && !uri.starts_with("/sap/bc/adt/interfaces/")
                && !uri.starts_with("/sap/bc/adt/functions/");

            out.push(SapPackageObjectSummary {
                uri: uri.clone(),
                source_uri: if is_structural_node { None } else { Some(uri) },
                name,
                object_type,
                package_name: None,
            });
        }

        cursor = start + end_rel + close.len();
    }

    out
}

fn xml_attr_any(attrs: &str, names: &[&str]) -> Option<String> {
    for name in names {
        for quote in ['"', '\''] {
            let needle = format!("{}={}", name, quote);
            if let Some(start) = attrs.find(&needle) {
                let rest = &attrs[start + needle.len()..];
                if let Some(end) = rest.find(quote) {
                    let value = xml_unescape(rest[..end].trim());
                    if !value.is_empty() {
                        return Some(value);
                    }
                }
            }
        }
    }
    None
}

fn package_object_name_is_placeholder(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty()
        || trimmed == "==============================CP"
        || trimmed == "OBJECT"
        || trimmed == "unnamed"
        || trimmed.chars().all(|ch| ch == '=')
}

fn canonical_name_from_object_uri(uri: &str) -> Option<String> {
    let trimmed = uri.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }

    let last = trimmed.rsplit('/').next()?.trim();
    if last.is_empty() {
        return None;
    }

    Some(last.to_ascii_uppercase())
}

fn normalize_package_object_summary(mut item: SapPackageObjectSummary) -> SapPackageObjectSummary {
    if package_object_name_is_placeholder(&item.name) {
        if let Some(name) = canonical_name_from_object_uri(&item.uri) {
            item.name = name;
        }
    }

    if item.package_name.as_deref().map(str::trim).unwrap_or("").is_empty() {
        item.package_name = None;
    }

    item
}

pub fn parse_package_tree_xml(xml: &str) -> Result<Vec<SapPackageObjectSummary>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    let asxml_items = parse_asxml_repository_nodes(xml);
    tracing::info!(
        target: "workflow_api::sap",
        xml_len = xml.len(),
        asxml_count = asxml_items.len(),
        xml_preview = %preview_xml(xml, 400),
        "package tree parser starting"
    );

    for item in asxml_items {
        if item.uri.trim().is_empty() || !seen.insert(item.uri.clone()) {
            continue;
        }
        out.push(item);
    }

    if out.is_empty() {
        let item_re = regex::Regex::new(r#"<(?:(?:[^\s>]+):)?(?:objectReference|treeNode|node|objectNode|packageNode|package)\b([^>]*)/?>"#)?;

        for caps in item_re.captures_iter(xml) {
            let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");

            let uri = xml_attr_any(attrs, &["adtcore:uri", "uri", "objectUri", "refUri", "resourceUri", "href"]);
            let Some(uri) = uri else {
                continue;
            };
            if uri.trim().is_empty() || !seen.insert(uri.clone()) {
                continue;
            }

            let source_uri = xml_attr_any(
                attrs,
                &["adtcore:sourceUri", "sourceUri", "adtcore:sourceResourceUri", "sourceResourceUri", "adtcore:contentUri", "contentUri", "sourceuri", "source-uri"]
            );
            let name = xml_attr_any(attrs, &["adtcore:name", "name", "techName", "displayName", "shortDescription", "label", "title"])
                .unwrap_or_else(|| uri.rsplit('/').next().unwrap_or(uri.as_str()).to_string());
            let object_type = xml_attr_any(attrs, &["adtcore:type", "type", "objectType", "nodeType", "adtcore:category", "objtype"])
                .unwrap_or_else(|| "OBJECT".to_string());
            let package_name = xml_attr_any(attrs, &["adtcore:packageName", "packageName", "devclass"]);

            out.push(SapPackageObjectSummary {
                uri,
                source_uri,
                name,
                object_type,
                package_name,
            });
        }
    }

    out = out.into_iter().map(normalize_package_object_summary).collect();

    out.sort_by(|a, b| {
        a.object_type
            .to_lowercase()
            .cmp(&b.object_type.to_lowercase())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.uri.cmp(&b.uri))
    });
    out.dedup_by(|a, b| a.uri == b.uri);

    tracing::info!(
        target: "workflow_api::sap",
        xml_len = xml.len(),
        parsed_count = out.len(),
        xml_preview = %preview_xml(xml, 400),
        "package tree parser completed"
    );

    Ok(out)
}

fn preview_xml(xml: &str, max_len: usize) -> String {
    let compact = xml.split_whitespace().collect::<Vec<_>>().join(" ");
    let char_count = compact.chars().count();
    if char_count <= max_len {
        compact
    } else {
        compact.chars().take(max_len).collect::<String>() + "…"
    }
}

fn extract_xml_attr(line: &str, names: &[&str]) -> Option<String> {
    for name in names {
        let pat = format!("{}=\"", name);
        if let Some(start) = line.find(&pat) {
            let rest = &line[start + pat.len()..];
            if let Some(end) = rest.find('"') {
                return Some(xml_unescape(&rest[..end]));
            }
        }
    }
    None
}

fn xml_unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

fn escape_xml_attr(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn parse_headers(value: Option<&Value>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(value) = value else { return out; };

    if let Some(arr) = value.as_array() {
        for item in arr {
            if let Some(pair) = item.as_array() {
                if pair.len() == 2 {
                    if let (Some(k), Some(v)) = (pair[0].as_str(), pair[1].as_str()) {
                        out.push((k.to_string(), v.to_string()));
                    }
                }
            } else if let Some(obj) = item.as_object() {
                if let (Some(k), Some(v)) = (obj.get("0").and_then(Value::as_str), obj.get("1").and_then(Value::as_str)) {
                    out.push((k.to_string(), v.to_string()));
                }
            }
        }
        return out;
    }

    if let Some(obj) = value.as_object() {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                out.push((k.clone(), s.to_string()));
            }
        }
    }

    out
}

pub fn header_lookup(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

fn extract_lock_handle(body: &str) -> Option<String> {
    ["lockHandle", "LOCK_HANDLE", "lock-handle"]
        .iter()
        .find_map(|name| {
            let pat = format!("{}=\"", name);
            body.find(&pat).and_then(|start| {
                let rest = &body[start + pat.len()..];
                rest.find('"').map(|end| rest[..end].to_string())
            })
        })
}

#[derive(Clone, Debug)]
struct ManifestLink {
    uri: String,
    rel: Option<String>,
    title: Option<String>,
}

fn collect_links(xml: &str) -> Vec<ManifestLink> {
    let mut out = Vec::new();
    for raw in xml.lines() {
        let line = raw.trim();
        if !line.contains("href=") && !line.contains("uri=") {
            continue;
        }
        if !(line.contains("link") || line.contains("objectReference") || line.contains("collection")) {
            continue;
        }
        let uri = extract_xml_attr(line, &["href", "uri", "adtcore:uri"]).unwrap_or_default();
        if uri.is_empty() {
            continue;
        }
        out.push(ManifestLink {
            uri,
            rel: extract_xml_attr(line, &["rel"]),
            title: extract_xml_attr(line, &["title", "name"]),
        });
    }
    out
}

fn resolve_relative_uri(base: &str, candidate: &str) -> String {
    if candidate.starts_with("http://") || candidate.starts_with("https://") {
        return candidate.to_string();
    }
    if candidate.starts_with('/') {
        if let Some((scheme, rest)) = base.split_once("://") {
            if let Some((host, _)) = rest.split_once('/') {
                return format!("{}://{}{}", scheme, host, candidate);
            }
            return format!("{}://{}{}", scheme, rest, candidate);
        }
    }
    if let Some((head, _)) = base.rsplit_once('/') {
        return format!("{}/{}", head, candidate.trim_start_matches('/'));
    }
    candidate.to_string()
}

fn sanitize_rel_path(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .collect::<Vec<_>>()
        .join("/")
}

fn source_resource_path(object_name: Option<&str>, object_type: Option<&str>, content_type: Option<&str>) -> String {
    let extension = if let Some(content_type) = content_type {
        let lower = content_type.to_ascii_lowercase();
        if lower.contains("xml") {
            "xml"
        } else if lower.contains("json") {
            "json"
        } else if lower.contains("html") {
            "html"
        } else {
            "abap"
        }
    } else {
        "abap"
    };

    let base = object_name.unwrap_or("source").trim().replace('/', "_");
    let kind = object_type.unwrap_or("object").trim().replace('/', "_");
    format!("src/{}_{}.{}", kind, base, extension)
}

pub fn export_candidates_from_manifest(manifest: &SapAdtObjectManifest) -> Vec<&SapAdtManifestResource> {
    manifest
        .resources
        .iter()
        .filter(|resource| {
            resource.editable
                && resource.activatable
                && resource.rel == "http://www.sap.com/adt/relations/source"
                && resource
                    .content_type
                    .as_deref()
                    .map(|s| s.to_ascii_lowercase().starts_with("text/plain"))
                    .unwrap_or(false)
        })
        .collect()
}

pub fn build_export_workflow(
    pushed_resource_uris: &[String],
    syntax_ok: bool,
    syntax_details: &str,
    auto_activate: bool,
    activation_ok: bool,
    activation_details: &str,
    message: &str,
) -> Value {
    let mut steps = Vec::new();

    steps.push(json!({
        "key": "push",
        "label": "Push",
        "outcome": if pushed_resource_uris.is_empty() { "skipped" } else { "success" },
        "summary": if pushed_resource_uris.is_empty() { "No resources pushed".to_string() } else { format!("Pushed {} SAP ADT resource(s)", pushed_resource_uris.len()) },
        "details": pushed_resource_uris
    }));

    steps.push(json!({
        "key": "syntax_check",
        "label": "Syntax check",
        "outcome": if syntax_ok { "success" } else { "error" },
        "summary": if syntax_ok { "Syntax check passed" } else { "Syntax check failed" },
        "details": syntax_details
    }));

    steps.push(json!({
        "key": "activate",
        "label": "Activate",
        "outcome": if !auto_activate { "skipped" } else if activation_ok { "success" } else { "error" },
        "summary": if !auto_activate {
            "Auto-activate disabled".to_string()
        } else if activation_ok {
            "Activated".to_string()
        } else {
            "Activation failed".to_string()
        },
        "details": if activation_details.trim().is_empty() { message.to_string() } else { activation_details.to_string() }
    }));

    json!({ "steps": steps })
}

pub fn activation_details_have_errors(details: &str) -> bool {
    details.lines().any(|line| {
        let lower = line.trim().to_ascii_lowercase();
        lower.contains("was not activated")
            || lower.contains("contains errors")
            || lower.contains("contains error")
            || lower.contains("syntax error")
            || lower.contains(" is unknown")
            || lower.contains(" unknown")
            || lower.contains("exception")
            || lower.contains("not caught")
    })
}

pub fn summarize_activation_details(details: &str) -> String {
    let mut important = Vec::new();

    for line in details.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("activation of worklist")
            || lower.starts_with("technical log for mass activation")
            || lower.starts_with("see log ")
            || lower.starts_with("checks ddl source ")
            || lower.starts_with("end of activation of worklist")
        {
            continue;
        }
        if lower.contains("line ")
            || lower.contains("column ")
            || lower.contains("was not activated")
            || lower.contains("contains errors")
            || lower.contains("syntax error")
            || lower.contains("contains error")
            || lower.contains(" is unknown")
            || lower.contains(" unknown")
        {
            important.push(line.to_string());
        }
    }

    important.sort();
    important.dedup();
    important.join("\n")
}
