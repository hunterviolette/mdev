use crate::app::actions::Action;
use crate::app::state::{AppState, WORKTREE_REF};
use crate::capabilities::{CapabilityRequest, CapabilityResponse, FileSource};
use serde_json::{json, Value};

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
            sap.last_status = Some("Loading package objects".to_string());

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
                            "Package tree loaded but 0 objects were recognized from {} bytes of ADT XML. Inspect 'Package tree XML' to verify the response shape and cargo logs for the XML preview/exception details.",
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

            let metadata_uri = selected
                .as_ref()
                .map(|o| o.uri.clone())
                .unwrap_or_else(|| object_uri.clone());

            let metadata_accept = match selected.as_ref().map(|o| o.object_type.as_str()) {
                Some("PROG/P") => "application/vnd.sap.adt.programs.programs.v2+xml, application/xml, text/xml, */*",
                _ => "application/xml, text/xml, */*",
            };

            match crate::app::adt_bridge::read_object(sap, &metadata_uri, Some(metadata_accept)) {
                Ok(metadata_result) => {
                    let metadata_xml = metadata_result.body.clone();
                    let metadata_content_type = metadata_result.content_type.clone();

                    let resolved_source_uri = crate::app::adt_bridge::extract_object_source_uri(&metadata_xml)
                        .and_then(|source_uri| match source_uri {
                            Some(source_uri) => crate::app::adt_bridge::resolve_relative_object_uri(&metadata_uri, &source_uri),
                            None => Ok(selected
                                .as_ref()
                                .and_then(|o| o.source_uri.clone())
                                .unwrap_or_else(|| object_uri.clone())),
                        });

                    match resolved_source_uri.and_then(|source_uri| {
                        crate::app::adt_bridge::read_object(sap, &source_uri, Some("text/plain, text/*, */*"))
                    }) {
                        Ok(source_result) => {
                            let clone_target_path = default_clone_target_path(
                                selected.as_ref().map(|o| o.name.as_str()),
                                selected.as_ref().map(|o| o.object_type.as_str()),
                                selected.as_ref().and_then(|o| o.package_name.as_deref()),
                                source_result.content_type.as_deref(),
                            );
                            sap.selected_object_uri = Some(source_result.object_uri.clone());
                            sap.selected_object_metadata_uri = Some(metadata_uri.clone());
                            sap.selected_object_name = selected.as_ref().map(|o| o.name.clone());
                            sap.selected_object_type = selected.as_ref().map(|o| o.object_type.clone());
                            sap.selected_object_content = source_result.body;
                            sap.selected_object_content_type = source_result.content_type;
                            sap.selected_object_headers = source_result.headers;
                            sap.selected_object_metadata = metadata_xml;
                            sap.selected_object_metadata_content_type = metadata_content_type;
                            sap.clone_target_path = clone_target_path;
                            sap.last_status = Some("SAP object loaded".to_string());
                        }
                        Err(err) => {
                            sap.selected_object_uri = None;
                            sap.selected_object_metadata_uri = Some(metadata_uri.clone());
                            sap.selected_object_name = selected.as_ref().map(|o| o.name.clone());
                            sap.selected_object_type = selected.as_ref().map(|o| o.object_type.clone());
                            sap.selected_object_content.clear();
                            sap.selected_object_content_type = None;
                            sap.selected_object_headers.clear();
                            sap.selected_object_metadata = metadata_xml;
                            sap.selected_object_metadata_content_type = metadata_content_type;
                            sap.clone_target_path.clear();
                            sap.last_error = Some(format!("{:#}", err));
                            sap.last_status = Some("Object read failed".to_string());
                        }
                    }
                }
                Err(err) => {
                    sap.selected_object_uri = None;
                    sap.selected_object_metadata_uri = None;
                    sap.selected_object_content.clear();
                    sap.selected_object_content_type = None;
                    sap.selected_object_headers.clear();
                    sap.selected_object_metadata.clear();
                    sap.selected_object_metadata_content_type = None;
                    sap.clone_target_path.clear();
                    sap.last_error = Some(format!("{:#}", err));
                    sap.last_status = Some("Object read failed".to_string());
                }
            }

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

            let source_uri = sap.selected_object_uri.clone();
            let metadata_uri = sap.selected_object_metadata_uri.clone();
            let object_name = sap.selected_object_name.clone();
            let object_type = sap.selected_object_type.clone();
            let target_path = if sap.clone_target_path.trim().is_empty() {
                default_clone_target_path(
                    sap.selected_object_name.as_deref(),
                    sap.selected_object_type.as_deref(),
                    Some(sap.package_query.as_str()),
                    sap.selected_object_content_type.as_deref(),
                )
            } else {
                sap.clone_target_path.trim().replace('\\', "/")
            };
            let source_body = sap.selected_object_content.clone();
            let source_content_type = sap.selected_object_content_type.clone();
            let metadata_xml = sap.selected_object_metadata.clone();
            let metadata_content_type = sap.selected_object_metadata_content_type.clone();
            let headers = sap.selected_object_headers.clone();

            if source_uri.as_deref().unwrap_or_default().is_empty() {
                sap.last_error = Some("No SAP ADT object is currently loaded".to_string());
                sap.last_status = Some("Clone failed".to_string());
                return true;
            }

            let _ = state.broker.exec(CapabilityRequest::CreateWorktreeDir {
                repo,
                path: "sap_adt".to_string(),
            });

            let sidecar_path = format!("{}.adt.json", target_path);
            let etag = header_value(&headers, "etag");
            let sidecar = json!({
                "version": 1,
                "object_name": object_name,
                "object_type": object_type,
                "metadata_uri": metadata_uri,
                "source_uri": source_uri,
                "source_content_type": source_content_type,
                "metadata_content_type": metadata_content_type,
                "corr_nr": sap.corr_nr.trim(),
                "etag": etag,
                "headers": headers,
                "metadata_xml": metadata_xml
            });

            match write_worktree_bytes(state, &target_path, source_body.into_bytes())
                .and_then(|_| write_worktree_bytes(state, &sidecar_path, serde_json::to_vec_pretty(&sidecar)?))
            {
                Ok(()) => {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.clone_target_path = target_path.clone();
                        sap.last_error = None;
                        sap.last_status = Some(format!("Cloned SAP object to {}", target_path));
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

            let source_path = path.trim().replace('\\', "/");
            let sidecar_path = format!("{}.adt.json", source_path);

            let source_text = match read_worktree_text(state, &source_path) {
                Ok(text) => text,
                Err(err) => {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.last_error = Some(format!("{:#}", err));
                        sap.last_status = Some("Push failed".to_string());
                    }
                    return true;
                }
            };

            let sidecar_text = match read_worktree_text(state, &sidecar_path) {
                Ok(text) => text,
                Err(err) => {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.last_error = Some(format!("{:#}", err));
                        sap.last_status = Some("Push failed".to_string());
                    }
                    return true;
                }
            };

            let mut sidecar: Value = match serde_json::from_str(&sidecar_text) {
                Ok(value) => value,
                Err(err) => {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.last_error = Some(format!("Invalid sidecar JSON in {}: {}", sidecar_path, err));
                        sap.last_status = Some("Push failed".to_string());
                    }
                    return true;
                }
            };

            let source_uri = sidecar
                .get("source_uri")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let metadata_uri = sidecar
                .get("metadata_uri")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let lock_uri = if !metadata_uri.is_empty() {
                metadata_uri.clone()
            } else {
                source_uri.clone()
            };
            let content_type = sidecar
                .get("source_content_type")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
                .or_else(|| Some("text/plain; charset=utf-8".to_string()));
            let corr_nr = sidecar
                .get("corr_nr")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
                .or_else(|| {
                    state
                        .sap_adts
                        .get(sap_adt_id)
                        .map(|sap| sap.corr_nr.trim().to_string())
                        .filter(|s| !s.is_empty())
                });
            let if_match = sidecar
                .get("etag")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
                .or_else(|| {
                    let xml = sidecar.get("metadata_xml").and_then(|v| v.as_str()).unwrap_or_default();
                    let marker = "etag=\"";
                    xml.find(marker).and_then(|idx| {
                        let start = idx + marker.len();
                        xml[start..].find('"').map(|end| xml[start..start + end].to_string())
                    })
                });
            let lock_handle = sidecar
                .get("lock_handle")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
                .or_else(|| {
                    sidecar
                        .get("headers")
                        .and_then(|v| v.as_array())
                        .and_then(|arr| {
                            let headers = arr
                                .iter()
                                .filter_map(|item| {
                                    let pair = item.as_array()?;
                                    if pair.len() != 2 {
                                        return None;
                                    }
                                    Some((pair[0].as_str()?.to_string(), pair[1].as_str()?.to_string()))
                                })
                                .collect::<Vec<_>>();
                            header_value(&headers, "lock_handle")
                                .or_else(|| header_value(&headers, "lock-handle"))
                                .or_else(|| header_value(&headers, "x-lock-handle"))
                                .or_else(|| header_value(&headers, "x-lockhandle"))
                        })
                });

            if source_uri.is_empty() {
                if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                    sap.last_error = Some(format!("{} is missing source_uri", sidecar_path));
                    sap.last_status = Some("Push failed".to_string());
                }
                return true;
            }

            if lock_uri.is_empty() {
                if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                    sap.last_error = Some(format!("{} is missing metadata_uri/source_uri", sidecar_path));
                    sap.last_status = Some("Push failed".to_string());
                }
                return true;
            }

            {
                let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                    return true;
                };
                sap.last_error = None;
                sap.last_status = Some("Pushing local SAP ADT changes".to_string());
            }


            let mut effective_if_match = if_match.clone();
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
                        sidecar["etag"] = Value::String(server_etag);
                        if let Ok(text) = serde_json::to_string_pretty(&sidecar) {
                            let _ = write_worktree_bytes(state, &sidecar_path, text.into_bytes());
                        }
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

            match update_result {
                Ok(update_result) => {
                    if let Some(handle) = header_value(&update_result.headers, "lock_handle")
                        .or_else(|| header_value(&update_result.headers, "lock-handle"))
                        .or_else(|| header_value(&update_result.headers, "x-lock-handle"))
                        .or_else(|| header_value(&update_result.headers, "x-lockhandle"))
                    {
                        sidecar["lock_handle"] = Value::String(handle);
                    } else if let Some(handle) = lock_handle.clone() {
                        sidecar["lock_handle"] = Value::String(handle);
                    }
                    if let Some(etag) = header_value(&update_result.headers, "etag")
                        .or_else(|| effective_if_match.clone())
                    {
                        sidecar["etag"] = Value::String(etag);
                    }
                    sidecar["headers"] = Value::Array(
                        update_result
                            .headers
                            .iter()
                            .map(|(k, v)| Value::Array(vec![Value::String(k.clone()), Value::String(v.clone())]))
                            .collect(),
                    );

                    if let Ok(text) = serde_json::to_string_pretty(&sidecar) {
                        let _ = write_worktree_bytes(state, &sidecar_path, text.into_bytes());
                    }

                    let syntax_result = {
                        let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                            return true;
                        };
                        crate::app::adt_bridge::syntax_check(sap, &source_uri)
                    };

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
                            sap.last_status = Some(format!("Pushed to ADT and syntax checked{}", suffix));
                            sap.selected_object_uri = Some(source_uri.clone());
                            if !metadata_uri.is_empty() {
                                sap.selected_object_metadata_uri = Some(metadata_uri);
                            }
                            sap.selected_object_content = source_text;
                            sap.selected_object_headers = update_result.headers;
                        }
                        Err(err) => {
                            sap.last_error = Some(format!("{:#}", err));
                            sap.last_status = Some("Pushed to ADT but syntax check failed".to_string());
                        }
                    }
                }
                Err(err) => {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.last_error = Some(format!("{:#}", err));
                        sap.last_status = Some("Push failed".to_string());
                    }
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

            let source_path = path.trim().replace('\\', "/");
            let sidecar_path = format!("{}.adt.json", source_path);
            let sidecar_text = match read_worktree_text(state, &sidecar_path) {
                Ok(text) => text,
                Err(err) => {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.last_error = Some(format!("{:#}", err));
                        sap.last_status = Some("Activate failed".to_string());
                    }
                    return true;
                }
            };

            let sidecar: Value = match serde_json::from_str(&sidecar_text) {
                Ok(value) => value,
                Err(err) => {
                    if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                        sap.last_error = Some(format!("Invalid sidecar JSON in {}: {}", sidecar_path, err));
                        sap.last_status = Some("Activate failed".to_string());
                    }
                    return true;
                }
            };

            let activate_uri = sidecar
                .get("metadata_uri")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .or_else(|| sidecar.get("source_uri").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .trim()
                .to_string();

            if activate_uri.is_empty() {
                if let Some(sap) = state.sap_adts.get_mut(sap_adt_id) {
                    sap.last_error = Some(format!("{} is missing metadata_uri/source_uri", sidecar_path));
                    sap.last_status = Some("Activate failed".to_string());
                }
                return true;
            }

            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };

            sap.last_error = None;
            sap.last_status = Some("Activating SAP object".to_string());

            match crate::app::adt_bridge::activate_object(sap, &activate_uri) {
                Ok(result) => {
                    let suffix = if result.body.trim().is_empty() {
                        String::new()
                    } else {
                        format!(": {}", result.body.trim())
                    };
                    sap.last_error = None;
                    sap.last_status = Some(format!("Activated SAP object{}", suffix));
                }
                Err(err) => {
                    sap.last_error = Some(format!("{:#}", err));
                    sap.last_status = Some("Activate failed".to_string());
                }
            }

            true
        }
        _ => false,
    }
}
