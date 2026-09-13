use std::path::Path;

use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine,
    models::{StageExecutionNode, StageExecutionNodeKind, WorkflowRun, WorkflowStepDefinition},
    supervisor::patches,
};

use super::{
    Stage,
    StageCapabilities,
    StageOutcome,
    StagePlanContext,
    StagePrepareContext,
    StageStatus,
    StageTransition,
};

pub struct MergePatchesStage;

pub static STAGE: MergePatchesStage = MergePatchesStage;

inventory::submit! {
    super::StageRegistration::new(&STAGE)
}

impl Stage for MergePatchesStage {
    fn stage_type(&self) -> &'static str {
        "merge_patches"
    }

    fn descriptor(&self) -> crate::models::WorkflowStageDescriptor {
        crate::routes::merge_patches_descriptor()
    }

    fn capabilities(&self) -> StageCapabilities {
        StageCapabilities::new(["git_patch_payload"])
    }

    fn prepare_state(
        &self,
        context: StagePrepareContext<'_>,
        local_state: Value,
    ) -> Result<Value> {
        prepare_merge_patches_state(context.step, local_state)
    }

    fn build_execution_plan(
        &self,
        _context: StagePlanContext<'_>,
    ) -> Result<Vec<StageExecutionNode>> {
        Ok(build_merge_patches_execution_plan())
    }
}

fn build_merge_patches_execution_plan() -> Vec<StageExecutionNode> {
    vec![StageExecutionNode {
        kind: StageExecutionNodeKind::Capability,
        key: "git_patch_payload".to_string(),
        enabled: true,
        config: json!({}),
        input_mapping: json!({}),
        output_mapping: json!({}),
        run_after: vec![],
        condition: Value::Null,
    }]
}

fn prepare_merge_patches_state(_step: &WorkflowStepDefinition, local_state: Value) -> Result<Value> {
    Ok(local_state)
}

pub async fn execute_stage(
    state: &AppState,
    run_id: Uuid,
    run: &mut WorkflowRun,
    step: &WorkflowStepDefinition,
    repo_ref: &str,
    mut local_state: Value,
) -> Result<StageOutcome> {
    let stage_execution_id = local_state
        .get("_stage_execution_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let supervisor_context = run.context.get("supervisor").cloned().unwrap_or_else(|| json!({}));
    let supervisor_run_id = supervisor_context
        .get("supervisor_id")
        .or_else(|| supervisor_context.get("supervisor_run_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let pool_type = supervisor_context
        .get("pool_type")
        .or_else(|| supervisor_context.get("pool_key"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("integration")
        .to_string();

    if supervisor_run_id.is_some() && pool_type != "integration" {
        return Err(anyhow!("merge_patches requires integration pool_type"));
    }

    let supervisor_runtime_context = match supervisor_run_id.as_deref() {
        Some(supervisor_run_id) => load_supervisor_runtime_context(state, supervisor_run_id).await?,
        None => json!({}),
    };
    let root_repo_path = supervisor_runtime_context
        .get("root_repo_path")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(repo_ref)
        .to_string();
    let sprint_id = supervisor_context
        .get("sprint_id")
        .or_else(|| supervisor_runtime_context.get("sprint_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let integration_batch_id = supervisor_run_id.clone().or_else(|| sprint_id.clone()).unwrap_or_else(|| run_id.to_string());

    let configured_patch_items = step
        .config
        .get("patches")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| {
            run.context
                .get("workflow_engine")
                .and_then(|value| value.get("global_state"))
                .and_then(|value| value.get("supervisor"))
                .and_then(|value| value.get("patches"))
                .and_then(Value::as_array)
                .cloned()
        })
        .unwrap_or_default();
    let patch_items = if let Some(supervisor_run_id) = supervisor_run_id.as_deref().filter(|value| !value.trim().is_empty()) {
        resolve_supervisor_integration_pool_patches(state, supervisor_run_id).await?
    } else if !configured_patch_items.is_empty() {
        configured_patch_items
    } else if let Some(sprint_id) = sprint_id.as_deref() {
        resolve_sprint_feature_patches(state, sprint_id).await?
    } else {
        return Err(anyhow!("merge_patches requires supervisor_run_id, configured patch inputs, or legacy sprint_id"));
    };
    let mut applied = Vec::new();
    let mut failed = Vec::new();
    let mut capability_results = Vec::new();

    if patch_items.is_empty() {
        failed.push(json!({
            "error": "merge_patches found no dynamic feature-pool or staged manual inputs with shard_path",
            "sprint_id": sprint_id,
            "integration_batch_id": integration_batch_id
        }));
    }

    for patch in patch_items {
        let shard_path = patch.get("shard_path").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        let workflow_type = patch.get("workflow_type").and_then(Value::as_str).unwrap_or("feature_development");
        let execution_item_id = patch.get("execution_item_id").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        let feature_id = if workflow_type == "manual_shard" { String::new() } else { execution_item_id.clone() };
        let workflow_run_id = patch.get("workflow_run_id").and_then(Value::as_str).map(str::to_string);
        let capability_invocation_id = Uuid::new_v4().to_string();
        let patch_source = patch
            .get("patch_owner")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| if workflow_type == "manual_shard" { "supervisor_manual_shards" } else { "feature_pool" });
        let capability_config = json!({
            "mode": "generate_apply_persist_patch_text",
            "source": patch_source,
            "sprint_id": sprint_id,
            "integration_batch_id": integration_batch_id,
            "execution_item_id": execution_item_id,
            "feature_id": if feature_id.is_empty() { Value::Null } else { Value::String(feature_id.clone()) },
            "workflow_type": workflow_type,
            "shard_path": shard_path,
            "target_repo_ref": repo_ref
        });

        if shard_path.is_empty() || (workflow_type != "manual_shard" && feature_id.is_empty()) || (workflow_type == "manual_shard" && execution_item_id.is_empty()) {
            failed.push(json!({
                "patch": patch,
                "error": "integration patch source requires shard_path and either feature_id or manual_shard_id"
            }));
            break;
        }

        append_git_patch_payload_event(
            state,
            run_id,
            step,
            stage_execution_id.as_deref(),
            capability_invocation_id.as_str(),
            "info",
            "git_patch_payload_started",
            "git patch payload started",
            capability_config.clone(),
            json!({}),
        ).await?;

        let patch_text = match if workflow_type == "manual_shard" {
            patches::generate_staged_patch_text(Path::new(&shard_path))
        } else {
            patches::generate_patch_text(Path::new(&shard_path))
        } {
            Ok(value) => value,
            Err(err) => {
                let result = json!({
                    "ok": false,
                    "mode": if workflow_type == "manual_shard" { "generate_staged_patch_text" } else { "generate_patch_text" },
                    "source": patch_source,
                    "sprint_id": sprint_id,
                    "feature_id": feature_id,
                    "shard_path": shard_path,
                    "error": format!("{:#}", err)
                });
                append_git_patch_payload_event(
                    state,
                    run_id,
                    step,
                    stage_execution_id.as_deref(),
                    capability_invocation_id.as_str(),
                    "error",
                    "git_patch_payload_failed",
                    "git patch payload failed",
                    capability_config,
                    result.clone(),
                ).await?;
                capability_results.push(json!({
                    "key": "git_patch_payload",
                    "ok": false,
                    "result": result
                }));
                failed.push(json!({
                    "patch": patch,
                    "error": format!("failed to generate patch text: {:#}", err)
                }));
                break;
            }
        };

        if patch_text.trim().is_empty() {
            let result = json!({
                "ok": true,
                "mode": if workflow_type == "manual_shard" { "generate_staged_patch_text" } else { "generate_patch_text" },
                "source": patch_source,
                "sprint_id": sprint_id,
                "feature_id": feature_id,
                "shard_path": shard_path,
                "patch_bytes": 0,
                "empty_patch": true
            });
            append_git_patch_payload_event(
                state,
                run_id,
                step,
                stage_execution_id.as_deref(),
                capability_invocation_id.as_str(),
                "info",
                "git_patch_payload_completed",
                "git patch payload completed",
                capability_config,
                result.clone(),
            ).await?;
            capability_results.push(json!({
                "key": "git_patch_payload",
                "ok": true,
                "result": result
            }));
            applied.push(json!({
                "execution_item_id": feature_id,
                "shard_path": shard_path,
                "workflow_run_id": workflow_run_id,
                "patch_id": null,
                "empty_patch": true
            }));
            continue;
        }

        match patches::apply_patch_text(Path::new(repo_ref), &patch_text) {
            Ok(()) => {
                let patch_id = if workflow_type == "manual_shard" {
                    persist_integrated_manual_patch(
                        state,
                        supervisor_run_id.as_deref(),
                        &execution_item_id,
                        workflow_run_id.as_deref(),
                        &shard_path,
                        &patch_text,
                    ).await?
                } else {
                    persist_integrated_feature_patch(
                        state,
                        &root_repo_path,
                        supervisor_run_id.as_deref(),
                        sprint_id.as_deref(),
                        &feature_id,
                        workflow_run_id.as_deref(),
                        &shard_path,
                        &patch_text,
                    ).await?
                };
                let result = json!({
                    "ok": true,
                    "mode": "generate_apply_persist_patch_text",
                    "source": "sprint_features",
                    "sprint_id": sprint_id,
                    "feature_id": feature_id,
                    "shard_path": shard_path,
                    "patch_id": patch_id,
                    "patch_bytes": patch_text.len()
                });
                append_git_patch_payload_event(
                    state,
                    run_id,
                    step,
                    stage_execution_id.as_deref(),
                    capability_invocation_id.as_str(),
                    "info",
                    "git_patch_payload_completed",
                    "git patch payload completed",
                    capability_config,
                    result.clone(),
                ).await?;
                capability_results.push(json!({
                    "key": "git_patch_payload",
                    "ok": true,
                    "result": result
                }));
                applied.push(json!({
                    "execution_item_id": feature_id,
                    "shard_path": shard_path,
                    "workflow_run_id": workflow_run_id,
                    "patch_id": patch_id,
                    "patch_bytes": patch_text.len()
                }));
            }
            Err(err) => {
                let result = json!({
                    "ok": false,
                    "mode": "apply_patch_text",
                    "source": "sprint_features",
                    "sprint_id": sprint_id,
                    "feature_id": feature_id,
                    "shard_path": shard_path,
                    "error": format!("{:#}", err)
                });
                append_git_patch_payload_event(
                    state,
                    run_id,
                    step,
                    stage_execution_id.as_deref(),
                    capability_invocation_id.as_str(),
                    "error",
                    "git_patch_payload_failed",
                    "git patch payload failed",
                    capability_config,
                    result.clone(),
                ).await?;
                capability_results.push(json!({
                    "key": "git_patch_payload",
                    "ok": false,
                    "result": result
                }));
                failed.push(json!({
                    "patch": patch,
                    "error": format!("{:#}", err)
                }));
                break;
            }
        }
    }

    let ok = failed.is_empty();
    let status = if ok { "merged" } else { "merge_failed" };
    let final_patch = if ok {
        Some(persist_final_integration_patch(
            state,
            supervisor_run_id.as_deref(),
            &root_repo_path,
            repo_ref,
            &integration_batch_id,
            &applied,
        ).await?)
    } else {
        None
    };
    local_state["merge_patches"] = json!({
        "status": status,
        "source": "supervisor_integration_pool",
        "sprint_id": sprint_id,
        "integration_batch_id": integration_batch_id,
        "supervisor_run_id": supervisor_run_id,
        "applied": applied,
        "failed": failed,
        "final_patch": final_patch
    });

    Ok(StageOutcome {
        ok,
        status: if ok { StageStatus::Success } else { StageStatus::Error },
        transition: if ok { StageTransition::MoveNext } else { StageTransition::Stop },
        message: format!("merge_patches stage {}", status),
        capability_results,
        local_state,
    })
}

async fn append_git_patch_payload_event(
    state: &AppState,
    run_id: Uuid,
    step: &WorkflowStepDefinition,
    stage_execution_id: Option<&str>,
    capability_invocation_id: &str,
    level: &str,
    kind: &str,
    message: &str,
    config: Value,
    result: Value,
) -> Result<()> {
    engine::append_engine_event(
        state,
        run_id,
        Some(step.id.as_str()),
        level,
        kind,
        message,
        json!({
            "capability": "git_patch_payload",
            "config": config,
            "ok": result.get("ok").and_then(Value::as_bool),
            "result": result,
            "event_meta": engine::event_meta(stage_execution_id, Some(capability_invocation_id), None, false)
        }),
    ).await
}

async fn persist_integrated_feature_patch(
    state: &AppState,
    root_repo_path: &str,
    supervisor_run_id: Option<&str>,
    sprint_id: Option<&str>,
    feature_id: &str,
    workflow_run_id: Option<&str>,
    shard_path: &str,
    patch_text: &str,
) -> Result<String> {
    let repo_id = ensure_planner_repo_id(state, root_repo_path).await?;
    let patch_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let base_commit = patches::current_head(Path::new(shard_path))?;
    let patch_hash = patches::patch_content_hash(patch_text);

    sqlx::query("INSERT INTO planner_feature_patches (id, feature_id, planner_id, supervisor_run_id, workflow_run_id, patch_kind, repo_ref, base_commit, head_commit, patch_text, patch_hash, patch_path, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(&patch_id)
        .bind(feature_id)
        .bind(&repo_id)
        .bind(supervisor_run_id)
        .bind(workflow_run_id)
        .bind("development")
        .bind(root_repo_path)
        .bind(base_commit.clone())
        .bind(base_commit)
        .bind(patch_text)
        .bind(patch_hash)
        .bind(Option::<String>::None)
        .bind(&now)
        .execute(&state.db)
        .await?;

    sqlx::query("UPDATE planner_features SET locked_supervisor_run_id = COALESCE(NULLIF(locked_supervisor_run_id, ''), ?), locked_at = COALESCE(locked_at, ?), updated_at = ? WHERE planner_id = ? AND id = ?")
        .bind(supervisor_run_id)
        .bind(&now)
        .bind(&now)
        .bind(&repo_id)
        .bind(feature_id)
        .execute(&state.db)
        .await?;

    Ok(patch_id)
}

async fn persist_integrated_manual_patch(
    state: &AppState,
    supervisor_run_id: Option<&str>,
    manual_shard_id: &str,
    workflow_run_id: Option<&str>,
    shard_path: &str,
    patch_text: &str,
) -> Result<String> {
    let supervisor_run_id = supervisor_run_id.ok_or_else(|| anyhow!("manual integration patch requires supervisor_run_id"))?;
    let patch_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let patch_hash = patches::patch_content_hash(patch_text);

    let row = sqlx::query("SELECT id, context_json FROM supervisor_work_units WHERE supervisor_run_id = ? AND kind = 'manual_shard' AND feature_id = ? LIMIT 1")
        .bind(supervisor_run_id)
        .bind(manual_shard_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| anyhow!("manual shard work unit {} is missing", manual_shard_id))?;

    let work_unit_id: String = row.get("id");
    let mut context = serde_json::from_str::<Value>(&row.get::<String, _>("context_json"))
        .unwrap_or_else(|_| json!({}));
    if let Some(obj) = context.as_object_mut() {
        obj.insert("workflow_type".to_string(), Value::String("manual_shard".to_string()));
        obj.insert("pool_key".to_string(), Value::String("manual_shard".to_string()));
        obj.insert("integration_input".to_string(), Value::Bool(true));
        obj.insert("staged_to_integration".to_string(), Value::Bool(true));
        obj.insert("integrated_patch_id".to_string(), Value::String(patch_id.clone()));
        obj.insert("integrated_patch_hash".to_string(), Value::String(patch_hash));
        obj.insert("integrated_patch_bytes".to_string(), Value::Number((patch_text.len() as u64).into()));
        obj.insert("integrated_at".to_string(), Value::String(now.clone()));
    }

    sqlx::query("UPDATE supervisor_work_units SET patch_id = ?, state = 'integrated', context_json = ?, updated_at = ? WHERE id = ?")
        .bind(&patch_id)
        .bind(serde_json::to_string(&context)?)
        .bind(&now)
        .bind(&work_unit_id)
        .execute(&state.db)
        .await?;

    if let Some(workflow_run_id) = workflow_run_id.filter(|value| !value.trim().is_empty()) {
        tracing::info!(
            supervisor_run_id,
            manual_shard_id,
            workflow_run_id,
            patch_id = %patch_id,
            shard_path,
            "persisted supervisor-owned manual integration patch without planner_feature FK"
        );
    }

    Ok(patch_id)
}

async fn persist_final_integration_patch(
    state: &AppState,
    supervisor_run_id: Option<&str>,
    root_repo_path: &str,
    integration_repo_path: &str,
    sprint_id: &str,
    applied: &[Value],
) -> Result<Value> {
    let supervisor_run_id = supervisor_run_id.ok_or_else(|| anyhow!("merge_patches requires supervisor_run_id to persist final patch"))?;
    let patch_text = patches::generate_patch_text(Path::new(integration_repo_path))?;
    let patch_hash = patches::patch_content_hash(&patch_text);
    let now = Utc::now().to_rfc3339();
    let final_patch_ref = format!("supervisor_work_units:{}:context_json.final_patch_text", supervisor_run_id);
    let report = json!({
        "ok": true,
        "status": "merged",
        "source": "supervisor_integration_pool",
        "sprint_id": sprint_id,
        "integration_path": integration_repo_path,
        "final_patch_ref": final_patch_ref,
        "final_patch_hash": patch_hash,
        "final_patch_bytes": patch_text.len(),
        "applied": applied,
        "persisted_at": now
    });

    sqlx::query(
        r#"
        UPDATE supervisor_work_units
        SET patch_id = COALESCE(NULLIF(patch_id, ''), ?),
            integration_path = COALESCE(NULLIF(integration_path, ''), ?),
            state = CASE WHEN state IN ('deleted', 'archived') THEN state ELSE 'patch_ready' END,
            context_json = json_set(
                CASE WHEN json_valid(context_json) THEN context_json ELSE '{}' END,
                '$.final_patch_ref', ?,
                '$.final_patch_text', ?,
                '$.final_patch_hash', ?,
                '$.final_patch_bytes', ?,
                '$.merge_report', json(?),
                '$.integration_path', ?
            ),
            updated_at = ?
        WHERE supervisor_run_id = ?
          AND kind = 'integration'
          AND archived_at IS NULL
        "#,
    )
    .bind(&patch_hash)
    .bind(integration_repo_path)
    .bind(&final_patch_ref)
    .bind(&patch_text)
    .bind(&patch_hash)
    .bind(patch_text.len() as i64)
    .bind(serde_json::to_string(&report)?)
    .bind(integration_repo_path)
    .bind(&now)
    .bind(supervisor_run_id)
    .execute(&state.db)
    .await?;

    sqlx::query("UPDATE supervisor_runs SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(supervisor_run_id)
        .execute(&state.db)
        .await?;

    Ok(report)
}

async fn ensure_planner_repo_id(state: &AppState, root_repo_path: &str) -> Result<String> {
    let normalized_root = root_repo_path.trim().replace('\\', "/");
    if let Some(row) = sqlx::query("SELECT id FROM planner_workspaces WHERE root_repo_path = ? ORDER BY is_default DESC, updated_at DESC, created_at DESC LIMIT 1")
        .bind(&normalized_root)
        .fetch_optional(&state.db)
        .await?
    {
        return Ok(row.get::<String, _>("id"));
    }

    let planner_id = Uuid::new_v4().to_string();
    let repo_key = repo_key_for(&normalized_root);
    let now = Utc::now().to_rfc3339();
    sqlx::query("INSERT INTO planner_workspaces (id, root_repo_path, repo_key, title, is_default, created_at, updated_at) VALUES (?, ?, ?, ?, 1, ?, ?)")
        .bind(&planner_id)
        .bind(&normalized_root)
        .bind(&repo_key)
        .bind(format!("{} Planner", repo_key))
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await?;
    Ok(planner_id)
}

fn repo_key_for(root_repo_path: &str) -> String {
    let normalized = root_repo_path.trim().replace('\\', "/");
    let raw = normalized
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("repo");
    let mut out = raw
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '-' })
        .collect::<String>();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "repo".to_string() } else { out }
}

async fn load_supervisor_runtime_context(state: &AppState, supervisor_run_id: &str) -> Result<Value> {
    let row = sqlx::query(
        "SELECT root_repo_path, context_json FROM supervisor_runs WHERE id = ?",
    )
    .bind(supervisor_run_id)
    .fetch_optional(&state.db)
    .await?;

    let Some(row) = row else {
        return Err(anyhow!("supervisor {} not found", supervisor_run_id));
    };

    let root_repo_path = row.get::<String, _>("root_repo_path");
    let context_json = row.get::<String, _>("context_json");
    let context = serde_json::from_str::<Value>(&context_json).unwrap_or_else(|_| json!({}));
    let workspace = crate::supervisor::repo_snapshot::workspace_for(
        &root_repo_path,
        Uuid::parse_str(supervisor_run_id)?,
    ).ok();

    Ok(json!({
        "root_repo_path": root_repo_path,
        "snapshot_path": workspace.as_ref().map(|item| item.snapshot.to_string_lossy().replace('\\', "/")),
        "integration_path": workspace.as_ref().map(|item| item.integration.to_string_lossy().replace('\\', "/")),
        "sprint_id": context.get("current_sprint_id").cloned().unwrap_or(Value::Null),
        "context": context
    }))
}

async fn resolve_supervisor_integration_pool_patches(state: &AppState, supervisor_run_id: &str) -> Result<Vec<Value>> {
    let rows = sqlx::query(
        r#"
        SELECT feature_id, title, workflow_run_id, patch_id, shard_path, kind, state, context_json
        FROM supervisor_work_units
        WHERE supervisor_run_id = ?
          AND kind IN ('feature_development', 'manual_shard')
          AND state NOT IN ('deleted', 'archived')
          AND TRIM(COALESCE(shard_path, '')) != ''
          AND COALESCE(json_extract(context_json, '$.integration_skipped'), 0) = 0
          AND (
              COALESCE(json_extract(context_json, '$.integration_input'), 0) = 1
              OR COALESCE(json_extract(context_json, '$.staged_to_integration'), 0) = 1
          )
        ORDER BY CASE kind WHEN 'feature_development' THEN 0 ELSE 1 END, queue_position ASC, updated_at ASC
        "#,
    )
    .bind(supervisor_run_id)
    .fetch_all(&state.db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let kind: String = row.get("kind");
            let feature_id = row
                .try_get::<Option<String>, _>("feature_id")
                .ok()
                .flatten()
                .unwrap_or_default();
            let workflow_type = if kind == "manual_shard" { "manual_shard" } else { "feature_development" };
            json!({
                "execution_item_id": feature_id,
                "feature_id": if kind == "manual_shard" { Value::Null } else { Value::String(feature_id.clone()) },
                "manual_shard_id": if kind == "manual_shard" { Value::String(feature_id.clone()) } else { Value::Null },
                "title": row.get::<String, _>("title"),
                "shard_path": row.get::<String, _>("shard_path"),
                "workflow_run_id": row.try_get::<Option<String>, _>("workflow_run_id").ok().flatten(),
                "patch_id": row.try_get::<Option<String>, _>("patch_id").ok().flatten(),
                "workflow_type": workflow_type,
                "patch_owner": if kind == "manual_shard" { "staged_manual_pool" } else { "feature_pool" },
                "pool_state": row.get::<String, _>("state")
            })
        })
        .collect())
}

async fn resolve_sprint_feature_patches(state: &AppState, sprint_id: &str) -> Result<Vec<Value>> {
    let rows = sqlx::query("SELECT sf.feature_id, COALESCE(pf.title, sf.feature_id) AS title, sf.shard_path, sf.current_workflow_run_id, sf.current_patch_id FROM sprint_features sf LEFT JOIN planner_features pf ON pf.id = sf.feature_id WHERE sf.sprint_id = ? AND sf.development_state IN ('development_succeeded', 'integrated', 'applied') AND COALESCE(sf.integration_skipped, 0) = 0 AND TRIM(COALESCE(sf.shard_path, '')) != '' ORDER BY sf.sort_order ASC, sf.created_at ASC")
        .bind(sprint_id)
        .fetch_all(&state.db)
        .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let workflow_run_id = row
                .try_get::<Option<String>, _>("current_workflow_run_id")
                .ok()
                .flatten();
            json!({
                "execution_item_id": row.get::<String, _>("feature_id"),
                "title": row.get::<String, _>("title"),
                "shard_path": row.get::<String, _>("shard_path"),
                "workflow_run_id": workflow_run_id,
                "patch_id": row.try_get::<Option<String>, _>("current_patch_id").ok().flatten()
            })
        })
        .collect())
}
