use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::capabilities::planner::FeaturePlanItem,
    supervisor::{patches, repo_snapshot, workflow_spawn},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorPoolKind {
    Refine,
    Feature,
    Manual,
    Integration,
}

impl SupervisorPoolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Refine => "refine",
            Self::Feature => "feature",
            Self::Manual => "manual",
            Self::Integration => "integration",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SupervisorWorkflowSpawnRequest {
    pub supervisor_run_id: Uuid,
    pub root_repo_path: String,
    pub pool_kind: SupervisorPoolKind,
    pub work_unit_id: String,
    pub shard_id: Option<Uuid>,
    pub feature_id: Option<String>,
    pub title: String,
    pub item: FeaturePlanItem,
    pub template_id: Option<Uuid>,
    pub workflow_context: Value,
    pub work_unit_context: Value,
    pub initial_state: crate::supervisor::models::SupervisorWorkUnitState,
    pub priority: i64,
    pub queue_position: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SupervisorWorkflowSpawnResult {
    pub work_unit_id: String,
    pub shard_id: Option<Uuid>,
    pub workflow_run_id: Uuid,
    pub workspace_path: String,
    pub pool_kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SupervisorWorkflowArchiveResult {
    pub work_unit_id: String,
    pub workflow_run_id: Option<String>,
    pub workspace_path: Option<String>,
    pub workspace_deleted: bool,
    pub archived: bool,
}

#[derive(Debug, Clone)]
pub struct SupervisorWorkUnitPromiseRequest {
    pub supervisor_run_id: Uuid,
    pub root_repo_path: String,
    pub pool_kind: SupervisorPoolKind,
    pub work_unit_id: String,
    pub feature_id: Option<String>,
    pub title: String,
    pub template_id: Option<Uuid>,
    pub context: crate::supervisor::models::SupervisorWorkUnitStoredContext,
    pub priority: i64,
    pub queue_position: Option<i64>,
}

pub async fn promise_supervisor_work_unit(
    state: &AppState,
    request: SupervisorWorkUnitPromiseRequest,
) -> Result<()> {
    let mut work_unit_context = serde_json::to_value(&request.context)?;
    if !work_unit_context.is_object() {
        work_unit_context = json!({});
    }

    if let Some(obj) = work_unit_context.as_object_mut() {
        obj.insert(
            "status".to_string(),
            Value::String("queued".to_string()),
        );
        obj.insert(
            "materialization_state".to_string(),
            Value::String("pending".to_string()),
        );
        obj.insert(
            "workflow_type".to_string(),
            Value::String(request.pool_kind.as_str().to_string()),
        );
        obj.insert(
            "pool_key".to_string(),
            Value::String(request.pool_kind.as_str().to_string()),
        );
        obj.insert(
            "work_unit_id".to_string(),
            Value::String(request.work_unit_id.clone()),
        );

        if let Some(feature_id) = request.feature_id.as_deref() {
            obj.insert(
                "feature_id".to_string(),
                Value::String(feature_id.to_string()),
            );
        }

        if let Some(template_id) = request.template_id {
            obj.insert(
                "template_id".to_string(),
                Value::String(template_id.to_string()),
            );
            obj.insert(
                "planned_workflow_template_id".to_string(),
                Value::String(template_id.to_string()),
            );
        }
    }

    let now = Utc::now().to_rfc3339();
    let context_json = serde_json::to_string(&work_unit_context)?;
    let feature_id = request
        .feature_id
        .as_deref()
        .filter(|value| !value.trim().is_empty());

    sqlx::query(
        r#"
        INSERT INTO supervisor_work_units (
            id,
            supervisor_run_id,
            repo_id,
            feature_id,
            workflow_run_id,
            patch_id,
            kind,
            title,
            state,
            root_repo_path,
            workspace_path,
            shard_id,
            shard_path,
            integration_path,
            integration_state,
            priority,
            queue_position,
            blocked_reason,
            waiting_user_input_json,
            context_json,
            archived_at,
            archived_reason,
            created_at,
            updated_at
        )
        VALUES (
            ?, ?, NULL, ?, NULL, NULL, ?, ?, 'queued', ?,
            NULL, NULL, NULL, NULL, 'available', ?, ?, NULL, '{}', ?,
            NULL, NULL, ?, ?
        )
        ON CONFLICT(id) DO UPDATE SET
            supervisor_run_id = excluded.supervisor_run_id,
            feature_id = excluded.feature_id,
            workflow_run_id = NULL,
            patch_id = NULL,
            kind = excluded.kind,
            title = excluded.title,
            state = 'queued',
            root_repo_path = excluded.root_repo_path,
            workspace_path = NULL,
            shard_id = NULL,
            shard_path = NULL,
            integration_path = NULL,
            integration_state = 'available',
            priority = excluded.priority,
            queue_position = excluded.queue_position,
            blocked_reason = NULL,
            waiting_user_input_json = '{}',
            context_json = excluded.context_json,
            archived_at = NULL,
            archived_reason = NULL,
            updated_at = excluded.updated_at
        WHERE supervisor_work_units.archived_at IS NOT NULL
           OR supervisor_work_units.state IN ('deleted', 'archived', 'cancelled', 'failed')
        "#,
    )
    .bind(&request.work_unit_id)
    .bind(request.supervisor_run_id.to_string())
    .bind(feature_id)
    .bind(request.pool_kind.as_str())
    .bind(&request.title)
    .bind(&request.root_repo_path)
    .bind(request.priority)
    .bind(request.queue_position)
    .bind(context_json)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;

    tracing::info!(
        supervisor_run_id = %request.supervisor_run_id,
        work_unit_id = %request.work_unit_id,
        feature_id = ?request.feature_id,
        pool_kind = request.pool_kind.as_str(),
        "supervisor work unit promised in queued state"
    );

    Ok(())
}

pub async fn spawn_supervisor_workflow(
    state: &AppState,
    request: SupervisorWorkflowSpawnRequest,
) -> Result<SupervisorWorkflowSpawnResult> {
    let workspace_id = request.shard_id.unwrap_or_else(Uuid::new_v4);
    let workspace_path = spawn_workspace(&request, workspace_id)?;
    let workflow_run_id = workflow_spawn::spawn_feature_plan_item_workflow(
        state,
        &request.item,
        &workspace_path,
        request.template_id,
        request.workflow_context.clone(),
    )
    .await?;

    let mut materialized_context = request.work_unit_context.clone();
    if !materialized_context.is_object() {
        materialized_context = json!({});
    }
    if let Some(obj) = materialized_context.as_object_mut() {
        obj.insert(
            "materialization_state".to_string(),
            Value::String("materialized".to_string()),
        );
    }

    let mut materialized_request = request.clone();
    materialized_request.work_unit_context = materialized_context;

    upsert_work_unit_for_spawn(
        &state.db,
        &materialized_request,
        workflow_run_id,
        &workspace_path,
    )
    .await?;

    Ok(SupervisorWorkflowSpawnResult {
        work_unit_id: request.work_unit_id,
        shard_id: Some(workspace_id),
        workflow_run_id,
        workspace_path,
        pool_kind: request.pool_kind.as_str().to_string(),
    })
}



pub async fn archive_supervisor_workflow_by_work_unit(
    state: &AppState,
    supervisor_run_id: Uuid,
    root_repo_path: &str,
    work_unit_id: &str,
    reason: &str,
) -> Result<SupervisorWorkflowArchiveResult> {
    let row = sqlx::query(
        "SELECT workflow_run_id, COALESCE(NULLIF(workspace_path, ''), NULLIF(shard_path, ''), NULLIF(integration_path, '')) AS workspace_path FROM supervisor_work_units WHERE id = ? AND supervisor_run_id = ? AND archived_at IS NULL LIMIT 1",
    )
    .bind(work_unit_id)
    .bind(supervisor_run_id.to_string())
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| anyhow!("supervisor work unit not found"))?;

    let workflow_run_id: Option<String> = row.get("workflow_run_id");
    let workspace_path: Option<String> = row.get("workspace_path");
    let now = Utc::now().to_rfc3339();

    if let Some(workflow_run_id) = workflow_run_id.as_deref().filter(|value| !value.trim().is_empty()) {
        sqlx::query("UPDATE workflow_runs SET status = 'archived', archived_at = ?, archived_reason = ?, updated_at = ? WHERE id = ?")
            .bind(&now)
            .bind(reason)
            .bind(&now)
            .bind(workflow_run_id)
            .execute(&state.db)
            .await?;
    }

    sqlx::query("UPDATE supervisor_work_units SET state = 'archived', archived_at = ?, archived_reason = ?, updated_at = ? WHERE id = ? AND supervisor_run_id = ?")
        .bind(&now)
        .bind(reason)
        .bind(&now)
        .bind(work_unit_id)
        .bind(supervisor_run_id.to_string())
        .execute(&state.db)
        .await?;

    let workspace_deleted = if let Some(workspace_path) = workspace_path.as_deref().filter(|value| !value.trim().is_empty()) {
        repo_snapshot::delete_supervisor_workspace_path(root_repo_path, supervisor_run_id, workspace_path)?
    } else {
        false
    };

    Ok(SupervisorWorkflowArchiveResult {
        work_unit_id: work_unit_id.to_string(),
        workflow_run_id,
        workspace_path,
        workspace_deleted,
        archived: true,
    })
}



fn spawn_workspace(request: &SupervisorWorkflowSpawnRequest, workspace_id: Uuid) -> Result<String> {
    let workspace_path = repo_snapshot::materialize_work_unit_workspace(
        &request.root_repo_path,
        request.supervisor_run_id,
        workspace_id,
    )?;
    patches::create_baseline(&workspace_path)?;
    Ok(workspace_path.to_string_lossy().to_string())
}

async fn upsert_work_unit_for_spawn(
    db: &SqlitePool,
    request: &SupervisorWorkflowSpawnRequest,
    workflow_run_id: Uuid,
    workspace_path: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let feature_id = request.feature_id.clone().unwrap_or_else(|| request.work_unit_id.clone());
    let context_json = serde_json::to_string(&request.work_unit_context)?;

    sqlx::query(
        r#"
        INSERT INTO supervisor_work_units (
            id, supervisor_run_id, repo_id, feature_id, workflow_run_id, patch_id,
            kind, title, state, root_repo_path, workspace_path,
            priority, queue_position, blocked_reason, waiting_user_input_json, context_json,
            archived_at, archived_reason, created_at, updated_at
        )
        VALUES (?, ?, NULL, ?, ?, NULL, ?, ?, ?, '', ?, ?, ?, NULL, '{}', ?, NULL, NULL, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            feature_id = excluded.feature_id,
            workflow_run_id = excluded.workflow_run_id,
            kind = excluded.kind,
            title = excluded.title,
            state = excluded.state,
            workspace_path = excluded.workspace_path,
            priority = excluded.priority,
            queue_position = excluded.queue_position,
            context_json = excluded.context_json,
            archived_at = NULL,
            archived_reason = NULL,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(&request.work_unit_id)
    .bind(request.supervisor_run_id.to_string())
    .bind(feature_id)
    .bind(workflow_run_id.to_string())
    .bind(request.pool_kind.as_str())
    .bind(&request.title)
    .bind(request.initial_state.as_str())
    .bind(workspace_path)
    .bind(request.priority)
    .bind(request.queue_position)
    .bind(context_json)
    .bind(&now)
    .bind(&now)
    .execute(db)
    .await?;

    Ok(())
}


