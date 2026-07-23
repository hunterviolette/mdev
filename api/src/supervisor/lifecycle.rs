use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::capabilities::planner::FeaturePlanItem,
    models::WorkflowTemplateDefinition,
    supervisor::{patches, repo_snapshot, workflow_spawn},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorPoolKind {
    FeatureDevelopment,
    Refine,
    ManualShard,
    Integration,
}

impl SupervisorPoolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FeatureDevelopment => "feature_development",
            Self::Refine => "refine",
            Self::ManualShard => "manual_shard",
            Self::Integration => "integration",
        }
    }

    pub fn workspace_kind(self) -> SupervisorWorkspaceKind {
        match self {
            Self::Integration => SupervisorWorkspaceKind::Integration,
            _ => SupervisorWorkspaceKind::Shard,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorWorkspaceKind {
    Shard,
    Integration,
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
    pub initial_state: String,
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

pub async fn promise_supervisor_work_unit(
    state: &AppState,
    supervisor_run_id: Uuid,
    root_repo_path: &str,
    pool_kind: SupervisorPoolKind,
    work_unit_id: &str,
    feature_id: Option<&str>,
    title: &str,
    template_id: Option<Uuid>,
    mut work_unit_context: Value,
    priority: i64,
    queue_position: Option<i64>,
) -> Result<()> {
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
            Value::String(pool_kind.as_str().to_string()),
        );
        obj.insert(
            "pool_key".to_string(),
            Value::String(pool_kind.as_str().to_string()),
        );
        obj.insert(
            "work_unit_id".to_string(),
            Value::String(work_unit_id.to_string()),
        );

        if let Some(feature_id) = feature_id {
            obj.insert(
                "feature_id".to_string(),
                Value::String(feature_id.to_string()),
            );
        }

        if let Some(template_id) = template_id {
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
    let feature_id = feature_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(work_unit_id);

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
            NULL, NULL, NULL, NULL, ?, ?, NULL, '{}', ?,
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
    .bind(work_unit_id)
    .bind(supervisor_run_id.to_string())
    .bind(feature_id)
    .bind(pool_kind.as_str())
    .bind(title)
    .bind(root_repo_path)
    .bind(priority)
    .bind(queue_position)
    .bind(context_json)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;

    tracing::info!(
        supervisor_run_id = %supervisor_run_id,
        work_unit_id,
        feature_id,
        pool_kind = pool_kind.as_str(),
        "supervisor work unit promised in queued state"
    );

    Ok(())
}

pub async fn spawn_supervisor_workflow(
    state: &AppState,
    request: SupervisorWorkflowSpawnRequest,
) -> Result<SupervisorWorkflowSpawnResult> {
    let shard_id = request.shard_id.or_else(|| {
        if request.pool_kind.workspace_kind() == SupervisorWorkspaceKind::Shard {
            Some(Uuid::new_v4())
        } else {
            None
        }
    });
    let workspace_path = spawn_workspace(&request, shard_id)?;
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
        shard_id,
    )
    .await?;

    Ok(SupervisorWorkflowSpawnResult {
        work_unit_id: request.work_unit_id,
        shard_id,
        workflow_run_id,
        workspace_path,
        pool_kind: request.pool_kind.as_str().to_string(),
    })
}

pub async fn archive_supervisor_workflow_by_feature(
    state: &AppState,
    supervisor_run_id: Uuid,
    root_repo_path: &str,
    pool_kind: SupervisorPoolKind,
    feature_id: &str,
    reason: &str,
) -> Result<SupervisorWorkflowArchiveResult> {
    let row = sqlx::query(
        "SELECT id, workflow_run_id, COALESCE(NULLIF(workspace_path, ''), NULLIF(shard_path, ''), NULLIF(integration_path, '')) AS workspace_path FROM supervisor_work_units WHERE supervisor_run_id = ? AND kind = ? AND feature_id = ? AND archived_at IS NULL LIMIT 1",
    )
    .bind(supervisor_run_id.to_string())
    .bind(pool_kind.as_str())
    .bind(feature_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| anyhow!("supervisor work unit not found"))?;

    let work_unit_id: String = row.get("id");
    archive_supervisor_workflow_by_work_unit(
        state,
        supervisor_run_id,
        root_repo_path,
        &work_unit_id,
        reason,
    )
    .await
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

pub async fn archive_supervisor_integration_workflow(
    state: &AppState,
    supervisor_run_id: Uuid,
    root_repo_path: &str,
    reason: &str,
) -> Result<SupervisorWorkflowArchiveResult> {
    let row = sqlx::query(
        "SELECT id FROM supervisor_work_units WHERE supervisor_run_id = ? AND kind = 'integration' AND archived_at IS NULL ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(supervisor_run_id.to_string())
    .fetch_optional(&state.db)
    .await?;

    if let Some(row) = row {
        let work_unit_id: String = row.get("id");
        return archive_supervisor_workflow_by_work_unit(
            state,
            supervisor_run_id,
            root_repo_path,
            &work_unit_id,
            reason,
        )
        .await;
    }

    let run_row = sqlx::query("SELECT integration_run_id, integration_path FROM supervisor_runs WHERE id = ? LIMIT 1")
        .bind(supervisor_run_id.to_string())
        .fetch_one(&state.db)
        .await?;

    let workflow_run_id: Option<String> = run_row.get("integration_run_id");
    let workspace_path: Option<String> = run_row.get("integration_path");
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

    let workspace_deleted = if let Some(workspace_path) = workspace_path.as_deref().filter(|value| !value.trim().is_empty()) {
        repo_snapshot::delete_supervisor_workspace_path(root_repo_path, supervisor_run_id, workspace_path)?
    } else {
        false
    };

    Ok(SupervisorWorkflowArchiveResult {
        work_unit_id: format!("{}:integration", supervisor_run_id),
        workflow_run_id,
        workspace_path,
        workspace_deleted,
        archived: true,
    })
}

fn spawn_workspace(request: &SupervisorWorkflowSpawnRequest, shard_id: Option<Uuid>) -> Result<String> {
    match request.pool_kind.workspace_kind() {
        SupervisorWorkspaceKind::Integration => {
            let workspace = repo_snapshot::refresh_integration_from_worktree(
                &request.root_repo_path,
                request.supervisor_run_id,
            )?;
            patches::create_baseline(&workspace.integration)?;
            Ok(workspace.integration.to_string_lossy().to_string())
        }
        SupervisorWorkspaceKind::Shard => {
            let shard_id = shard_id.ok_or_else(|| anyhow!("shard_id is required for shard workspace"))?;
            let shard = repo_snapshot::refresh_shard_from_worktree(
                &request.root_repo_path,
                request.supervisor_run_id,
                shard_id,
            )?;
            patches::create_baseline(&shard)?;
            Ok(shard.to_string_lossy().to_string())
        }
    }
}

async fn upsert_work_unit_for_spawn(
    db: &SqlitePool,
    request: &SupervisorWorkflowSpawnRequest,
    workflow_run_id: Uuid,
    workspace_path: &str,
    shard_id: Option<Uuid>,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let feature_id = request.feature_id.clone().unwrap_or_else(|| request.work_unit_id.clone());
    let context_json = serde_json::to_string(&request.work_unit_context)?;
    let shard_path = if request.pool_kind == SupervisorPoolKind::Integration { None } else { Some(workspace_path) };
    let integration_path = if request.pool_kind == SupervisorPoolKind::Integration { Some(workspace_path) } else { None };

    sqlx::query(
        r#"
        INSERT INTO supervisor_work_units (
            id, supervisor_run_id, repo_id, feature_id, workflow_run_id, patch_id,
            kind, title, state, root_repo_path, workspace_path, shard_id, shard_path, integration_path,
            priority, queue_position, blocked_reason, waiting_user_input_json, context_json,
            archived_at, archived_reason, created_at, updated_at
        )
        VALUES (?, ?, NULL, ?, ?, NULL, ?, ?, ?, '', ?, ?, ?, ?, ?, ?, NULL, '{}', ?, NULL, NULL, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            feature_id = excluded.feature_id,
            workflow_run_id = excluded.workflow_run_id,
            kind = excluded.kind,
            title = excluded.title,
            state = excluded.state,
            workspace_path = excluded.workspace_path,
            shard_id = excluded.shard_id,
            shard_path = excluded.shard_path,
            integration_path = excluded.integration_path,
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
    .bind(&request.initial_state)
    .bind(workspace_path)
    .bind(shard_id.map(|value| value.to_string()))
    .bind(shard_path)
    .bind(integration_path)
    .bind(request.priority)
    .bind(request.queue_position)
    .bind(context_json)
    .bind(&now)
    .bind(&now)
    .execute(db)
    .await?;

    Ok(())
}

pub fn workflow_context_with_pool(mut context: Value, pool_kind: SupervisorPoolKind, work_unit_id: &str, workspace_path: Option<&str>) -> Value {
    if !context.is_object() {
        context = json!({});
    }
    if let Some(obj) = context.as_object_mut() {
        obj.insert("pool_key".to_string(), Value::String(pool_kind.as_str().to_string()));
        obj.insert("workflow_type".to_string(), Value::String(pool_kind.as_str().to_string()));
        obj.insert("work_unit_id".to_string(), Value::String(work_unit_id.to_string()));
        if let Some(workspace_path) = workspace_path {
            obj.insert("workspace_path".to_string(), Value::String(workspace_path.to_string()));
        }
    }
    context
}
