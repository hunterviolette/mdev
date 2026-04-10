use std::{
    collections::BTreeSet,
    fs,
    path::Path,
};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    migration::{
        adt_bridge,
        common::{
            harvest_cookie_header_for_runtime,
            parse_connection,
            resolve_repo_path,
        },
        sap_adt_manifest::{SapAdtManifestResource, SapAdtObjectManifest},
    },
    state::SapAdtState,
};
use crate::engine::capabilities::{
    git::git::git_status,
    registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult},
};

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
    #[serde(default = "default_mode")]
    pub mode: String,
}

fn default_git_ref() -> String {
    "WORKTREE".to_string()
}

fn default_auto_activate() -> bool {
    true
}

fn default_mode() -> String {
    "export".to_string()
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

fn build_sap_state(connection: &super::migration::common::SapConnectionConfig) -> Result<SapAdtState> {
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
        browser_user_data_dir: default_browser_user_data_dir(),
        discovery_url: Some(default_discovery_url(&connection.base_url, connection.client.as_deref())),
        ..SapAdtState::default()
    })
}

fn is_exportable_source_resource(resource: &SapAdtManifestResource) -> bool {
    resource.editable
        && resource.activatable
        && resource.rel == "http://www.sap.com/adt/relations/source"
        && resource
            .content_type
            .as_deref()
            .map(|s| s.to_ascii_lowercase().starts_with("text/plain"))
            .unwrap_or(false)
}

fn normalize_rel_path(input: &str) -> String {
    input.trim().replace('\\', "/").trim_start_matches("./").to_string()
}

fn split_multiline_items(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(normalize_rel_path)
        .collect()
}

fn manifest_resource_repo_path(manifest_path: &str, resource_path: &str) -> String {
    let manifest_dir = Path::new(manifest_path)
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    if manifest_dir.is_empty() {
        normalize_rel_path(resource_path)
    } else {
        normalize_rel_path(&format!("{}/{}", manifest_dir, resource_path))
    }
}

fn collect_manifest_paths(repo: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![repo.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let read_dir = match fs::read_dir(&dir) {
            Ok(value) => value,
            Err(_) => continue,
        };

        for entry in read_dir {
            let entry = match entry {
                Ok(value) => value,
                Err(_) => continue,
            };
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(value) => value,
                Err(_) => continue,
            };

            if file_type.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if matches!(name.as_str(), ".git" | "node_modules" | "target") {
                    continue;
                }
                stack.push(path);
                continue;
            }

            if file_type.is_file() && entry.file_name().to_string_lossy() == "manifest.adt.json" {
                if let Ok(rel) = path.strip_prefix(repo) {
                    out.push(normalize_rel_path(&rel.to_string_lossy()));
                }
            }
        }
    }

    out.sort();
    out.dedup();
    Ok(out)
}

fn load_manifest(repo: &Path, manifest_path: &str) -> Result<SapAdtObjectManifest> {
    let path = repo.join(manifest_path);
    let bytes = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("failed to parse {}", path.display()))
}

fn collect_unstaged_paths(repo: &Path) -> Result<BTreeSet<String>> {
    let status = git_status(repo)?;
    let mut out = BTreeSet::new();

    for file in status.files {
        if file.untracked {
            continue;
        }
        if file.worktree_status.trim() == "." || file.worktree_status.trim().is_empty() {
            continue;
        }
        out.insert(normalize_rel_path(&file.path));
    }

    Ok(out)
}

fn resolve_payload(ctx: &CapabilityContext<'_>, config: Value) -> Value {
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
        .and_then(|v| v.get("sap/export"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let requested_mode = config
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("export")
        .trim()
        .to_ascii_lowercase();
    let config_key = if requested_mode == "syntax" { "sap_syntax" } else { "sap_export" };

    let mut payload = match ctx
        .local_state
        .get("config")
        .and_then(|v| v.get(config_key))
        .cloned()
    {
        Some(Value::Object(map)) => Value::Object(map),
        _ => json!({}),
    };

    if !payload.is_object() {
        payload = json!({});
    }

    let obj = payload.as_object_mut().expect("sap export payload must be object");

    if !obj.contains_key("connection") {
        if let Some(connection) = capability_state.get("connection") {
            obj.insert("connection".to_string(), connection.clone());
        }
    }

    if let Some(config_obj) = config.as_object() {
        for (key, value) in config_obj {
            obj.insert(key.clone(), value.clone());
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
        .or_insert_with(|| Value::String(requested_mode));
    obj.entry("manifest_paths_text".to_string())
        .or_insert_with(|| Value::String(String::new()));
    obj.entry("auto_activate".to_string())
        .or_insert_with(|| Value::Bool(true));
    obj.entry("corr_nr".to_string())
        .or_insert_with(|| Value::String(String::new()));
    obj.entry("connection".to_string())
        .or_insert_with(|| json!({}));

    payload
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

    let mode = request.mode.trim().to_ascii_lowercase();
    let is_syntax_mode = mode == "syntax";
    let repo = resolve_repo_path(&request.repo_ref)?;
    let changed_paths = collect_unstaged_paths(&repo)?;

    if changed_paths.is_empty() {
        return Ok(CapabilityResult {
            ok: false,
            capability: "sap/export".to_string(),
            payload: json!({
                "ok": false,
                "mode": mode,
                "summary": "No unstaged Git changes were found in the worktree.",
                "changed_paths": []
            }),
            follow_ups: CapabilityInvocationRequest::None,
        });
    }

    let explicit_manifest_paths = split_multiline_items(&request.manifest_paths_text);
    let manifest_paths = if explicit_manifest_paths.is_empty() {
        collect_manifest_paths(&repo)?
    } else {
        explicit_manifest_paths
    };

    let connection = parse_connection(&payload)?;
    let mut sap = build_sap_state(&connection)?;

    let mut object_results = Vec::new();
    let mut selected_manifest_paths = Vec::new();

    for manifest_path in manifest_paths {
        let manifest = match load_manifest(&repo, &manifest_path) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let changed_resources = manifest
            .resources
            .iter()
            .filter(|resource| is_exportable_source_resource(resource))
            .filter_map(|resource| {
                let repo_path = manifest_resource_repo_path(&manifest_path, &resource.path);
                if changed_paths.contains(&repo_path) {
                    Some((resource.clone(), repo_path))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        if changed_resources.is_empty() {
            continue;
        }

        selected_manifest_paths.push(manifest_path.clone());

        let object_name = manifest.object_name.clone().unwrap_or_else(|| "object".to_string());
        let object_type = manifest.object_type.clone().unwrap_or_else(|| "OBJECT".to_string());
        let package_name = manifest.package_name.clone();
        let activate_uri = if !manifest.metadata_uri.trim().is_empty() {
            manifest.metadata_uri.trim().to_string()
        } else {
            changed_resources
                .first()
                .map(|(resource, _)| resource.uri.clone())
                .unwrap_or_default()
        };

        let check_uri = if !manifest.metadata_uri.trim().is_empty() {
            manifest.metadata_uri.trim().to_string()
        } else {
            changed_resources
                .first()
                .map(|(resource, _)| resource.uri.clone())
                .unwrap_or_default()
        };

        let mut resource_results = Vec::new();
        let mut pushed_count = 0usize;
        let mut syntax_ok = true;
        let mut syntax_problem_count = 0usize;

        for (resource, repo_rel_path) in &changed_resources {
            let abs_path = repo.join(repo_rel_path);
            let source = fs::read_to_string(&abs_path)
                .with_context(|| format!("failed to read {}", abs_path.display()))?;

            let update_result = adt_bridge::update_object(
                &mut sap,
                &resource.uri,
                &source,
                resource.content_type.as_deref(),
                None,
                Some(request.corr_nr.as_str()).filter(|value| !value.trim().is_empty()),
                None,
            )?;

            let check_result = if update_result.ok {
                adt_bridge::syntax_check(&mut sap, if check_uri.is_empty() { &resource.uri } else { &check_uri })?
            } else {
                adt_bridge::AdtCheckResult {
                    status: update_result.status,
                    body: update_result.body.clone(),
                    problems: vec!["Syntax check skipped because inactive update failed.".to_string()],
                    ok: false,
                }
            };

            if update_result.ok {
                pushed_count += 1;
            }
            if !update_result.ok || !check_result.ok {
                syntax_ok = false;
            }
            syntax_problem_count += update_result.problems.len() + check_result.problems.len();

            resource_results.push(json!({
                "resource_id": resource.id,
                "uri": resource.uri,
                "path": repo_rel_path,
                "content_type": resource.content_type,
                "update": {
                    "ok": update_result.ok,
                    "status": update_result.status,
                    "problems": update_result.problems,
                    "body": update_result.body
                },
                "syntax": {
                    "ok": check_result.ok,
                    "status": check_result.status,
                    "problems": check_result.problems,
                    "body": check_result.body
                }
            }));
        }

        let activation = if !is_syntax_mode && request.auto_activate && syntax_ok && !activate_uri.is_empty() {
            let result = adt_bridge::activate_object(&mut sap, &activate_uri)?;
            json!({
                "attempted": true,
                "ok": result.ok,
                "status": result.status,
                "problems": result.problems,
                "body": result.body,
                "uri": activate_uri
            })
        } else {
            json!({
                "attempted": false,
                "ok": Value::Null,
                "status": Value::Null,
                "problems": [],
                "body": "",
                "uri": activate_uri
            })
        };

        let activation_ok = activation.get("ok").and_then(Value::as_bool).unwrap_or(false);
        let object_ok = if is_syntax_mode {
            syntax_ok
        } else if request.auto_activate {
            syntax_ok && activation_ok
        } else {
            syntax_ok
        };

        object_results.push(json!({
            "manifest_path": manifest_path,
            "object_name": object_name,
            "object_type": object_type,
            "package_name": package_name,
            "changed_resource_count": changed_resources.len(),
            "pushed_resource_count": pushed_count,
            "syntax_ok": syntax_ok,
            "syntax_problem_count": syntax_problem_count,
            "activation": activation,
            "ok": object_ok,
            "resources": resource_results
        }));
    }

    if object_results.is_empty() {
        return Ok(CapabilityResult {
            ok: false,
            capability: "sap/export".to_string(),
            payload: json!({
                "ok": false,
                "mode": mode,
                "summary": "No exportable SAP ADT resources matched unstaged Git changes.",
                "changed_paths": changed_paths.into_iter().collect::<Vec<_>>(),
                "manifest_paths": []
            }),
            follow_ups: CapabilityInvocationRequest::None,
        });
    }

    let ok = object_results
        .iter()
        .all(|item| item.get("ok").and_then(Value::as_bool) == Some(true));
    let verb = if is_syntax_mode { "Prepared syntax for" } else { "Exported" };

    Ok(CapabilityResult {
        ok,
        capability: "sap/export".to_string(),
        payload: json!({
            "ok": ok,
            "mode": mode,
            "summary": format!("{} {} SAP ADT object(s) selected from unstaged worktree changes", verb, object_results.len()),
            "manifest_paths": selected_manifest_paths,
            "changed_paths": changed_paths.into_iter().collect::<Vec<_>>(),
            "objects": object_results
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}
