use std::{fs, path::Path};

use axum::{extract::Json, routing::post, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::task::JoinSet;

use crate::{
    app_state::AppState,
    engine::capabilities::sap::common::{
        ensure_adt_bridge_connected,
        export_candidates_from_manifest,
        import_object_to_worktree,
        parse_connection,
        read_manifest,
        resolve_bridge_dir,
        resolve_package_objects,
        resolve_repo_path,
        split_multiline_items,
        AdtBridgeProcess,
    },
};

#[derive(Debug, Deserialize, Default)]
struct SapDiscoverRequest {}

#[derive(Debug, Serialize)]
struct SapDiscoverResponse {
    ok: bool,
    message: String,
}

#[derive(Debug, Deserialize)]
struct SapSearchRequest {
    package_name: String,
    #[serde(default)]
    include_subpackages: bool,
}

#[derive(Debug, Serialize)]
struct SapObjectItem {
    uri: String,
    source_uri: Option<String>,
    name: String,
    object_type: String,
    package_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct SapSearchResponse {
    ok: bool,
    package_name: String,
    objects: Vec<SapObjectItem>,
    count: usize,
}

#[derive(Debug, Deserialize, Clone)]
struct SapImportSelection {
    object_uri: String,
    #[serde(default)]
    object_name: String,
    #[serde(default)]
    object_type: String,
    #[serde(default)]
    package_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SapImportRequest {
    repo_ref: String,
    #[serde(default)]
    package_name: String,
    #[serde(default)]
    include_subpackages: bool,
    #[serde(default)]
    include_xml_artifacts: bool,
    #[serde(default)]
    object_uris: Vec<String>,
    #[serde(default)]
    object_uris_text: String,
    #[serde(default)]
    selected_objects: Vec<SapImportSelection>,
    #[serde(default)]
    connection: Value,
}

#[derive(Debug, Serialize)]
struct SapImportItem {
    object_uri: String,
    object_name: String,
    object_type: String,
    package_name: Option<String>,
    manifest_path: String,
    manifest_dir: String,
    resource_count: usize,
    document_count: usize,
}

#[derive(Debug, Serialize)]
struct SapImportResponse {
    ok: bool,
    imported: Vec<SapImportItem>,
    failures: Vec<String>,
    count: usize,
}

#[derive(Debug, Deserialize)]
struct SapExportScanRequest {
    repo_ref: String,
}

#[derive(Debug, Serialize)]
struct SapExportCandidateItem {
    manifest_path: String,
    object_name: String,
    object_type: String,
    package_name: Option<String>,
    candidate_count: usize,
    resource_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SapExportScanResponse {
    ok: bool,
    manifests: Vec<SapExportCandidateItem>,
    count: usize,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sap/discover", post(discover))
        .route("/api/sap/search", post(search))
        .route("/api/sap/import", post(import))
        .route("/api/sap/export-scan", post(export_scan))
}

async fn discover(
    Json(_req): Json<SapDiscoverRequest>,
) -> Result<Json<SapDiscoverResponse>, (axum::http::StatusCode, String)> {
    tracing::info!(target: "workflow_api::sap", "sap discover requested");

    let connection = parse_connection(&serde_json::json!({})).map_err(|err| {
        tracing::error!(target: "workflow_api::sap", error = %err, "sap discover failed while resolving connection config");
        internal(err)
    })?;
    let bridge_dir = resolve_bridge_dir(&connection).map_err(|err| {
        tracing::error!(target: "workflow_api::sap", base_url = %connection.base_url, error = %err, "sap discover failed while resolving bridge dir");
        internal(err)
    })?;
    let mut bridge = AdtBridgeProcess::start(&bridge_dir).map_err(|err| {
        tracing::error!(target: "workflow_api::sap", bridge_dir = %bridge_dir.display(), error = %err, "sap discover failed while starting ADT bridge");
        internal(err)
    })?;
    let effective = ensure_adt_bridge_connected(&mut bridge, &connection).await.map_err(|err| {
        tracing::error!(target: "workflow_api::sap", base_url = %connection.base_url, auth_type = %connection.auth_type.clone().unwrap_or_default(), error = %err, "sap discover failed while bootstrapping SAP auth/session");
        internal(err)
    })?;

    tracing::info!(target: "workflow_api::sap", base_url = %effective.base_url, "sap discover succeeded");

    Ok(Json(SapDiscoverResponse {
        ok: true,
        message: format!(
            "SAP ADT backend connection is ready for {}.",
            effective.base_url
        ),
    }))
}

async fn search(
    Json(req): Json<SapSearchRequest>,
) -> Result<Json<SapSearchResponse>, (axum::http::StatusCode, String)> {
    tracing::info!(
        target: "workflow_api::sap",
        package_name = %req.package_name,
        include_subpackages = req.include_subpackages,
        "sap search requested"
    );

    let connection = parse_connection(&serde_json::json!({})).map_err(|err| {
        tracing::error!(target: "workflow_api::sap", package_name = %req.package_name, error = %err, "sap search failed while resolving connection config");
        internal(err)
    })?;
    let bridge_dir = resolve_bridge_dir(&connection).map_err(|err| {
        tracing::error!(target: "workflow_api::sap", package_name = %req.package_name, base_url = %connection.base_url, error = %err, "sap search failed while resolving bridge dir");
        internal(err)
    })?;
    let mut bridge = AdtBridgeProcess::start(&bridge_dir).map_err(|err| {
        tracing::error!(target: "workflow_api::sap", package_name = %req.package_name, bridge_dir = %bridge_dir.display(), error = %err, "sap search failed while starting ADT bridge");
        internal(err)
    })?;
    let effective = ensure_adt_bridge_connected(&mut bridge, &connection).await.map_err(|err| {
        tracing::error!(target: "workflow_api::sap", package_name = %req.package_name, base_url = %connection.base_url, error = %err, "sap search failed while bootstrapping SAP auth/session");
        internal(err)
    })?;

    let objects = resolve_package_objects(&mut bridge, &effective, &req.package_name, req.include_subpackages)
        .await
        .map_err(|err| {
            tracing::error!(target: "workflow_api::sap", package_name = %req.package_name, include_subpackages = req.include_subpackages, error = %err, "sap search failed while resolving package objects");
            internal(err)
        })?
        .into_iter()
        .map(|item| SapObjectItem {
            uri: item.uri,
            source_uri: item.source_uri,
            name: item.name,
            object_type: item.object_type,
            package_name: item.package_name,
        })
        .collect::<Vec<_>>();

    tracing::info!(target: "workflow_api::sap", package_name = %req.package_name, count = objects.len(), "sap search succeeded");

    Ok(Json(SapSearchResponse {
        ok: true,
        package_name: req.package_name,
        count: objects.len(),
        objects,
    }))
}

async fn import(
    Json(req): Json<SapImportRequest>,
) -> Result<Json<SapImportResponse>, (axum::http::StatusCode, String)> {
    let repo = resolve_repo_path(&req.repo_ref).map_err(internal)?;

    let payload = json!({
        "connection": req.connection
    });

    let connection = parse_connection(&payload).map_err(internal)?;
    let bridge_dir = resolve_bridge_dir(&connection).map_err(internal)?;

    let mut selected_objects = req.selected_objects;

    if selected_objects.is_empty() {
        selected_objects = req
            .object_uris
            .into_iter()
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .map(|object_uri| SapImportSelection {
                object_uri,
                object_name: String::new(),
                object_type: String::new(),
                package_name: None,
            })
            .collect::<Vec<_>>();
    }

    if selected_objects.is_empty() {
        selected_objects = split_multiline_items(&req.object_uris_text)
            .into_iter()
            .map(|object_uri| SapImportSelection {
                object_uri,
                object_name: String::new(),
                object_type: String::new(),
                package_name: None,
            })
            .collect::<Vec<_>>();
    }

    let needs_package_resolution = selected_objects.is_empty() && !req.package_name.trim().is_empty();

    if needs_package_resolution {
        let mut bridge = AdtBridgeProcess::start(&bridge_dir).map_err(internal)?;
        let effective = ensure_adt_bridge_connected(&mut bridge, &connection)
            .await
            .map_err(internal)?;

        let package_objects = resolve_package_objects(
            &mut bridge,
            &effective,
            &req.package_name,
            req.include_subpackages,
        )
        .await
        .map_err(internal)?;

        if !selected_objects.is_empty() {
            let requested = selected_objects;
            selected_objects = package_objects
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
            selected_objects = package_objects
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
        return Err(internal("sap import resolved zero importable object selections"));
    }

    let max_concurrency = 6usize;
    let mut pending = selected_objects.into_iter();
    let mut running = JoinSet::new();
    let mut imported = Vec::new();
    let mut failures = Vec::new();

    let spawn_one = |running: &mut JoinSet<anyhow::Result<SapImportItem>>, item: SapImportSelection| {
        let repo = repo.clone();
        let payload = payload.clone();
        let include_xml_artifacts = req.include_xml_artifacts;
        running.spawn(async move {
            let connection = parse_connection(&payload)?;
            let bridge_dir = resolve_bridge_dir(&connection)?;
            let mut bridge = AdtBridgeProcess::start(&bridge_dir)?;
            let _effective = ensure_adt_bridge_connected(&mut bridge, &connection).await?;
            let imported = import_object_to_worktree(
                &mut bridge,
                &repo,
                &item.object_uri,
                if item.object_name.trim().is_empty() { None } else { Some(item.object_name.as_str()) },
                if item.object_type.trim().is_empty() { None } else { Some(item.object_type.as_str()) },
                item.package_name.as_deref(),
                None,
                include_xml_artifacts,
            )?;
            Ok(SapImportItem {
                object_uri: imported.object_uri,
                object_name: imported.object_name,
                object_type: imported.object_type,
                package_name: imported.package_name,
                manifest_path: imported.manifest_path,
                manifest_dir: imported.manifest_dir,
                resource_count: imported.resource_count,
                document_count: imported.document_count,
            })
        });
    };

    for _ in 0..max_concurrency {
        if let Some(item) = pending.next() {
            spawn_one(&mut running, item);
        } else {
            break;
        }
    }

    while let Some(result) = running.join_next().await {
        match result {
            Ok(Ok(item)) => imported.push(item),
            Ok(Err(err)) => failures.push(format!("{:#}", err)),
            Err(err) => failures.push(err.to_string()),
        }

        if let Some(item) = pending.next() {
            spawn_one(&mut running, item);
        }
    }

    Ok(Json(SapImportResponse {
        ok: failures.is_empty(),
        count: imported.len(),
        imported,
        failures,
    }))
}

async fn export_scan(
    Json(req): Json<SapExportScanRequest>,
) -> Result<Json<SapExportScanResponse>, (axum::http::StatusCode, String)> {
    let repo = resolve_repo_path(&req.repo_ref).map_err(internal)?;
    let mut manifest_paths = Vec::new();
    collect_manifest_paths(&repo, &repo, &mut manifest_paths).map_err(internal)?;
    manifest_paths.sort();
    manifest_paths.dedup();

    let mut manifests = Vec::new();
    for manifest_path in manifest_paths {
        let manifest = read_manifest(&repo, &manifest_path).map_err(internal)?;
        let candidates = export_candidates_from_manifest(&manifest);
        if candidates.is_empty() {
            continue;
        }

        let object_name = manifest.object_name.clone().unwrap_or_default();
        let object_type = manifest.object_type.clone().unwrap_or_default();
        let package_name = manifest.package_name.clone();
        let candidate_count = candidates.len();
        let resource_paths = candidates.iter().map(|resource| resource.path.clone()).collect();

        manifests.push(SapExportCandidateItem {
            manifest_path,
            object_name,
            object_type,
            package_name,
            candidate_count,
            resource_paths,
        });
    }

    Ok(Json(SapExportScanResponse {
        ok: true,
        count: manifests.len(),
        manifests,
    }))
}

fn collect_manifest_paths(root: &Path, current: &Path, out: &mut Vec<String>) -> anyhow::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_manifest_paths(root, &path, out)?;
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) != Some("manifest.adt.json") {
            continue;
        }
        let rel = path.strip_prefix(root)?.to_string_lossy().replace('\\', "/");
        if rel.starts_with("sap_adt/") {
            out.push(rel);
        }
    }
    Ok(())
}

fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
