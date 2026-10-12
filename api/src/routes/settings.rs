use axum::{extract::{Path, State}, http::StatusCode, routing::{get, post, delete}, Json, Router};
use serde::{Deserialize, Serialize};
use std::path::Path as FsPath;
use std::collections::HashSet;
use tokio::process::Command;
use std::time::Duration;
use crate::environment_file::{self, EnvironmentFileStatus};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::Row;

use crate::environment_file::{default_browser_bridge_url, default_browser_cdp_url};

use crate::{
    app_state::AppState,
    models::{AppSettings, PatchAppSettingsRequest},
};

const APP_SETTINGS_ROW_ID: &str = "global";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/app-settings", get(get_app_settings).patch(patch_app_settings))
        .route("/api/app-settings/environment-file", get(get_environment_file).put(put_environment_file))
        .route("/api/app-settings/browsers", get(list_browsers).post(add_browser))
        .route("/api/app-settings/browsers/:id", delete(remove_browser))
}

#[derive(Debug, Clone, Serialize)]
struct BrowserOption {
    id: String,
    name: String,
    executable_path: String,
    source: String,
    selected: bool,
}

#[derive(Deserialize)]
struct AddBrowserRequest {
    executable_path: String,
}

fn browser_name(path: &str) -> String {
    let name = FsPath::new(path).file_name().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
    if name.contains("msedge") || name.contains("microsoft edge") { "Microsoft Edge".to_string() }
    else if name.contains("brave") { "Brave".to_string() }
    else if name.contains("chrome") || name.contains("google chrome") { "Google Chrome".to_string() }
    else if name.contains("chromium") { "Chromium".to_string() }
    else { "Custom Chromium Browser".to_string() }
}

fn browser_id(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}



fn browser_options(settings: &Value) -> Vec<BrowserOption> {
    let browser = settings.get("browser").unwrap_or(&Value::Null);
    let selected = browser.get("edge_executable_path").and_then(Value::as_str).unwrap_or("");
    let mut paths = crate::browser_registry::discovered_browser_paths().into_iter().map(|path| (path, "detected".to_string())).collect::<Vec<_>>();
    if let Some(custom) = browser.get("custom_browsers").and_then(Value::as_array) {
        for item in custom {
            if let Some(path) = item.as_str() {
                paths.push((path.to_string(), "custom".to_string()));
            }
        }
    }
    if !selected.is_empty() && !paths.iter().any(|(path, _)| browser_id(path) == browser_id(selected)) {
        paths.push((selected.to_string(), "legacy".to_string()));
    }
    let mut seen = HashSet::new();
    paths.into_iter().filter(|(path, _)| FsPath::new(path).is_file()).filter(|(path, _)| seen.insert(browser_id(path))).map(|(path, source)| BrowserOption {
        id: browser_id(&path),
        name: browser_name(&path),
        selected: browser_id(&path) == browser_id(selected),
        executable_path: path,
        source,
    }).collect()
}

async fn list_browsers(State(state): State<AppState>) -> Result<Json<Vec<BrowserOption>>, (StatusCode, String)> {
    let settings = load_app_settings_value(&state).await?;
    Ok(Json(browser_options(&settings)))
}

async fn validate_browser(path: &str) -> Result<Value, (StatusCode, String)> {
    let executable = FsPath::new(path);
    if !executable.is_absolute() || !executable.is_file() {
        return Err((StatusCode::BAD_REQUEST, "browser executable must be an existing absolute file".to_string()));
    }
    let current_exe = std::env::current_exe().map_err(internal)?;
    let mut roots = vec![std::env::current_dir().map_err(internal)?];
    if let Some(parent) = current_exe.parent() { roots.push(parent.to_path_buf()); }
    let bridge_dir = roots.iter().flat_map(|root| root.ancestors()).map(|root| root.join("bridge/dist/validate_browser.js")).find(|path| path.is_file()).ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, "browser validation bridge is unavailable".to_string()))?;
    let runtime_root = bridge_dir.parent().and_then(|path| path.parent()).and_then(|path| path.parent()).unwrap_or(FsPath::new("."));
    let bundled_node = runtime_root.join("runtime").join(if cfg!(windows) { "node.exe" } else { "node" });
    let node = if bundled_node.is_file() { bundled_node.to_string_lossy().into_owned() } else { "node".to_string() };
    let mut command = Command::new(node);
    command.arg(&bridge_dir).arg(path).current_dir(runtime_root).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(25), command.output()).await.map_err(|_| (StatusCode::REQUEST_TIMEOUT, "browser validation timed out".to_string()))?.map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    if !output.status.success() {
        return Err((StatusCode::BAD_REQUEST, String::from_utf8_lossy(&output.stderr).trim().to_string()));
    }
    let result: Value = serde_json::from_slice(&output.stdout).map_err(internal)?;
    if result.get("valid").and_then(Value::as_bool) != Some(true) || result.get("playwright_connected").and_then(Value::as_bool) != Some(true) {
        return Err((StatusCode::BAD_REQUEST, "browser did not pass Playwright CDP validation".to_string()));
    }
    Ok(result)
}

async fn persist_browser_settings(state: &AppState, mut settings: Value) -> Result<(), (StatusCode, String)> {
    settings = normalize_app_settings_value(settings);
    let now = Utc::now().to_rfc3339();
    let serialized = serde_json::to_string(&settings).map_err(internal)?;
    sqlx::query("INSERT INTO app_settings (id, settings_json, created_at, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET settings_json = excluded.settings_json, updated_at = excluded.updated_at")
        .bind(APP_SETTINGS_ROW_ID).bind(serialized).bind(&now).bind(&now).execute(&state.db).await.map_err(internal)?;
    Ok(())
}

async fn add_browser(State(state): State<AppState>, Json(request): Json<AddBrowserRequest>) -> Result<Json<Value>, (StatusCode, String)> {
    let canonical = std::fs::canonicalize(request.executable_path.trim()).map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    let path = canonical.to_string_lossy().into_owned();
    let validation = validate_browser(&path).await?;
    let mut settings = load_app_settings_value(&state).await?;
    let browser = settings.get_mut("browser").and_then(Value::as_object_mut).ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, "browser settings unavailable".to_string()))?;
    let custom = browser.entry("custom_browsers").or_insert_with(|| json!([])).as_array_mut().ok_or_else(|| (StatusCode::BAD_REQUEST, "invalid custom browser registry".to_string()))?;
    if !custom.iter().any(|value| value.as_str().map(|item| browser_id(item) == browser_id(&path)).unwrap_or(false)) {
        custom.push(Value::String(path.clone()));
    }
    persist_browser_settings(&state, settings).await?;
    Ok(Json(json!({ "id": browser_id(&path), "name": browser_name(&path), "executable_path": path, "source": "custom", "validation": validation })))
}

async fn remove_browser(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, (StatusCode, String)> {
    let mut settings = load_app_settings_value(&state).await?;
    let browser = settings.get_mut("browser").and_then(Value::as_object_mut).ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, "browser settings unavailable".to_string()))?;
    let selected = browser.get("edge_executable_path").and_then(Value::as_str).unwrap_or("").to_string();
    let custom = browser.entry("custom_browsers").or_insert_with(|| json!([])).as_array_mut().ok_or_else(|| (StatusCode::BAD_REQUEST, "invalid custom browser registry".to_string()))?;
    let before = custom.len();
    custom.retain(|value| value.as_str().map(|path| browser_id(path) != id).unwrap_or(true));
    if custom.len() == before { return Err((StatusCode::NOT_FOUND, "custom browser not found".to_string())); }
    if browser_id(&selected) == id { browser.insert("edge_executable_path".to_string(), Value::String(String::new())); }
    persist_browser_settings(&state, settings).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct UpdateEnvironmentFileRequest {
    path: String,
}

async fn get_environment_file() -> Result<Json<EnvironmentFileStatus>, (StatusCode, String)> {
    let app_root = environment_file::app_root().map_err(internal)?;
    let status = environment_file::status(&app_root).map_err(internal)?;
    Ok(Json(status))
}

async fn put_environment_file(
    Json(request): Json<UpdateEnvironmentFileRequest>,
) -> Result<Json<EnvironmentFileStatus>, (StatusCode, String)> {
    let app_root = environment_file::app_root().map_err(internal)?;
    let result = environment_file::save_config(&app_root, &request.path)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    Ok(Json(result))
}

async fn get_app_settings(
    State(state): State<AppState>,
) -> Result<Json<AppSettings>, (axum::http::StatusCode, String)> {
    let settings = load_app_settings(&state).await?;
    Ok(Json(settings))
}

async fn patch_app_settings(
    State(state): State<AppState>,
    Json(req): Json<PatchAppSettingsRequest>,
) -> Result<Json<AppSettings>, (axum::http::StatusCode, String)> {
    if let Some(browser) = req.patch.get("browser").and_then(Value::as_object) {
        if let Some(path) = browser.get("edge_executable_path") {
            let path = path.as_str().ok_or_else(|| (StatusCode::BAD_REQUEST, "browser executable path must be a string".to_string()))?;
            let path = path.trim();
            if !path.is_empty() {
                let executable = std::path::Path::new(path);
                if !executable.is_absolute() || !executable.is_file() {
                    return Err((StatusCode::BAD_REQUEST, "browser executable must be an existing absolute file path".to_string()));
                }
                let settings = load_app_settings_value(&state).await?;
                if !browser_options(&settings).iter().any(|option| browser_id(&option.executable_path) == browser_id(path)) {
                    return Err((StatusCode::BAD_REQUEST, "browser must be discovered or validated and added before selection".to_string()));
                }
            }
        }
    }
    let now = Utc::now();
    let current_value = load_app_settings_value(&state).await?;
    let mut merged = current_value;
    merge_json(&mut merged, req.patch);
    let normalized = normalize_app_settings_value(merged);
    let settings_json = serde_json::to_string_pretty(&normalized).map_err(internal)?;

    let created_at = load_existing_created_at(&state)
        .await?
        .unwrap_or(now);

    sqlx::query(
        r#"
        INSERT INTO app_settings (id, settings_json, created_at, updated_at)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            settings_json = excluded.settings_json,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(APP_SETTINGS_ROW_ID)
    .bind(settings_json)
    .bind(created_at.to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(&state.db)
    .await
    .map_err(internal)?;

    let settings: AppSettings = serde_json::from_value(normalized).map_err(internal)?;
    Ok(Json(settings))
}

async fn load_app_settings(
    state: &AppState,
) -> Result<AppSettings, (axum::http::StatusCode, String)> {
    let value = load_app_settings_value(state).await?;
    serde_json::from_value(value).map_err(internal)
}

async fn load_app_settings_value(
    state: &AppState,
) -> Result<Value, (axum::http::StatusCode, String)> {
    let row = sqlx::query("SELECT settings_json FROM app_settings WHERE id = ?")
        .bind(APP_SETTINGS_ROW_ID)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?;

    let value = match row {
        Some(row) => serde_json::from_str::<Value>(row.get::<String, _>("settings_json").as_str()).map_err(internal)?,
        None => default_app_settings_value(),
    };

    Ok(normalize_app_settings_value(value))
}

async fn load_existing_created_at(
    state: &AppState,
) -> Result<Option<DateTime<Utc>>, (axum::http::StatusCode, String)> {
    let row = sqlx::query("SELECT created_at FROM app_settings WHERE id = ?")
        .bind(APP_SETTINGS_ROW_ID)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?;

    match row {
        Some(row) => parse_ts(row.get("created_at")).map(Some),
        None => Ok(None),
    }
}

fn default_app_settings_value() -> Value {
    json!({
        "browser": {
            "edge_executable_path": "",
            "chrome_executable_path": "",
            "custom_browsers": [],
            "default_cdp_url": default_browser_cdp_url()
                .expect("WORKFLOW_BROWSER_CDP_HOST and WORKFLOW_BROWSER_CDP_PORT must be set"),
            "default_inference_browser_url": "https://website.com/",
            "launch_on_connect": true
        },
        "bridges": {
            "browser_bridge_url": default_browser_bridge_url()
                .expect("WORKFLOW_BROWSER_BRIDGE_HOST and WORKFLOW_BROWSER_BRIDGE_PORT must be set"),
            "auto_start": false,
            "poll_interval_ms": 2000,
            "connect_timeout_ms": 10000
        },
        "git": {
            "poll_enabled": true,
            "poll_interval_ms": 2000
        },
        "repo_sync": {
            "enabled": false,
            "listen_host": "0.0.0.0",
            "listen_port": 8787,
            "mappings": []
        }
    })
}

fn normalize_app_settings_value(value: Value) -> Value {
    let defaults = default_app_settings_value();
    let mut normalized = match value {
        Value::Object(map) => Value::Object(map),
        _ => defaults.clone(),
    };

    let obj = normalized.as_object_mut().expect("app settings must be object");

    for key in ["browser", "bridges", "git", "repo_sync"] {
        let fallback = defaults.get(key).cloned().unwrap_or_else(|| json!({}));
        let slot = obj.entry(key.to_string()).or_insert_with(|| fallback.clone());
        if !slot.is_object() {
            *slot = fallback.clone();
            continue;
        }
        if let (Some(slot_obj), Some(fallback_obj)) = (slot.as_object_mut(), fallback.as_object()) {
            for (fallback_key, fallback_value) in fallback_obj {
                slot_obj.entry(fallback_key.clone()).or_insert_with(|| fallback_value.clone());
            }
        }
    }

    Value::Object(obj.clone())
}

fn merge_json(target: &mut Value, patch: Value) {
    match (target, patch) {
        (Value::Object(target_map), Value::Object(patch_map)) => {
            for (key, patch_value) in patch_map {
                match target_map.get_mut(&key) {
                    Some(target_value) => merge_json(target_value, patch_value),
                    None => {
                        target_map.insert(key, patch_value);
                    }
                }
            }
        }
        (target_slot, patch_value) => {
            *target_slot = patch_value;
        }
    }
}

fn parse_ts(value: String) -> Result<DateTime<Utc>, (axum::http::StatusCode, String)> {
    chrono::DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(internal)
}

fn internal(err: impl std::fmt::Display) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
