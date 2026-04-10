use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::capabilities::sap::state::SapAdtState;

use super::sap_adt_manifest::{
    SapAdtManifestResource,
    SapAdtObjectManifest,
};

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SapConnectionConfig {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub auth_type: Option<String>,
    #[serde(default)]
    pub cookie_header: Option<String>,
    #[serde(default)]
    pub bridge_dir: Option<String>,
    #[serde(default)]
    pub cdp_url: Option<String>,
    #[serde(default)]
    pub edge_executable: Option<String>,
    #[serde(default)]
    pub page_url_contains: Option<String>,
    #[serde(default)]
    pub target_url: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SapImportObjectResult {
    #[serde(default)]
    pub object_uri: String,
    #[serde(default)]
    pub object_name: String,
    #[serde(default)]
    pub object_type: String,
    #[serde(default)]
    pub package_name: Option<String>,
    #[serde(default)]
    pub manifest_path: String,
    #[serde(default)]
    pub manifest_dir: String,
    #[serde(default)]
    pub resource_count: usize,
    #[serde(default)]
    pub document_count: usize,
}

fn runtime_cookie_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn runtime_cookie_cache_key(base_url: &str, client: Option<&str>) -> String {
    let base = base_url.trim().trim_end_matches('/').to_ascii_lowercase();
    let client = client.unwrap_or("").trim().to_ascii_lowercase();
    format!("{}|{}", base, client)
}

fn cached_runtime_cookie_header(base_url: &str, client: Option<&str>) -> Option<String> {
    let key = runtime_cookie_cache_key(base_url, client);
    runtime_cookie_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&key).cloned())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn store_runtime_cookie_header(base_url: &str, client: Option<&str>, cookie_header: &str) {
    let trimmed = cookie_header.trim();
    if trimmed.is_empty() {
        return;
    }

    if let Ok(mut cache) = runtime_cookie_cache().lock() {
        cache.insert(
            runtime_cookie_cache_key(base_url, client),
            trimmed.to_string(),
        );
    }
}

pub fn parse_connection(payload: &Value) -> Result<SapConnectionConfig> {
    let connection = payload.get("connection").cloned().unwrap_or(Value::Null);

    let mut config: SapConnectionConfig = match connection {
        Value::Null => SapConnectionConfig::default(),
        Value::Object(_) => serde_json::from_value(connection).context("invalid sap connection payload")?,
        _ => bail!("sap/import connection must be an object"),
    };

    config.base_url = config.base_url.trim().trim_end_matches('/').to_string();
    if config.base_url.is_empty() {
        config.base_url = std::env::var("ADT_HOST_URL")
            .unwrap_or_default()
            .trim()
            .trim_end_matches('/')
            .to_string();
    }
    if config.base_url.is_empty() {
        bail!("sap/import connection.base_url is required (or ADT_HOST_URL must be set)");
    }

    config.client = config
        .client
        .take()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    config.auth_type = Some(
        config
            .auth_type
            .take()
            .unwrap_or_else(|| "cookie".to_string())
            .trim()
            .to_ascii_lowercase(),
    );

    config.cookie_header = config
        .cookie_header
        .take()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    Ok(config)
}

pub fn resolve_repo_path(repo_ref: &str) -> Result<PathBuf> {
    let trimmed = repo_ref.trim();
    if trimmed.is_empty() {
        bail!("sap/import repo_ref is required");
    }

    let repo = PathBuf::from(trimmed);
    if !repo.exists() {
        bail!("repository path does not exist: {}", repo.display());
    }
    if !repo.is_dir() {
        bail!("repository path is not a directory: {}", repo.display());
    }

    Ok(repo)
}

pub fn resolve_bridge_dir(config: &SapConnectionConfig) -> Result<PathBuf> {
    let explicit = config
        .bridge_dir
        .as_deref()
        .map(str::trim)
        .unwrap_or("");

    if !explicit.is_empty() {
        return Ok(PathBuf::from(explicit));
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let parent = cwd.parent().map(PathBuf::from).unwrap_or_else(|| cwd.clone());
    let candidates = [
        parent.join("adt-bridge"),
        parent.join("bridge"),
        cwd.join("adt-bridge"),
        cwd.join("bridge"),
    ];

    for candidate in candidates {
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }

    Ok(parent.join("bridge"))
}

pub fn harvest_cookie_header_for_runtime(base_url: &str, client: Option<String>) -> Result<String> {
    let base = base_url.trim();
    if base.is_empty() {
        bail!("SAP base_url is required for cookie harvest");
    }

    let normalized_base = base.trim_end_matches('/').to_string();
    let client_trimmed = client
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    if let Some(cookie_header) = cached_runtime_cookie_header(&normalized_base, client_trimmed.as_deref()) {
        return Ok(cookie_header);
    }

    let discovery_url = match client_trimmed.as_deref() {
        Some(client_id) => format!("{}/sap/bc/adt/discovery?sap-client={}", normalized_base, client_id),
        None => format!("{}/sap/bc/adt/discovery", normalized_base),
    };

    let mut sap = SapAdtState {
        base_url: normalized_base.clone(),
        auth_type: "cookie".to_string(),
        transport: "fetch".to_string(),
        authorization: String::new(),
        cookie_header: None,
        client: client_trimmed.clone().unwrap_or_default(),
        discovery_url: Some(discovery_url.clone()),
        ..SapAdtState::default()
    };

    let cookie_header = super::adt_bridge::refresh_cookie_header(&mut sap, &discovery_url)?;
    store_runtime_cookie_header(&normalized_base, client_trimmed.as_deref(), &cookie_header);
    Ok(cookie_header)
}

pub fn split_multiline_items(input: &str) -> Vec<String> {
    input
        .lines()
        .flat_map(|line| line.split(','))
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .map(|item| item.to_string())
        .collect()
}

pub fn manifest_directory_name(
    object_name: Option<&str>,
    object_type: Option<&str>,
    package_name: Option<&str>,
) -> String {
    fn slug(value: &str) -> String {
        let mut out = String::new();
        let mut last_was_sep = false;
        for ch in value.trim().chars() {
            let keep = ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.');
            if keep {
                out.push(ch);
                last_was_sep = false;
            } else if !last_was_sep {
                out.push('_');
                last_was_sep = true;
            }
        }
        out.trim_matches('_').to_string()
    }

    let package = package_name.map(slug).filter(|v| !v.is_empty()).unwrap_or_else(|| "package".to_string());
    let typ = object_type.map(slug).filter(|v| !v.is_empty()).unwrap_or_else(|| "object".to_string());
    let name = object_name.map(slug).filter(|v| !v.is_empty()).unwrap_or_else(|| "unnamed".to_string());

    format!("sap_adt/{}/{}/{}", package, typ, name)
}

pub fn should_persist_sap_adt_resource(resource: &SapAdtManifestResource, include_xml_artifacts: bool) -> bool {
    if resource.path.trim().is_empty() {
        return false;
    }

    if include_xml_artifacts {
        return true;
    }

    resource.readable || resource.editable || resource.activatable || !resource.body.trim().is_empty()
}

pub fn write_manifest_tree(repo: &Path, manifest_dir: &str, manifest: &SapAdtObjectManifest) -> Result<()> {
    let manifest_dir = normalize_relative_dir(manifest_dir);
    if manifest_dir.is_empty() {
        bail!("manifest directory cannot be empty");
    }

    let root = repo.join(&manifest_dir);
    fs::create_dir_all(&root)
        .with_context(|| format!("failed to create manifest directory {}", root.display()))?;

    let manifest_path = root.join("manifest.adt.json");
    let manifest_bytes = serde_json::to_vec_pretty(manifest)?;
    fs::write(&manifest_path, manifest_bytes)
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    for resource in &manifest.resources {
        if should_persist_sap_adt_resource(resource, true) {
            write_text_file(&root, &resource.path, &resource.body)?;
        }
    }

    for document in &manifest.documents {
        write_text_file(&root, &document.path, &document.body)?;
    }

    Ok(())
}

fn write_text_file(root: &Path, relative_path: &str, body: &str) -> Result<()> {
    let rel = normalize_relative_file(relative_path);
    if rel.is_empty() {
        return Ok(());
    }

    let path = root.join(&rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    fs::write(&path, body.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn normalize_relative_dir(input: &str) -> String {
    input.trim().replace('\\', "/").trim_matches('/').to_string()
}

fn normalize_relative_file(input: &str) -> String {
    let normalized = normalize_relative_dir(input);
    if normalized.contains("..") {
        String::new()
    } else {
        normalized
    }
}
