use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use super::{
    migration::{
        adt_bridge,
        common,
        sap_adt_manifest::SapAdtObjectManifest,
    },
    state::{SapAdtObjectSummary, SapAdtState},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchRequest {
    pub package_name: String,
    #[serde(default)]
    pub include_subpackages: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SearchResponse {
    pub ok: bool,
    pub package_name: String,
    pub objects: Vec<SapSearchObject>,
    pub count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ObjectRequest {
    pub object_uri: String,
    #[serde(default)]
    pub object_name: Option<String>,
    #[serde(default)]
    pub object_type: Option<String>,
    #[serde(default)]
    pub package_name: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ObjectResponse {
    pub ok: bool,
    pub manifest: SapAdtObjectManifest,
    pub selected_resource_id: Option<String>,
    pub suggested_directory: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SapSearchObject {
    pub uri: String,
    pub source_uri: Option<String>,
    pub name: String,
    pub object_type: String,
    pub package_name: Option<String>,
}

fn read_required_env(name: &str) -> Result<String> {
    let value = std::env::var(name).unwrap_or_default();
    let trimmed = value.trim().trim_end_matches('/').to_string();
    if trimmed.is_empty() {
        return Err(anyhow!(format!("{} is not set", name)));
    }
    Ok(trimmed)
}

fn default_discovery_url(base_url: &str, client: Option<&str>) -> String {
    let mut url = if base_url.trim().to_ascii_lowercase().contains("/sap/bc/adt/discovery") {
        base_url.trim().to_string()
    } else {
        format!("{}/sap/bc/adt/discovery", base_url.trim_end_matches('/'))
    };

    if let Some(client_id) = client.map(str::trim).filter(|value| !value.is_empty()) {
        if url.contains('?') {
            url.push('&');
        } else {
            url.push('?');
        }
        url.push_str("sap-client=");
        url.push_str(client_id);
    }

    url
}

fn default_browser_user_data_dir() -> String {
    let mut dir = std::env::temp_dir();
    dir.push("mdev-sap-adt-profile");
    dir.to_string_lossy().replace('\\', "/")
}

fn first_existing_candidate(candidates: &[PathBuf]) -> Option<String> {
    candidates
        .iter()
        .find(|path| path.is_dir())
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}

fn resolve_transport_bridge_dir() -> Result<String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let parent = cwd.parent().map(PathBuf::from).unwrap_or_else(|| cwd.clone());

    let candidates = vec![
        parent.join("adt-bridge"),
        parent.join("bridge"),
        cwd.join("adt-bridge"),
        cwd.join("bridge"),
    ];

    if let Some(found) = first_existing_candidate(&candidates) {
        return Ok(found);
    }

    Err(anyhow!(format!(
        "Could not resolve SAP transport bridge dir. Checked: {}",
        candidates
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

fn build_state() -> Result<SapAdtState> {
    let base_url = read_required_env("ADT_HOST_URL")?;
    let discovery_url = default_discovery_url(&base_url, None);
    let bridge_dir = resolve_transport_bridge_dir()?;
    let cookie_header = common::harvest_cookie_header_for_runtime(&base_url, None)?;

    Ok(SapAdtState {
        base_url,
        auth_type: "cookie".to_string(),
        transport: "fetch".to_string(),
        authorization: String::new(),
        cookie_header: Some(cookie_header),
        client: String::new(),
        bridge_dir: bridge_dir.clone(),
        browser_bridge_dir: bridge_dir,
        browser_user_data_dir: default_browser_user_data_dir(),
        discovery_url: Some(discovery_url),
        ..SapAdtState::default()
    })
}

fn to_search_object(item: SapAdtObjectSummary) -> SapSearchObject {
    SapSearchObject {
        uri: item.uri,
        source_uri: item.source_uri,
        name: item.name,
        object_type: item.object_type,
        package_name: item.package_name,
    }
}

pub fn search_package_objects(req: SearchRequest) -> Result<SearchResponse> {
    let mut sap = build_state()?;
    let package_name = req.package_name.trim().to_string();
    if package_name.is_empty() {
        return Err(anyhow!("package_name is required"));
    }

    let xml = adt_bridge::list_package_objects(&mut sap, &package_name, req.include_subpackages)?;
    let objects = adt_bridge::parse_package_tree_xml(&xml)?;
    let count = objects.len();

    Ok(SearchResponse {
        ok: true,
        package_name,
        objects: objects.into_iter().map(to_search_object).collect(),
        count,
    })
}

pub fn fetch_object_manifest(req: ObjectRequest) -> Result<ObjectResponse> {
    let mut sap = build_state()?;

    let object_uri = req.object_uri.trim().to_string();
    if object_uri.is_empty() {
        return Err(anyhow!("object_uri is required"));
    }

    let manifest = adt_bridge::crawl_object_manifest(
        &mut sap,
        &object_uri,
        req.object_name.as_deref(),
        req.object_type.as_deref(),
        req.package_name.as_deref(),
    )?;

    let selected_resource_id = manifest.primary_resource_id();
    let suggested_directory = adt_bridge::manifest_directory_name(
        manifest.object_name.as_deref().or(req.object_name.as_deref()),
        manifest.object_type.as_deref().or(req.object_type.as_deref()),
        manifest.package_name.as_deref().or(req.package_name.as_deref()),
    );

    Ok(ObjectResponse {
        ok: true,
        manifest,
        selected_resource_id,
        suggested_directory,
    })
}
