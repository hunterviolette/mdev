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
    supervisor::workflow_spawn,
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

#[derive(Debug, Clone, Serialize)]
pub struct DeletePlannerResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RefinePlannerFeatureRequest {
    #[serde(default)]
    pub supervisor_id: Option<Uuid>,
    #[serde(default)]
    pub workflow_template_id: Option<Uuid>,
    #[serde(default)]
    pub template_id: Option<Uuid>,
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
    let feature_plan_items = serde_json::from_str::<Vec<FeaturePlanItem>>(&features_json)
        .unwrap_or_default()
        .into_iter()
        .filter(|item| !item.id.starts_with("manual-"))
        .collect::<Vec<_>>();
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
        let rows = sqlx::query("SELECT id, title, status, payload_json, created_at, updated_at FROM planner_features WHERE repo_id = ? AND id NOT LIKE 'manual-%' ORDER BY sort_order ASC, created_at ASC")
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

async fn hydrate_planner_features(_state: &AppState, planner: PlannerWorkspace) -> anyhow::Result<PlannerWorkspace> {
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
        planners.push(row_to_planner(row).map_err(internal)?);
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
    let features_json = serde_json::to_string(
        &req.feature_plan_items
            .into_iter()
            .filter(|item| !item.id.starts_with("manual-"))
            .collect::<Vec<_>>()
    ).map_err(internal)?;

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
    Ok(Json(planner))
}

async fn update_planner_features(
    State(state): State<AppState>,
    Path(planner_id): Path<String>,
    Json(req): Json<UpdatePlannerFeaturesRequest>,
) -> Result<Json<PlannerWorkspace>, (axum::http::StatusCode, String)> {
    ensure_planner_tables(&state).await.map_err(internal)?;
    let now = Utc::now().to_rfc3339();
    let features_json = serde_json::to_string(
        &req.feature_plan_items
            .into_iter()
            .filter(|item| !item.id.starts_with("manual-"))
            .collect::<Vec<_>>()
    ).map_err(internal)?;
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

const DEFAULT_REFINEMENT_TEMPLATE_NAME: &str = "Default refinement workflow";

async fn default_refinement_workflow_template_id(state: &AppState) -> anyhow::Result<Option<Uuid>> {
    let row = sqlx::query("SELECT id FROM workflow_templates WHERE name = ?")
        .bind(DEFAULT_REFINEMENT_TEMPLATE_NAME)
        .fetch_optional(&state.db)
        .await?;
    row.map(|row| Uuid::parse_str(row.get::<String, _>("id").as_str()).map_err(Into::into))
        .transpose()
}

async fn refine_planner_feature(
    State(state): State<AppState>,
    Path((planner_id, feature_id)): Path<(String, String)>,
    Json(req): Json<RefinePlannerFeatureRequest>,
) -> Result<Json<RefinePlannerFeatureResponse>, (axum::http::StatusCode, String)> {
    ensure_planner_tables(&state).await.map_err(internal)?;
    let planner = get_planner(State(state.clone()), Path(planner_id.clone())).await?.0;
    let feature = planner
        .feature_plan_items
        .iter()
        .find(|item| item.id == feature_id)
        .cloned()
        .ok_or_else(|| (axum::http::StatusCode::NOT_FOUND, format!("planner feature {} not found", feature_id)))?;

    let workflow_template_id = req.workflow_template_id
        .or(req.template_id)
        .or(default_refinement_workflow_template_id(&state).await.map_err(internal)?)
        .ok_or_else(|| (axum::http::StatusCode::BAD_REQUEST, "workflow_template_id is required for planner feature refinement".to_string()))?;

    let workflow_run_id = workflow_spawn::spawn_feature_plan_item_workflow(
        &state,
        &feature,
        &planner.root_repo_path,
        Some(workflow_template_id),
        json!({
            "supervisor_run_id": req.supervisor_id.map(|value| value.to_string()),
            "planner_workspace_id": planner.id,
            "planner_title": planner.title,
            "feature_id": feature.id,
            "input_source": "planner_workspace_feature",
            "structured_output": {
                "enabled": true,
                "schema_armed": true,
                "schema_id": "supervisor_feature_plan_item_v1",
                "auto_apply_armed": true,
                "preserve_rough_definition": true,
                "apply_handler": "planner_workspace_item",
                "rough_definition": feature.rough_summary.clone().unwrap_or_else(|| feature.summary.clone())
            }
        }),
    ).await.map_err(internal)?;

    if let Some(supervisor_id) = req.supervisor_id {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO supervisor_work_units (
                id, supervisor_run_id, repo_id, feature_id, workflow_run_id, patch_id,
                kind, title, state, root_repo_path, shard_path, integration_path,
                priority, queue_position, blocked_reason, waiting_user_input_json, context_json,
                created_at, updated_at
            )
            VALUES (?, ?, NULL, ?, ?, NULL, 'refine', ?, 'queued', ?, ?, NULL, 0, NULL, NULL, '{}', ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                workflow_run_id = excluded.workflow_run_id,
                title = excluded.title,
                state = excluded.state,
                root_repo_path = excluded.root_repo_path,
                shard_path = excluded.shard_path,
                context_json = excluded.context_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(format!("{}:refine:{}", supervisor_id, feature.id))
        .bind(supervisor_id.to_string())
        .bind(feature.id.as_str())
        .bind(workflow_run_id.to_string())
        .bind(feature.title.as_str())
        .bind(planner.root_repo_path.as_str())
        .bind(planner.root_repo_path.as_str())
        .bind(serde_json::to_string(&json!({
            "source": "planner_workspace_refine",
            "workflow_type": "refine",
            "pool_key": "refine",
            "planner_workspace_id": planner.id,
            "planner_title": planner.title,
            "feature_id": feature.id,
            "template_id": workflow_template_id,
            "workflow_run_id": workflow_run_id
        })).map_err(internal)?)
        .bind(now.as_str())
        .bind(now.as_str())
        .execute(&state.db)
        .await
        .map_err(internal)?;
    }

    Ok(Json(RefinePlannerFeatureResponse {
        ok: true,
        workflow_run_id,
        reused: false,
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
