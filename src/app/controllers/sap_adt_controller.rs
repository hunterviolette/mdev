use crate::app::actions::Action;
use crate::app::sap_adt_manifest::SapAdtObjectManifest;
use crate::app::state::{AppState, WORKTREE_REF};
use crate::capabilities::{CapabilityRequest, CapabilityResponse, FileSource};

fn normalize_bridge_dir(raw: &str) -> String {
    let trimmed = raw.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed);
    unquoted.replace('/', std::path::MAIN_SEPARATOR_STR)
}

fn resolve_bridge_dir(preferred: &str) -> String {
    let normalized = normalize_bridge_dir(preferred);
    if !normalized.is_empty() && std::path::Path::new(&normalized).is_dir() {
        return normalized;
    }
    normalized
}

fn sanitize_segment(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "object".to_string()
    } else {
        trimmed.to_string()
    }
}

fn default_extension_for(object_type: Option<&str>, content_type: Option<&str>) -> &'static str {
    match object_type.unwrap_or_default() {
        "PROG/P" => "abap",
        _ => {
            let ct = content_type.unwrap_or_default().to_ascii_lowercase();
            if ct.contains("xml") {
                "xml"
            } else if ct.contains("json") {
                "json"
            } else {
                "txt"
            }
        }
    }
}

fn default_clone_target_path(
    object_name: Option<&str>,
    object_type: Option<&str>,
    package_name: Option<&str>,
    content_type: Option<&str>,
) -> String {
    let package = sanitize_segment(package_name.unwrap_or("package"));
    let object_type = sanitize_segment(object_type.unwrap_or("OBJECT"));
    let object_name = sanitize_segment(object_name.unwrap_or("object"));
    let ext = default_extension_for(object_type.as_str().into(), content_type);
    format!("sap_adt/{}__{}__{}.{}", package, object_type, object_name, ext)
}

fn normalize_repo_relative_path(path: &str) -> String {
    path.trim().replace('\\', "/").trim_end_matches('/').to_string()
}

fn manifest_candidate_paths(path: &str) -> Vec<String> {
    let normalized = normalize_repo_relative_path(path);
    let mut out = Vec::new();

    let push_candidate = |out: &mut Vec<String>, candidate: String| {
        if !candidate.trim().is_empty() && !out.iter().any(|existing| existing == &candidate) {
            out.push(candidate);
        }
    };

    if normalized.ends_with("/manifest.adt.json") || normalized == "manifest.adt.json" {
        push_candidate(&mut out, normalized.clone());
    } else if !normalized.is_empty() {
        push_candidate(&mut out, format!("{}/manifest.adt.json", normalized));
    }

    let mut cursor = std::path::Path::new(&normalized);
    while let Some(parent) = cursor.parent() {
        let candidate = parent.join("manifest.adt.json").to_string_lossy().replace('\\', "/");
        push_candidate(&mut out, candidate);
        cursor = parent;
        if parent.as_os_str().is_empty() {
            break;
        }
    }

    push_candidate(&mut out, "manifest.adt.json".to_string());
    out
}

fn load_manifest_for_local_path(
    state: &mut AppState,
    path: &str,
) -> anyhow::Result<(String, SapAdtObjectManifest, usize, String)> {
    let original_path = normalize_repo_relative_path(path);
    let mut last_err: Option<String> = None;

    for manifest_path in manifest_candidate_paths(&original_path) {
        match read_worktree_text(state, &manifest_path) {
            Ok(text) => {
                let manifest: SapAdtObjectManifest = serde_json::from_str(&text)
                    .map_err(|err| anyhow::anyhow!("Invalid manifest JSON in {}: {}", manifest_path, err))?;

                let manifest_dir = manifest_path
                    .strip_suffix("/manifest.adt.json")
                    .unwrap_or("")
                    .trim_matches('/')
                    .to_string();

                let relative_path = if original_path == manifest_path || original_path == manifest_dir {
                    None
                } else if manifest_dir.is_empty() {
                    Some(original_path.as_str())
                } else {
                    original_path.strip_prefix(&(manifest_dir.clone() + "/"))
                };

                let resource_index = if let Some(rel) = relative_path {
                    if rel.is_empty() {
                        None
                    } else {
                        manifest.resources.iter().position(|resource| resource.path == rel)
                    }
                } else {
                    None
                }
                .or_else(|| {
                    manifest
                        .primary_resource_id()
                        .and_then(|id| manifest.resources.iter().position(|resource| resource.id == id))
                })
                .ok_or_else(|| anyhow::anyhow!("Manifest {} does not contain a pushable resource", manifest_path))?;

                let resource_rel_path = manifest
                    .resources
                    .get(resource_index)
                    .map(|resource| resource.path.clone())
                    .ok_or_else(|| anyhow::anyhow!("Manifest {} resource index out of range", manifest_path))?;

                let resource_local_path = if manifest_dir.is_empty() {
                    resource_rel_path
                } else {
                    format!("{}/{}", manifest_dir, resource_rel_path)
                };

                return Ok((manifest_path, manifest, resource_index, resource_local_path));
            }
            Err(err) => {
                last_err = Some(format!("{:#}", err));
            }
        }
    }

    Err(anyhow::anyhow!(
        "Could not locate manifest.adt.json for {}{}",
        original_path,
        last_err
            .as_deref()
            .map(|err| format!(" ({})", err))
            .unwrap_or_default()
    ))
}

fn write_manifest_to_worktree(
    state: &mut AppState,
    manifest_path: &str,
    manifest: &SapAdtObjectManifest,
) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest)?;
    write_worktree_bytes(state, manifest_path, bytes)
}

fn write_worktree_bytes(state: &mut AppState, path: &str, contents: Vec<u8>) -> anyhow::Result<()> {
    let repo = state
        .inputs
        .repo
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Local repository is not set"))?;

    match state.broker.exec(CapabilityRequest::WriteWorktreeFile {
        repo,
        path: path.to_string(),
        contents,
    }) {
        Ok(CapabilityResponse::Unit) => Ok(()),
        Ok(other) => Err(anyhow::anyhow!("Unexpected capability response: {:?}", other)),
        Err(err) => Err(err),
    }
}

fn read_worktree_text(state: &mut AppState, path: &str) -> anyhow::Result<String> {
    let repo = state
        .inputs
        .repo
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Local repository is not set"))?;

    let resp = state.broker.exec(CapabilityRequest::ReadFile {
        repo,
        path: path.to_string(),
        source: FileSource::Worktree,
    })?;

    let CapabilityResponse::Bytes(bytes) = resp else {
        return Err(anyhow::anyhow!("Unexpected capability response reading {}", path));
    };

    String::from_utf8(bytes).map_err(|e| anyhow::anyhow!("{} is not valid UTF-8: {}", path, e))
}

fn header_value(headers: &[(String, String)], key: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
}

fn extract_server_etag_from_adt_error(body: &str) -> Option<String> {
    let marker = "object ETag ";
    let start = body.find(marker)? + marker.len();
    let rest = &body[start..];
    let end = rest.find(' ')
        .or_else(|| rest.find('<'))
        .unwrap_or(rest.len());
    let value = rest[..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn manifest_dir_from_manifest_path(manifest_path: &str) -> String {
    manifest_path
        .strip_suffix("/manifest.adt.json")
        .unwrap_or("")
        .trim_matches('/')
        .to_string()
}

fn join_manifest_relative_path(manifest_dir: &str, relative_path: &str) -> String {
    let relative_path = relative_path.trim().trim_start_matches('/');
    if manifest_dir.is_empty() {
        relative_path.to_string()
    } else if relative_path.is_empty() {
        manifest_dir.to_string()
    } else {
        format!("{}/{}", manifest_dir, relative_path)
    }
}

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::SapAdtConnect { sap_adt_id } => {
            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };

            sap.last_error = None;
            sap.last_status = Some("Connecting".to_string());

            match crate::app::adt_bridge::connect(sap) {
                Ok(()) => {
                    sap.connected = sap.discovery.as_ref().map(|d| d.enabled).unwrap_or(false);
                    if sap.last_status.is_none() {
                        sap.last_status = Some("Connected".to_string());
                    }
                }
                Err(err) => {
                    sap.connected = false;
                    sap.discovery = None;
                    sap.adt_session_id = None;
                    sap.last_error = Some(format!("{:#}", err));
                    sap.last_status = Some("Connection failed".to_string());
                }
            }

            true
        }
        Action::SapAdtLoadPackage { sap_adt_id } => {
            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };

            sap.last_error = None;
            sap.last_status = Some("Loading package tree".to_string());

            let package_name = sap.package_query.trim().to_string();
            let include_subpackages = sap.include_subpackages;

            match crate::app::adt_bridge::list_package_objects(sap, &package_name, include_subpackages)
                .and_then(|xml| {
                    let objects = crate::app::adt_bridge::parse_package_tree_xml(&xml)?;
                    Ok((xml, objects))
                }) {
                Ok((xml, objects)) => {
                    let object_count = objects.len();
                    let xml_len = xml.len();
                    eprintln!(
                        "[sap_adt] controller package load success package={} include_subpackages={} objects={} xml_bytes={}",
                        package_name,
                        include_subpackages,
                        object_count,
                        xml_len
                    );
                    sap.package_tree_xml = xml;
                    sap.package_objects = objects;
                    sap.selected_object_uri = None;
                    sap.selected_object_metadata_uri = None;
                    sap.selected_object_name = None;
                    sap.selected_object_type = None;
                    sap.selected_object_content.clear();
                    sap.selected_object_content_type = None;
                    sap.selected_object_headers.clear();
                    sap.selected_object_metadata.clear();
                    sap.selected_object_metadata_content_type = None;
                    sap.clone_target_path.clear();
                    if object_count == 0 {
                        sap.last_error = Some(format!(
                            "Package tree loaded but 0 objects were recognized from {} bytes of ADT XML. The package tree endpoint may have returned an empty tree, or the response shape may need parser support. Inspect 'Package tree XML' to verify the response shape and cargo logs for the XML preview/exception details.",
                            xml_len
                        ));
                        sap.last_status = Some(format!(
                            "Loaded package {} (0 recognized objects)",
                            package_name
                        ));
                    } else {
                        sap.last_error = None;
                        sap.last_status = Some(format!(
                            "Loaded package {} ({} objects)",
                            package_name,
                            object_count
                        ));
                    }
                }
                Err(err) => {
                    eprintln!(
                        "[sap_adt] controller package load failed package={} include_subpackages={} error={:#}",
                        package_name,
                        include_subpackages,
                        err
                    );
                    sap.last_error = Some(format!("{:#}", err));
                    sap.last_status = Some("Package load failed".to_string());
                }
            }

            true
        }
        Action::SapAdtReadObject {
            sap_adt_id,
            object_uri,
        } => {
            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };

            sap.last_error = None;
            sap.last_status = Some("Reading SAP object".to_string());

            let selected = sap
                .package_objects
                .iter()
                .find(|o| o.uri == *object_uri || o.source_uri.as_deref() == Some(object_uri.as_str()))
                .cloned();

            if let Some(selected) = selected.as_ref() {
                let is_package_node = selected.object_type == "DEVC/K"
                    || selected.uri.starts_with("/sap/bc/adt/packages/");
                if is_package_node {
                    sap.selected_object_uri = None;
                    sap.selected_object_metadata_uri = Some(selected.uri.clone());
                    sap.selected_object_name = Some(selected.name.clone());
                    sap.selected_object_type = Some(selected.object_type.clone());
                    sap.selected_object_content.clear();
                    sap.selected_object_content_type = None;
                    sap.selected_object_headers.clear();
                    sap.selected_object_metadata.clear();
                    sap.selected_object_metadata_content_type = None;
                    sap.selected_manifest = None;
                    sap.selected_resource_id = None;
                    sap.clone_target_path.clear();
                    sap.last_error = Some("Package nodes are browse-only until package-content expansion is resolved.".to_string());
                    sap.last_status = Some("Package selected".to_string());
                    return true;
                }
            }

            if let Some(selected) = selected.as_ref() {
                let is_package_node = selected.object_type == "DEVC/K"
                    || selected.uri.starts_with("/sap/bc/adt/packages/");
                if is_package_node {
                    sap.selected_object_uri = None;
                    sap.selected_object_metadata_uri = Some(selected.uri.clone());
                    sap.selected_object_name = Some(selected.name.clone());
                    sap.selected_object_type = Some(selected.object_type.clone());
                    sap.selected_object_content.clear();
                    sap.selected_object_content_type = None;
                    sap.selected_object_headers.clear();
                    sap.selected_object_metadata.clear();
                    sap.selected_object_metadata_content_type = None;
                    sap.selected_manifest = None;
                    sap.selected_resource_id = None;
                    sap.clone_target_path.clear();
                    sap.last_error = Some("Package nodes are browse-only until package-content expansion is resolved.".to_string());
                    sap.last_status = Some("Package selected".to_string());
                    return true;
                }
            }

            if let Some(selected) = selected.as_ref() {
                let uri = selected.uri.as_str();
                let is_real_leaf_uri = uri.starts_with("/sap/bc/adt/programs/")
                    || uri.starts_with("/sap/bc/adt/oo/classes/")
                    || uri.starts_with("/sap/bc/adt/oo/interfaces/")
                    || uri.starts_with("/sap/bc/adt/ddic/")
                    || uri.starts_with("/sap/bc/adt/ddls/")
                    || uri.starts_with("/sap/bc/adt/cds/")
                    || uri.starts_with("/sap/bc/adt/functions/")
                    || uri.starts_with("/sap/bc/adt/vit/wb/object_type/");

                let is_structural_node = !is_real_leaf_uri
                    && (selected.object_type.starts_with("DEVC/")
                        || uri.starts_with("/sap/bc/adt/packages/")
                        || uri.trim().is_empty());

                if is_structural_node {
                    sap.selected_object_uri = None;
                    sap.selected_object_metadata_uri = Some(selected.uri.clone());
                    sap.selected_object_name = Some(selected.name.clone());
                    sap.selected_object_type = Some(selected.object_type.clone());
                    sap.selected_object_content.clear();
                    sap.selected_object_content_type = None;
                    sap.selected_object_headers.clear();
                    sap.selected_object_metadata.clear();
                    sap.selected_object_metadata_content_type = None;
                    sap.selected_manifest = None;
                    sap.selected_resource_id = None;
                    sap.clone_target_path.clear();
                    sap.last_error = Some("Structural package nodes are browse-only. Select a leaf object to import it.".to_string());
                    sap.last_status = Some("Package node selected".to_string());
                    return true;
                }
            }

            let metadata_uri = selected
                .as_ref()
                .and_then(|o| o.source_uri.clone().or_else(|| Some(o.uri.clone())))
                .unwrap_or_else(|| object_uri.clone());

            match crate::app::adt_bridge::crawl_object_manifest(
                sap,
                &metadata_uri,
                selected.as_ref().map(|o| o.name.as_str()),
                selected.as_ref().map(|o| o.object_type.as_str()),
                selected.as_ref().and_then(|o| o.package_name.as_deref()),
            ) {
                Ok(manifest) => {
                    let selected_resource_id = manifest.primary_resource_id();
                    let clone_target_path = crate::app::adt_bridge::manifest_directory_name(
                        manifest.object_name.as_deref(),
                        manifest.object_type.as_deref(),
                        manifest.package_name.as_deref(),
                    );

                    let selected_resource = selected_resource_id
                        .as_deref()
                        .and_then(|id| manifest.resources.iter().find(|resource| resource.id == id))
                        .or_else(|| manifest.resources.first());

                    sap.selected_object_uri = selected_resource.map(|resource| resource.uri.clone());
                    sap.selected_object_metadata_uri = Some(manifest.metadata_uri.clone());
                    sap.selected_object_name = selected
                        .as_ref()
                        .map(|o| o.name.clone())
                        .or_else(|| manifest.object_name.clone());
                    sap.selected_object_type = selected
                        .as_ref()
                        .map(|o| o.object_type.clone())
                        .or_else(|| manifest.object_type.clone());
                    sap.selected_object_content = selected_resource
                        .map(|resource| resource.body.clone())
                        .unwrap_or_default();
                    sap.selected_object_content_type = selected_resource
                        .and_then(|resource| resource.content_type.clone());
                    sap.selected_object_headers = selected_resource
                        .map(|resource| resource.headers.clone())
                        .unwrap_or_default();
                    sap.selected_object_metadata = manifest.metadata_xml.clone();
                    sap.selected_object_metadata_content_type = Some("application/xml".to_string());
                    sap.selected_manifest = Some(manifest);
                    sap.selected_resource_id = selected_resource_id;
                    sap.clone_target_path = clone_target_path;
                    sap.last_error = None;
                    sap.last_status = Some("SAP object loaded".to_string());
                }
                Err(err) => {
                    sap.selected_object_uri = None;
                    sap.selected_object_metadata_uri = None;
                    sap.selected_object_name = selected.as_ref().map(|o| o.name.clone());
                    sap.selected_object_type = selected.as_ref().map(|o| o.object_type.clone());
                    sap.selected_object_content.clear();
                    sap.selected_object_content_type = None;
                    sap.selected_object_headers.clear();
                    sap.selected_object_metadata.clear();
                    sap.selected_object_metadata_content_type = None;
                    sap.selected_manifest = None;
                    sap.selected_resource_id = None;
                    sap.clone_target_path.clear();
                    sap.last_error = Some(format!("{:#}", err));
                    sap.last_status = Some("Object read failed".to_string());
                }
            }

            true
        }

        Action::SapAdtDebugAcceptMatrix {
            sap_adt_id,
            object_uri,
        } => {
            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };

            sap.last_error = None;
            sap.last_status = Some("Running ADT Accept matrix".to_string());
            sap.accept_probe_results.clear();

            match crate::app::adt_bridge::debug_accept_matrix(sap, object_uri) {
                Ok(results) => {
                    let failures = results.iter().filter(|r| r.error.is_some()).count();
                    sap.accept_probe_results = results;
                    sap.last_error = None;
                    sap.last_status = Some(format!(
                        "ADT Accept matrix complete: {} probe(s), {} failure(s)",
                        sap.accept_probe_results.len(),
                        failures
                    ));
                }
                Err(err) => {
                    sap.accept_probe_results.clear();
                    sap.last_error = Some(format!("{:#}", err));
                    sap.last_status = Some("ADT Accept matrix failed".to_string());
                }
            }

            true
        }
        Action::SapAdtClearHttpTrace { sap_adt_id } => {
            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };
            sap.last_http_trace = None;
            sap.accept_probe_results.clear();
            sap.last_error = None;
            sap.last_status = Some("ADT debug trace cleared".to_string());
            true
        }
        Action::SapAdtCloneSelectedToWorktree { sap_adt_id } => {
            if state.inputs.git_ref != WORKTREE_REF {
                if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                    sap.last_error = Some("Switch to WORKTREE before cloning SAP ADT objects locally".to_string());
                    sap.last_status = Some("Clone blocked".to_string());
                }
                return true;
            }

            let Some(repo) = state.inputs.repo.clone() else {
                if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                    sap.last_error = Some("Local repository is not set".to_string());
                    sap.last_status = Some("Clone failed".to_string());
                }
                return true;
            };

            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };

            let Some(mut manifest) = sap.selected_manifest.clone() else {
                sap.last_error = Some("No SAP ADT manifest is currently loaded".to_string());
                sap.last_status = Some("Clone failed".to_string());
                return true;
            };

            if manifest.schema_version == 0 {
                manifest.schema_version = 1;
            }

            let manifest_dir = if sap.clone_target_path.trim().is_empty() {
                crate::app::adt_bridge::manifest_directory_name(
                    manifest.object_name.as_deref().or(sap.selected_object_name.as_deref()),
                    manifest.object_type.as_deref().or(sap.selected_object_type.as_deref()),
                    manifest.package_name.as_deref(),
                )
            } else {
                sap.clone_target_path.trim().replace('\\', "/")
            };
            let manifest_dir = manifest_dir.trim().trim_end_matches('/').to_string();
            let manifest_path = format!("{}/manifest.adt.json", manifest_dir);

            let _ = state.broker.exec(CapabilityRequest::CreateWorktreeDir {
                repo,
                path: "sap_adt".to_string(),
            });

            let mut writes: Vec<(String, Vec<u8>)> = Vec::new();
            for doc in manifest.documents.iter() {
                let rel = doc.path.trim().trim_start_matches('/');
                if !rel.is_empty() {
                    writes.push((format!("{}/{}", manifest_dir, rel), doc.body.clone().into_bytes()));
                }
            }
            for resource in manifest.resources.iter() {
                let rel = resource.path.trim().trim_start_matches('/');
                if !rel.is_empty() {
                    writes.push((format!("{}/{}", manifest_dir, rel), resource.body.clone().into_bytes()));
                }
            }

            let manifest_bytes = match serde_json::to_vec_pretty(&manifest) {
                Ok(bytes) => bytes,
                Err(err) => {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.last_error = Some(format!("{:#}", err));
                        sap.last_status = Some("Clone failed".to_string());
                    }
                    return true;
                }
            };
            writes.push((manifest_path.clone(), manifest_bytes));

            let mut result = Ok(());
            for (path, bytes) in writes {
                if let Err(err) = write_worktree_bytes(state, &path, bytes) {
                    result = Err(err);
                    break;
                }
            }

            match result {
                Ok(()) => {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.clone_target_path = manifest_dir.clone();
                        sap.last_error = None;
                        sap.last_status = Some(format!(
                            "Cloned SAP manifest to {} ({} resources, {} documents)",
                            manifest_dir,
                            manifest.resources.len(),
                            manifest.documents.len()
                        ));
                    }
                    state.start_analysis_refresh_async();
                }
                Err(err) => {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.last_error = Some(format!("{:#}", err));
                        sap.last_status = Some("Clone failed".to_string());
                    }
                }
            }

            true
        }
        Action::SapAdtPushWorktreeToAdt { sap_adt_id, path } => {
            if state.inputs.git_ref != WORKTREE_REF {
                if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                    sap.last_error = Some("Switch to WORKTREE before pushing SAP ADT changes".to_string());
                    sap.last_status = Some("Push blocked".to_string());
                }
                return true;
            }

            let (manifest_path, mut manifest, resource_index, _) = match load_manifest_for_local_path(state, path) {
                Ok(value) => value,
                Err(err) => {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.last_error = Some(format!("{:#}", err));
                        sap.last_status = Some("Push failed".to_string());
                    }
                    return true;
                }
            };

            let manifest_dir = manifest_dir_from_manifest_path(&manifest_path);
            let requested_path = normalize_repo_relative_path(path);
            let requested_relative_path = if requested_path == manifest_path || requested_path == manifest_dir {
                None
            } else if manifest_dir.is_empty() {
                Some(requested_path.as_str())
            } else {
                requested_path.strip_prefix(&(manifest_dir.clone() + "/"))
            };

            let requested_resource_index = requested_relative_path
                .and_then(|rel| manifest.resources.iter().position(|resource| resource.path == rel && resource.editable))
                .or_else(|| manifest.resources.get(resource_index).and_then(|resource| {
                    if resource.editable {
                        Some(resource_index)
                    } else {
                        None
                    }
                }));

            let metadata_uri = manifest.metadata_uri.trim().to_string();
            let object_name = manifest.object_name.clone();
            let object_type = manifest.object_type.clone();
            let package_name = manifest.package_name.clone();
            let clone_target_path = manifest_dir.clone();

            let mut pending_updates: Vec<(usize, String)> = Vec::new();

            if let Some(index) = requested_resource_index {
                let Some(resource) = manifest.resources.get(index) else {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.last_error = Some("Manifest resource index is out of range".to_string());
                        sap.last_status = Some("Push failed".to_string());
                    }
                    return true;
                };
                let local_path = join_manifest_relative_path(&manifest_dir, &resource.path);
                let source_text = match read_worktree_text(state, &local_path) {
                    Ok(text) => text,
                    Err(err) => {
                        if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                            sap.last_error = Some(format!("{:#}", err));
                            sap.last_status = Some("Push failed".to_string());
                        }
                        return true;
                    }
                };
                pending_updates.push((index, source_text));
            }

            for index in 0..manifest.resources.len() {
                let Some(resource) = manifest.resources.get(index) else {
                    continue;
                };
                if !resource.editable || resource.uri.trim().is_empty() {
                    continue;
                }
                if pending_updates.iter().any(|(pending_index, _)| *pending_index == index) {
                    continue;
                }
                let local_path = join_manifest_relative_path(&manifest_dir, &resource.path);
                let source_text = match read_worktree_text(state, &local_path) {
                    Ok(text) => text,
                    Err(err) => {
                        if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                            sap.last_error = Some(format!("{:#}", err));
                            sap.last_status = Some("Push failed".to_string());
                        }
                        return true;
                    }
                };
                if source_text != resource.body {
                    pending_updates.push((index, source_text));
                }
            }

            if pending_updates.is_empty() {
                if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                    sap.last_error = None;
                    sap.last_status = Some("No local SAP ADT changes to push".to_string());
                }
                return true;
            }

            {
                let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                    return true;
                };
                sap.last_error = None;
                sap.last_status = Some(format!("Pushing {} local SAP ADT resource(s)", pending_updates.len()));
            }

            let corr_nr = state
                .sap_adts
                .get(sap_adt_id)
                .map(|sap| sap.corr_nr.trim().to_string())
                .filter(|value| !value.is_empty());

            let mut pushed_resource_uris: Vec<String> = Vec::new();

            for (current_index, source_text) in pending_updates {
                let (source_uri, content_type, lock_handle, fallback_etag) = match manifest.resources.get(current_index) {
                    Some(resource) => (
                        resource.uri.trim().to_string(),
                        resource
                            .content_type
                            .clone()
                            .or_else(|| Some("text/plain; charset=utf-8".to_string())),
                        resource.lock_handle.clone(),
                        resource.etag.clone().or_else(|| manifest.etag.clone()),
                    ),
                    None => {
                        if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                            sap.last_error = Some("Manifest resource index is out of range".to_string());
                            sap.last_status = Some("Push failed".to_string());
                        }
                        return true;
                    }
                };

                if source_uri.is_empty() {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.last_error = Some(format!("{} has a manifest resource with no uri", manifest_path));
                        sap.last_status = Some("Push failed".to_string());
                    }
                    return true;
                }

                let mut effective_if_match = fallback_etag.clone();
                let mut update_result = {
                    let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                        return true;
                    };
                    crate::app::adt_bridge::update_object(
                        sap,
                        &source_uri,
                        &source_text,
                        content_type.as_deref(),
                        lock_handle.as_deref(),
                        corr_nr.as_deref(),
                        effective_if_match.as_deref(),
                    )
                };

                if let Err(err) = &update_result {
                    let err_text = format!("{:#}", err);
                    if err_text.contains("412") {
                        if let Some(server_etag) = extract_server_etag_from_adt_error(&err_text) {
                            effective_if_match = Some(server_etag.clone());
                            if let Some(resource) = manifest.resources.get_mut(current_index) {
                                resource.etag = Some(server_etag.clone());
                            }
                            manifest.etag = Some(server_etag);
                            let _ = write_manifest_to_worktree(state, &manifest_path, &manifest);
                            update_result = {
                                let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                                    return true;
                                };
                                crate::app::adt_bridge::update_object(
                                    sap,
                                    &source_uri,
                                    &source_text,
                                    content_type.as_deref(),
                                    lock_handle.as_deref(),
                                    corr_nr.as_deref(),
                                    effective_if_match.as_deref(),
                                )
                            };
                        }
                    }
                }

                let update_result = match update_result {
                    Ok(value) => value,
                    Err(err) => {
                        if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                            sap.last_error = Some(format!("{:#}", err));
                            sap.last_status = Some("Push failed".to_string());
                        }
                        return true;
                    }
                };

                if let Some(resource) = manifest.resources.get_mut(current_index) {
                    resource.body = source_text;
                    resource.headers = update_result.headers.clone();
                    if let Some(handle) = header_value(&update_result.headers, "lock_handle")
                        .or_else(|| header_value(&update_result.headers, "lock-handle"))
                        .or_else(|| header_value(&update_result.headers, "x-lock-handle"))
                        .or_else(|| header_value(&update_result.headers, "x-lockhandle"))
                        .or_else(|| lock_handle.clone())
                    {
                        resource.lock_handle = Some(handle);
                    }
                    if let Some(etag) = header_value(&update_result.headers, "etag")
                        .or_else(|| effective_if_match.clone())
                    {
                        resource.etag = Some(etag.clone());
                        manifest.etag = Some(etag);
                    }
                }

                pushed_resource_uris.push(source_uri);
            }

            if let Err(err) = write_manifest_to_worktree(state, &manifest_path, &manifest) {
                if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                    sap.last_error = Some(format!("Push succeeded, but manifest write failed: {:#}", err));
                    sap.last_status = Some("Push incomplete".to_string());
                }
                return true;
            }

            let check_uri = if !metadata_uri.is_empty() {
                metadata_uri.clone()
            } else {
                pushed_resource_uris.first().cloned().unwrap_or_default()
            };

            let syntax_result = {
                let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                    return true;
                };
                crate::app::adt_bridge::syntax_check(sap, &check_uri)
            };

            let refresh_result = {
                let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                    return true;
                };
                crate::app::adt_bridge::crawl_object_manifest(
                    sap,
                    &check_uri,
                    object_name.as_deref(),
                    object_type.as_deref(),
                    package_name.as_deref(),
                )
            };

            let selection_uri = requested_resource_index
                .and_then(|index| manifest.resources.get(index))
                .map(|resource| resource.uri.clone())
                .filter(|uri| !uri.trim().is_empty())
                .or_else(|| pushed_resource_uris.last().cloned());

            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };

            match syntax_result {
                Ok(check) => {
                    sap.last_error = None;
                    let suffix = if check.body.trim().is_empty() {
                        String::new()
                    } else {
                        format!(": {}", check.body.trim())
                    };
                    sap.last_status = Some(format!(
                        "Pushed {} SAP ADT resource(s) and syntax checked{}",
                        pushed_resource_uris.len(),
                        suffix
                    ));

                    match refresh_result {
                        Ok(refreshed) => {
                            let selected_resource_id = selection_uri
                                .as_deref()
                                .and_then(|uri| refreshed.resources.iter().find(|resource| resource.uri == uri))
                                .map(|resource| resource.id.clone())
                                .or_else(|| refreshed.primary_resource_id());

                            let selected_resource = selected_resource_id
                                .as_deref()
                                .and_then(|id| refreshed.resources.iter().find(|resource| resource.id == id))
                                .or_else(|| refreshed.resources.first());

                            sap.selected_object_uri = selected_resource.map(|resource| resource.uri.clone());
                            sap.selected_object_metadata_uri = Some(refreshed.metadata_uri.clone());
                            sap.selected_object_name = refreshed.object_name.clone().or_else(|| object_name.clone());
                            sap.selected_object_type = refreshed.object_type.clone().or_else(|| object_type.clone());
                            sap.selected_object_content = selected_resource
                                .map(|resource| resource.body.clone())
                                .unwrap_or_default();
                            sap.selected_object_content_type = selected_resource
                                .and_then(|resource| resource.content_type.clone());
                            sap.selected_object_headers = selected_resource
                                .map(|resource| resource.headers.clone())
                                .unwrap_or_default();
                            sap.selected_object_metadata = refreshed.metadata_xml.clone();
                            sap.selected_object_metadata_content_type = Some("application/xml".to_string());
                            sap.selected_manifest = Some(refreshed);
                            sap.selected_resource_id = selected_resource_id;
                            sap.clone_target_path = clone_target_path;
                        }
                        Err(err) => {
                            sap.last_error = Some(format!("Push succeeded, but manifest refresh failed: {:#}", err));
                        }
                    }
                }
                Err(err) => {
                    sap.last_error = Some(format!("{:#}", err));
                    sap.last_status = Some("Pushed to ADT but syntax check failed".to_string());
                }
            }

            true
        }
        Action::SapAdtActivateWorktreeObject { sap_adt_id, path } => {
            if state.inputs.git_ref != WORKTREE_REF {
                if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                    sap.last_error = Some("Switch to WORKTREE before activating SAP ADT objects".to_string());
                    sap.last_status = Some("Activate blocked".to_string());
                }
                return true;
            }

            let (manifest_path, manifest, resource_index, _) = match load_manifest_for_local_path(state, path) {
                Ok(value) => value,
                Err(err) => {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.last_error = Some(format!("{:#}", err));
                        sap.last_status = Some("Activate failed".to_string());
                    }
                    return true;
                }
            };

            let object_name = manifest.object_name.clone();
            let object_type = manifest.object_type.clone();
            let package_name = manifest.package_name.clone();
            let fallback_source_uri = manifest
                .resources
                .get(resource_index)
                .map(|resource| resource.uri.clone())
                .unwrap_or_default();
            let activate_uri = if !manifest.metadata_uri.trim().is_empty() {
                manifest.metadata_uri.trim().to_string()
            } else {
                fallback_source_uri.clone()
            };

            if activate_uri.is_empty() {
                if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                    sap.last_error = Some(format!("{} is missing metadata_uri/source_uri", manifest_path));
                    sap.last_status = Some("Activate failed".to_string());
                }
                return true;
            }

            {
                let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                    return true;
                };
                sap.last_error = None;
                sap.last_status = Some("Activating SAP object".to_string());
            }

            let activate_result = {
                let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                    return true;
                };
                crate::app::adt_bridge::activate_object(sap, &activate_uri)
            };

            match activate_result {
                Ok(result) => {
                    let suffix = if result.body.trim().is_empty() {
                        String::new()
                    } else {
                        format!(": {}", result.body.trim())
                    };

                    let refresh_result = {
                        let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                            return true;
                        };
                        crate::app::adt_bridge::crawl_object_manifest(
                            sap,
                            &activate_uri,
                            object_name.as_deref(),
                            object_type.as_deref(),
                            package_name.as_deref(),
                        )
                    };

                    let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                        return true;
                    };
                    sap.last_error = None;
                    sap.last_status = Some(format!("Activated SAP object{}", suffix));

                    if let Ok(refreshed) = refresh_result {
                        let selected_resource_id = refreshed
                            .resources
                            .iter()
                            .find(|resource| resource.uri == fallback_source_uri)
                            .map(|resource| resource.id.clone())
                            .or_else(|| refreshed.primary_resource_id());
                        sap.selected_object_name = refreshed.object_name.clone().or_else(|| object_name.clone());
                        sap.selected_object_type = refreshed.object_type.clone().or_else(|| object_type.clone());
                        sap.selected_manifest = Some(refreshed);
                        sap.selected_resource_id = selected_resource_id;
                        sap.clone_target_path = manifest_path
                            .strip_suffix("/manifest.adt.json")
                            .unwrap_or("")
                            .to_string();
                    }
                }
                Err(err) => {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.last_error = Some(format!("{:#}", err));
                        sap.last_status = Some("Activate failed".to_string());
                    }
                }
            }

            true
        }
        _ => false,
    }
}
