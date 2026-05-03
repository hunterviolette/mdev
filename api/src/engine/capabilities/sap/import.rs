use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    migration::{
        adt_bridge,
        common::{
            harvest_cookie_header_for_runtime,
            manifest_directory_name,
            parse_connection,
            resolve_bridge_dir,
            resolve_repo_path,
            should_persist_sap_adt_resource,
            split_multiline_items,
            write_manifest_tree,
            SapImportObjectResult,
        },
    },
    state::SapAdtState,
};
use crate::engine::capabilities::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult};

#[derive(Debug, Clone, Deserialize)]
struct SapImportSelectedObject {
    #[serde(default)]
    pub object_uri: String,
    #[serde(default)]
    pub source_uri: Option<String>,
    #[serde(default)]
    pub object_name: Option<String>,
    #[serde(default)]
    pub object_type: Option<String>,
    #[serde(default)]
    pub package_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SapImportRequest {
    pub repo_ref: String,
    #[serde(default = "default_git_ref")]
    pub git_ref: String,
    #[serde(default)]
    pub package_name: String,
    #[serde(default)]
    pub include_subpackages: bool,
    #[serde(default)]
    pub include_xml_artifacts: bool,
    #[serde(default)]
    pub object_uris_text: String,
    #[serde(default)]
    pub selected_objects: Vec<SapImportSelectedObject>,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub connection: Value,
}

fn default_git_ref() -> String {
    "WORKTREE".to_string()
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

fn find_selected_object<'a>(
    selected_objects: &'a [SapImportSelectedObject],
    object_uri: &str,
) -> Option<&'a SapImportSelectedObject> {
    let object_uri = object_uri.trim();
    selected_objects.iter().find(|item| {
        item.object_uri.trim() == object_uri
            || item
                .source_uri
                .as_deref()
                .map(str::trim)
                .map(|uri| uri == object_uri)
                .unwrap_or(false)
    })
}

fn build_sap_state(connection: &super::migration::common::SapConnectionConfig) -> Result<SapAdtState> {
    let bridge_dir = resolve_bridge_dir(connection)?;
    let bridge_dir = bridge_dir.to_string_lossy().replace('\\', "/");

    let cookie_header = connection
        .cookie_header
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(harvest_cookie_header_for_runtime(&connection.base_url, connection.client.clone())?);

    Ok(SapAdtState {
        base_url: connection.base_url.clone(),
        auth_type: "cookie".to_string(),
        transport: "fetch".to_string(),
        authorization: String::new(),
        cookie_header: Some(cookie_header),
        client: connection.client.clone().unwrap_or_default(),
        bridge_dir: bridge_dir.clone(),
        browser_bridge_dir: bridge_dir,
        browser_user_data_dir: default_browser_user_data_dir(),
        discovery_url: Some(default_discovery_url(&connection.base_url, connection.client.as_deref())),
        ..SapAdtState::default()
    })
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let payload = resolve_payload(ctx, config);
    let request: SapImportRequest = serde_json::from_value(payload.clone()).context("invalid sap/import payload")?;

    if request.git_ref != "WORKTREE" {
        bail!("sap/import requires git_ref=WORKTREE")
    }

    let repo = resolve_repo_path(&request.repo_ref)?;
    let connection = parse_connection(&payload)?;
    let mut sap = build_sap_state(&connection)?;

    let mut object_uris = request
        .selected_objects
        .iter()
        .map(|item| item.source_uri.clone().unwrap_or_else(|| item.object_uri.clone()))
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();

    if object_uris.is_empty() {
        object_uris = split_multiline_items(&request.object_uris_text);
    }

    if object_uris.is_empty() {
        if request.package_name.trim().is_empty() {
            bail!("sap/import requires package_name, selected_objects, or object_uris_text")
        }
        let xml = adt_bridge::list_package_objects(&mut sap, &request.package_name, request.include_subpackages)?;
        let objects = adt_bridge::parse_package_tree_xml(&xml)?;
        object_uris = objects
            .into_iter()
            .filter(|object| object.object_type != "DEVC/K" && !object.uri.contains("/packages/"))
            .map(|object| object.source_uri.unwrap_or(object.uri))
            .collect();
    }

    if object_uris.is_empty() {
        bail!("sap/import resolved zero importable object URIs")
    }

    let mut imported: Vec<SapImportObjectResult> = Vec::new();
    for object_uri in object_uris {
        let selected = find_selected_object(&request.selected_objects, &object_uri);

        let mut manifest = adt_bridge::crawl_object_manifest(
            &mut sap,
            &object_uri,
            selected.and_then(|item| item.object_name.as_deref()),
            selected.and_then(|item| item.object_type.as_deref()),
            selected.and_then(|item| item.package_name.as_deref()),
        )
        .with_context(|| format!("failed to import {}", object_uri))?;

        if manifest.object_name.as_deref().map(str::trim).unwrap_or("").is_empty() {
            manifest.object_name = selected.and_then(|item| item.object_name.clone());
        }
        if manifest.object_type.as_deref().map(str::trim).unwrap_or("").is_empty() {
            manifest.object_type = selected.and_then(|item| item.object_type.clone());
        }
        if manifest.package_name.as_deref().map(str::trim).unwrap_or("").is_empty() {
            manifest.package_name = selected.and_then(|item| item.package_name.clone());
        }

        if !request.include_xml_artifacts {
            manifest.metadata_xml.clear();
            manifest.documents.clear();
            manifest.resources.retain(|resource| should_persist_sap_adt_resource(resource, false));
        }

        let manifest_dir = manifest_directory_name(
            manifest.object_name.as_deref(),
            manifest.object_type.as_deref(),
            manifest.package_name.as_deref(),
        );
        write_manifest_tree(&repo, &manifest_dir, &manifest)
            .with_context(|| format!("failed to persist imported manifest for {}", object_uri))?;

        imported.push(SapImportObjectResult {
            object_uri: object_uri.clone(),
            object_name: manifest.object_name.clone().unwrap_or_default(),
            object_type: manifest.object_type.clone().unwrap_or_default(),
            package_name: manifest.package_name.clone(),
            manifest_path: format!("{}/manifest.adt.json", manifest_dir),
            manifest_dir,
            resource_count: manifest.resources.len(),
            document_count: manifest.documents.len(),
        });
    }

    let manifest_paths = imported
        .iter()
        .map(|item| item.manifest_path.clone())
        .collect::<Vec<_>>();
    let imported_objects = imported
        .iter()
        .map(|item| json!({
            "object_uri": item.object_uri,
            "object_name": item.object_name,
            "object_type": item.object_type,
            "package_name": item.package_name,
            "manifest_path": item.manifest_path,
            "manifest_dir": item.manifest_dir,
            "resource_count": item.resource_count,
            "document_count": item.document_count
        }))
        .collect::<Vec<_>>();

    Ok(CapabilityResult {
        ok: true,
        capability: "sap/import".to_string(),
        payload: json!({
            "ok": true,
            "summary": format!("Imported {} SAP ADT object(s) into the worktree", imported.len()),
            "count": imported.len(),
            "manifest_paths": manifest_paths,
            "imported_objects": imported_objects
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}

fn resolve_payload(ctx: &CapabilityContext<'_>, _config: Value) -> Value {
    let repo_resource = ctx
        .local_state
        .get("resources")
        .and_then(|v| v.get("repo"))
        .cloned()
        .unwrap_or_else(|| json!({
            "repo_ref": ctx.repo_ref,
            "git_ref": "WORKTREE"
        }));

    let capability_state = ctx
        .local_state
        .get("capabilities")
        .and_then(|v| v.get("sap/import"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let mut payload = match ctx
        .local_state
        .get("config")
        .and_then(|v| v.get("sap_import"))
        .cloned()
    {
        Some(Value::Object(map)) => Value::Object(map),
        _ => json!({}),
    };

    if !payload.is_object() {
        payload = json!({});
    }

    let obj = payload.as_object_mut().expect("sap import payload must be object");

    if !obj.contains_key("connection") {
        if let Some(connection) = capability_state.get("connection") {
            obj.insert("connection".to_string(), connection.clone());
        }
    }

    obj.entry("repo_ref".to_string()).or_insert_with(|| {
        repo_resource
            .get("repo_ref")
            .cloned()
            .unwrap_or_else(|| Value::String(ctx.repo_ref.to_string()))
    });
    obj.entry("git_ref".to_string()).or_insert_with(|| {
        repo_resource
            .get("git_ref")
            .cloned()
            .unwrap_or_else(|| Value::String("WORKTREE".to_string()))
    });
    obj.entry("mode".to_string())
        .or_insert_with(|| Value::String("import".to_string()));
    obj.entry("include_xml_artifacts".to_string())
        .or_insert_with(|| Value::Bool(false));
    obj.entry("include_subpackages".to_string())
        .or_insert_with(|| Value::Bool(false));
    obj.entry("package_name".to_string())
        .or_insert_with(|| Value::String(String::new()));
    obj.entry("object_uris_text".to_string())
        .or_insert_with(|| Value::String(String::new()));
    obj.entry("selected_objects".to_string())
        .or_insert_with(|| json!([]));
    obj.entry("connection".to_string())
        .or_insert_with(|| json!({}));

    payload
}
