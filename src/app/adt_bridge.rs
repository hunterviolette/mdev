use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, COOKIE, USER_AGENT};
use reqwest::{StatusCode, Url};
use serde_json::{json, Value};

use crate::app::browser_bridge::{self, BrowserTurnConfig};
use crate::app::state::{
    SapAdtDiscoveryCollection,
    SapAdtDiscoveryState,
    SapAdtObjectSummary,
    SapAdtState,
    SapAdtTemplateLink,
};

#[derive(Clone, Debug)]
pub struct AdtReadObjectResult {
    pub object_uri: String,
    pub content_type: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct AdtLockObjectResult {
    pub object_uri: String,
    pub lock_handle: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct AdtUpdateObjectResult {
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct AdtCheckResult {
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct AdtActivateResult {
    pub body: String,
}

pub fn resolve_relative_object_uri(base_object_uri: &str, child_uri: &str) -> Result<String> {
    let base_object_uri = base_object_uri.trim();
    let child_uri = child_uri.trim();

    if base_object_uri.is_empty() {
        return Err(anyhow!("Base object URI is required"));
    }
    if child_uri.is_empty() {
        return Err(anyhow!("Child object URI is required"));
    }
    if child_uri.starts_with('/') {
        return Ok(child_uri.to_string());
    }
    if child_uri.starts_with("http://") || child_uri.starts_with("https://") {
        let url = Url::parse(child_uri)?;
        let mut uri = url.path().to_string();
        if let Some(query) = url.query() {
            uri.push('?');
            uri.push_str(query);
        }
        return Ok(uri);
    }

    let normalized_base_path = if base_object_uri.ends_with('/') {
        base_object_uri.to_string()
    } else {
        format!("{}/", base_object_uri)
    };

    let base = if normalized_base_path.starts_with('/') {
        format!("https://dummy{}", normalized_base_path)
    } else {
        format!("https://dummy/{}", normalized_base_path)
    };
    let joined = Url::parse(&base)?.join(child_uri)?;
    let mut uri = joined.path().to_string();
    if let Some(query) = joined.query() {
        uri.push('?');
        uri.push_str(query);
    }
    Ok(uri)
}

pub fn extract_object_source_uri(metadata_xml: &str) -> Result<Option<String>> {
    if let Some(message) = extract_adt_exception_message(metadata_xml) {
        return Err(anyhow!("ADT object metadata returned exception XML: {}", message));
    }

    let link_re = Regex::new(r#"<(?:(?:[^\s>]+):)?link\b([^>]*)/?>"#)?;
    for caps in link_re.captures_iter(metadata_xml) {
        let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let rel = xml_attr(attrs, "rel");
        let href = xml_attr(attrs, "href");
        let content_type = xml_attr(attrs, "type");

        if rel.as_deref() == Some("http://www.sap.com/adt/relations/source") {
            if let Some(href) = href {
                if !href.trim().is_empty() {
                    if content_type.as_deref() == Some("text/plain") {
                        return Ok(Some(href));
                    }
                    if content_type.is_none() {
                        return Ok(Some(href));
                    }
                }
            }
        }
    }

    let root_re = Regex::new(r#"<(?:(?:[^\s>]+):)?[^\s>/]+\b([^>]*)>"#)?;
    if let Some(caps) = root_re.captures(metadata_xml) {
        let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        if let Some(source_uri) = xml_attr(attrs, "abapsource:sourceUri")
            .or_else(|| xml_attr(attrs, "sourceUri"))
            .or_else(|| xml_attr(attrs, "adtcore:sourceUri"))
            .or_else(|| xml_attr(attrs, "contentUri"))
            .or_else(|| xml_attr(attrs, "adtcore:contentUri"))
        {
            if !source_uri.trim().is_empty() {
                return Ok(Some(source_uri));
            }
        }
    }

    Ok(None)
}

struct AdtBridgeClient {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    next_id: u64,
    bridge_dir: Option<String>,
    base_url: Option<String>,
}

impl AdtBridgeClient {
    fn new() -> Self {
        Self {
            child: None,
            stdin: None,
            stdout: None,
            next_id: 1,
            bridge_dir: None,
            base_url: None,
        }
    }

    fn ensure_started(&mut self, bridge_dir: &str, base_url: &str) -> Result<()> {
        if self.child.is_some()
            && self.bridge_dir.as_deref() == Some(bridge_dir)
            && self.base_url.as_deref() == Some(base_url)
        {
            return Ok(());
        }

        self.child = None;
        self.stdin = None;
        self.stdout = None;

        let npm = if cfg!(target_os = "windows") { "npm.cmd" } else { "npm" };
        let mut child = Command::new(npm)
            .arg("start")
            .current_dir(bridge_dir)
            .env("ADT_HOST_URL", base_url)
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
        self.bridge_dir = Some(bridge_dir.to_string());
        self.base_url = Some(base_url.to_string());

        std::thread::sleep(Duration::from_millis(1200));
        Ok(())
    }

    fn command_id(&mut self) -> String {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        format!("adt-{}", id)
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
            if parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Ok(parsed);
            }
            let msg = parsed
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("ADT bridge command failed");
            return Err(anyhow!(msg.to_string()));
        }
    }
}

fn bridge_client() -> &'static Mutex<AdtBridgeClient> {
    static CLIENT: OnceLock<Mutex<AdtBridgeClient>> = OnceLock::new();
    CLIENT.get_or_init(|| Mutex::new(AdtBridgeClient::new()))
}

fn transport_bridge_dir(sap: &SapAdtState) -> String {
    for key in ["SAP_ADT_TRANSPORT_BRIDGE_DIR", "ADT_BRIDGE_DIR"] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }

    let browser_dir = std::path::Path::new(&sap.browser_bridge_dir);
    if let Some(parent) = browser_dir.parent() {
        let candidate = parent.join("adt-bridge");
        if candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }

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

fn browser_profile_for_adt(sap: &SapAdtState) -> String {
    let profile = sap.browser_channel.trim();
    if profile.is_empty() {
        "msedge".to_string()
    } else {
        profile.to_string()
    }
}

fn normalize_bridge_dir(raw: &str) -> String {
    let trimmed = raw.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed);
    unquoted.replace('/', std::path::MAIN_SEPARATOR_STR)
}

fn resolve_browser_bridge_dir(sap: &SapAdtState) -> String {
    let normalized = normalize_bridge_dir(&sap.browser_bridge_dir);
    if !normalized.is_empty() && std::path::Path::new(&normalized).is_dir() {
        return normalized;
    }

    let browser_dir = std::path::Path::new(&normalized);
    if let Some(parent) = browser_dir.parent() {
        let candidate = parent.join("adt-bridge");
        if candidate.is_dir() {
            return candidate.to_string_lossy().into_owned();
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join("adt-bridge");
            if candidate.is_dir() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("adt-bridge");
        if candidate.is_dir() {
            return candidate.to_string_lossy().into_owned();
        }
    }

    normalized
}

fn browser_cfg_from_state(sap: &SapAdtState) -> BrowserTurnConfig {
    BrowserTurnConfig {
        bridge_dir: resolve_browser_bridge_dir(sap),
        edge_executable: if cfg!(target_os = "windows") {
            "msedge".to_string()
        } else {
            "msedge".to_string()
        },
        user_data_dir: sap.browser_user_data_dir.clone(),
        cdp_url: "http://127.0.0.1:9222".to_string(),
        page_url_contains: "sap".to_string(),
        profile: browser_profile_for_adt(sap),
        session_id: sap.browser_session_id.clone(),
        auto_launch_edge: false,
        runtime_key: "sap_adt".to_string(),
        response_timeout_ms: 60_000,
        response_poll_ms: 1_000,
        dom_poll_ms: 1_000,
    }
}

fn require_discovery_url(sap: &SapAdtState) -> Result<String> {
    sap.discovery_url
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("SAP ADT discovery URL is not set"))
}

fn validate_bridge_settings(sap: &SapAdtState) -> Result<()> {
    let resolved_bridge_dir = resolve_browser_bridge_dir(sap);
    if resolved_bridge_dir.trim().is_empty() {
        return Err(anyhow!("SAP ADT browser bridge directory is not set"));
    }
    if !std::path::Path::new(&resolved_bridge_dir).is_dir() {
        return Err(anyhow!(format!("SAP ADT browser bridge directory does not exist: {}", resolved_bridge_dir)));
    }
    if sap.browser_user_data_dir.trim().is_empty() {
        return Err(anyhow!("SAP ADT browser user data dir is not set"));
    }
    Ok(())
}

fn cookie_urls_for_discovery(discovery_url: &str) -> Vec<String> {
    let mut urls = vec![discovery_url.to_string()];
    if let Ok(url) = Url::parse(discovery_url) {
        let origin = format!(
            "{}://{}{}",
            url.scheme(),
            url.host_str().unwrap_or_default(),
            url.port().map(|p| format!(":{}", p)).unwrap_or_default()
        );
        if !urls.iter().any(|u| u == &origin) {
            urls.push(origin);
        }
    }
    urls
}

fn refresh_cookie_header(sap: &mut SapAdtState, discovery_url: &str) -> Result<String> {
    validate_bridge_settings(sap)?;

    let mut cfg = browser_cfg_from_state(sap);
    if cfg.session_id.is_none() {
        browser_bridge::launch_browser(&mut cfg, Some(discovery_url))
            .context("Failed to launch browser bridge session for SAP ADT")?;
    }

    let urls = cookie_urls_for_discovery(discovery_url);
    let cookie_header = browser_bridge::get_session_cookies(&mut cfg, &urls)
        .context("Failed to harvest SAP ADT cookies from browser bridge")?;

    sap.browser_session_id = cfg.session_id.clone();
    sap.cookie_header = Some(cookie_header.clone());
    Ok(cookie_header)
}

fn build_http_client() -> Result<Client> {
    Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(60))
        .build()
        .context("Failed to build SAP ADT HTTP client")
}

fn send_discovery_request(client: &Client, discovery_url: &str, cookie_header: &str) -> Result<(StatusCode, String)> {
    let response = client
        .get(discovery_url)
        .header(COOKIE, cookie_header)
        .header(ACCEPT, "application/xml, text/xml, */*")
        .header(USER_AGENT, "mdev-sap-adt/1.0")
        .send()
        .with_context(|| format!("Failed to send SAP ADT discovery request to {}", discovery_url))?;

    let status = response.status();
    let body = response.text().unwrap_or_default();
    Ok((status, body))
}

fn xml_decode(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn xml_attr(attrs: &str, name: &str) -> Option<String> {
    let needle = format!("{}=\"", name);
    let start = attrs.find(&needle)? + needle.len();
    let rest = &attrs[start..];
    let end = rest.find('"')?;
    Some(xml_decode(&rest[..end]))
}

fn first_tag_text(block: &str, tag: &str) -> Option<String> {
    let pattern = format!(r"(?s)<{}\b[^>]*>(.*?)</{}>", regex::escape(tag), regex::escape(tag));
    let re = Regex::new(&pattern).ok()?;
    let caps = re.captures(block)?;
    Some(xml_decode(caps.get(1)?.as_str().trim()))
}

fn collect_tag_texts(block: &str, tag: &str) -> Vec<String> {
    let pattern = format!(r"(?s)<{}\b[^>]*>(.*?)</{}>", regex::escape(tag), regex::escape(tag));
    let Ok(re) = Regex::new(&pattern) else {
        return vec![];
    };

    re.captures_iter(block)
        .filter_map(|caps| caps.get(1).map(|m| xml_decode(m.as_str().trim())))
        .filter(|s| !s.is_empty())
        .collect()
}

fn compact_xml_preview(xml: &str, limit: usize) -> String {
    let compact = xml.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(limit).collect()
}

fn extract_adt_exception_message(xml: &str) -> Option<String> {
    let type_re = Regex::new(r#"<type\b[^>]*id=\"([^\"]+)\"[^>]*/?>"#).ok()?;
    let message_re = Regex::new(r#"(?s)<(?:[^\s>]+:)?(?:localizedMessage|message)\b[^>]*>(.*?)</(?:[^\s>]+:)?(?:localizedMessage|message)>"#).ok()?;

    let type_id = type_re
        .captures(xml)
        .and_then(|caps| caps.get(1).map(|m| xml_decode(m.as_str())));

    let message = message_re
        .captures_iter(xml)
        .filter_map(|caps| caps.get(1).map(|m| xml_decode(m.as_str().trim())))
        .find(|s| !s.is_empty());

    if type_id.is_none() && message.is_none() {
        return None;
    }

    Some(match (type_id, message) {
        (Some(t), Some(m)) => format!("{}: {}", t, m),
        (Some(t), None) => t,
        (None, Some(m)) => m,
        (None, None) => String::new(),
    })
}

fn build_package_search_queries(package_name: &str) -> Vec<String> {
    let trimmed = package_name.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let mut queries = Vec::new();
    queries.push(format!("{}*", trimmed));

    if let Some((stem, _)) = trimmed.rsplit_once('_') {
        let stem = stem.trim();
        if !stem.is_empty() {
            queries.push(format!("{}*", stem));
        }
    }

    queries.push(trimmed.to_string());
    queries.dedup();
    queries
}

fn parse_discovery(xml: &str) -> Result<SapAdtDiscoveryState> {
    let workspace_re = Regex::new(r"(?s)<app:workspace\b[^>]*>(.*?)</app:workspace>")?;
    let collection_re = Regex::new(r"(?s)<app:collection\b([^>]*)>(.*?)</app:collection>")?;
    let category_re = Regex::new(r#"<atom:category\b([^>]*)/?>"#)?;
    let template_link_re = Regex::new(r#"<adtcomp:templateLink\b([^>]*)/?>"#)?;

    let mut workspaces = Vec::new();
    for caps in workspace_re.captures_iter(xml) {
        let body = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        if let Some(title) = first_tag_text(body, "atom:title") {
            if !title.is_empty() {
                workspaces.push(title);
            }
        }
    }

    let mut collections = Vec::new();
    for caps in collection_re.captures_iter(xml) {
        let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let body = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let href = xml_attr(attrs, "href").unwrap_or_default();
        if href.is_empty() {
            continue;
        }

        let title = first_tag_text(body, "atom:title").unwrap_or_else(|| href.clone());
        let accepts = collect_tag_texts(body, "app:accept");

        let category_caps = category_re.captures(body);
        let (category_term, category_scheme) = if let Some(cat_caps) = category_caps {
            let cat_attrs = cat_caps.get(1).map(|m| m.as_str()).unwrap_or("");
            (xml_attr(cat_attrs, "term"), xml_attr(cat_attrs, "scheme"))
        } else {
            (None, None)
        };

        let template_links = template_link_re
            .captures_iter(body)
            .map(|tpl_caps| {
                let tpl_attrs = tpl_caps.get(1).map(|m| m.as_str()).unwrap_or("");
                SapAdtTemplateLink {
                    rel: xml_attr(tpl_attrs, "rel").unwrap_or_default(),
                    template: xml_attr(tpl_attrs, "template").unwrap_or_default(),
                    title: xml_attr(tpl_attrs, "title"),
                }
            })
            .filter(|tpl| !tpl.template.is_empty() || !tpl.rel.is_empty())
            .collect::<Vec<_>>();

        collections.push(SapAdtDiscoveryCollection {
            title,
            href,
            category_term,
            category_scheme,
            accepts,
            template_links,
        });
    }

    if collections.is_empty() {
        return Err(anyhow!("SAP ADT discovery XML contained no collections"));
    }

    let package_collection_href = collections
        .iter()
        .find(|c| c.href == "/sap/bc/adt/packages")
        .map(|c| c.href.clone());

    let package_tree_href = package_collection_href
        .as_ref()
        .map(|href| format!("{}/$tree", href.trim_end_matches('/')));

    let repository_search_collection = collections
        .iter()
        .find(|c| c.href == "/sap/bc/adt/repository/informationsystem/search");

    let repository_search_href = repository_search_collection.map(|c| c.href.clone());
    let repository_search_template = repository_search_collection
        .and_then(|c| {
            c.template_links
                .iter()
                .find(|tpl| tpl.template.contains("/sap/bc/adt/repository/informationsystem/search"))
                .map(|tpl| tpl.template.clone())
        })
        .or_else(|| repository_search_href.clone());

    let object_types_href = collections
        .iter()
        .find(|c| c.href == "/sap/bc/adt/repository/informationsystem/objecttypes")
        .map(|c| c.href.clone());

    let enabled = package_tree_href.is_some() && repository_search_template.is_some();

    Ok(SapAdtDiscoveryState {
        workspaces,
        collections,
        package_collection_href,
        package_tree_href,
        repository_search_href,
        repository_search_template,
        object_types_href,
        enabled,
    })
}

fn bridge_base_url(discovery_url: &str) -> Result<String> {
    let url = Url::parse(discovery_url)
        .with_context(|| format!("Invalid SAP ADT discovery URL: {}", discovery_url))?;
    let host = url.host_str().ok_or_else(|| anyhow!("SAP ADT discovery URL missing host"))?;
    let mut out = format!("{}://{}", url.scheme(), host);
    if let Some(port) = url.port() {
        out.push(':');
        out.push_str(&port.to_string());
    }
    Ok(out)
}

fn connect_transport_session(sap: &mut SapAdtState, cookie_header: &str) -> Result<String> {
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    let resp = client.send_json(json!({
        "cmd": "connect",
        "session_id": sap.adt_session_id.clone(),
        "base_url": base_url,
        "auth_type": "cookie",
        "cookie_header": cookie_header,
        "timeout_ms": 60000
    }))?;

    let session_id = resp
        .get("session_id")
        .and_then(|v| v.as_str())
        .or_else(|| resp.get("data").and_then(|v| v.get("session_id")).and_then(|v| v.as_str()))
        .ok_or_else(|| anyhow!("ADT connect response missing session_id"))?
        .to_string();

    sap.adt_session_id = Some(session_id.clone());
    Ok(session_id)
}

fn ensure_transport_session(sap: &mut SapAdtState) -> Result<String> {
    if let Some(session_id) = sap.adt_session_id.clone() {
        if !session_id.trim().is_empty() {
            return Ok(session_id);
        }
    }

    let cookie_header = sap
        .cookie_header
        .clone()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| anyhow!("SAP ADT cookie header is not available"))?;

    connect_transport_session(sap, &cookie_header)
}

pub fn parse_package_tree_xml(xml: &str) -> Result<Vec<SapAdtObjectSummary>> {
    if let Some(message) = extract_adt_exception_message(xml) {
        return Err(anyhow!("ADT repository search returned exception XML: {}", message));
    }

    if let Some(message) = extract_adt_exception_message(xml) {
        return Err(anyhow!("ADT package tree returned exception XML: {}", message));
    }

    let item_re = Regex::new(r#"<(?:(?:[^\s>]+):)?objectReference\b([^>]*)/?>"#)?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for caps in item_re.captures_iter(xml) {
        let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");

        let uri = xml_attr(attrs, "adtcore:uri")
            .or_else(|| xml_attr(attrs, "uri"))
            .or_else(|| xml_attr(attrs, "objectUri"))
            .or_else(|| xml_attr(attrs, "href"));
        let Some(uri) = uri else {
            continue;
        };
        if uri.trim().is_empty() || !seen.insert(uri.clone()) {
            continue;
        }

        let source_uri = xml_attr(attrs, "adtcore:sourceUri")
            .or_else(|| xml_attr(attrs, "sourceUri"))
            .or_else(|| xml_attr(attrs, "adtcore:sourceResourceUri"))
            .or_else(|| xml_attr(attrs, "sourceResourceUri"))
            .or_else(|| xml_attr(attrs, "adtcore:contentUri"))
            .or_else(|| xml_attr(attrs, "contentUri"));

        let name = xml_attr(attrs, "adtcore:name")
            .or_else(|| xml_attr(attrs, "name"))
            .or_else(|| xml_attr(attrs, "displayName"))
            .or_else(|| xml_attr(attrs, "label"))
            .or_else(|| xml_attr(attrs, "title"))
            .unwrap_or_else(|| uri.rsplit('/').next().unwrap_or(uri.as_str()).to_string());

        let object_type = xml_attr(attrs, "adtcore:type")
            .or_else(|| xml_attr(attrs, "type"))
            .or_else(|| xml_attr(attrs, "objectType"))
            .unwrap_or_else(|| "OBJECT".to_string());

        let package_name = xml_attr(attrs, "adtcore:packageName")
            .or_else(|| xml_attr(attrs, "packageName"));

        let description = xml_attr(attrs, "adtcore:description")
            .or_else(|| xml_attr(attrs, "description"))
            .or_else(|| xml_attr(attrs, "title"));

        out.push(SapAdtObjectSummary {
            uri,
            source_uri,
            name,
            object_type,
            package_name,
            description,
        });
    }

    out.sort_by(|a, b| {
        a.object_type
            .to_lowercase()
            .cmp(&b.object_type.to_lowercase())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.uri.cmp(&b.uri))
    });

    Ok(out)
}

pub fn connect(sap: &mut SapAdtState) -> Result<()> {
    let discovery_url = require_discovery_url(sap)?;

    sap.connected = false;
    sap.last_error = None;
    sap.last_status = Some("Refreshing SAP ADT authentication".to_string());

    let mut cookie_header = sap.cookie_header.clone().unwrap_or_default();
    if cookie_header.trim().is_empty() {
        cookie_header = refresh_cookie_header(sap, &discovery_url)?;
    }

    let client = build_http_client()?;
    let (status, body) = send_discovery_request(&client, &discovery_url, &cookie_header)?;

    let final_body = if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        let refreshed_cookie_header = refresh_cookie_header(sap, &discovery_url)?;
        let (retry_status, retry_body) = send_discovery_request(&client, &discovery_url, &refreshed_cookie_header)?;
        if !retry_status.is_success() {
            return Err(anyhow!(
                "SAP ADT discovery returned {} after cookie refresh: {}",
                retry_status,
                retry_body
            ));
        }
        retry_body
    } else if !status.is_success() {
        return Err(anyhow!("SAP ADT discovery returned {}: {}", status, body));
    } else {
        body
    };

    let discovery = parse_discovery(&final_body)?;
    if !discovery.enabled {
        return Err(anyhow!(
            "SAP ADT discovery was retrieved but did not expose the package tree and repository search metadata needed by mdev"
        ));
    }

    let session_id = connect_transport_session(sap, &cookie_header)?;
    let workspace_count = discovery.workspaces.len();
    let collection_count = discovery.collections.len();

    sap.connected = true;
    sap.discovery_xml = final_body;
    sap.discovery = Some(discovery);
    sap.adt_session_id = Some(session_id);
    sap.last_error = None;
    sap.last_status = Some(format!(
        "Connected: discovery ingested ({} workspaces, {} collections)",
        workspace_count,
        collection_count
    ));

    Ok(())
}

pub fn list_package_objects(sap: &mut SapAdtState, package_name: &str, include_subpackages: bool) -> Result<String> {
    let package_name = package_name.trim();
    if package_name.is_empty() {
        return Err(anyhow!("Package name is required"));
    }

    let session_id = ensure_transport_session(sap)?;
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);
    let queries = build_package_search_queries(package_name);

    eprintln!(
        "[sap_adt] list_package_objects start package={} include_subpackages={} session_id={}",
        package_name,
        include_subpackages,
        session_id
    );

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    let mut last_xml = String::new();

    for query in queries {
        let mut search_url = Url::parse(&format!("{}/sap/bc/adt/repository/informationsystem/search", base_url))
            .with_context(|| format!("Invalid SAP ADT repository search base URL: {}", base_url))?;
        search_url
            .query_pairs_mut()
            .append_pair("operation", "quickSearch")
            .append_pair("query", &query)
            .append_pair("maxResults", "100");

        let uri = if let Some(q) = search_url.query() {
            format!("{}?{}", search_url.path(), q)
        } else {
            search_url.path().to_string()
        };

        let resp = client.send_json(json!({
            "cmd": "call_endpoint",
            "session_id": session_id,
            "method": "GET",
            "uri": uri,
            "accept": "application/xml, text/xml, */*"
        }))?;

        let xml = resp
            .get("data")
            .and_then(|v| v.get("body").or_else(|| v.get("xml")))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("ADT repository search response missing body"))?
            .to_string();

        let preview = compact_xml_preview(&xml, 400);
        eprintln!(
            "[sap_adt] repository search package={} query={} xml_bytes={} xml_preview={}",
            package_name,
            query,
            xml.len(),
            preview
        );

        if let Some(message) = extract_adt_exception_message(&xml) {
            eprintln!(
                "[sap_adt] repository search exception package={} query={} message={}",
                package_name,
                query,
                message
            );
        }

        last_xml = xml.clone();

        if xml.contains("adtcore:objectReference") || xml.contains("objectReference") {
            return Ok(xml);
        }
    }

    if let Some(message) = extract_adt_exception_message(&last_xml) {
        return Err(anyhow!("ADT repository search failed: {}", message));
    }

    Ok(last_xml)
}

pub fn read_object(sap: &mut SapAdtState, object_uri: &str, accept: Option<&str>) -> Result<AdtReadObjectResult> {
    let object_uri = object_uri.trim();
    if object_uri.is_empty() {
        return Err(anyhow!("Object URI is required"));
    }

    let session_id = ensure_transport_session(sap)?;
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    let resp = client.send_json(json!({
        "cmd": "read_object",
        "session_id": session_id,
        "object_uri": object_uri,
        "accept": accept.unwrap_or("text/plain, text/*")
    }))?;

    let data = resp.get("data").ok_or_else(|| anyhow!("ADT read_object response missing data"))?;
    let body = data
        .get("body")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("ADT read_object response missing body"))?
        .to_string();

    let headers = data
        .get("headers")
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| v.to_string()),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(AdtReadObjectResult {
        object_uri: object_uri.to_string(),
        content_type: data.get("content_type").and_then(|v| v.as_str()).map(|s| s.to_string()),
        headers,
        body,
    })
}

pub fn lock_object(sap: &mut SapAdtState, object_uri: &str) -> Result<AdtLockObjectResult> {
    let object_uri = object_uri.trim();
    if object_uri.is_empty() {
        return Err(anyhow!("Object URI is required"));
    }

    let session_id = ensure_transport_session(sap)?;
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    let resp = client.send_json(json!({
        "cmd": "lock_object",
        "session_id": session_id,
        "object_uri": object_uri
    }))?;

    let data = resp.get("data").ok_or_else(|| anyhow!("ADT lock_object response missing data"))?;
    let headers = data
        .get("headers")
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| v.to_string()),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let body = data
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let lock_handle = data
        .get("lock_handle")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("ADT lock_object response missing lock_handle"))?
        .to_string();

    Ok(AdtLockObjectResult {
        object_uri: object_uri.to_string(),
        lock_handle,
        headers,
        body,
    })
}

pub fn update_object(
    sap: &mut SapAdtState,
    object_uri: &str,
    body: &str,
    content_type: Option<&str>,
    lock_handle: Option<&str>,
    corr_nr: Option<&str>,
    if_match: Option<&str>,
) -> Result<AdtUpdateObjectResult> {
    let object_uri = object_uri.trim();
    if object_uri.is_empty() {
        return Err(anyhow!("Object URI is required"));
    }

    let session_id = ensure_transport_session(sap)?;
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    let mut headers = serde_json::Map::new();
    if let Some(if_match) = if_match.filter(|s| !s.trim().is_empty()) {
        headers.insert("If-Match".to_string(), serde_json::Value::String(if_match.to_string()));
    }

    let resp = client.send_json(json!({
        "cmd": "update_object",
        "session_id": session_id,
        "object_uri": object_uri,
        "source": body,
        "content_type": content_type.unwrap_or("text/plain; charset=utf-8"),
        "lock_handle": lock_handle,
        "corr_nr": corr_nr,
        "headers": headers
    }))?;

    let data = resp.get("data").ok_or_else(|| anyhow!("ADT update_object response missing data"))?;
    let headers = data
        .get("headers")
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| v.to_string()),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let body = data
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    Ok(AdtUpdateObjectResult { headers, body })
}

pub fn syntax_check(sap: &mut SapAdtState, object_uri: &str) -> Result<AdtCheckResult> {
    let object_uri = object_uri.trim();
    if object_uri.is_empty() {
        return Err(anyhow!("Object URI is required"));
    }

    let session_id = ensure_transport_session(sap)?;
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    let resp = client.send_json(json!({
        "cmd": "syntax_check",
        "session_id": session_id,
        "object_uri": object_uri
    }))?;

    let data = resp.get("data").unwrap_or(&resp);
    let body = data
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    Ok(AdtCheckResult { body })
}

pub fn activate_object(sap: &mut SapAdtState, object_uri: &str) -> Result<AdtActivateResult> {
    let object_uri = object_uri.trim();
    if object_uri.is_empty() {
        return Err(anyhow!("Object URI is required"));
    }

    let session_id = ensure_transport_session(sap)?;
    let discovery_url = require_discovery_url(sap)?;
    let base_url = bridge_base_url(&discovery_url)?;
    let bridge_dir = transport_bridge_dir(sap);

    let mutex = bridge_client();
    let mut client = mutex.lock().map_err(|_| anyhow!("ADT bridge mutex poisoned"))?;
    client.ensure_started(&bridge_dir, &base_url)?;

    let resp = client.send_json(json!({
        "cmd": "activate_object",
        "session_id": session_id,
        "object_uri": object_uri
    }))?;

    let data = resp.get("data").unwrap_or(&resp);
    let body = data
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    Ok(AdtActivateResult { body })
}
