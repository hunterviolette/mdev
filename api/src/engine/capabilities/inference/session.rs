use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{sqlite::SqliteRow, Row, SqlitePool};
use uuid::Uuid;

use super::{InferenceCapabilityConfig, InferenceConfig, InferenceRunContext, InferenceTransport};
use super::super::registry::CapabilityContext;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedInferenceRoute {
    pub name: String,
    pub stage_type: String,
    pub existing_session_id: Option<String>,
    pub config: InferenceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceSession {
    pub id: String,
    pub repo_ref: String,
    pub title: String,
    pub transport: InferenceTransport,
    pub lifecycle: String,
    pub config: InferenceConfig,
    pub transport_state: Value,
    pub automation_state: Value,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_used_at: Option<String>,
}

pub fn resolve_inference_route_from_run(
    run: &crate::models::WorkflowRun,
    step: &crate::models::WorkflowStepDefinition,
) -> Result<ResolvedInferenceRoute> {
    let inference = inference_config_from_run(run)?;
    let stage_binding = inference
        .stage_sessions
        .get(step.step_type.as_str())
        .ok_or_else(|| anyhow!("inference stage '{}' does not have a session binding", step.step_type))?;
    let session_name = stage_binding
        .session_name()
        .trim();
    if session_name.is_empty() {
        bail!("inference stage '{}' has an empty session binding", step.step_type);
    }

    let mut resolved = resolve_named_inference_session_from_config(&inference, session_name)?;
    resolved.stage_type = step.step_type.clone();
    resolved.existing_session_id = stage_binding
        .existing_session_id()
        .map(str::to_string);
    Ok(resolved)
}

pub fn resolve_named_inference_session_from_run(
    run: &crate::models::WorkflowRun,
    session_name: &str,
) -> Result<ResolvedInferenceRoute> {
    let inference = inference_config_from_run(run)?;
    resolve_named_inference_session_from_config(&inference, session_name)
}

fn inference_config_from_run(run: &crate::models::WorkflowRun) -> Result<InferenceCapabilityConfig> {
    let context = serde_json::from_value::<InferenceRunContext>(run.context.clone())
        .map_err(|err| anyhow!("failed to decode workflow inference context: {}", err))?;

    Ok(context
        .workflow_engine
        .map(|engine| engine.global_state)
        .or(context.global_state)
        .and_then(|global_state| global_state.capabilities.inference)
        .unwrap_or_default())
}

fn resolve_named_inference_session_from_config(
    inference: &InferenceCapabilityConfig,
    session_name: &str,
) -> Result<ResolvedInferenceRoute> {
    let config = inference
        .sessions
        .get(session_name)
        .cloned()
        .ok_or_else(|| anyhow!("inference session configuration '{}' is not configured", session_name))?;

    Ok(ResolvedInferenceRoute {
        name: session_name.to_string(),
        stage_type: String::new(),
        existing_session_id: None,
        config,
    })
}

pub async fn resolve_inference_route(
    ctx: &CapabilityContext<'_>,
) -> Result<ResolvedInferenceRoute> {
    let run = crate::engine::load_run(ctx.state, ctx.run_id).await?;
    resolve_inference_route_from_run(&run, ctx.step)
}

pub async fn bound_session(
    ctx: &CapabilityContext<'_>,
    stage_type: &str,
) -> Result<Option<InferenceSession>> {
    load_bound_session(&ctx.state.db, ctx.run_id, stage_type).await
}

pub async fn load_bound_session(
    db: &SqlitePool,
    run_id: Uuid,
    stage_type: &str,
) -> Result<Option<InferenceSession>> {
    let row = sqlx::query(
        r#"
        SELECT s.id, s.repo_ref, s.title, s.transport, s.lifecycle,
               s.config_json, s.transport_state_json, s.automation_state_json,
               s.status, s.created_at, s.updated_at, s.last_used_at
        FROM workflow_inference_stage_bindings b
        JOIN inference_sessions s ON s.id = b.inference_session_id
        WHERE b.workflow_run_id = ? AND b.stage_type = ?
        "#,
    )
    .bind(run_id.to_string())
    .bind(stage_type)
    .fetch_optional(db)
    .await?;

    row.map(row_to_inference_session).transpose()
}

pub async fn create_session(
    db: &SqlitePool,
    repo_ref: &str,
    run_id: Uuid,
    route: &ResolvedInferenceRoute,
) -> Result<InferenceSession> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let title = format!("{} · {}", route.name, &id[..8]);
    let transport = transport_name(&route.config.transport);
    let mut tx = db.begin().await?;

    sqlx::query(
        r#"
        INSERT INTO inference_sessions (
            id, repo_ref, title, transport, lifecycle,
            config_json, transport_state_json, automation_state_json,
            status, created_at, updated_at, last_used_at
        ) VALUES (?, ?, ?, ?, ?, ?, '{}', '{}', 'pending', ?, ?, NULL)
        "#,
    )
    .bind(&id)
    .bind(repo_ref)
    .bind(&title)
    .bind(transport)
    .bind(lifecycle_name(&route.config.lifecycle))
    .bind(serde_json::to_string(&route.config)?)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO workflow_inference_stage_bindings (
            workflow_run_id, stage_type, inference_session_id, updated_at
        ) VALUES (?, ?, ?, ?)
        ON CONFLICT(workflow_run_id, stage_type)
        DO UPDATE SET inference_session_id = excluded.inference_session_id, updated_at = excluded.updated_at
        "#,
    )
    .bind(run_id.to_string())
    .bind(&route.stage_type)
    .bind(&id)
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    get_session(db, &id)
        .await?
        .ok_or_else(|| anyhow!("created inference session could not be loaded"))
}

pub async fn activate_session(
    db: &SqlitePool,
    session_id: &str,
    transport_state: Value,
) -> Result<InferenceSession> {
    let now = Utc::now().to_rfc3339();

    sqlx::query(
        "UPDATE inference_sessions SET transport_state_json = ?, status = 'active', updated_at = ?, last_used_at = ? WHERE id = ?",
    )
    .bind(serde_json::to_string(&transport_state)?)
    .bind(&now)
    .bind(&now)
    .bind(session_id)
    .execute(db)
    .await?;

    get_session(db, session_id)
        .await?
        .ok_or_else(|| anyhow!("inference session '{}' was not found", session_id))
}

pub async fn mark_session_failed(
    db: &SqlitePool,
    session_id: &str,
    error: &anyhow::Error,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();

    sqlx::query(
        "UPDATE inference_sessions SET transport_state_json = ?, status = 'error', updated_at = ? WHERE id = ?",
    )
    .bind(serde_json::to_string(&json!({
        "error": format!("{:#}", error)
    }))?)
    .bind(now)
    .bind(session_id)
    .execute(db)
    .await?;

    Ok(())
}

pub async fn archive_session(
    db: &SqlitePool,
    session_id: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let mut tx = db.begin().await?;

    sqlx::query(
        "UPDATE inference_sessions SET status = 'archived', updated_at = ? WHERE id = ? AND status != 'archived'",
    )
    .bind(&now)
    .bind(session_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "DELETE FROM workflow_inference_stage_bindings WHERE inference_session_id = ?",
    )
    .bind(session_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

pub async fn archive_non_persistent_sessions_on_startup(
    db: &SqlitePool,
) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let mut tx = db.begin().await?;

    let result = sqlx::query(
        "UPDATE inference_sessions SET status = 'archived', updated_at = ? WHERE lifecycle = 'non_persistent' AND status != 'archived'",
    )
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "DELETE FROM workflow_inference_stage_bindings WHERE inference_session_id IN (SELECT id FROM inference_sessions WHERE lifecycle = 'non_persistent' AND status = 'archived')",
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(result.rows_affected())
}

pub async fn touch_session(
    db: &SqlitePool,
    session_id: &str,
    transport_state: Value,
) -> Result<InferenceSession> {
    activate_session(db, session_id, transport_state).await
}

pub async fn get_session(
    db: &SqlitePool,
    session_id: &str,
) -> Result<Option<InferenceSession>> {
    let row = sqlx::query(
        r#"
        SELECT id, repo_ref, title, transport, lifecycle,
               config_json, transport_state_json, automation_state_json,
               status, created_at, updated_at, last_used_at
        FROM inference_sessions
        WHERE id = ?
        "#,
    )
    .bind(session_id)
    .fetch_optional(db)
    .await?;

    row.map(row_to_inference_session).transpose()
}

pub async fn list_repo_sessions(
    db: &SqlitePool,
    repo_ref: &str,
) -> Result<Vec<InferenceSession>> {
    let rows = sqlx::query(
        r#"
        SELECT id, repo_ref, title, transport, lifecycle,
               config_json, transport_state_json, automation_state_json,
               status, created_at, updated_at, last_used_at
        FROM inference_sessions
        WHERE repo_ref = ? AND status != 'archived'
        ORDER BY COALESCE(last_used_at, updated_at) DESC, created_at DESC
        "#,
    )
    .bind(repo_ref)
    .fetch_all(db)
    .await?;

    rows.into_iter().map(row_to_inference_session).collect()
}

pub async fn bindings_for_run(
    db: &SqlitePool,
    run_id: Uuid,
) -> Result<BTreeMap<String, String>> {
    let rows = sqlx::query(
        "SELECT stage_type, inference_session_id FROM workflow_inference_stage_bindings WHERE workflow_run_id = ? ORDER BY stage_type",
    )
    .bind(run_id.to_string())
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("stage_type"),
                row.get::<String, _>("inference_session_id"),
            )
        })
        .collect())
}

pub async fn binding_id(
    db: &SqlitePool,
    run_id: Uuid,
    stage_type: &str,
) -> Result<Option<String>> {
    let row = sqlx::query(
        "SELECT inference_session_id FROM workflow_inference_stage_bindings WHERE workflow_run_id = ? AND stage_type = ?",
    )
    .bind(run_id.to_string())
    .bind(stage_type)
    .fetch_optional(db)
    .await?;

    Ok(row.map(|row| row.get::<String, _>("inference_session_id")))
}

pub async fn bind_session(
    db: &SqlitePool,
    run_id: Uuid,
    stage_type: &str,
    session_id: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();

    sqlx::query(
        r#"
        INSERT INTO workflow_inference_stage_bindings (
            workflow_run_id, stage_type, inference_session_id, updated_at
        ) VALUES (?, ?, ?, ?)
        ON CONFLICT(workflow_run_id, stage_type)
        DO UPDATE SET inference_session_id = excluded.inference_session_id, updated_at = excluded.updated_at
        "#,
    )
    .bind(run_id.to_string())
    .bind(stage_type)
    .bind(session_id)
    .bind(now)
    .execute(db)
    .await?;

    Ok(())
}

pub async fn bound_automation_state(
    db: &SqlitePool,
    run_id: Uuid,
    route_name: &str,
) -> Result<Value> {
    Ok(load_bound_session(db, run_id, route_name)
        .await?
        .map(|session| session.automation_state)
        .unwrap_or_else(|| json!({})))
}

pub async fn update_bound_automation_state(
    db: &SqlitePool,
    run_id: Uuid,
    route_name: &str,
    automation_state: &Value,
) -> Result<()> {
    let Some(session_id) = binding_id(db, run_id, route_name).await? else {
        return Ok(());
    };

    sqlx::query(
        "UPDATE inference_sessions SET automation_state_json = ?, updated_at = ? WHERE id = ?",
    )
    .bind(serde_json::to_string(automation_state)?)
    .bind(Utc::now().to_rfc3339())
    .bind(session_id)
    .execute(db)
    .await?;

    Ok(())
}

pub fn validate_session_for_route(
    session: &InferenceSession,
    route: &ResolvedInferenceRoute,
) -> Result<()> {
    if session.transport != route.config.transport {
        bail!(
            "inference session '{}' uses transport '{}' but route '{}' uses '{}'",
            session.id,
            transport_name(&session.transport),
            route.name,
            transport_name(&route.config.transport)
        );
    }

    Ok(())
}

pub fn resolve_stage_session_name(
    inference: &InferenceCapabilityConfig,
    step_type: &str,
) -> Option<String> {
    inference
        .stage_sessions
        .get(step_type)
        .map(|binding| binding.session_name())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn row_to_inference_session(row: SqliteRow) -> Result<InferenceSession> {
    let transport = match row.get::<String, _>("transport").as_str() {
        "browser" => InferenceTransport::Browser,
        "api" => InferenceTransport::Api,
        other => return Err(anyhow!("unknown inference transport '{}'", other)),
    };

    let config_text = row
        .try_get::<String, _>("config_json")
        .unwrap_or_else(|_| "{}".to_string());
    let mut config = serde_json::from_str::<InferenceConfig>(&config_text)
        .unwrap_or_default();
    config.transport = transport.clone();

    let transport_state_text = row.get::<String, _>("transport_state_json");
    let transport_state = serde_json::from_str::<Value>(&transport_state_text)
        .unwrap_or_else(|_| json!({}));
    let automation_state_text = row
        .try_get::<String, _>("automation_state_json")
        .unwrap_or_else(|_| "{}".to_string());
    let automation_state = serde_json::from_str::<Value>(&automation_state_text)
        .unwrap_or_else(|_| json!({}));

    Ok(InferenceSession {
        id: row.get("id"),
        repo_ref: row.get("repo_ref"),
        title: row.get("title"),
        transport,
        lifecycle: row
            .try_get::<String, _>("lifecycle")
            .unwrap_or_else(|_| "persistent".to_string()),
        config,
        transport_state,
        automation_state,
        status: row.get("status"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        last_used_at: row
            .try_get::<Option<String>, _>("last_used_at")
            .unwrap_or(None),
    })
}

fn lifecycle_name(lifecycle: &super::InferenceSessionLifecycle) -> &'static str {
    match lifecycle {
        super::InferenceSessionLifecycle::Persistent => "persistent",
        super::InferenceSessionLifecycle::NonPersistent => "non_persistent",
    }
}

fn transport_name(transport: &InferenceTransport) -> &'static str {
    match transport {
        InferenceTransport::Api => "api",
        InferenceTransport::Browser => "browser",
    }
}
