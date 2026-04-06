use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use super::common::{
    crawl_object_manifest,
    manifest_directory_name,
    parse_connection,
    resolve_bridge_dir,
    resolve_repo_path,
    should_persist_sap_adt_resource,
    split_multiline_items,
    write_manifest_tree,
    AdtBridgeProcess,
    SapImportObjectResult,
};
use super::super::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult};

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
    pub mode: String,
    #[serde(default)]
    pub connection: Value,
}

fn default_git_ref() -> String {
    "WORKTREE".to_string()
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
    let bridge_dir = resolve_bridge_dir(&connection)?;

    let mut bridge = AdtBridgeProcess::start(&bridge_dir)?;
    bridge.connect(&connection)?;

    let mut object_uris = split_multiline_items(&request.object_uris_text);
    if object_uris.is_empty() {
        if request.package_name.trim().is_empty() {
            bail!("sap/import requires package_name or object_uris_text")
        }
        let objects = bridge.list_package_objects(&request.package_name, request.include_subpackages)?;
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
        let mut manifest = crawl_object_manifest(&mut bridge, &object_uri, None, None, None)
            .with_context(|| format!("failed to import {}", object_uri))?;

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

    Ok(CapabilityResult {
        ok: true,
        capability: "sap/import".to_string(),
        payload: json!({
            "ok": true,
            "summary": format!("Imported {} SAP ADT object(s) into the worktree", imported.len()),
            "request": payload,
            "imported": imported,
            "count": imported.len()
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}

fn resolve_payload(ctx: &CapabilityContext<'_>, config: Value) -> Value {
    let capability_state = ctx
        .local_state
        .get("capabilities")
        .and_then(|v| v.get("sap/import"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let stage_config_state = ctx
        .step
        .config
        .get("sap_import")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let mut payload = if config.is_null() || config == json!({}) {
        if capability_state.as_object().map(|obj| obj.is_empty()).unwrap_or(true) {
            stage_config_state
        } else {
            capability_state
        }
    } else {
        config
    };

    if !payload.is_object() {
        payload = json!({});
    }

    let obj = payload.as_object_mut().expect("sap import payload must be object");
    obj.entry("repo_ref".to_string())
        .or_insert_with(|| Value::String(ctx.repo_ref.to_string()));
    obj.entry("git_ref".to_string())
        .or_insert_with(|| Value::String("WORKTREE".to_string()));
    obj.entry("mode".to_string())
        .or_insert_with(|| Value::String("import".to_string()));
    obj.entry("include_xml_artifacts".to_string())
        .or_insert_with(|| Value::Bool(false));
    obj.entry("object_uris_text".to_string())
        .or_insert_with(|| Value::String(String::new()));

    payload
}
