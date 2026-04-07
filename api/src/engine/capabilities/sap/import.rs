use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use super::common::{
    ensure_adt_bridge_connected,
    import_object_to_worktree,
    parse_connection,
    resolve_bridge_dir,
    resolve_package_objects,
    resolve_repo_path,
    split_multiline_items,
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
    pub selected_objects: Vec<SapImportSelection>,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub connection: Value,
}

#[derive(Debug, Deserialize, Clone)]
struct SapImportSelection {
    pub object_uri: String,
    #[serde(default)]
    pub object_name: String,
    #[serde(default)]
    pub object_type: String,
    #[serde(default)]
    pub package_name: Option<String>,
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
    let effective = ensure_adt_bridge_connected(&mut bridge, &connection).await?;

    let mut selected_objects = request.selected_objects.clone();
    if selected_objects.is_empty() {
        let object_uris = split_multiline_items(&request.object_uris_text);
        if !object_uris.is_empty() {
            selected_objects = object_uris
                .into_iter()
                .map(|object_uri| SapImportSelection {
                    object_uri,
                    object_name: String::new(),
                    object_type: String::new(),
                    package_name: None,
                })
                .collect();
        }
    }

    let needs_package_resolution = selected_objects.is_empty() && !request.package_name.trim().is_empty();

    if needs_package_resolution {
        let objects = resolve_package_objects(&mut bridge, &effective, &request.package_name, request.include_subpackages).await?;
        if !selected_objects.is_empty() {
            let requested = selected_objects;
            selected_objects = objects
                .into_iter()
                .filter(|object| object.object_type != "DEVC/K" && !object.uri.contains("/packages/"))
                .filter(|object| {
                    requested.iter().any(|item| {
                        item.object_uri == object.uri
                            || object.source_uri.as_deref() == Some(item.object_uri.as_str())
                    })
                })
                .map(|object| SapImportSelection {
                    object_uri: object.uri.clone(),
                    object_name: object.name.clone(),
                    object_type: object.object_type.clone(),
                    package_name: object.package_name.clone(),
                })
                .collect();
        } else {
            selected_objects = objects
                .into_iter()
                .filter(|object| object.object_type != "DEVC/K" && !object.uri.contains("/packages/"))
                .map(|object| SapImportSelection {
                    object_uri: object.uri.clone(),
                    object_name: object.name.clone(),
                    object_type: object.object_type.clone(),
                    package_name: object.package_name.clone(),
                })
                .collect();
        }
    }

    if selected_objects.is_empty() {
        bail!("sap/import resolved zero importable object selections")
    }

    let mut imported: Vec<SapImportObjectResult> = Vec::new();
    for item in selected_objects {
        imported.push(import_object_to_worktree(
            &mut bridge,
            &repo,
            &item.object_uri,
            if item.object_name.trim().is_empty() { None } else { Some(item.object_name.as_str()) },
            if item.object_type.trim().is_empty() { None } else { Some(item.object_type.as_str()) },
            item.package_name.as_deref(),
            None,
            request.include_xml_artifacts,
        )?);
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
    obj.entry("selected_objects".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));

    payload
}
