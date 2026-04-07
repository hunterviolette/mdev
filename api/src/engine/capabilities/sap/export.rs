use std::fs;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use super::common::{
    activation_details_have_errors,
    build_export_workflow,
    ensure_adt_bridge_connected,
    export_candidates_from_manifest,
    join_manifest_relative_path,
    manifest_dir_from_manifest_path,
    normalize_repo_relative_path,
    parse_connection,
    read_manifest,
    resolve_bridge_dir,
    resolve_repo_path,
    split_multiline_items,
    summarize_activation_details,
    AdtBridgeProcess,
    SapExportObjectResult,
};
use super::super::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult};

#[derive(Debug, Deserialize)]
struct SapExportRequest {
    pub repo_ref: String,
    #[serde(default = "default_git_ref")]
    pub git_ref: String,
    #[serde(default)]
    pub manifest_paths_text: String,
    #[serde(default = "default_auto_activate")]
    pub auto_activate: bool,
    #[serde(default)]
    pub corr_nr: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub connection: Value,
}

fn default_git_ref() -> String {
    "WORKTREE".to_string()
}

fn default_auto_activate() -> bool {
    true
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let payload = resolve_payload(ctx, config);
    let request: SapExportRequest = serde_json::from_value(payload.clone()).context("invalid sap/export payload")?;

    if request.git_ref != "WORKTREE" {
        bail!("sap/export requires git_ref=WORKTREE")
    }

    let repo = resolve_repo_path(&request.repo_ref)?;
    let manifest_paths = resolve_manifest_paths(&repo, &request.manifest_paths_text)?;
    if manifest_paths.is_empty() {
        bail!("sap/export resolved zero manifest paths")
    }

    let connection = parse_connection(&payload)?;
    let bridge_dir = resolve_bridge_dir(&connection)?;
    let mut bridge = AdtBridgeProcess::start(&bridge_dir)?;
    let _effective = ensure_adt_bridge_connected(&mut bridge, &connection).await?;

    let corr_nr = request.corr_nr.trim();
    let corr_nr = if corr_nr.is_empty() { None } else { Some(corr_nr) };

    let mut exported: Vec<SapExportObjectResult> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for manifest_path in manifest_paths {
        match export_one_manifest(&repo, &mut bridge, &manifest_path, request.auto_activate, corr_nr) {
            Ok(result) => exported.push(result),
            Err(err) => failures.push(format!("{}: {:#}", manifest_path, err)),
        }
    }

    let ok = failures.is_empty();
    Ok(CapabilityResult {
        ok,
        capability: "sap/export".to_string(),
        payload: json!({
            "ok": ok,
            "summary": if ok {
                format!("Exported {} SAP ADT manifest(s)", exported.len())
            } else {
                format!("Exported {} SAP ADT manifest(s) with {} failure(s)", exported.len(), failures.len())
            },
            "request": payload,
            "exported": exported,
            "failures": failures,
            "count": exported.len()
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}

fn resolve_payload(ctx: &CapabilityContext<'_>, config: Value) -> Value {
    let capability_state = ctx
        .local_state
        .get("capabilities")
        .and_then(|v| v.get("sap/export"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let stage_config_state = ctx
        .step
        .config
        .get("sap_export")
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

    let obj = payload.as_object_mut().expect("sap export payload must be object");
    obj.entry("repo_ref".to_string())
        .or_insert_with(|| Value::String(ctx.repo_ref.to_string()));
    obj.entry("git_ref".to_string())
        .or_insert_with(|| Value::String("WORKTREE".to_string()));
    obj.entry("mode".to_string())
        .or_insert_with(|| Value::String("export".to_string()));
    obj.entry("manifest_paths_text".to_string())
        .or_insert_with(|| Value::String(String::new()));
    obj.entry("auto_activate".to_string())
        .or_insert_with(|| Value::Bool(true));
    obj.entry("corr_nr".to_string())
        .or_insert_with(|| Value::String(String::new()));

    payload
}

fn resolve_manifest_paths(repo: &std::path::Path, manifest_paths_text: &str) -> Result<Vec<String>> {
    let explicit = split_multiline_items(manifest_paths_text);
    if !explicit.is_empty() {
        let mut out = explicit
            .into_iter()
            .map(|path| normalize_repo_relative_path(&path))
            .collect::<Vec<_>>();
        out.sort();
        out.dedup();
        return Ok(out);
    }

    let mut out = Vec::new();
    collect_manifest_paths(repo, repo, &mut out)?;
    out.sort();
    out.dedup();
    Ok(out)
}

fn collect_manifest_paths(root: &std::path::Path, current: &std::path::Path, out: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(current).with_context(|| format!("failed to read {}", current.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_manifest_paths(root, &path, out)?;
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) == Some("manifest.adt.json") {
            let rel = path
                .strip_prefix(root)
                .with_context(|| format!("failed to compute relative path for {}", path.display()))?
                .to_string_lossy()
                .replace('\\', "/");
            if rel.starts_with("sap_adt/") {
                out.push(rel);
            }
        }
    }
    Ok(())
}

fn export_one_manifest(
    repo: &std::path::Path,
    bridge: &mut AdtBridgeProcess,
    manifest_path: &str,
    auto_activate: bool,
    corr_nr: Option<&str>,
) -> Result<SapExportObjectResult> {
    let mut manifest = read_manifest(repo, manifest_path)?;
    let manifest_dir = manifest_dir_from_manifest_path(manifest_path);
    let object_name = manifest.object_name.clone().unwrap_or_default();
    let object_type = manifest.object_type.clone().unwrap_or_default();
    let package_name = manifest.package_name.clone();
    let metadata_uri = manifest.metadata_uri.trim().to_string();

    let export_candidate_keys: Vec<(String, String)> = export_candidates_from_manifest(&manifest)
        .into_iter()
        .map(|resource| (resource.id.clone(), resource.uri.clone()))
        .collect();
    let mut pushed_resource_uris: Vec<String> = Vec::new();

    for current_index in 0..manifest.resources.len() {
        let (source_uri, local_path, content_type) = match manifest.resources.get(current_index) {
            Some(resource) if resource.editable && !resource.uri.trim().is_empty() => {
                let in_export_candidates = export_candidate_keys.iter().any(|(candidate_id, candidate_uri)| {
                    (!candidate_id.is_empty() && candidate_id == &resource.id)
                        || (!candidate_uri.trim().is_empty() && candidate_uri.trim() == resource.uri.trim())
                });
                if !in_export_candidates {
                    continue;
                }
                (
                    resource.uri.trim().to_string(),
                    join_manifest_relative_path(&manifest_dir, &resource.path),
                    resource
                        .content_type
                        .clone()
                        .unwrap_or_else(|| "text/plain; charset=utf-8".to_string()),
                )
            }
            _ => continue,
        };

        let source_text = fs::read_to_string(repo.join(&local_path))
            .with_context(|| format!("failed to read {}", local_path))?;

        let lock = bridge.lock_object(&source_uri)
            .with_context(|| format!("failed to lock {}", source_uri))?;

        let update_result = bridge.update_object(&source_uri, &source_text, &content_type, Some(lock.lock_handle.as_str()), corr_nr)
            .with_context(|| format!("failed to update {}", source_uri));

        let unlock_result = bridge.unlock_object(&source_uri, &lock.lock_handle);

        if let Err(err) = unlock_result {
            return Err(anyhow!("updated {} but unlock failed: {:#}", source_uri, err));
        }

        let update_result = update_result?;
        if update_result.status.unwrap_or(200) >= 400 {
            return Err(anyhow!("ADT update failed for {}: {}", source_uri, update_result.body));
        }

        pushed_resource_uris.push(source_uri.clone());
        if let Some(resource) = manifest.resources.get_mut(current_index) {
            resource.lock_handle = None;
            if let Some(etag) = update_result
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("etag"))
                .map(|(_, v)| v.clone())
            {
                resource.etag = Some(etag);
            }
        }
    }

    let syntax_result = if !metadata_uri.is_empty() {
        bridge.syntax_check(&metadata_uri)
    } else if let Some(primary) = manifest.resources.iter().find(|resource| resource.editable) {
        bridge.syntax_check(&primary.uri)
    } else {
        Err(anyhow!("manifest has no metadata_uri or editable resource to syntax check"))
    }?;

    let syntax_ok = syntax_result.status.unwrap_or(200) < 400;
    let syntax_details = syntax_result.body.trim().to_string();

    let mut activation_ok = true;
    let mut activation_details = String::new();

    if auto_activate {
        let activate_uri = if !metadata_uri.is_empty() {
            metadata_uri.clone()
        } else {
            manifest
                .resources
                .iter()
                .find(|resource| resource.activatable)
                .map(|resource| resource.uri.clone())
                .unwrap_or_default()
        };

        if activate_uri.is_empty() {
            activation_ok = false;
            activation_details = "manifest is missing metadata_uri/source_uri".to_string();
        } else {
            let activation = bridge.activate_object(&activate_uri)
                .with_context(|| format!("failed to activate {}", activate_uri))?;
            let mut details = summarize_activation_details(&activation.body);
            let header_result_uri = activation
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("location") || k.eq_ignore_ascii_case("result-location"))
                .map(|(_, v)| v.clone());

            if details.trim().is_empty() {
                if let Some(result_uri) = header_result_uri {
                    if let Ok(problems) = bridge.get_problems(&result_uri) {
                        let summarized = summarize_activation_details(&problems.body);
                        if !summarized.trim().is_empty() {
                            details = summarized;
                        }
                    }
                }
            }

            if details.trim().is_empty() && activation.body.trim().is_empty() {
                if let Ok(checkruns) = bridge.run_checkruns(&activate_uri) {
                    let summarized = summarize_activation_details(&checkruns.body);
                    if !summarized.trim().is_empty() {
                        details = summarized;
                    }
                }
            }

            activation_ok = activation.status.unwrap_or(200) < 400 && !activation_details_have_errors(&details);
            activation_details = details;
        }
    }

    let workflow = build_export_workflow(
        &pushed_resource_uris,
        syntax_ok,
        &syntax_details,
        auto_activate,
        activation_ok,
        &activation_details,
        if syntax_ok { "Exported" } else { "Syntax check failed" },
    );

    let result = SapExportObjectResult {
        manifest_path: manifest_path.to_string(),
        object_name,
        object_type,
        package_name,
        pushed_resource_uris,
        syntax_ok,
        syntax_details,
        activation_ok,
        activation_details,
        workflow,
    };

    Ok(result)
}
