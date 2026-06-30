use std::path::Path;

use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine,
    models::{WorkflowRun, WorkflowStepDefinition},
    supervisor::patches,
};

use super::{StageDisposition, StageOutcome};

pub fn capabilities() -> crate::engine::stages::capability_contract::StageCapabilities {
    crate::engine::stages::capability_contract::StageCapabilities::new(["git_patch_payload"])
}

pub fn prepare_stage_state(_step: &WorkflowStepDefinition, local_state: Value) -> Result<Value> {
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
    let root_repo_path = supervisor_context
        .get("root_repo_path")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(repo_ref)
        .to_string();
    let supervisor_run_id = supervisor_context
        .get("supervisor_run_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let sprint_id = supervisor_context
        .get("sprint_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("merge_patches requires supervisor.sprint_id in workflow context"))?;

    let patch_items = resolve_sprint_feature_patches(state, &sprint_id).await?;
    let mut applied = Vec::new();
    let mut failed = Vec::new();
    let mut capability_results = Vec::new();

    if patch_items.is_empty() {
        failed.push(json!({
            "error": "merge_patches found no development_succeeded sprint_features with shard_path",
            "sprint_id": sprint_id
        }));
    }

    for patch in patch_items {
        let shard_path = patch.get("shard_path").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        let feature_id = patch.get("execution_item_id").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        let workflow_run_id = patch.get("workflow_run_id").and_then(Value::as_str).map(str::to_string);
        let capability_invocation_id = Uuid::new_v4().to_string();
        let capability_config = json!({
            "mode": "generate_apply_persist_patch_text",
            "source": "sprint_features",
            "sprint_id": sprint_id,
            "feature_id": feature_id,
            "shard_path": shard_path,
            "target_repo_ref": repo_ref
        });

        if shard_path.is_empty() || feature_id.is_empty() {
            failed.push(json!({
                "patch": patch,
                "error": "sprint_features patch source requires feature_id and shard_path"
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

        let patch_text = match patches::generate_patch_text(Path::new(&shard_path)) {
            Ok(value) => value,
            Err(err) => {
                let result = json!({
                    "ok": false,
                    "mode": "generate_patch_text",
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
                    "error": format!("failed to generate patch text: {:#}", err)
                }));
                break;
            }
        };

        if patch_text.trim().is_empty() {
            let result = json!({
                "ok": true,
                "mode": "generate_patch_text",
                "source": "sprint_features",
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
                let patch_id = persist_integrated_feature_patch(
                    state,
                    &root_repo_path,
                    supervisor_run_id.as_deref(),
                    Some(&sprint_id),
                    &feature_id,
                    workflow_run_id.as_deref(),
                    &shard_path,
                    &patch_text,
                ).await?;
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
    local_state["merge_patches"] = json!({
        "status": status,
        "source": "sprint_features",
        "sprint_id": sprint_id,
        "applied": applied,
        "failed": failed
    });

    Ok(StageOutcome {
        ok,
        disposition: if ok { StageDisposition::MoveNext } else { StageDisposition::Stay },
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

    sqlx::query("INSERT INTO planner_feature_patches (id, feature_id, repo_id, sprint_id, supervisor_run_id, workflow_run_id, patch_kind, repo_ref, base_commit, head_commit, patch_text, patch_hash, patch_path, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(&patch_id)
        .bind(feature_id)
        .bind(&repo_id)
        .bind(sprint_id)
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

    sqlx::query("UPDATE planner_features SET current_sprint_id = ?, current_supervisor_run_id = ?, current_workflow_run_id = ?, current_patch_id = ?, development_state = 'integrated', integration_completed_at = COALESCE(integration_completed_at, ?), updated_at = ? WHERE repo_id = ? AND id = ?")
        .bind(sprint_id)
        .bind(supervisor_run_id)
        .bind(workflow_run_id)
        .bind(&patch_id)
        .bind(&now)
        .bind(&now)
        .bind(&repo_id)
        .bind(feature_id)
        .execute(&state.db)
        .await?;

    if let Some(sprint_id) = sprint_id {
        sqlx::query("UPDATE sprint_features SET current_patch_id = ?, development_state = 'integrated', integration_completed_at = COALESCE(integration_completed_at, ?), updated_at = ? WHERE sprint_id = ? AND feature_id = ?")
            .bind(&patch_id)
            .bind(&now)
            .bind(&now)
            .bind(sprint_id)
            .bind(feature_id)
            .execute(&state.db)
            .await?;
    }

    Ok(patch_id)
}

async fn ensure_planner_repo_id(state: &AppState, root_repo_path: &str) -> Result<String> {
    if let Some(row) = sqlx::query("SELECT id FROM planner_repos WHERE root_repo_path = ?")
        .bind(root_repo_path)
        .fetch_optional(&state.db)
        .await?
    {
        return Ok(row.get::<String, _>("id"));
    }

    let repo_id = Uuid::new_v4().to_string();
    let repo_key = repo_key_for(root_repo_path);
    let now = Utc::now().to_rfc3339();
    sqlx::query("INSERT INTO planner_repos (id, root_repo_path, repo_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?)")
        .bind(&repo_id)
        .bind(root_repo_path)
        .bind(repo_key)
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await?;
    Ok(repo_id)
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

async fn resolve_sprint_feature_patches(state: &AppState, sprint_id: &str) -> Result<Vec<Value>> {
    let rows = sqlx::query("SELECT sf.feature_id, COALESCE(pf.title, sf.feature_id) AS title, sf.shard_path, sf.current_workflow_run_id, sf.current_patch_id FROM sprint_features sf LEFT JOIN planner_features pf ON pf.id = sf.feature_id WHERE sf.sprint_id = ? AND sf.development_state IN ('development_succeeded', 'integrated', 'applied') AND TRIM(COALESCE(sf.shard_path, '')) != '' ORDER BY sf.sort_order ASC, sf.created_at ASC")
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
