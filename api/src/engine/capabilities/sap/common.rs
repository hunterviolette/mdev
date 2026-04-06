use std::{
    collections::HashSet,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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

#[derive(Debug, Deserialize)]
pub struct SapConnectionConfig {
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
    let mut config: SapConnectionConfig = serde_json::from_value(connection).context("invalid SAP connection config")?;

    if config.base_url.trim().is_empty() {
        config.base_url = std::env::var("MDEV_SAP_ADT_BASE_URL")
            .ok()
            .or_else(|| std::env::var("SAP_ADT_BASE_URL").ok())
            .unwrap_or_default();
    }

    if config.base_url.trim().is_empty() {
        bail!("SAP connection.base_url is required")
    }
    Ok(config)
}

pub fn resolve_bridge_dir(config: &SapConnectionConfig) -> Result<PathBuf> {
    if let Some(dir) = config.bridge_dir.as_deref() {
        let path = PathBuf::from(dir);
        if path.exists() {
            return Ok(path);
        }
    }
    let cwd = std::env::current_dir().context("failed to resolve current_dir for ADT bridge")?;
    let candidate = cwd.join("adt-bridge");
    if candidate.exists() {
        return Ok(candidate);
    }
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
    format!("sap_adt/{}/{}/{}", package, kind, object)
}

pub fn should_persist_sap_adt_resource(resource: &SapAdtManifestResource, include_xml_artifacts: bool) -> bool {
    if include_xml_artifacts {
        return true;
    }

    let content_type = resource.content_type.clone().unwrap_or_default().to_ascii_lowercase();
    let path = resource.path.to_ascii_lowercase();
    !content_type.contains("xml") && !path.ends_with(".xml")
}

pub fn read_manifest(repo: &Path, manifest_path: &str) -> Result<SapAdtObjectManifest> {
    let full_path = repo.join(manifest_path);
    let bytes = fs::read(&full_path).with_context(|| format!("failed to read {}", manifest_path))?;
    serde_json::from_slice(&bytes).with_context(|| format!("invalid manifest JSON in {}", manifest_path))
}

pub fn write_manifest_tree(repo: &Path, manifest_dir: &str, manifest: &SapAdtObjectManifest) -> Result<()> {
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

pub fn parse_package_tree_xml(xml: &str) -> Result<Vec<SapPackageObjectSummary>> {
    let mut out = Vec::new();
    let mut current_package: Option<String> = None;

    for raw in xml.lines() {
        let line = raw.trim();
        if line.contains("packageNode") || line.contains("packageTree:package") || line.contains("adtcore:objectReference") || line.contains("objectReference") {
            let uri = extract_xml_attr(line, &["uri", "adtcore:uri"]).unwrap_or_default();
            let name = extract_xml_attr(line, &["name", "adtcore:name", "label"]).unwrap_or_default();
            let object_type = extract_xml_attr(line, &["type", "adtcore:type", "objtype", "objectType"]).unwrap_or_default();
            let package_name = extract_xml_attr(line, &["packageName", "adtcore:packageName", "devclass"])
                .or_else(|| current_package.clone());
            let source_uri = extract_xml_attr(line, &["sourceUri", "sourceuri", "source-uri"]);

            if !uri.is_empty() {
                if object_type == "DEVC/K" || uri.contains("/packages/") {
                    current_package = package_name.clone().or_else(|| Some(name.clone()));
                }
                out.push(SapPackageObjectSummary {
                    uri,
                    name,
                    object_type,
                    package_name,
                    source_uri,
                });
            }
        }
    }

    out.sort_by(|a, b| a.uri.cmp(&b.uri));
    out.dedup_by(|a, b| a.uri == b.uri);
    Ok(out)
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

pub fn crawl_object_manifest(
    bridge: &mut AdtBridgeProcess,
    object_uri: &str,
    object_name: Option<&str>,
    object_type: Option<&str>,
    package_name: Option<&str>,
) -> Result<SapAdtObjectManifest> {
    let metadata_result = bridge.read_object(object_uri, Some("application/xml, text/xml, */*"))?;
    let metadata_uri = if metadata_result.object_uri.trim().is_empty() {
        object_uri.to_string()
    } else {
        metadata_result.object_uri.clone()
    };
    let metadata_xml = metadata_result.body.clone();

    let mut resources: Vec<SapAdtManifestResource> = Vec::new();
    let mut documents: Vec<SapAdtManifestDocument> = Vec::new();

    let links = collect_links(&metadata_xml);
    let mut seen_resource_uris = HashSet::new();

    for (index, link) in links.iter().enumerate() {
        let resolved_uri = resolve_relative_uri(&metadata_uri, &link.uri);
        let title = link.title.clone().or_else(|| Some(format!("resource_{}", index + 1)));
        let rel = link.rel.clone().unwrap_or_default();
        let role = link.title.clone().unwrap_or_else(|| rel.clone());

        let looks_like_source = rel.contains("relations/source") || role.eq_ignore_ascii_case("source");
        let looks_like_doc = rel.contains("documentation") || rel.contains("relations/doc") || role.contains("doc");

        let read = match bridge.read_object(&resolved_uri, Some("text/plain, application/xml, text/xml, */*")) {
            Ok(value) => value,
            Err(_) => continue,
        };

        if looks_like_doc {
            let path = link
                .title
                .clone()
                .map(|t| sanitize_rel_path(&format!("docs/{}.txt", t)))
                .unwrap_or_else(|| format!("docs/document_{}.txt", index + 1));
            documents.push(SapAdtManifestDocument {
                id: format!("document_{}", index + 1),
                uri: resolved_uri,
                title,
                content_type: read.content_type.clone(),
                headers: read.headers.clone(),
                path,
                body: read.body,
            });
            continue;
        }

        if !seen_resource_uris.insert(resolved_uri.clone()) {
            continue;
        }

        let path = if looks_like_source {
            source_resource_path(object_name, object_type, read.content_type.as_deref())
        } else {
            sanitize_rel_path(&format!("resources/{}_{}", index + 1, title.clone().unwrap_or_else(|| "resource".to_string())))
        };

        resources.push(SapAdtManifestResource {
            id: format!("resource_{}", resources.len() + 1),
            uri: resolved_uri,
            rel: rel.clone(),
            title,
            content_type: read.content_type.clone(),
            etag: header_lookup(&read.headers, "etag"),
            lock_handle: None,
            headers: read.headers.clone(),
            path,
            readable: true,
            editable: looks_like_source,
            activatable: looks_like_source,
            role,
            body: read.body,
        });
    }

    if resources.is_empty() {
        let source_read = bridge.read_object(object_uri, Some("text/plain, application/xml, text/xml, */*"))?;
        resources.push(SapAdtManifestResource {
            id: "resource_1".to_string(),
            uri: object_uri.to_string(),
            rel: "http://www.sap.com/adt/relations/source".to_string(),
            title: Some("source".to_string()),
            content_type: source_read.content_type.clone(),
            etag: header_lookup(&source_read.headers, "etag"),
            lock_handle: None,
            headers: source_read.headers.clone(),
            path: source_resource_path(object_name, object_type, source_read.content_type.as_deref()),
            readable: true,
            editable: true,
            activatable: true,
            role: "source".to_string(),
            body: source_read.body,
        });
    }

    let root_object_name = object_name.map(|v| v.to_string()).or_else(|| extract_xml_attr(&metadata_xml, &["adtcore:name", "name", "objName", "objectName"]));
    let root_object_type = object_type.map(|v| v.to_string()).or_else(|| extract_xml_attr(&metadata_xml, &["adtcore:type", "type", "objType", "objectType"]));
    let root_package_name = package_name.map(|v| v.to_string()).or_else(|| extract_xml_attr(&metadata_xml, &["adtcore:packageName", "packageName", "devclass"]));

    Ok(SapAdtObjectManifest {
        schema_version: 1,
        metadata_uri,
        object_uri: resources.first().map(|resource| resource.uri.clone()),
        object_name: root_object_name,
        object_type: root_object_type,
        package_name: root_package_name,
        etag: header_lookup(&metadata_result.headers, "etag").or_else(|| resources.iter().find_map(|r| r.etag.clone())),
        metadata_xml,
        resources,
        documents,
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
