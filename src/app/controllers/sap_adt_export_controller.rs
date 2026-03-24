use anyhow::{anyhow, Result};

use crate::app::actions::ComponentId;
use crate::app::sap_adt_manifest::{SapAdtManifestResource, SapAdtObjectManifest};
use crate::app::state::{AppState, SapAdtExportRow, WORKTREE_REF};
use crate::capabilities::{CapabilityRequest, CapabilityResponse, FileSource};

pub fn toggle_export_manifest_selection(state: &mut AppState, sap_adt_id: ComponentId, manifest_path: &str) {
    let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
        return;
    };

    if sap.export_selected_manifest_paths.contains(manifest_path) {
        sap.export_selected_manifest_paths.remove(manifest_path);
    } else {
        sap.export_selected_manifest_paths.insert(manifest_path.to_string());
    }
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
        && resource.etag.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false)
}

fn infer_transport_from_manifest(manifest: &SapAdtObjectManifest) -> Option<String> {
    for document in &manifest.documents {
        if document.uri.ends_with("/transports") || document.title.as_deref() == Some("Related Transport Requests") {
            if let Some(start) = document.body.find("<CORRNR>") {
                let rest = &document.body[start + 8..];
                if let Some(end) = rest.find("</CORRNR>") {
                    let corrnr = rest[..end].trim();
                    if !corrnr.is_empty() {
                        return Some(corrnr.to_string());
                    }
                }
            }
        }
    }
    None
}

fn export_candidates_from_manifest(manifest: &SapAdtObjectManifest) -> Vec<crate::app::state::SapAdtExportCandidate> {
    manifest
        .resources
        .iter()
        .filter(|resource| is_exportable_source_resource(resource))
        .map(|resource| crate::app::state::SapAdtExportCandidate {
            resource_id: resource.id.clone(),
            uri: resource.uri.clone(),
            path: resource.path.clone(),
            etag: resource.etag.clone(),
            content_type: resource.content_type.clone().unwrap_or_else(|| "text/plain".to_string()),
            activatable: resource.activatable,
        })
        .collect()
}

pub fn scan_export_objects(state: &mut AppState, sap_adt_id: ComponentId) {
    let Some(repo) = state.inputs.repo.clone() else {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some("Local repository is not set".to_string());
            sap.last_status = Some("Export scan failed".to_string());
        }
        return;
    };

    let git_status = match state.broker.exec(CapabilityRequest::GitStatus { repo }) {
        Ok(CapabilityResponse::GitStatus(status)) => status,
        Ok(other) => {
            if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                sap.last_error = Some(format!("Unexpected capability response: {:?}", other));
                sap.last_status = Some("Export scan failed".to_string());
            }
            return;
        }
        Err(err) => {
            if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                sap.last_error = Some(format!("{:#}", err));
                sap.last_status = Some("Export scan failed".to_string());
            }
            return;
        }
    };

    let manifest_paths = match crate::git::list_worktree_files(
        state.inputs.repo.as_ref().expect("repo checked above")
    ) {
        Ok(paths) => paths
            .into_iter()
            .filter(|p| p.starts_with("sap_adt/") && p.ends_with("/manifest.adt.json"))
            .collect::<Vec<_>>(),
        Err(err) => {
            if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                sap.last_error = Some(format!("{:#}", err));
                sap.last_status = Some("Export scan failed".to_string());
            }
            return;
        }
    };

    let mut rows = Vec::new();
    for manifest_path in manifest_paths {
        if let Ok(manifest) = read_manifest(state, &manifest_path) {
            let changed_files = count_changed_manifest_files(&manifest_path, &manifest, &git_status.files);
            let transport = infer_transport_from_manifest(&manifest);
            rows.push(SapAdtExportRow {
                object_name: manifest.object_name.clone().unwrap_or_else(|| "object".to_string()),
                object_type: manifest.object_type.clone().unwrap_or_else(|| "OBJECT".to_string()),
                manifest_path,
                changed_files,
                pushed_files: 0,
                syntax_ok: false,
                activation_ok: false,
                message: String::new(),
                transport: transport.clone(),
                export_candidates: export_candidates_from_manifest(&manifest),
            });
        }
    }

    rows.sort_by(|a, b| a.manifest_path.cmp(&b.manifest_path));

    if let Some(default_transport) = rows.iter().find_map(|row| row.transport.clone()) {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            if sap.corr_nr.trim().is_empty() {
                sap.corr_nr = default_transport;
            }
        }
    }

    if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
        sap.export_results_scan = rows;
        sap.last_error = None;
        sap.last_status = Some(format!("Scanned {} imported SAP ADT object(s)", sap.export_results_scan.len()));
    }
}

fn normalize_transport(input: &str) -> Option<String> {
    let value = input.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub fn export_selected_worktree_objects(state: &mut AppState, sap_adt_id: ComponentId) {
    if state.inputs.git_ref != WORKTREE_REF {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some("Switch to WORKTREE before exporting SAP ADT objects".to_string());
            sap.last_status = Some("Export blocked".to_string());
        }
        return;
    }

    let selected_manifest_paths = match state.sap_adts.get(&sap_adt_id) {
        Some(sap) => sap.export_selected_manifest_paths.iter().cloned().collect::<Vec<_>>(),
        None => return,
    };

    if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
        sap.export_results.clear();
    }

    let mut success_count = 0usize;
    let mut error_count = 0usize;

    for manifest_path in selected_manifest_paths {
        let manifest = match read_manifest(state, &manifest_path) {
            Ok(m) => m,
            Err(err) => {
                error_count += 1;
                push_result(state, sap_adt_id, SapAdtExportRow {
                    object_name: manifest_path.clone(),
                    object_type: "MANIFEST".to_string(),
                    manifest_path: manifest_path.clone(),
                    changed_files: 0,
                    pushed_files: 0,
                    syntax_ok: false,
                    activation_ok: false,
                    message: format!("Failed to read manifest: {:#}", err),
                    transport: None,
                    export_candidates: Vec::new(),
                });
                continue;
            }
        };

        let changed_files = current_changed_files_for_manifest(state, &manifest_path, &manifest).unwrap_or_default();
        let local_path = manifest_dir_from_manifest_path(&manifest_path);
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            if sap.corr_nr.trim().is_empty() {
                if let Some(row) = sap.export_results_scan.iter().find(|row| row.manifest_path == manifest_path) {
                    if let Some(transport) = row.transport.as_deref() {
                        sap.corr_nr = transport.to_string();
                    }
                }
            }
        }

        push_worktree_to_adt(state, sap_adt_id, &local_path);

        let after_push_error = state.sap_adts.get(&sap_adt_id).and_then(|sap| sap.last_error.clone());
        let after_push_status = state.sap_adts.get(&sap_adt_id).and_then(|sap| sap.last_status.clone()).unwrap_or_default();
        let push_ok = after_push_error.is_none();

        let mut activation_ok = false;
        let mut final_message = after_push_status;

        if push_ok {
            activate_worktree_object(state, sap_adt_id, &local_path);
            let after_activate_error = state.sap_adts.get(&sap_adt_id).and_then(|sap| sap.last_error.clone());
            final_message = state.sap_adts.get(&sap_adt_id).and_then(|sap| sap.last_status.clone()).unwrap_or_default();
            activation_ok = after_activate_error.is_none();
        }

        let transport = state
            .sap_adts
            .get(&sap_adt_id)
            .map(|sap| normalize_transport(&sap.corr_nr))
            .unwrap_or(None)
            .or_else(|| infer_transport_from_manifest(&manifest));

        if push_ok && activation_ok {
            success_count += 1;
        } else {
            error_count += 1;
        }

        push_result(state, sap_adt_id, SapAdtExportRow {
            object_name: manifest.object_name.clone().unwrap_or_else(|| "object".to_string()),
            object_type: manifest.object_type.clone().unwrap_or_else(|| "OBJECT".to_string()),
            manifest_path,
            changed_files: changed_files.len(),
            pushed_files: if push_ok { changed_files.len() } else { 0 },
            syntax_ok: push_ok,
            activation_ok,
            message: final_message,
            transport,
            export_candidates: export_candidates_from_manifest(&manifest),
        });
    }

    if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
        sap.last_error = None;
        if error_count > 0 {
            sap.last_status = Some(format!("Export finished: {} succeeded, {} failed", success_count, error_count));
        } else {
            sap.last_status = Some(format!("Exported and activated {} SAP ADT object(s)", success_count));
        }
    }
}

pub fn export_manifest_job(
    broker: &crate::capabilities::CapabilityBroker,
    repo: &std::path::PathBuf,
    sap: &mut crate::app::state::SapAdtState,
    manifest_path: &str,
    auto_activate: bool,
    corr_nr: Option<&str>,
) -> Result<crate::app::state::SapAdtExportRow, String> {
    export_manifest_impl(broker, repo, sap, manifest_path, auto_activate, corr_nr)
        .map_err(|e| format!("{:#}", e))
}

pub fn export_manifest_impl(
    broker: &crate::capabilities::CapabilityBroker,
    repo: &std::path::PathBuf,
    sap: &mut crate::app::state::SapAdtState,
    manifest_path: &str,
    auto_activate: bool,
    corr_nr: Option<&str>,
) -> anyhow::Result<crate::app::state::SapAdtExportRow> {
    let resp = broker.exec(crate::capabilities::CapabilityRequest::ReadFile {
        repo: repo.clone(),
        path: manifest_path.to_string(),
        source: crate::capabilities::FileSource::Worktree,
    })?;

    let crate::capabilities::CapabilityResponse::Bytes(bytes) = resp else {
        return Err(anyhow::anyhow!("Unexpected capability response reading {}", manifest_path));
    };

    let mut manifest: crate::app::sap_adt_manifest::SapAdtObjectManifest = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("Invalid manifest JSON in {}: {}", manifest_path, e))?;

    let manifest_dir = manifest_dir_from_manifest_path(manifest_path);
    let object_name = manifest.object_name.clone().unwrap_or_default();
    let object_type = manifest.object_type.clone().unwrap_or_default();
    let package_name = manifest.package_name.clone();
    let metadata_uri = manifest.metadata_uri.trim().to_string();
    let clone_target_path = manifest_dir.clone();
    let corr_nr = corr_nr.map(|value| value.to_string()).filter(|value| !value.trim().is_empty());

    let mut pending_updates: Vec<(usize, String)> = Vec::new();

    for index in 0..manifest.resources.len() {
        let Some(resource) = manifest.resources.get(index) else {
            continue;
        };
        if !resource.editable || resource.uri.trim().is_empty() {
            continue;
        }
        let local_path = join_manifest_relative_path(&manifest_dir, &resource.path);
        let resp = broker.exec(crate::capabilities::CapabilityRequest::ReadFile {
            repo: repo.clone(),
            path: local_path.clone(),
            source: crate::capabilities::FileSource::Worktree,
        })?;
        let crate::capabilities::CapabilityResponse::Bytes(local_bytes) = resp else {
            return Err(anyhow::anyhow!("Unexpected capability response reading {}", local_path));
        };
        let source_text = String::from_utf8(local_bytes)
            .map_err(|e| anyhow::anyhow!("{} is not valid UTF-8: {}", local_path, e))?;
        if source_text != resource.body {
            pending_updates.push((index, source_text));
        }
    }

    let export_candidates = manifest
        .resources
        .iter()
        .filter(|resource| resource.editable && !resource.uri.trim().is_empty())
        .map(|resource| crate::app::state::SapAdtExportCandidate {
            resource_id: resource.id.clone(),
            uri: resource.uri.clone(),
            path: resource.path.clone(),
            etag: resource.etag.clone().or_else(|| manifest.etag.clone()),
            content_type: resource
                .content_type
                .clone()
                .unwrap_or_else(|| "text/plain; charset=utf-8".to_string()),
            activatable: resource.activatable,
        })
        .collect::<Vec<_>>();

    if pending_updates.is_empty() {
        return Ok(crate::app::state::SapAdtExportRow {
            object_name,
            object_type,
            manifest_path: manifest_path.to_string(),
            changed_files: 0,
            pushed_files: 0,
            syntax_ok: false,
            activation_ok: false,
            message: "No local SAP ADT changes to push".to_string(),
            transport: corr_nr,
            export_candidates,
        });
    }

    let mut pushed_resource_uris: Vec<String> = Vec::new();

    for (current_index, source_text) in pending_updates.iter() {
        let (source_uri, content_type, lock_handle, fallback_etag) = match manifest.resources.get(*current_index) {
            Some(resource) => (
                resource.uri.trim().to_string(),
                resource
                    .content_type
                    .clone()
                    .or_else(|| Some("text/plain; charset=utf-8".to_string())),
                resource.lock_handle.clone(),
                resource.etag.clone().or_else(|| manifest.etag.clone()),
            ),
            None => return Err(anyhow::anyhow!("Manifest resource index is out of range")),
        };

        if source_uri.is_empty() {
            return Err(anyhow::anyhow!("{} has a manifest resource with no uri", manifest_path));
        }

        let mut effective_if_match = fallback_etag.clone();
        let mut update_result = crate::app::adt_bridge::update_object(
            sap,
            &source_uri,
            source_text,
            content_type.as_deref(),
            lock_handle.as_deref(),
            corr_nr.as_deref(),
            effective_if_match.as_deref(),
        );

        if let Err(err) = &update_result {
            let err_text = format!("{:#}", err);
            if err_text.contains("412") {
                if let Some(server_etag) = extract_server_etag_from_adt_error(&err_text) {
                    effective_if_match = Some(server_etag.clone());
                    if let Some(resource) = manifest.resources.get_mut(*current_index) {
                        resource.etag = Some(server_etag.clone());
                        manifest.etag = Some(server_etag);
                    }
                    update_result = crate::app::adt_bridge::update_object(
                        sap,
                        &source_uri,
                        source_text,
                        content_type.as_deref(),
                        lock_handle.as_deref(),
                        corr_nr.as_deref(),
                        effective_if_match.as_deref(),
                    );
                }
            }
        }

        let update_result = update_result?;

        if let Some(resource) = manifest.resources.get_mut(*current_index) {
            resource.body = source_text.clone();
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

    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    match broker.exec(crate::capabilities::CapabilityRequest::WriteWorktreeFile {
        repo: repo.clone(),
        path: manifest_path.to_string(),
        contents: manifest_bytes,
    }) {
        Ok(crate::capabilities::CapabilityResponse::Unit) => {}
        Ok(other) => return Err(anyhow::anyhow!("Unexpected capability response: {:?}", other)),
        Err(err) => return Err(err.into()),
    }

    let check_uri = if !metadata_uri.is_empty() {
        metadata_uri.clone()
    } else {
        pushed_resource_uris.first().cloned().unwrap_or_default()
    };

    let syntax_result = crate::app::adt_bridge::syntax_check(sap, &check_uri);
    let refresh_result = crate::app::adt_bridge::crawl_object_manifest(
        sap,
        &check_uri,
        Some(object_name.as_str()),
        Some(object_type.as_str()),
        package_name.as_deref(),
    );

    let mut syntax_ok = false;
    let mut activation_ok = false;
    let mut message = String::new();

    match syntax_result {
        Ok(check) => {
            syntax_ok = true;
            if check.body.trim().is_empty() {
                message = format!("Pushed {} SAP ADT resource(s)", pushed_resource_uris.len());
            } else {
                message = format!("Pushed {} SAP ADT resource(s) and syntax checked: {}", pushed_resource_uris.len(), check.body.trim());
            }

            if let Ok(refreshed) = refresh_result {
                let selection_uri = pushed_resource_uris.first().cloned();
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
                sap.selected_object_name = refreshed.object_name.clone().or_else(|| Some(object_name.clone()));
                sap.selected_object_type = refreshed.object_type.clone().or_else(|| Some(object_type.clone()));
                sap.selected_object_content = selected_resource.map(|resource| resource.body.clone()).unwrap_or_default();
                sap.selected_object_content_type = selected_resource.and_then(|resource| resource.content_type.clone());
                sap.selected_object_headers = selected_resource.map(|resource| resource.headers.clone()).unwrap_or_default();
                sap.selected_object_metadata = refreshed.metadata_xml.clone();
                sap.selected_object_metadata_content_type = Some("application/xml".to_string());
                sap.selected_manifest = Some(refreshed);
                sap.selected_resource_id = selected_resource_id;
                sap.clone_target_path = clone_target_path;
            }
        }
        Err(err) => {
            message = format!("{:#}", err);
        }
    }

    if syntax_ok && auto_activate && !check_uri.trim().is_empty() {
        match crate::app::adt_bridge::activate_object(sap, &check_uri) {
            Ok(result) => {
                activation_ok = true;
                if result.body.trim().is_empty() {
                    message = format!("{} and activated", message);
                } else {
                    message = format!("{} and activated: {}", message, result.body.trim());
                }
            }
            Err(err) => {
                if message.trim().is_empty() {
                    message = format!("{:#}", err);
                } else {
                    message = format!("{}; activation failed: {:#}", message, err);
                }
            }
        }
    }

    Ok(crate::app::state::SapAdtExportRow {
        object_name,
        object_type,
        manifest_path: manifest_path.to_string(),
        changed_files: pending_updates.len(),
        pushed_files: pushed_resource_uris.len(),
        syntax_ok,
        activation_ok,
        message,
        transport: corr_nr,
        export_candidates,
    })
}

pub fn push_worktree_to_adt(state: &mut AppState, sap_adt_id: ComponentId, path: &str) {
    if state.inputs.git_ref != WORKTREE_REF {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some("Switch to WORKTREE before pushing SAP ADT changes".to_string());
            sap.last_status = Some("Push blocked".to_string());
        }
        return;
    }

    let (manifest_path, mut manifest, requested_resource_index, manifest_dir) = match load_manifest_for_local_path(state, path) {
        Ok(value) => value,
        Err(err) => {
            if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                sap.last_error = Some(format!("{:#}", err));
                sap.last_status = Some("Push failed".to_string());
            }
            return;
        }
    };

    let requested_path = normalize_repo_relative_path(path);
    let requested_relative_path = if requested_path == manifest_path || requested_path == manifest_dir {
        None
    } else if manifest_dir.is_empty() {
        Some(requested_path.as_str())
    } else {
        requested_path.strip_prefix(&(manifest_dir.clone() + "/"))
    };

    let object_name = manifest.object_name.clone();
    let object_type = manifest.object_type.clone();
    let package_name = manifest.package_name.clone();
    let metadata_uri = manifest.metadata_uri.trim().to_string();
    let corr_nr = state
        .sap_adts
        .get(&sap_adt_id)
        .map(|sap| normalize_transport(&sap.corr_nr))
        .unwrap_or(None)
        .or_else(|| infer_transport_from_manifest(&manifest));

    let mut resource_indexes = Vec::new();
    for (idx, resource) in manifest.resources.iter().enumerate() {
        if !is_exportable_source_resource(resource) {
            continue;
        }
        let rel = resource.path.trim().trim_start_matches('/');
        if rel.is_empty() {
            continue;
        }
        let matches_requested = match requested_relative_path {
            None => true,
            Some(requested_rel) => requested_rel == rel,
        };
        if matches_requested {
            resource_indexes.push(idx);
        }
    }

    if resource_indexes.is_empty() {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some(format!("No exportable SAP ADT source resource matched {}", requested_path));
            sap.last_status = Some("Push failed".to_string());
        }
        return;
    }

    {
        let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
            return;
        };
        sap.last_error = None;
        sap.last_status = Some("Pushing local SAP ADT changes".to_string());
    }

    let mut pushed_resource_uris = Vec::new();

    for current_index in resource_indexes {
        let source_uri = manifest.resources[current_index].uri.clone();
        let rel = manifest.resources[current_index].path.trim().trim_start_matches('/').to_string();
        let repo_path = if manifest_dir.is_empty() {
            rel.clone()
        } else {
            format!("{}/{}", manifest_dir, rel)
        };
        let content_type = manifest.resources[current_index].content_type.clone();
        let lock_handle = manifest.resources[current_index].lock_handle.clone();
        let if_match = manifest.resources[current_index].etag.clone().or_else(|| manifest.etag.clone());

        let source_bytes = match read_worktree_bytes(state, &repo_path) {
            Ok(bytes) => bytes,
            Err(err) => {
                if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                    sap.last_error = Some(format!("Failed reading {}: {:#}", repo_path, err));
                    sap.last_status = Some("Push failed".to_string());
                }
                return;
            }
        };

        let source_text = match String::from_utf8(source_bytes) {
            Ok(text) => text,
            Err(err) => {
                if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                    sap.last_error = Some(format!("{} is not valid UTF-8: {}", repo_path, err));
                    sap.last_status = Some("Push failed".to_string());
                }
                return;
            }
        };

        let update_result = {
            let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
                return;
            };
            crate::app::adt_bridge::update_object(
                sap,
                &source_uri,
                &source_text,
                content_type.as_deref(),
                lock_handle.as_deref(),
                corr_nr.as_deref(),
                if_match.as_deref(),
            )
        };

        let update_result = match update_result {
            Ok(value) => value,
            Err(err) => {
                if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                    sap.last_error = Some(format!("{:#}", err));
                    sap.last_status = Some("Push failed".to_string());
                }
                return;
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
                .or_else(|| if_match.clone())
            {
                resource.etag = Some(etag.clone());
                manifest.etag = Some(etag);
            }
        }

        pushed_resource_uris.push(source_uri);
    }

    if let Err(err) = write_manifest_to_worktree(state, &manifest_path, &manifest) {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some(format!("Push succeeded, but manifest write failed: {:#}", err));
            sap.last_status = Some("Push incomplete".to_string());
        }
        return;
    }

    let check_uri = if !metadata_uri.is_empty() {
        metadata_uri.clone()
    } else {
        pushed_resource_uris.first().cloned().unwrap_or_default()
    };

    let syntax_result = {
        let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
            return;
        };
        crate::app::adt_bridge::syntax_check(sap, &check_uri)
    };

    let refresh_result = {
        let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
            return;
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

    let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
        return;
    };

    match syntax_result {
        Ok(check) => {
            let suffix = if check.body.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", check.body.trim())
            };
            sap.last_error = None;
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
                    sap.selected_object_content = selected_resource.map(|resource| resource.body.clone()).unwrap_or_default();
                    sap.selected_object_content_type = selected_resource.and_then(|resource| resource.content_type.clone());
                    sap.selected_object_headers = selected_resource.map(|resource| resource.headers.clone()).unwrap_or_default();
                    sap.selected_object_metadata = refreshed.metadata_xml.clone();
                    sap.selected_object_metadata_content_type = Some("application/xml".to_string());
                    sap.selected_manifest = Some(refreshed);
                    sap.selected_resource_id = selected_resource_id;
                    sap.clone_target_path = manifest_dir;
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
}

pub fn activate_worktree_object(state: &mut AppState, sap_adt_id: ComponentId, path: &str) {
    if state.inputs.git_ref != WORKTREE_REF {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some("Switch to WORKTREE before activating SAP ADT objects".to_string());
            sap.last_status = Some("Activate blocked".to_string());
        }
        return;
    }

    let (manifest_path, manifest, resource_index, _) = match load_manifest_for_local_path(state, path) {
        Ok(value) => value,
        Err(err) => {
            if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                sap.last_error = Some(format!("{:#}", err));
                sap.last_status = Some("Activate failed".to_string());
            }
            return;
        }
    };

    let object_name = manifest.object_name.clone();
    let object_type = manifest.object_type.clone();
    let package_name = manifest.package_name.clone();
    let fallback_source_uri = resource_index
        .and_then(|index| manifest.resources.get(index))
        .map(|resource| resource.uri.clone())
        .unwrap_or_default();
    let activate_uri = if !manifest.metadata_uri.trim().is_empty() {
        manifest.metadata_uri.trim().to_string()
    } else {
        fallback_source_uri.clone()
    };

    if activate_uri.is_empty() {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some(format!("{} is missing metadata_uri/source_uri", manifest_path));
            sap.last_status = Some("Activate failed".to_string());
        }
        return;
    }

    {
        let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
            return;
        };
        sap.last_error = None;
        sap.last_status = Some("Activating SAP object".to_string());
    }

    let activate_result = {
        let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
            return;
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
                let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
                    return;
                };
                crate::app::adt_bridge::crawl_object_manifest(
                    sap,
                    &activate_uri,
                    object_name.as_deref(),
                    object_type.as_deref(),
                    package_name.as_deref(),
                )
            };

            let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
                return;
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
                let selected_resource = selected_resource_id
                    .as_deref()
                    .and_then(|id| refreshed.resources.iter().find(|resource| resource.id == id))
                    .or_else(|| refreshed.resources.first());
                sap.selected_object_uri = selected_resource.map(|resource| resource.uri.clone());
                sap.selected_object_metadata_uri = Some(refreshed.metadata_uri.clone());
                sap.selected_object_name = refreshed.object_name.clone().or_else(|| object_name.clone());
                sap.selected_object_type = refreshed.object_type.clone().or_else(|| object_type.clone());
                sap.selected_object_content = selected_resource.map(|resource| resource.body.clone()).unwrap_or_default();
                sap.selected_object_content_type = selected_resource.and_then(|resource| resource.content_type.clone());
                sap.selected_object_headers = selected_resource.map(|resource| resource.headers.clone()).unwrap_or_default();
                sap.selected_object_metadata = refreshed.metadata_xml.clone();
                sap.selected_object_metadata_content_type = Some("application/xml".to_string());
                sap.selected_manifest = Some(refreshed);
                sap.selected_resource_id = selected_resource_id;
                sap.clone_target_path = manifest_path.strip_suffix("/manifest.adt.json").unwrap_or("").to_string();
            }
        }
        Err(err) => {
            if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                sap.last_error = Some(format!("{:#}", err));
                sap.last_status = Some("Activate failed".to_string());
            }
        }
    }
}

fn push_result(state: &mut AppState, sap_adt_id: ComponentId, row: SapAdtExportRow) {
    if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
        sap.export_results.push(row);
    }
}

fn extract_server_etag_from_adt_error(err_text: &str) -> Option<String> {
    for line in err_text.lines() {
        let lower = line.to_ascii_lowercase();
        if !lower.contains("etag") {
            continue;
        }
        let bytes = line.as_bytes();
        let mut start = None;
        for (idx, ch) in bytes.iter().enumerate() {
            if *ch == b'"' {
                if let Some(open) = start {
                    if idx > open + 1 {
                        return Some(line[open..=idx].to_string());
                    }
                    start = None;
                } else {
                    start = Some(idx);
                }
            }
        }
    }
    None
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

fn write_manifest_to_worktree(state: &mut AppState, manifest_path: &str, manifest: &SapAdtObjectManifest) -> Result<()> {
    let repo = state
        .inputs
        .repo
        .clone()
        .ok_or_else(|| anyhow!("Local repository is not set"))?;

    let contents = serde_json::to_vec_pretty(manifest)?;
    let resp = state.broker.exec(CapabilityRequest::WriteWorktreeFile {
        repo,
        path: manifest_path.to_string(),
        contents,
    })?;

    match resp {
        CapabilityResponse::Unit => Ok(()),
        other => Err(anyhow!("Unexpected capability response writing {}: {:?}", manifest_path, other)),
    }
}

fn read_manifest(state: &mut AppState, manifest_path: &str) -> Result<SapAdtObjectManifest> {
    let repo = state
        .inputs
        .repo
        .clone()
        .ok_or_else(|| anyhow!("Local repository is not set"))?;

    let resp = state.broker.exec(CapabilityRequest::ReadFile {
        repo,
        path: manifest_path.to_string(),
        source: FileSource::Worktree,
    })?;

    let CapabilityResponse::Bytes(bytes) = resp else {
        return Err(anyhow!("Unexpected capability response reading {}", manifest_path));
    };

    let text = String::from_utf8(bytes)?;
    Ok(serde_json::from_str(&text)?)
}

fn read_worktree_bytes(state: &mut AppState, path: &str) -> Result<Vec<u8>> {
    let repo = state
        .inputs
        .repo
        .clone()
        .ok_or_else(|| anyhow!("Local repository is not set"))?;

    let resp = state.broker.exec(CapabilityRequest::ReadFile {
        repo,
        path: path.to_string(),
        source: FileSource::Worktree,
    })?;

    match resp {
        CapabilityResponse::Bytes(bytes) => Ok(bytes),
        other => Err(anyhow!("Unexpected capability response reading {}: {:?}", path, other)),
    }
}

fn current_changed_files_for_manifest(
    state: &mut AppState,
    manifest_path: &str,
    manifest: &SapAdtObjectManifest,
) -> Result<Vec<String>> {
    let repo = state
        .inputs
        .repo
        .clone()
        .ok_or_else(|| anyhow!("Local repository is not set"))?;

    let resp = state.broker.exec(CapabilityRequest::GitStatus { repo })?;
    let CapabilityResponse::GitStatus(status) = resp else {
        return Err(anyhow!("Unexpected capability response getting git status"));
    };

    let manifest_files = manifest_tracked_files(manifest_path, manifest);
    Ok(status
        .files
        .into_iter()
        .filter(|entry| manifest_files.iter().any(|p| p == &entry.path))
        .map(|entry| entry.path)
        .collect())
}

fn count_changed_manifest_files(
    manifest_path: &str,
    manifest: &SapAdtObjectManifest,
    entries: &[crate::capabilities::GitStatusEntry],
) -> usize {
    let manifest_files = manifest_tracked_files(manifest_path, manifest);
    entries
        .iter()
        .filter(|entry| manifest_files.iter().any(|p| p == &entry.path))
        .count()
}

fn manifest_tracked_files(manifest_path: &str, manifest: &SapAdtObjectManifest) -> Vec<String> {
    let manifest_dir = manifest_dir_from_manifest_path(manifest_path);
    let mut out = vec![manifest_path.to_string()];

    for resource in &manifest.resources {
        out.push(join_manifest_relative_path(&manifest_dir, &resource.path));
    }
    for document in &manifest.documents {
        out.push(join_manifest_relative_path(&manifest_dir, &document.path));
    }

    out.sort();
    out.dedup();
    out
}

fn load_manifest_for_local_path(
    state: &mut AppState,
    path: &str,
) -> Result<(String, SapAdtObjectManifest, Option<usize>, String)> {
    for candidate in manifest_candidate_paths(path) {
        if let Ok(manifest) = read_manifest(state, &candidate) {
            let manifest_dir = manifest_dir_from_manifest_path(&candidate);
            let normalized = normalize_repo_relative_path(path);
            let requested_relative = if normalized == candidate || normalized == manifest_dir {
                None
            } else if manifest_dir.is_empty() {
                Some(normalized.as_str())
            } else {
                normalized.strip_prefix(&(manifest_dir.clone() + "/"))
            };

            let resource_index = requested_relative
                .and_then(|rel| {
                    manifest.resources.iter().position(|resource| {
                        resource.path.trim().trim_start_matches('/') == rel
                    })
                })
                .or_else(|| {
                    manifest.primary_resource_id().and_then(|id| {
                        manifest.resources.iter().position(|resource| resource.id == id)
                    })
                });

            return Ok((candidate, manifest, resource_index, manifest_dir));
        }
    }

    Err(anyhow!("Could not find manifest.adt.json for {}", path))
}

fn normalize_repo_relative_path(path: &str) -> String {
    path.trim().replace('\\', "/").trim_end_matches('/').to_string()
}

fn manifest_candidate_paths(path: &str) -> Vec<String> {
    let normalized = normalize_repo_relative_path(path);
    let mut out = Vec::new();

    if normalized.ends_with("/manifest.adt.json") {
        out.push(normalized.clone());
    }

    let mut current = normalized.as_str();
    loop {
        if !current.is_empty() {
            out.push(format!("{}/manifest.adt.json", current));
        }
        match current.rsplit_once('/') {
            Some((parent, _)) => current = parent,
            None => break,
        }
    }

    out.push("manifest.adt.json".to_string());
    out.sort();
    out.dedup();
    out.reverse();
    out
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
