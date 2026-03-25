use crate::app::actions::Action;
use crate::app::state::{AppState, SapAdtExportJobInput, SapAdtExportJobResult, SapAdtImportJobInput, SapAdtImportJobResult, SapAdtLogEntry, SapAdtObjectOperationRow};
use crate::capabilities::{CapabilityBroker, CapabilityRequest};

fn summarize_activation_details(details: &str) -> String {
    let mut important = Vec::new();

    for line in details.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.chars().all(|c| c == '=') {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("activation of worklist")
            || lower.starts_with("technical log for mass activation")
            || lower.starts_with("see log ")
            || lower.starts_with("checks ddl source ")
            || lower.starts_with("end of activation of worklist")
        {
            continue;
        }
        if lower.contains("line ")
            || lower.contains("column ")
            || lower.contains("was not activated")
            || lower.contains("contains errors")
            || lower.contains(" unknown")
            || lower.contains("syntax error")
            || lower.contains("contains error")
            || lower.contains(" is unknown")
        {
            important.push(line.to_string());
        }
    }

    important.dedup();
    important.join("\n")
}

fn summarize_export_message(row: &crate::app::state::SapAdtExportRow) -> String {
    if row.activation_ok {
        return row.message.clone();
    }

    let activation_summary = summarize_activation_details(&row.activation_details);
    if !activation_summary.is_empty() {
        return format!("Activation failed\n{}", activation_summary);
    }

    if !row.syntax_details.trim().is_empty() {
        return format!("Syntax check failed\n{}", row.syntax_details.trim());
    }

    row.message.clone()
}

fn push_sap_adt_activity(
    state: &mut AppState,
    sap_adt_id: crate::app::actions::ComponentId,
    key: String,
    object_name: String,
    object_type: String,
    action: String,
    op_state: String,
    message: String,
) {
    let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
        return;
    };

    if let Some(row) = sap.object_operations.iter_mut().find(|row| row.key == key && row.action == action) {
        row.object_name = object_name.clone();
        row.object_type = object_type.clone();
        row.state = op_state.clone();
        row.message = message.clone();
    } else {
        sap.object_operations.push(crate::app::state::SapAdtObjectOperationRow {
            key: key.clone(),
            object_name: object_name.clone(),
            object_type: object_type.clone(),
            action: action.clone(),
            state: op_state.clone(),
            message: message.clone(),
        });
    }

    sap.logs.push(crate::app::state::SapAdtLogEntry {
        key,
        object_name,
        object_type,
        action,
        state: op_state,
        message,
    });
}

fn sync_export_activity_rows(state: &mut AppState, sap_adt_id: crate::app::actions::ComponentId) {
    let results = match state.sap_adts.get(&sap_adt_id) {
        Some(sap) => sap.export_results.clone(),
        None => return,
    };

    for row in results {
        let state_text = if row.activation_ok {
            "activated".to_string()
        } else if !row.activation_details.trim().is_empty() || !row.syntax_details.trim().is_empty() {
            "error".to_string()
        } else if row.syntax_ok {
            "saved".to_string()
        } else if row.pushed_files > 0 {
            "saved".to_string()
        } else if !row.message.trim().is_empty() {
            "error".to_string()
        } else {
            "idle".to_string()
        };

        let message_text = summarize_export_message(&row);

        push_sap_adt_activity(
            state,
            sap_adt_id,
            row.manifest_path.clone(),
            row.object_name.clone(),
            row.object_type.clone(),
            "export".to_string(),
            state_text,
            message_text,
        );
    }
}

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::OpenSapAdtImportPopup { sap_adt_id } => {
            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };
            sap.import_popup_open = true;
            true
        }
        Action::CloseSapAdtImportPopup { sap_adt_id } => {
            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };
            sap.import_popup_open = false;
            true
        }
        Action::OpenSapAdtExportPopup { sap_adt_id } => {
            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };
            sap.export_popup_open = true;
            true
        }
        Action::CloseSapAdtExportPopup { sap_adt_id } => {
            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };
            sap.export_popup_open = false;
            true
        }
        Action::OpenSapAdtLogsPopup { sap_adt_id } => {
            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };
            sap.logs_popup_open = true;
            true
        }
        Action::CloseSapAdtLogsPopup { sap_adt_id } => {
            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };
            sap.logs_popup_open = false;
            true
        }
        Action::SapAdtConnect { sap_adt_id } => {
            let Some(sap) = state.sap_adts.get_mut(sap_adt_id) else {
                return true;
            };

            sap.last_error = None;
            sap.last_status = Some("Connecting to SAP ADT".to_string());

            match crate::app::adt_bridge::connect(sap) {
                Ok(()) => {
                    sap.connected = true;
                    sap.last_error = None;
                    sap.last_status = Some("Connected to SAP ADT".to_string());
                }
                Err(err) => {
                    sap.connected = false;
                    sap.last_error = Some(format!("{:#}", err));
                    sap.last_status = Some("SAP ADT connect failed".to_string());
                }
            }

            true
        }
        Action::SapAdtLoadPackage { sap_adt_id } => {
            crate::app::controllers::sap_adt_import_controller::load_package(state, *sap_adt_id);
            true
        }
        Action::SapAdtReadObject { sap_adt_id, object_uri } => {
            crate::app::controllers::sap_adt_import_controller::read_object(state, *sap_adt_id, object_uri);
            true
        }
        Action::SapAdtCloneSelectedToWorktree { sap_adt_id } => {
            crate::app::controllers::sap_adt_import_controller::clone_selected_to_worktree(state, *sap_adt_id);
            true
        }
        Action::SapAdtToggleImportObjectSelection { sap_adt_id, object_uri } => {
            crate::app::controllers::sap_adt_import_controller::toggle_import_object_selection(state, *sap_adt_id, object_uri);
            true
        }
        Action::SapAdtImportSelectedPackageObjects { sap_adt_id } => {
            start_import_queue(state, *sap_adt_id);
            true
        }
        Action::SapAdtPushWorktreeToAdt { sap_adt_id, path } => {
            crate::app::controllers::sap_adt_export_controller::push_worktree_to_adt(state, *sap_adt_id, path);
            true
        }
        Action::SapAdtActivateWorktreeObject { sap_adt_id, path } => {
            crate::app::controllers::sap_adt_export_controller::activate_worktree_object(state, *sap_adt_id, path);
            true
        }
        Action::SapAdtScanExportObjects { sap_adt_id } => {
            crate::app::controllers::sap_adt_export_controller::scan_export_objects(state, *sap_adt_id);
            true
        }
        Action::SapAdtToggleExportManifestSelection { sap_adt_id, manifest_path } => {
            crate::app::controllers::sap_adt_export_controller::toggle_export_manifest_selection(state, *sap_adt_id, manifest_path);
            true
        }
        Action::SapAdtExportSelectedWorktreeObjects { sap_adt_id } => {
            start_export_queue(state, *sap_adt_id);
            true
        }
        _ => false,
    }
}

fn set_object_operation(
    state: &mut AppState,
    sap_adt_id: crate::app::actions::ComponentId,
    key: String,
    object_name: String,
    object_type: String,
    action: String,
    op_state: String,
    message: String,
) {
    let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) else {
        return;
    };

    if let Some(row) = sap.object_operations.iter_mut().find(|row| row.key == key) {
        row.object_name = object_name.clone();
        row.object_type = object_type.clone();
        row.action = action.clone();
        row.state = op_state.clone();
        row.message = message.clone();
    } else {
        sap.object_operations.push(SapAdtObjectOperationRow {
            key: key.clone(),
            object_name: object_name.clone(),
            object_type: object_type.clone(),
            action: action.clone(),
            state: op_state.clone(),
            message: message.clone(),
        });
    }

    sap.logs.push(SapAdtLogEntry {
        key,
        object_name,
        object_type,
        action,
        state: op_state,
        message,
    });
}

fn start_export_queue(state: &mut AppState, sap_adt_id: crate::app::actions::ComponentId) {
    if state.inputs.git_ref != crate::app::state::WORKTREE_REF {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some("Switch to WORKTREE before exporting SAP ADT objects".to_string());
            sap.last_status = Some("Export blocked".to_string());
        }
        return;
    }

    let Some(repo) = state.inputs.repo.clone() else {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some("Local repository is not set".to_string());
            sap.last_status = Some("Export failed".to_string());
        }
        return;
    };

    let (connected, rows, selected_manifest_paths, already_pending, auto_activate, corr_nr) = match state.sap_adts.get(&sap_adt_id) {
        Some(sap) => (
            sap.connected,
            sap.export_results_scan.clone(),
            sap.export_selected_manifest_paths.clone(),
            sap.export_job.is_pending(),
            sap.export_auto_activate,
            sap.corr_nr.trim().to_string(),
        ),
        None => return,
    };

    if !connected {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some("Connect to SAP ADT first".to_string());
            sap.last_status = Some("Export blocked".to_string());
        }
        return;
    }

    if already_pending {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = None;
            sap.last_status = Some("Export already running".to_string());
        }
        return;
    }

    let corr_nr = if corr_nr.is_empty() { None } else { Some(corr_nr) };
    let mut items = Vec::new();

    for row in rows.into_iter().filter(|row| selected_manifest_paths.contains(&row.manifest_path)) {
        set_object_operation(
            state,
            sap_adt_id,
            row.manifest_path.clone(),
            row.object_name.clone(),
            row.object_type.clone(),
            "export".to_string(),
            "queued".to_string(),
            String::new(),
        );
        items.push(SapAdtExportJobInput {
            manifest_path: row.manifest_path,
            object_name: row.object_name,
            object_type: row.object_type,
            auto_activate,
            corr_nr: corr_nr.clone(),
        });
    }

    if items.is_empty() {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some("Select at least one SAP ADT manifest to export".to_string());
            sap.last_status = Some("Export blocked".to_string());
        }
        return;
    }

    if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
        let concurrency = items.len().max(1);
        sap.last_error = None;
        sap.last_status = Some(format!("Queued {} SAP ADT export object(s)", items.len()));
        sap.export_job.start_batch(items, concurrency);
    }

    let _ = state.broker.exec(crate::capabilities::CapabilityRequest::EnsureGitRepo { repo });
}

fn start_import_queue(state: &mut AppState, sap_adt_id: crate::app::actions::ComponentId) {
    if state.inputs.git_ref != crate::app::state::WORKTREE_REF {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some("Switch to WORKTREE before importing SAP ADT objects locally".to_string());
            sap.last_status = Some("Import blocked".to_string());
        }
        return;
    }

    let Some(repo) = state.inputs.repo.clone() else {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some("Local repository is not set".to_string());
            sap.last_status = Some("Import failed".to_string());
        }
        return;
    };

    let (connected, package_objects, selected_uris, already_pending) = match state.sap_adts.get(&sap_adt_id) {
        Some(sap) => (
            sap.connected,
            sap.package_objects.clone(),
            sap.import_selected_object_uris.clone(),
            sap.import_job.is_pending(),
        ),
        None => return,
    };

    if !connected {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some("Connect to SAP ADT first".to_string());
            sap.last_status = Some("Import blocked".to_string());
        }
        return;
    }

    if already_pending {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = None;
            sap.last_status = Some("Import already running".to_string());
        }
        return;
    }

    let mut items = Vec::new();
    for object in package_objects.into_iter().filter(|o| selected_uris.contains(&o.uri)) {
        let key = object.uri.clone();
        set_object_operation(
            state,
            sap_adt_id,
            key.clone(),
            object.name.clone(),
            object.object_type.clone(),
            "import".to_string(),
            "queued".to_string(),
            String::new(),
        );
        items.push(SapAdtImportJobInput {
            object_uri: object.uri.clone(),
            object_name: object.name.clone(),
            object_type: object.object_type.clone(),
            package_name: object.package_name.clone(),
            clone_target_path: Some(crate::app::adt_bridge::manifest_directory_name(
                Some(object.name.as_str()),
                Some(object.object_type.as_str()),
                object.package_name.as_deref(),
            )),
        });
    }

    if items.is_empty() {
        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
            sap.last_error = Some("Select at least one SAP ADT object to import".to_string());
            sap.last_status = Some("Import blocked".to_string());
        }
        return;
    }

    if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
        let concurrency = items.len().max(1);
        sap.last_error = None;
        sap.last_status = Some(format!("Queued {} SAP ADT import object(s)", items.len()));
        sap.import_job.start_batch(items, concurrency);
    }

    let _ = state.broker.exec(CapabilityRequest::EnsureGitRepo { repo });
}

fn run_import_job(
    broker: CapabilityBroker,
    discovery_url: String,
    browser_bridge_dir: String,
    cookie_header: Option<String>,
    adt_session_id: Option<String>,
    repo: std::path::PathBuf,
    item: SapAdtImportJobInput,
) -> Result<SapAdtImportJobResult, String> {
    let mut sap = crate::app::state::SapAdtState::new();
    sap.discovery_url = Some(discovery_url);
    sap.browser_bridge_dir = browser_bridge_dir;
    sap.cookie_header = cookie_header;
    sap.adt_session_id = adt_session_id;
    sap.connected = true;

    let manifest = crate::app::adt_bridge::crawl_object_manifest(
        &mut sap,
        &item.object_uri,
        Some(item.object_name.as_str()),
        Some(item.object_type.as_str()),
        item.package_name.as_deref(),
    ).map_err(|e| format!("{:#}", e))?;

    let manifest_dir = item
        .clone_target_path
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| crate::app::adt_bridge::manifest_directory_name(
            manifest.object_name.as_deref().or(Some(item.object_name.as_str())),
            manifest.object_type.as_deref().or(Some(item.object_type.as_str())),
            manifest.package_name.as_deref().or(item.package_name.as_deref()),
        ));

    let manifest_dir = manifest_dir.trim().trim_end_matches('/').replace('\\', "/");
    let manifest_path = format!("{}/manifest.adt.json", manifest_dir);

    match broker.exec(CapabilityRequest::CreateWorktreeDir {
        repo: repo.clone(),
        path: "sap_adt".to_string(),
    }) {
        Ok(_) | Err(_) => {}
    }

    broker.exec(CapabilityRequest::CreateWorktreeDir {
        repo: repo.clone(),
        path: manifest_dir.clone(),
    }).map_err(|e| format!("{:#}", e))?;

    let manifest_json = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    broker.exec(CapabilityRequest::WriteWorktreeFile {
        repo: repo.clone(),
        path: manifest_path,
        contents: manifest_json,
    }).map_err(|e| format!("{:#}", e))?;

    for resource in &manifest.resources {
        broker.exec(CapabilityRequest::WriteWorktreeFile {
            repo: repo.clone(),
            path: format!("{}/{}", manifest_dir, resource.path),
            contents: resource.body.as_bytes().to_vec(),
        }).map_err(|e| format!("{:#}", e))?;
    }

    for document in &manifest.documents {
        broker.exec(CapabilityRequest::WriteWorktreeFile {
            repo: repo.clone(),
            path: format!("{}/{}", manifest_dir, document.path),
            contents: document.body.as_bytes().to_vec(),
        }).map_err(|e| format!("{:#}", e))?;
    }

    Ok(SapAdtImportJobResult {
        key: item.object_uri,
        object_name: item.object_name,
        object_type: item.object_type,
        action: "import".to_string(),
        state: "imported".to_string(),
        message: format!("Imported into {}", manifest_dir),
    })
}

fn run_export_job(
    broker: crate::capabilities::CapabilityBroker,
    discovery_url: String,
    browser_bridge_dir: String,
    cookie_header: Option<String>,
    adt_session_id: Option<String>,
    repo: std::path::PathBuf,
    item: SapAdtExportJobInput,
) -> Result<SapAdtExportJobResult, String> {
    let mut sap = crate::app::state::SapAdtState::new();
    sap.discovery_url = Some(discovery_url);
    sap.browser_bridge_dir = browser_bridge_dir;
    sap.cookie_header = cookie_header;
    sap.adt_session_id = adt_session_id;
    sap.connected = true;
    sap.corr_nr = item.corr_nr.clone().unwrap_or_default();
    sap.export_auto_activate = item.auto_activate;

    let row = crate::app::controllers::sap_adt_export_controller::export_manifest_job(
        &broker,
        &repo,
        &mut sap,
        &item.manifest_path,
        item.auto_activate,
        item.corr_nr.as_deref(),
    )?;

    let state = if row.activation_ok {
        "activated".to_string()
    } else if !row.activation_details.trim().is_empty() || !row.syntax_details.trim().is_empty() {
        "error".to_string()
    } else if row.syntax_ok || row.pushed_files > 0 {
        "saved".to_string()
    } else if row.message.trim().eq_ignore_ascii_case("No local SAP ADT changes to push") || row.message.to_ascii_lowercase().contains("no changes to push") {
        "warning".to_string()
    } else {
        "error".to_string()
    };

    let export_message = summarize_export_message(&row);

    Ok(SapAdtExportJobResult {
        key: item.manifest_path,
        object_name: item.object_name,
        object_type: item.object_type,
        action: "export".to_string(),
        state,
        message: export_message,
        row,
    })
}

pub fn finalize_frame(state: &mut AppState) {
    let ids: Vec<_> = state.sap_adts.keys().copied().collect();
    for sap_adt_id in ids {
        let (repo, broker, discovery_url, browser_bridge_dir, cookie_header, adt_session_id, import_pending, export_pending) = match state.sap_adts.get(&sap_adt_id) {
            Some(sap) => (
                state.inputs.repo.clone(),
                state.broker.clone(),
                sap.discovery_url.clone(),
                sap.browser_bridge_dir.clone(),
                sap.cookie_header.clone(),
                sap.adt_session_id.clone(),
                sap.import_job.is_pending(),
                sap.export_job.is_pending(),
            ),
            None => continue,
        };

        if import_pending {
            let Some(repo) = repo.clone() else {
                if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                    sap.import_job.clear();
                    sap.last_error = Some("Local repository is not set".to_string());
                    sap.last_status = Some("Import failed".to_string());
                }
                continue;
            };

            let Some(discovery_url) = discovery_url.clone() else {
                if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                    sap.import_job.clear();
                    sap.last_error = Some("SAP ADT discovery URL is not set".to_string());
                    sap.last_status = Some("Import failed".to_string());
                }
                continue;
            };

            let events = match state.sap_adts.get_mut(&sap_adt_id) {
                Some(sap) => sap.import_job.poll_events_with({
                    let broker = broker.clone();
                    let discovery_url = discovery_url.clone();
                    let browser_bridge_dir = browser_bridge_dir.clone();
                    let cookie_header = cookie_header.clone();
                    let adt_session_id = adt_session_id.clone();
                    let repo = repo.clone();
                    move |item| run_import_job(
                        broker.clone(),
                        discovery_url.clone(),
                        browser_bridge_dir.clone(),
                        cookie_header.clone(),
                        adt_session_id.clone(),
                        repo.clone(),
                        item,
                    )
                }),
                None => Vec::new(),
            };

            for event in events {
                match event {
                    crate::app::async_job::AsyncParallelEvent::Started { item, completed, total } => {
                        set_object_operation(
                            state,
                            sap_adt_id,
                            item.object_uri.clone(),
                            item.object_name.clone(),
                            item.object_type.clone(),
                            "import".to_string(),
                            "running".to_string(),
                            String::new(),
                        );
                        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                            sap.last_error = None;
                            sap.last_status = Some(format!("Importing {}/{} SAP ADT object(s)", completed + 1, total));
                        }
                    }
                    crate::app::async_job::AsyncParallelEvent::Finished { item, completed, total, result } => {
                        match result {
                            Ok(done) => {
                                set_object_operation(
                                    state,
                                    sap_adt_id,
                                    done.key.clone(),
                                    done.object_name.clone(),
                                    done.object_type.clone(),
                                    done.action.clone(),
                                    done.state.clone(),
                                    done.message.clone(),
                                );
                                if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                                    sap.last_error = None;
                                    sap.last_status = Some(format!("Imported {}/{} SAP ADT object(s)", completed, total));
                                }
                            }
                            Err(err) => {
                                set_object_operation(
                                    state,
                                    sap_adt_id,
                                    item.object_uri.clone(),
                                    item.object_name.clone(),
                                    item.object_type.clone(),
                                    "import".to_string(),
                                    "error".to_string(),
                                    err.clone(),
                                );
                                if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                                    sap.last_error = Some(err);
                                    sap.last_status = Some(format!("Import failed after {}/{} object(s)", completed, total));
                                }
                            }
                        }
                    }
                }
            }
        }

        if export_pending {
            let Some(repo) = repo.clone() else {
                if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                    sap.export_job.clear();
                    sap.last_error = Some("Local repository is not set".to_string());
                    sap.last_status = Some("Export failed".to_string());
                }
                continue;
            };

            let Some(discovery_url) = discovery_url.clone() else {
                if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                    sap.export_job.clear();
                    sap.last_error = Some("SAP ADT discovery URL is not set".to_string());
                    sap.last_status = Some("Export failed".to_string());
                }
                continue;
            };

            let events = match state.sap_adts.get_mut(&sap_adt_id) {
                Some(sap) => sap.export_job.poll_events_with({
                    let broker = broker.clone();
                    let discovery_url = discovery_url.clone();
                    let browser_bridge_dir = browser_bridge_dir.clone();
                    let cookie_header = cookie_header.clone();
                    let adt_session_id = adt_session_id.clone();
                    let repo = repo.clone();
                    move |item| run_export_job(
                        broker.clone(),
                        discovery_url.clone(),
                        browser_bridge_dir.clone(),
                        cookie_header.clone(),
                        adt_session_id.clone(),
                        repo.clone(),
                        item,
                    )
                }),
                None => Vec::new(),
            };

            for event in events {
                match event {
                    crate::app::async_job::AsyncParallelEvent::Started { item, completed, total } => {
                        set_object_operation(
                            state,
                            sap_adt_id,
                            item.manifest_path.clone(),
                            item.object_name.clone(),
                            item.object_type.clone(),
                            "export".to_string(),
                            "running".to_string(),
                            String::new(),
                        );
                        if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                            sap.last_error = None;
                            sap.last_status = Some(format!("Exporting {}/{} SAP ADT object(s)", completed + 1, total));
                        }
                    }
                    crate::app::async_job::AsyncParallelEvent::Finished { item, completed, total, result } => {
                        match result {
                            Ok(done) => {
                                if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                                    if let Some(existing) = sap.export_results.iter_mut().find(|row| row.manifest_path == done.row.manifest_path) {
                                        *existing = done.row.clone();
                                    } else {
                                        sap.export_results.push(done.row.clone());
                                    }
                                }
                                set_object_operation(
                                    state,
                                    sap_adt_id,
                                    done.key.clone(),
                                    done.object_name.clone(),
                                    done.object_type.clone(),
                                    done.action.clone(),
                                    done.state.clone(),
                                    done.message.clone(),
                                );
                                if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                                    sap.last_error = None;
                                    sap.last_status = Some(if done.state == "warning" {
                                        format!("Checked {}/{} SAP ADT object(s)", completed, total)
                                    } else {
                                        format!("Exported {}/{} SAP ADT object(s)", completed, total)
                                    });
                                }
                            }
                            Err(err) => {
                                set_object_operation(
                                    state,
                                    sap_adt_id,
                                    item.manifest_path.clone(),
                                    item.object_name.clone(),
                                    item.object_type.clone(),
                                    "export".to_string(),
                                    "error".to_string(),
                                    err.clone(),
                                );
                                if let Some(sap) = state.sap_adts.get_mut(&sap_adt_id) {
                                    sap.last_error = Some(err);
                                    sap.last_status = Some(format!("Export failed after {}/{} object(s)", completed, total));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
