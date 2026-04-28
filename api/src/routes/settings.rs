use axum::{extract::State, routing::{get, patch}, Json, Router};
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::Row;

use crate::runtime_env::{default_browser_bridge_url, default_browser_cdp_url};

use crate::{
    app_state::AppState,
    models::{AppSettings, PatchAppSettingsRequest},
};

const APP_SETTINGS_ROW_ID: &str = "global";

pub fn router() -> Router<AppState> {
    Router::new().route("/api/app-settings", get(get_app_settings).patch(patch_app_settings))
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

    for key in ["browser", "bridges", "git"] {
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
