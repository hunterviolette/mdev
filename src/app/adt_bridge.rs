use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct AdtConnectConfig {
    pub bridge_dir: String,
    pub base_url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub authorization: Option<String>,
    pub client: Option<String>,
    pub timeout_ms: u64,
    pub session_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AdtObjectUpdateRequest {
    pub bridge_dir: String,
    pub session_id: String,
    pub object_uri: String,
    pub source: String,
    pub content_type: Option<String>,
    pub lock_handle: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AdtProblem {
    pub severity: String,
    pub message: String,
    pub line: Option<u64>,
    pub column: Option<u64>,
    pub object_uri: Option<String>,
    pub code: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AdtOperationResult {
    pub status: Option<u16>,
    pub body: Option<String>,
    pub xml: Option<String>,
    pub activated: Option<bool>,
    pub problems: Vec<AdtProblem>,
    pub raw: Value,
}

struct AdtBridgeClient {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    next_id: u64,
}

impl AdtBridgeClient {
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
            if status.is_none() {
                return Ok(());
            }
        }

        self.child = None;
        self.stdin = None;
        self.stdout = None;

        let npm = if cfg!(target_os = "windows") { "npm.cmd" } else { "npm" };
        let mut child = Command::new(npm)
            .arg("start")
            .current_dir(bridge_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("Failed to start ADT bridge from {}", bridge_dir))?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("ADT bridge stdin unavailable"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("ADT bridge stdout unavailable"))?;

        self.stdin = Some(stdin);
        self.stdout = Some(BufReader::new(stdout));
        self.child = Some(child);

        std::thread::sleep(Duration::from_millis(1200));
        Ok(())
    }

    fn send_json(&mut self, mut payload: Value) -> Result<Value> {
        let id = self.command_id();
        payload["id"] = Value::String(id.clone());

        let stdin = self.stdin.as_mut().ok_or_else(|| anyhow!("ADT bridge stdin not connected"))?;
        writeln!(stdin, "{}", payload).context("Failed writing ADT bridge command")?;
        stdin.flush().context("Failed flushing ADT bridge stdin")?;

        let stdout = self.stdout.as_mut().ok_or_else(|| anyhow!("ADT bridge stdout not connected"))?;
        let mut line = String::new();
        loop {
            line.clear();
            let n = stdout.read_line(&mut line).context("Failed reading ADT bridge response")?;
            if n == 0 {
                return Err(anyhow!("ADT bridge exited before sending a response"));
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
            let ok = parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
            if ok {
                return Ok(parsed);
            }
            let err = parsed
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown ADT bridge error");
            return Err(anyhow!(err.to_string()));
        }
    }
}

fn client() -> &'static Mutex<AdtBridgeClient> {
    static CELL: OnceLock<Mutex<AdtBridgeClient>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(AdtBridgeClient::new()))
}

pub fn resolve_adt_bridge_dir() -> String {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join("adt-bridge");
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("adt-bridge");
        if candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }

    "adt-bridge".to_string()
}

pub fn connect(cfg: &mut AdtConnectConfig) -> Result<String> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&cfg.bridge_dir)?;

    let resp = client.send_json(json!({
        "cmd": "connect",
        "session_id": cfg.session_id.clone(),
        "base_url": cfg.base_url,
        "username": cfg.username,
        "password": cfg.password,
        "authorization": cfg.authorization,
        "client": cfg.client,
        "timeout_ms": cfg.timeout_ms.max(1000)
    }))?;

    let session_id = resp
        .get("session_id")
        .and_then(|v| v.as_str())
        .or_else(|| resp.get("data").and_then(|v| v.get("session_id")).and_then(|v| v.as_str()))
        .ok_or_else(|| anyhow!("ADT connect response missing session_id"))?
        .to_string();

    cfg.session_id = Some(session_id.clone());
    Ok(session_id)
}

pub fn list_package_objects(bridge_dir: &str, session_id: &str, package_name: &str, include_subpackages: bool) -> Result<Value> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(bridge_dir)?;
    client.send_json(json!({
        "cmd": "list_package_objects",
        "session_id": session_id,
        "package_name": package_name,
        "include_subpackages": include_subpackages
    }))
}

pub fn read_object(bridge_dir: &str, session_id: &str, object_uri: &str, accept: Option<&str>) -> Result<Value> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(bridge_dir)?;
    client.send_json(json!({
        "cmd": "read_object",
        "session_id": session_id,
        "object_uri": object_uri,
        "accept": accept
    }))
}

pub fn update_object(req: &AdtObjectUpdateRequest) -> Result<Value> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&req.bridge_dir)?;
    client.send_json(json!({
        "cmd": "update_object",
        "session_id": req.session_id,
        "object_uri": req.object_uri,
        "source": req.source,
        "content_type": req.content_type,
        "lock_handle": req.lock_handle
    }))
}

pub fn syntax_check(bridge_dir: &str, session_id: &str, object_uri: &str) -> Result<AdtOperationResult> {
    operation(bridge_dir, json!({
        "cmd": "syntax_check",
        "session_id": session_id,
        "object_uri": object_uri
    }))
}

pub fn activate_object(bridge_dir: &str, session_id: &str, object_uri: &str) -> Result<AdtOperationResult> {
    operation(bridge_dir, json!({
        "cmd": "activate_object",
        "session_id": session_id,
        "object_uri": object_uri
    }))
}

pub fn activate_package(bridge_dir: &str, session_id: &str, package_name: &str) -> Result<AdtOperationResult> {
    operation(bridge_dir, json!({
        "cmd": "activate_package",
        "session_id": session_id,
        "package_name": package_name
    }))
}

pub fn get_problems(bridge_dir: &str, session_id: &str, result_uri: Option<&str>, xml: Option<&str>) -> Result<AdtOperationResult> {
    operation(bridge_dir, json!({
        "cmd": "get_problems",
        "session_id": session_id,
        "result_uri": result_uri,
        "xml": xml
    }))
}

pub fn close_session(bridge_dir: &str, session_id: &str) -> Result<Value> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(bridge_dir)?;
    client.send_json(json!({
        "cmd": "close_session",
        "session_id": session_id
    }))
}

fn operation(bridge_dir: &str, payload: Value) -> Result<AdtOperationResult> {
    let mutex = client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(bridge_dir)?;
    let raw = client.send_json(payload)?;
    let data = raw.get("data").cloned().unwrap_or(Value::Null);

    let problems = data
        .get("problems")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|p| AdtProblem {
                    severity: p.get("severity").and_then(|v| v.as_str()).unwrap_or("error").to_string(),
                    message: p.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    line: p.get("line").and_then(|v| v.as_u64()),
                    column: p.get("column").and_then(|v| v.as_u64()),
                    object_uri: p.get("object_uri").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    code: p.get("code").and_then(|v| v.as_str()).map(|s| s.to_string()),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(AdtOperationResult {
        status: data.get("status").and_then(|v| v.as_u64()).map(|v| v as u16),
        body: data.get("body").and_then(|v| v.as_str()).map(|s| s.to_string()),
        xml: data.get("xml").and_then(|v| v.as_str()).map(|s| s.to_string()),
        activated: data.get("activated").and_then(|v| v.as_bool()),
        problems,
        raw,
    })
}
