use std::{fs, path::Path};

use axum::{extract::Json, routing::post, Router};
use serde::{Deserialize, Serialize};

use crate::{
    app_state::AppState,
    engine::capabilities::sap::common::{
        export_candidates_from_manifest,
        parse_connection,
        read_manifest,
        resolve_bridge_dir,
        resolve_repo_path,
        AdtBridgeProcess,
    },
};

#[derive(Debug, Deserialize)]
struct SapDiscoverRequest {
    #[serde(default)]
    connection: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct SapDiscoverResponse {
    ok: bool,
    message: String,
}

#[derive(Debug, Deserialize)]
struct SapSearchRequest {
    #[serde(default)]
    connection: serde_json::Value,
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
        .route("/api/sap/export-scan", post(export_scan))
}

async fn discover(
    Json(req): Json<SapDiscoverRequest>,
) -> Result<Json<SapDiscoverResponse>, (axum::http::StatusCode, String)> {
    let payload = serde_json::json!({
        "connection": req.connection,
    });
    let connection = parse_connection(&payload).map_err(internal)?;
    let bridge_dir = resolve_bridge_dir(&connection).map_err(internal)?;
    let mut bridge = AdtBridgeProcess::start(&bridge_dir).map_err(internal)?;
    bridge.connect(&connection).map_err(internal)?;

    Ok(Json(SapDiscoverResponse {
        ok: true,
        message: "SAP ADT connection validated successfully.".to_string(),
    }))
}

async fn search(
    Json(req): Json<SapSearchRequest>,
) -> Result<Json<SapSearchResponse>, (axum::http::StatusCode, String)> {
    let payload = serde_json::json!({
        "connection": req.connection,
    });
    let connection = parse_connection(&payload).map_err(internal)?;
    let bridge_dir = resolve_bridge_dir(&connection).map_err(internal)?;
    let mut bridge = AdtBridgeProcess::start(&bridge_dir).map_err(internal)?;
    bridge.connect(&connection).map_err(internal)?;

    let objects = bridge
        .list_package_objects(&req.package_name, req.include_subpackages)
        .map_err(internal)?
        .into_iter()
        .map(|item| SapObjectItem {
            uri: item.uri,
            source_uri: item.source_uri,
            name: item.name,
            object_type: item.object_type,
            package_name: item.package_name,
        })
        .collect::<Vec<_>>();

    Ok(Json(SapSearchResponse {
        ok: true,
        package_name: req.package_name,
        count: objects.len(),
        objects,
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
