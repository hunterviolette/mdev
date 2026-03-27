use crate::app::actions::ComponentId;
use crate::app::state::{AppState, WORKTREE_REF};
use crate::capabilities::{CapabilityRequest, CapabilityResponse};

pub fn load_package(state: &mut AppState, sap_adt_id: ComponentId) {
    let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
        return;
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
            sap.package_tree_xml = xml;
            sap.package_objects = objects;
            sap.import_selected_object_uris.clear();
            sap.selected_object_uri = None;
            sap.selected_object_metadata_uri = None;
            sap.selected_object_name = None;
            sap.selected_object_type = None;
            sap.selected_object_content.clear();
            sap.selected_object_content_type = None;
            sap.selected_object_headers.clear();
            sap.selected_object_metadata.clear();
            sap.selected_object_metadata_content_type = None;
            sap.selected_manifest = None;
            sap.selected_resource_id = None;
            sap.clone_target_path.clear();
            if object_count == 0 {
                sap.last_error = Some(format!(
                    "Package tree loaded but 0 objects were recognized from {} bytes of ADT XML. Inspect 'Package tree XML' to verify the response shape.",
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
            sap.last_error = Some(format!("{:#}", err));
            sap.last_status = Some("Package load failed".to_string());
        }
    }
}

pub fn toggle_import_object_selection(state: &mut AppState, sap_adt_id: ComponentId, object_uri: &str) {
    let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
        return;
    };

    if sap.import_selected_object_uris.contains(object_uri) {
        sap.import_selected_object_uris.remove(object_uri);
    } else {
        sap.import_selected_object_uris.insert(object_uri.to_string());
    }
}

pub fn read_object(state: &mut AppState, sap_adt_id: ComponentId, object_uri: &str) {
    let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
        return;
    };

    sap.last_error = None;
    sap.last_status = Some("Reading SAP object".to_string());

    let selected = sap
        .package_objects
        .iter()
        .find(|o| o.uri == object_uri || o.source_uri.as_deref() == Some(object_uri))
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
            return;
        }
    }

    match crate::app::adt_bridge::crawl_object_manifest(
        sap,
        object_uri,
        selected.as_ref().map(|o| o.name.as_str()),
        selected.as_ref().map(|o| o.object_type.as_str()),
        selected.as_ref().and_then(|o| o.package_name.as_deref()),
    ) {
        Ok(manifest) => {
            let selected_resource_id = manifest.primary_resource_id();
            let selected_resource = selected_resource_id
                .as_deref()
                .and_then(|id| manifest.resources.iter().find(|resource| resource.id == id))
                .or_else(|| manifest.resources.first());

            let clone_target_path = crate::app::adt_bridge::manifest_directory_name(
                manifest.object_name.as_deref().or(selected.as_ref().map(|o| o.name.as_str())),
                manifest.object_type.as_deref().or(selected.as_ref().map(|o| o.object_type.as_str())),
                manifest.package_name.as_deref().or(selected.as_ref().and_then(|o| o.package_name.as_deref())),
            );

            sap.selected_object_uri = selected_resource.map(|resource| resource.uri.clone());
            sap.selected_object_metadata_uri = Some(manifest.metadata_uri.clone());
            sap.selected_object_name = manifest.object_name.clone().or_else(|| selected.as_ref().map(|o| o.name.clone()));
            sap.selected_object_type = manifest.object_type.clone().or_else(|| selected.as_ref().map(|o| o.object_type.clone()));
            sap.selected_object_content = selected_resource.map(|resource| resource.body.clone()).unwrap_or_default();
            sap.selected_object_content_type = selected_resource.and_then(|resource| resource.content_type.clone());
            sap.selected_object_headers = selected_resource.map(|resource| resource.headers.clone()).unwrap_or_default();
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
}

pub fn clone_selected_to_worktree(state: &mut AppState, sap_adt_id: ComponentId) {
    if state.inputs.git_ref != WORKTREE_REF {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some("Switch to WORKTREE before cloning SAP ADT objects locally".to_string());
            sap.last_status = Some("Clone blocked".to_string());
        }
        return;
    }

    let Some(repo) = state.inputs.repo.clone() else {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some("Local repository is not set".to_string());
            sap.last_status = Some("Clone failed".to_string());
        }
        return;
    };

    let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
        return;
    };

    let Some(mut manifest) = sap.selected_manifest.clone() else {
        sap.last_error = Some("No SAP ADT manifest is currently loaded".to_string());
        sap.last_status = Some("Clone failed".to_string());
        return;
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
            if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                sap.last_error = Some(format!("{:#}", err));
                sap.last_status = Some("Clone failed".to_string());
            }
            return;
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
            if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
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
            if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                sap.last_error = Some(format!("{:#}", err));
                sap.last_status = Some("Clone failed".to_string());
            }
        }
    }
}

pub fn import_selected_package_objects(state: &mut AppState, sap_adt_id: ComponentId) {
    let selected: Vec<String> = match state.sap_adts.get(&sap_adt_id) {
        Some(sap) => sap.import_selected_object_uris.iter().cloned().collect(),
        None => return,
    };

    for object_uri in selected {
        read_object(state, sap_adt_id, &object_uri);
        let should_clone = state
            .sap_adts
            .get(&sap_adt_id)
            .map(|sap| sap.selected_manifest.is_some())
            .unwrap_or(false);
        if should_clone {
            clone_selected_to_worktree(state, sap_adt_id);
        }
    }
}

fn write_worktree_bytes(state: &mut AppState, path: &str, bytes: Vec<u8>) -> anyhow::Result<()> {
    let repo = state
        .inputs
        .repo
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Local repository is not set"))?;

    let resp = state.broker.exec(CapabilityRequest::WriteWorktreeFile {
        repo,
        path: path.to_string(),
        contents: bytes,
    })?;

    match resp {
        CapabilityResponse::Unit => Ok(()),
        other => Err(anyhow::anyhow!("Unexpected capability response writing {}: {:?}", path, other)),
    }
}
