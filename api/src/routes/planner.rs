use axum::{extract::{Path, Query, State}, routing::{get, post, put}, Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::capabilities::planner::FeaturePlanItem,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannerWorkspace {
    pub id: String,
    pub root_repo_path: String,
    pub title: String,
    pub is_default: bool,
    pub feature_plan_items: Vec<FeaturePlanItem>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreatePlannerWorkspaceRequest {
    pub root_repo_path: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub feature_plan_items: Vec<FeaturePlanItem>,
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
    pub feature_plan_items: Vec<FeaturePlanItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetDefaultPlannerResponse {
    pub ok: bool,
    pub planner: PlannerWorkspace,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/planners", get(list_planners).post(create_planner))
        .route("/api/planners/ensure", post(ensure_planner))
        .route("/api/planners/:planner_id", get(get_planner).put(update_planner_features))
        .route("/api/planners/:planner_id/default", post(set_default_planner))
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
            title TEXT NOT NULL,
            is_default INTEGER NOT NULL DEFAULT 0,
            features_json TEXT NOT NULL DEFAULT '[]',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(&state.db)
    .await?;

    ensure_column(state, "planner_workspaces", "is_default", "INTEGER NOT NULL DEFAULT 0").await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_planner_workspaces_root_default_updated ON planner_workspaces (root_repo_path, is_default, updated_at)")
        .execute(&state.db)
        .await?;

    Ok(())
}

fn row_to_planner(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<PlannerWorkspace> {
    let features_json: String = row.get("features_json");
    let feature_plan_items = serde_json::from_str::<Vec<FeaturePlanItem>>(&features_json).unwrap_or_default();
    let is_default = row.try_get::<i64, _>("is_default").unwrap_or(0) != 0;
    Ok(PlannerWorkspace {
        id: row.get("id"),
        root_repo_path: row.get("root_repo_path"),
        title: row.get("title"),
        is_default,
        feature_plan_items,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

async fn load_repo_feature_plan_items(state: &AppState, root_repo_path: &str) -> anyhow::Result<Vec<FeaturePlanItem>> {
    let root = normalize_repo_root(root_repo_path);
    if root.trim().is_empty() {
        return Ok(Vec::new());
    }

    let repo_row = sqlx::query(
        "SELECT id FROM planner_repos WHERE LOWER(REPLACE(root_repo_path, char(92), '/')) = ? OR ? LIKE LOWER(REPLACE(root_repo_path, char(92), '/')) || '/%' ORDER BY LENGTH(root_repo_path) DESC LIMIT 1"
    )
        .bind(&root)
        .bind(&root)
        .fetch_optional(&state.db)
        .await?;

    if let Some(repo_row) = repo_row {
        let repo_id: String = repo_row.get("id");
        let rows = sqlx::query("SELECT id, title, status, payload_json, created_at, updated_at FROM planner_features WHERE repo_id = ? ORDER BY sort_order ASC, created_at ASC")
            .bind(repo_id)
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

    let supervisor_rows = sqlx::query(
        "SELECT features_json FROM supervisor_runs WHERE features_json != '[]' AND (LOWER(REPLACE(root_repo_path, char(92), '/')) = ? OR ? LIKE LOWER(REPLACE(root_repo_path, char(92), '/')) || '/%') ORDER BY updated_at DESC"
    )
        .bind(&root)
        .bind(&root)
        .fetch_all(&state.db)
        .await?;

    for row in supervisor_rows {
        let features_json: String = row.get("features_json");
        let items = serde_json::from_str::<Vec<FeaturePlanItem>>(&features_json).unwrap_or_default();
        if !items.is_empty() {
            return Ok(items);
        }
    }

    Ok(Vec::new())
}

async fn hydrate_planner_features(state: &AppState, mut planner: PlannerWorkspace) -> anyhow::Result<PlannerWorkspace> {
    if !planner.feature_plan_items.is_empty() {
        return Ok(planner);
    }

    let items = load_repo_feature_plan_items(state, &planner.root_repo_path).await?;
    if items.is_empty() {
        return Ok(planner);
    }

    let now = Utc::now().to_rfc3339();
    let features_json = serde_json::to_string(&items)?;
    sqlx::query("UPDATE planner_workspaces SET features_json = ?, updated_at = ? WHERE id = ? AND features_json = '[]'")
        .bind(&features_json)
        .bind(&now)
        .bind(&planner.id)
        .execute(&state.db)
        .await?;

    planner.feature_plan_items = items;
    planner.updated_at = now;
    Ok(planner)
}

async fn list_planners(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Vec<PlannerWorkspace>>, (axum::http::StatusCode, String)> {
    ensure_planner_tables(&state).await.map_err(internal)?;
    let root = query.get("root_repo_path").map(|value| normalize_repo_root(value)).unwrap_or_default();
    let rows = if root.trim().is_empty() {
        sqlx::query("SELECT * FROM planner_workspaces ORDER BY root_repo_path ASC, is_default DESC, updated_at DESC")
            .fetch_all(&state.db)
            .await
            .map_err(internal)?
    } else {
        sqlx::query("SELECT * FROM planner_workspaces WHERE root_repo_path = ? OR ? LIKE root_repo_path || '/%' ORDER BY LENGTH(root_repo_path) DESC, is_default DESC, updated_at DESC")
            .bind(&root)
            .bind(&root)
            .fetch_all(&state.db)
            .await
            .map_err(internal)?
    };

    let mut planners = Vec::new();
    for row in rows {
        let planner = row_to_planner(row).map_err(internal)?;
        planners.push(hydrate_planner_features(&state, planner).await.map_err(internal)?);
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

    let existing_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM planner_workspaces WHERE root_repo_path = ?")
        .bind(&root)
        .fetch_one(&state.db)
        .await
        .map_err(internal)?;
    let is_default = req.make_default || existing_count == 0;

    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let title = req.title.filter(|value| !value.trim().is_empty()).unwrap_or_else(|| planner_title(&root));
    let features_json = serde_json::to_string(&req.feature_plan_items).map_err(internal)?;

    if is_default {
        sqlx::query("UPDATE planner_workspaces SET is_default = 0, updated_at = ? WHERE root_repo_path = ?")
            .bind(&now)
            .bind(&root)
            .execute(&state.db)
            .await
            .map_err(internal)?;
    }

    sqlx::query("INSERT INTO planner_workspaces (id, root_repo_path, title, is_default, features_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(&id)
        .bind(&root)
        .bind(&title)
        .bind(if is_default { 1_i64 } else { 0_i64 })
        .bind(features_json)
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .map_err(internal)?;

    get_planner(State(state), Path(id)).await
}

async fn ensure_planner(
    State(state): State<AppState>,
    Json(req): Json<EnsurePlannerWorkspaceRequest>,
) -> Result<Json<EnsurePlannerWorkspaceResponse>, (axum::http::StatusCode, String)> {
    ensure_planner_tables(&state).await.map_err(internal)?;
    let root = normalize_repo_root(&req.root_repo_path);
    if root.trim().is_empty() {
        return Err((axum::http::StatusCode::BAD_REQUEST, "root_repo_path is required".to_string()));
    }

    if let Some(row) = sqlx::query("SELECT * FROM planner_workspaces WHERE root_repo_path = ? OR ? LIKE root_repo_path || '/%' ORDER BY LENGTH(root_repo_path) DESC, is_default DESC, updated_at DESC LIMIT 1")
        .bind(&root)
        .bind(&root)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?
    {
        let planner = row_to_planner(row).map_err(internal)?;
        let planner = hydrate_planner_features(&state, planner).await.map_err(internal)?;
        return Ok(Json(EnsurePlannerWorkspaceResponse {
            created: false,
            planner,
        }));
    }

    let Json(planner) = create_planner(State(state), Json(CreatePlannerWorkspaceRequest {
        root_repo_path: root,
        title: req.title,
        feature_plan_items: Vec::new(),
        make_default: true,
    })).await?;

    Ok(Json(EnsurePlannerWorkspaceResponse {
        created: true,
        planner,
    }))
}

async fn get_planner(
    State(state): State<AppState>,
    Path(planner_id): Path<String>,
) -> Result<Json<PlannerWorkspace>, (axum::http::StatusCode, String)> {
    ensure_planner_tables(&state).await.map_err(internal)?;
    let row = sqlx::query("SELECT * FROM planner_workspaces WHERE id = ?")
        .bind(planner_id)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?
        .ok_or_else(|| (axum::http::StatusCode::NOT_FOUND, "planner not found".to_string()))?;
    let planner = row_to_planner(row).map_err(internal)?;
    let planner = hydrate_planner_features(&state, planner).await.map_err(internal)?;
    Ok(Json(planner))
}

async fn update_planner_features(
    State(state): State<AppState>,
    Path(planner_id): Path<String>,
    Json(req): Json<UpdatePlannerFeaturesRequest>,
) -> Result<Json<PlannerWorkspace>, (axum::http::StatusCode, String)> {
    ensure_planner_tables(&state).await.map_err(internal)?;
    let now = Utc::now().to_rfc3339();
    let features_json = serde_json::to_string(&req.feature_plan_items).map_err(internal)?;
    let result = sqlx::query("UPDATE planner_workspaces SET features_json = ?, updated_at = ? WHERE id = ?")
        .bind(features_json)
        .bind(now)
        .bind(&planner_id)
        .execute(&state.db)
        .await
        .map_err(internal)?;

    if result.rows_affected() == 0 {
        return Err((axum::http::StatusCode::NOT_FOUND, "planner not found".to_string()));
    }

    get_planner(State(state), Path(planner_id)).await
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
