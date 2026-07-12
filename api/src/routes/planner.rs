use axum::{extract::{Path, Query, State}, routing::{get, post, put}, Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::capabilities::planner::FeaturePlanItem,
    supervisor,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannerWorkspace {
    pub id: String,
    pub root_repo_path: String,
    pub title: String,
    pub is_default: bool,
    pub feature_count: i64,
    pub features: Vec<FeaturePlanItem>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreatePlannerWorkspaceRequest {
    pub root_repo_path: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, alias = "feature_plan_items")]
    pub features: Vec<FeaturePlanItem>,
    #[serde(default)]
    pub make_default: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnsurePlannerWorkspaceRequest {
    pub root_repo_path: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnsurePlannerWorkspaceResponse {
    pub created: bool,
    pub planner: PlannerWorkspace,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdatePlannerFeaturesRequest {
    #[serde(default, alias = "feature_plan_items")]
    pub features: Vec<FeaturePlanItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetDefaultPlannerResponse {
    pub ok: bool,
    pub planner: PlannerWorkspace,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeletePlannerResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RefinePlannerFeatureRequest {
    pub supervisor_id: Uuid,
    #[serde(default)]
    pub workflow_template_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefinePlannerFeatureResponse {
    pub ok: bool,
    pub workflow_run_id: Uuid,
    pub reused: bool,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/planners", get(list_planners).post(create_planner))
        .route("/api/planners/create", post(create_planner))
        .route("/api/planners/ensure", post(ensure_planner))
        .route("/api/planners/:planner_id", get(get_planner).put(update_planner_features).delete(delete_planner))
        .route("/api/planner-features/:feature_id", get(get_planner_feature))
        .route("/api/planners/:planner_id/default", post(set_default_planner))
        .route("/api/planners/:planner_id/features/:feature_id/refine", post(refine_planner_feature))
}

fn normalize_repo_root(value: &str) -> String {
    let replaced = value.trim().replace('\\', "/");
    let trimmed = replaced.trim_end_matches('/').to_string();
    if cfg!(windows) {
        trimmed.to_lowercase()
    } else {
        trimmed
    }
}

fn planner_title(root: &str) -> String {
    let name = root
        .rsplit('/')
        .find(|part| !part.trim().is_empty())
        .unwrap_or("Repo");
    format!("{} Planner", name)
}

fn planner_repo_key(root: &str) -> String {
    let key = normalize_repo_root(root)
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .replace("--", "-")
        .trim_matches('-')
        .to_string();
    if key.is_empty() { Uuid::new_v4().to_string() } else { key }
}

async fn ensure_column(state: &AppState, table: &str, column: &str, definition: &str) -> anyhow::Result<()> {
    let rows = sqlx::query(&format!("PRAGMA table_info({})", table))
        .fetch_all(&state.db)
        .await?;
    let exists = rows
        .iter()
        .any(|row| row.get::<String, _>("name") == column);
    if !exists {
        sqlx::query(&format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, definition))
            .execute(&state.db)
            .await?;
    }
    Ok(())
}

async fn ensure_planner_tables(state: &AppState) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS planner_workspaces (
            id TEXT PRIMARY KEY,
            root_repo_path TEXT NOT NULL,
            repo_key TEXT NOT NULL DEFAULT '',
            title TEXT NOT NULL,
            is_default INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(&state.db)
    .await?;

    ensure_column(state, "planner_workspaces", "repo_key", "TEXT NOT NULL DEFAULT ''").await?;
    ensure_column(state, "planner_workspaces", "is_default", "INTEGER NOT NULL DEFAULT 0").await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS planner_features (
            id TEXT PRIMARY KEY,
            planner_id TEXT NOT NULL,
            title TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'rough',
            sort_order INTEGER NOT NULL DEFAULT 0,
            payload_json TEXT NOT NULL DEFAULT '{}',
            refined_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(&state.db)
    .await?;

    ensure_column(state, "planner_features", "planner_id", "TEXT NOT NULL DEFAULT ''").await?;
    ensure_column(state, "planner_features", "refined_at", "TEXT").await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_planner_workspaces_root_default_updated ON planner_workspaces (root_repo_path, is_default, updated_at)")
        .execute(&state.db)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_planner_features_planner_order ON planner_features (planner_id, sort_order, created_at)")
        .execute(&state.db)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_planner_features_planner_status ON planner_features (planner_id, status, updated_at)")
        .execute(&state.db)
        .await?;

    Ok(())
}

fn row_to_planner(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<PlannerWorkspace> {
    let is_default = row.try_get::<i64, _>("is_default").unwrap_or(0) != 0;
    Ok(PlannerWorkspace {
        id: row.get("id"),
        root_repo_path: row.get("root_repo_path"),
        title: row.get("title"),
        is_default,
        feature_count: row.try_get::<i64, _>("feature_count").unwrap_or(0),
        features: Vec::new(),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

async fn load_repo_feature_plan_items(state: &AppState, root_repo_path: &str) -> anyhow::Result<Vec<FeaturePlanItem>> {
    let root = normalize_repo_root(root_repo_path);
    if root.trim().is_empty() {
        return Ok(Vec::new());
    }

    let planner_row = sqlx::query(
        "SELECT id FROM planner_workspaces WHERE LOWER(REPLACE(root_repo_path, char(92), '/')) = ? OR ? LIKE LOWER(REPLACE(root_repo_path, char(92), '/')) || '/%' ORDER BY is_default DESC, LENGTH(root_repo_path) DESC, updated_at DESC LIMIT 1"
    )
        .bind(&root)
        .bind(&root)
        .fetch_optional(&state.db)
        .await?;

    if let Some(planner_row) = planner_row {
        let planner_id: String = planner_row.get("id");
        let rows = sqlx::query("SELECT id, title, status, payload_json, created_at, updated_at FROM planner_features WHERE planner_id = ? AND id NOT LIKE 'manual-%' AND COALESCE(status, '') != 'deleted' ORDER BY sort_order ASC, created_at ASC")
            .bind(planner_id)
            .fetch_all(&state.db)
            .await?;

        let items = rows.into_iter()
            .filter_map(|row| {
                let payload_json: String = row.get("payload_json");
                let mut item = serde_json::from_str::<FeaturePlanItem>(&payload_json).ok()?;
                if item.id.trim().is_empty() {
                    item.id = row.get("id");
                }
                if item.title.trim().is_empty() {
                    item.title = row.get("title");
                }
                let status_text: String = row.get("status");
                if let Ok(status) = serde_json::from_value::<crate::engine::capabilities::planner::FeaturePlanItemStatus>(serde_json::Value::String(status_text)) {
                    item.status = status;
                }
                Some(item)
            })
            .collect::<Vec<_>>();

        if !items.is_empty() {
            return Ok(items);
        }
    }

    Ok(Vec::new())
}

async fn load_planner_feature_plan_items(state: &AppState, planner_id: &str) -> anyhow::Result<Vec<FeaturePlanItem>> {
    let rows = sqlx::query("SELECT id, title, status, payload_json FROM planner_features WHERE planner_id = ? AND id NOT LIKE 'manual-%' AND COALESCE(status, '') != 'deleted' ORDER BY sort_order ASC, created_at ASC")
        .bind(planner_id)
        .fetch_all(&state.db)
        .await?;

    rows.into_iter()
        .map(|row| {
            let payload_json: String = row.get("payload_json");
            let mut item = serde_json::from_str::<FeaturePlanItem>(&payload_json)?;
            if item.id.trim().is_empty() {
                item.id = row.get("id");
            }
            if item.title.trim().is_empty() {
                item.title = row.get("title");
            }
            let status_text: String = row.get("status");
            if let Ok(status) = serde_json::from_value::<crate::engine::capabilities::planner::FeaturePlanItemStatus>(serde_json::Value::String(status_text)) {
                item.status = status;
            }
            Ok(item)
        })
        .collect()
}

async fn save_planner_feature_plan_items(state: &AppState, planner_id: &str, items: Vec<FeaturePlanItem>) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    let items = items
        .into_iter()
        .filter(|item| !item.id.starts_with("manual-"))
        .collect::<Vec<_>>();
    let ids = items.iter().map(|item| item.id.clone()).collect::<Vec<_>>();

    for (index, item) in items.iter().enumerate() {
        sqlx::query("INSERT INTO planner_features (id, planner_id, title, status, sort_order, payload_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET planner_id = excluded.planner_id, title = excluded.title, status = excluded.status, sort_order = excluded.sort_order, payload_json = excluded.payload_json, updated_at = excluded.updated_at")
            .bind(&item.id)
            .bind(planner_id)
            .bind(&item.title)
            .bind(serde_json::to_value(&item.status)?.as_str().unwrap_or("rough"))
            .bind(index as i64)
            .bind(serde_json::to_string(item)?)
            .bind(&now)
            .bind(&now)
            .execute(&state.db)
            .await?;
    }

    sqlx::query("UPDATE planner_features SET status = 'deleted', updated_at = ? WHERE planner_id = ? AND id NOT LIKE 'manual-%' AND id NOT IN (SELECT value FROM json_each(?))")
        .bind(&now)
        .bind(planner_id)
        .bind(serde_json::to_string(&ids)?)
        .execute(&state.db)
        .await?;

    sqlx::query("UPDATE planner_workspaces SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(planner_id)
        .execute(&state.db)
        .await?;

    Ok(())
}

async fn hydrate_planner_features(state: &AppState, mut planner: PlannerWorkspace) -> anyhow::Result<PlannerWorkspace> {
    planner.features = load_planner_feature_plan_items(state, &planner.id).await?;
    planner.feature_count = planner.features.len() as i64;
    Ok(planner)
}

async fn list_planners(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Vec<PlannerWorkspace>>, (axum::http::StatusCode, String)> {
    ensure_planner_tables(&state).await.map_err(internal)?;
    let root = query.get("root_repo_path").map(|value| normalize_repo_root(value)).unwrap_or_default();

    let rows = if root.trim().is_empty() {
        sqlx::query("SELECT id, root_repo_path, title, is_default, (SELECT COUNT(*) FROM planner_features pf WHERE pf.planner_id = planner_workspaces.id AND pf.id NOT LIKE 'manual-%' AND COALESCE(pf.status, '') != 'deleted') AS feature_count, created_at, updated_at FROM planner_workspaces ORDER BY root_repo_path ASC, is_default DESC, updated_at DESC")
            .fetch_all(&state.db)
            .await
            .map_err(internal)?
    } else {
        sqlx::query("SELECT id, root_repo_path, title, is_default, (SELECT COUNT(*) FROM planner_features pf WHERE pf.planner_id = planner_workspaces.id AND pf.id NOT LIKE 'manual-%' AND COALESCE(pf.status, '') != 'deleted') AS feature_count, created_at, updated_at FROM planner_workspaces WHERE LOWER(REPLACE(root_repo_path, char(92), '/')) = ? OR ? LIKE LOWER(REPLACE(root_repo_path, char(92), '/')) || '/%' ORDER BY LENGTH(root_repo_path) DESC, is_default DESC, updated_at DESC")
            .bind(&root)
            .bind(&root)
            .fetch_all(&state.db)
            .await
            .map_err(internal)?
    };

    let mut planners = Vec::new();
    for row in rows {
        planners.push(hydrate_planner_features(&state, row_to_planner(row).map_err(internal)?).await.map_err(internal)?);
    }

    if planners.is_empty() && !root.trim().is_empty() {
        let _ = ensure_planner_repo_for_root(&state, &root).await.map_err(internal)?;
        let now = Utc::now().to_rfc3339();
        let canonical = PlannerWorkspace {
            id: format!("canonical:{}", root),
            root_repo_path: root.clone(),
            title: planner_title(&root),
            is_default: true,
            features: load_repo_feature_plan_items(&state, &root).await.map_err(internal)?,
            feature_count: 0,
            created_at: now.clone(),
            updated_at: now,
        };
        planners.push(canonical);
    }

    Ok(Json(planners))
}

async fn create_planner(
    State(state): State<AppState>,
    Json(req): Json<CreatePlannerWorkspaceRequest>,
) -> Result<Json<PlannerWorkspace>, (axum::http::StatusCode, String)> {
    ensure_planner_tables(&state).await.map_err(internal)?;
    let root = normalize_repo_root(&req.root_repo_path);
    if root.trim().is_empty() {
        return Err((axum::http::StatusCode::BAD_REQUEST, "root_repo_path is required".to_string()));
    }

    let _ = ensure_planner_repo_for_root(&state, &root).await.map_err(internal)?;
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let title = req.title.filter(|value| !value.trim().is_empty()).unwrap_or_else(|| planner_title(&root));
    let make_default = req.make_default
        || sqlx::query("SELECT id FROM planner_workspaces WHERE root_repo_path = ? LIMIT 1")
            .bind(&root)
            .fetch_optional(&state.db)
            .await
            .map_err(internal)?
            .is_none();

    if make_default {
        sqlx::query("UPDATE planner_workspaces SET is_default = 0, updated_at = ? WHERE root_repo_path = ?")
            .bind(&now)
            .bind(&root)
            .execute(&state.db)
            .await
            .map_err(internal)?;
    }

    sqlx::query("INSERT INTO planner_workspaces (id, root_repo_path, repo_key, title, is_default, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(&id)
        .bind(&root)
        .bind(planner_repo_key(&root))
        .bind(&title)
        .bind(if make_default { 1_i64 } else { 0_i64 })
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .map_err(internal)?;

    save_planner_feature_plan_items(&state, &id, req.features).await.map_err(internal)?;

    get_planner(State(state), Path(id)).await
}

async fn ensure_planner(
    State(state): State<AppState>,
    Json(req): Json<EnsurePlannerWorkspaceRequest>,
) -> Result<Json<EnsurePlannerWorkspaceResponse>, (axum::http::StatusCode, String)> {
    let root = normalize_repo_root(&req.root_repo_path);
    if root.trim().is_empty() {
        return Err((axum::http::StatusCode::BAD_REQUEST, "root_repo_path is required".to_string()));
    }

    let existing_repo = sqlx::query("SELECT id FROM planner_workspaces WHERE root_repo_path = ? AND is_default = 1 LIMIT 1")
        .bind(&root)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?;

    let created = existing_repo.is_none();
    let repo_id = ensure_planner_repo_for_root(&state, &root).await.map_err(internal)?;
    let now = Utc::now().to_rfc3339();
    let row = sqlx::query("SELECT id FROM planner_workspaces WHERE root_repo_path = ? AND is_default = 1 LIMIT 1")
        .bind(&root)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?;

    let planner_id = if let Some(row) = row {
        row.get::<String, _>("id")
    } else {
        let id = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO planner_workspaces (id, root_repo_path, repo_key, title, is_default, created_at, updated_at) VALUES (?, ?, ?, ?, 1, ?, ?)")
            .bind(&id)
            .bind(&root)
            .bind(planner_repo_key(&root))
            .bind(req.title.filter(|value| !value.trim().is_empty()).unwrap_or_else(|| planner_title(&root)))
            .bind(&now)
            .bind(&now)
            .execute(&state.db)
            .await
            .map_err(internal)?;
        id
    };

    let _ = repo_id;
    let planner = get_planner(State(state), Path(planner_id)).await?.0;
    Ok(Json(EnsurePlannerWorkspaceResponse { created, planner }))
}

async fn get_planner(
    State(state): State<AppState>,
    Path(planner_id): Path<String>,
) -> Result<Json<PlannerWorkspace>, (axum::http::StatusCode, String)> {
    ensure_planner_tables(&state).await.map_err(internal)?;

    if let Some(row) = sqlx::query("SELECT id, root_repo_path, title, is_default, (SELECT COUNT(*) FROM planner_features pf WHERE pf.planner_id = planner_workspaces.id AND pf.id NOT LIKE 'manual-%' AND COALESCE(pf.status, '') != 'deleted') AS feature_count, created_at, updated_at FROM planner_workspaces WHERE id = ? LIMIT 1")
        .bind(&planner_id)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?
    {
        return Ok(Json(hydrate_planner_features(&state, row_to_planner(row).map_err(internal)?).await.map_err(internal)?));
    }

    let root = planner_root_from_id(&state, &planner_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| (axum::http::StatusCode::NOT_FOUND, "planner not found".to_string()))?;
    let now = Utc::now().to_rfc3339();

    Ok(Json(PlannerWorkspace {
        id: format!("canonical:{}", root),
        root_repo_path: root.clone(),
        title: planner_title(&root),
        is_default: true,
        features: load_repo_feature_plan_items(&state, &root).await.map_err(internal)?,
        feature_count: 0,
        created_at: now.clone(),
        updated_at: now,
    }))
}

async fn get_planner_feature(
    State(state): State<AppState>,
    Path(feature_id): Path<String>,
) -> Result<Json<FeaturePlanItem>, (axum::http::StatusCode, String)> {
    let row = sqlx::query("SELECT id, title, status, payload_json FROM planner_features WHERE id = ? AND id NOT LIKE 'manual-%' LIMIT 1")
        .bind(&feature_id)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?
        .ok_or_else(|| (axum::http::StatusCode::NOT_FOUND, "planner feature not found".to_string()))?;

    let payload_json: String = row.get("payload_json");
    let mut item = serde_json::from_str::<FeaturePlanItem>(&payload_json).map_err(internal)?;
    if item.id.trim().is_empty() {
        item.id = row.get("id");
    }
    if item.title.trim().is_empty() {
        item.title = row.get("title");
    }
    let status_text: String = row.get("status");
    if let Ok(status) = serde_json::from_value::<crate::engine::capabilities::planner::FeaturePlanItemStatus>(serde_json::Value::String(status_text)) {
        item.status = status;
    }

    Ok(Json(item))
}

async fn ensure_planner_repo_for_root(state: &AppState, root: &str) -> anyhow::Result<String> {
    let normalized_root = normalize_repo_root(root);
    if normalized_root.trim().is_empty() {
        anyhow::bail!("root_repo_path is required");
    }

    if let Some(row) = sqlx::query("SELECT id FROM planner_workspaces WHERE root_repo_path = ? ORDER BY is_default DESC, updated_at DESC, created_at DESC LIMIT 1")
        .bind(&normalized_root)
        .fetch_optional(&state.db)
        .await?
    {
        return Ok(row.get("id"));
    }

    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let repo_key = normalized_root
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .replace("--", "-")
        .trim_matches('-')
        .to_string();
    let repo_key = if repo_key.is_empty() { id.clone() } else { repo_key };

    sqlx::query("INSERT INTO planner_workspaces (id, root_repo_path, repo_key, title, is_default, created_at, updated_at) VALUES (?, ?, ?, ?, 1, ?, ?)")
        .bind(&id)
        .bind(&normalized_root)
        .bind(repo_key)
        .bind(planner_title(&normalized_root))
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await?;

    Ok(id)
}

async fn planner_root_from_id(state: &AppState, planner_id: &str) -> anyhow::Result<Option<String>> {
    if let Some(root) = planner_id.strip_prefix("canonical:") {
        let root = normalize_repo_root(root);
        if !root.trim().is_empty() {
            return Ok(Some(root));
        }
    }

    let row = sqlx::query("SELECT root_repo_path FROM planner_workspaces WHERE id = ? LIMIT 1")
        .bind(planner_id)
        .fetch_optional(&state.db)
        .await?;

    Ok(row.map(|row| row.get::<String, _>("root_repo_path")))
}

async fn persist_canonical_planner_features(state: &AppState, root: &str, items: Vec<FeaturePlanItem>) -> anyhow::Result<PlannerWorkspace> {
    let root = normalize_repo_root(root);
    let planner_id = ensure_planner_repo_for_root(state, &root).await?;
    let now = Utc::now().to_rfc3339();
    let items = items
        .into_iter()
        .filter(|item| !item.id.starts_with("manual-"))
        .collect::<Vec<_>>();
    let ids = items.iter().map(|item| item.id.clone()).collect::<Vec<_>>();

    for (index, item) in items.iter().enumerate() {
        sqlx::query("INSERT INTO planner_features (id, planner_id, title, status, sort_order, payload_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET planner_id = excluded.planner_id, title = excluded.title, status = excluded.status, sort_order = excluded.sort_order, payload_json = excluded.payload_json, updated_at = excluded.updated_at")
            .bind(&item.id)
            .bind(&planner_id)
            .bind(&item.title)
            .bind(serde_json::to_value(&item.status)?.as_str().unwrap_or("fine"))
            .bind(index as i64)
            .bind(serde_json::to_string(item)?)
            .bind(&now)
            .bind(&now)
            .execute(&state.db)
            .await?;
    }

    sqlx::query("UPDATE planner_features SET status = 'deleted', updated_at = ? WHERE planner_id = ? AND id NOT LIKE 'manual-%' AND id NOT IN (SELECT value FROM json_each(?))")
        .bind(&now)
        .bind(&planner_id)
        .bind(serde_json::to_string(&ids)?)
        .execute(&state.db)
        .await?;

    Ok(PlannerWorkspace {
        id: planner_id.clone(),
        root_repo_path: root.clone(),
        title: planner_title(&root),
        is_default: true,
        features: load_repo_feature_plan_items(state, &root).await?,
        feature_count: 0,
        created_at: now.clone(),
        updated_at: now,
    })
}

async fn update_planner_features(
    State(state): State<AppState>,
    Path(planner_id): Path<String>,
    Json(req): Json<UpdatePlannerFeaturesRequest>,
) -> Result<Json<PlannerWorkspace>, (axum::http::StatusCode, String)> {
    ensure_planner_tables(&state).await.map_err(internal)?;

    let exists = sqlx::query("SELECT id FROM planner_workspaces WHERE id = ? LIMIT 1")
        .bind(&planner_id)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?
        .is_some();

    if !exists {
        return Err((axum::http::StatusCode::NOT_FOUND, "planner not found".to_string()));
    }

    save_planner_feature_plan_items(&state, &planner_id, req.features)
        .await
        .map_err(internal)?;

    get_planner(State(state), Path(planner_id)).await
}

async fn delete_planner(
    State(state): State<AppState>,
    Path(planner_id): Path<String>,
) -> Result<Json<DeletePlannerResponse>, (axum::http::StatusCode, String)> {
    ensure_planner_tables(&state).await.map_err(internal)?;
    let result = sqlx::query("DELETE FROM planner_workspaces WHERE id = ?")
        .bind(planner_id)
        .execute(&state.db)
        .await
        .map_err(internal)?;

    if result.rows_affected() == 0 {
        return Err((axum::http::StatusCode::NOT_FOUND, "planner not found".to_string()));
    }

    Ok(Json(DeletePlannerResponse { ok: true }))
}

async fn refine_planner_feature(
    State(state): State<AppState>,
    Path((_planner_id, feature_id)): Path<(String, String)>,
    Json(req): Json<RefinePlannerFeatureRequest>,
) -> Result<Json<RefinePlannerFeatureResponse>, (axum::http::StatusCode, String)> {
    ensure_planner_tables(&state).await.map_err(internal)?;

    let result = supervisor::refine_supervisor_feature(
        &state,
        req.supervisor_id,
        json!({
            "feature_id": feature_id,
            "workflow_template_id": req.workflow_template_id
        }),
    )
    .await
    .map_err(internal)?;

    let workflow_run_id = result
        .get("workflow_run_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| internal("supervisor refinement did not return workflow_run_id"))?;

    Ok(Json(RefinePlannerFeatureResponse {
        ok: true,
        workflow_run_id,
        reused: result
            .get("reused")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }))
}

async fn set_default_planner(
    State(state): State<AppState>,
    Path(planner_id): Path<String>,
) -> Result<Json<SetDefaultPlannerResponse>, (axum::http::StatusCode, String)> {
    ensure_planner_tables(&state).await.map_err(internal)?;
    let planner = get_planner(State(state.clone()), Path(planner_id.clone())).await?.0;
    let now = Utc::now().to_rfc3339();

    sqlx::query("UPDATE planner_workspaces SET is_default = 0, updated_at = ? WHERE root_repo_path = ?")
        .bind(&now)
        .bind(&planner.root_repo_path)
        .execute(&state.db)
        .await
        .map_err(internal)?;

    sqlx::query("UPDATE planner_workspaces SET is_default = 1, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&planner_id)
        .execute(&state.db)
        .await
        .map_err(internal)?;

    let planner = get_planner(State(state), Path(planner_id)).await?.0;
    Ok(Json(SetDefaultPlannerResponse {
        ok: true,
        planner,
    }))
}

fn internal(err: impl std::fmt::Display) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
