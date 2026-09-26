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
    supervisor::{
        load_supervisor_integration_inputs,
        models::{SupervisorIntegrationInput, SupervisorWorkPoolKind},
        patches,
    },
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

fn patch_apply_failed_files(error: &str) -> Vec<String> {
    let mut files = Vec::new();

    for line in error.lines().map(str::trim) {
        let path = if let Some(rest) = line.strip_prefix("error: patch failed: ") {
            rest.rsplit_once(':').map(|(path, _)| path.trim())
        } else if let Some(rest) = line.strip_prefix("error: ") {
            rest.strip_suffix(": patch does not apply").map(str::trim)
        } else {
            None
        };

        let Some(path) = path.filter(|value| !value.is_empty()) else {
            continue;
        };

        if !files.iter().any(|existing| existing == path) {
            files.push(path.to_string());
        }
    }

    files
}

fn patch_apply_failure_summary(failed_files: &[String]) -> String {
    match failed_files.len() {
        0 => "Patch could not be merged into the integration workspace".to_string(),
        1 => format!("1 file could not be merged: {}", failed_files[0]),
        count => format!("{count} files could not be merged into the integration workspace"),
    }
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

    let integration_batch_id = supervisor_run_id.clone().unwrap_or_else(|| run_id.to_string());

    let configured_inputs = supervisor_context
        .get("integration_inputs")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| {
            step.config
                .get("patches")
                .and_then(Value::as_array)
                .cloned()
        })
        .or_else(|| {
            run.context
                .get("workflow_engine")
                .and_then(|value| value.get("global_state"))
                .and_then(|value| value.get("supervisor"))
                .and_then(|value| value.get("patches"))
                .and_then(Value::as_array)
                .cloned()
        })
        .unwrap_or_default()
        .into_iter()
        .map(serde_json::from_value)
        .collect::<std::result::Result<Vec<SupervisorIntegrationInput>, _>>()?;

    let patch_items = if !configured_inputs.is_empty() {
        configured_inputs
    } else if let Some(supervisor_run_id) = supervisor_run_id.as_deref().filter(|value| !value.trim().is_empty()) {
        load_supervisor_integration_inputs(state, Uuid::parse_str(supervisor_run_id)?).await?
    } else {
        return Err(anyhow!("merge_patches requires integration inputs"));
    };
    let mut applied = Vec::new();
    let mut pending_patches = Vec::new();
    let mut failed = Vec::new();
    let mut capability_results = Vec::new();

    if patch_items.is_empty() {
        failed.push(json!({
            "error": "merge_patches found no staged integration inputs with workspace_path",
            "integration_batch_id": integration_batch_id
        }));
    }

    for patch in patch_items {
        let workspace_path = patch.workspace_path.to_string_lossy().trim().to_string();
        let work_unit_id = patch.work_unit_id.trim().to_string();
        let feature_id = patch.feature_id.clone().filter(|value| !value.trim().is_empty());
        let workflow_run_id = patch.workflow_run_id.map(|value| value.to_string());
        let capability_invocation_id = Uuid::new_v4().to_string();
        let capability_config = json!({
            "work_unit_id": work_unit_id,
            "workspace_path": workspace_path,
            "target_repo_ref": repo_ref
        });

        if workspace_path.is_empty() || work_unit_id.is_empty() {
            failed.push(json!({
                "patch": patch,
                "error": "integration input requires work_unit_id and workspace_path"
            }));
            break;
        }

        append_git_patch_payload_event(
            state,
            GitPatchPayloadEvent {
                run_id,
                step,
                stage_execution_id: stage_execution_id.as_deref(),
                capability_invocation_id: capability_invocation_id.as_str(),
                level: "info",
                kind: "git_patch_payload_started",
                message: "git patch payload started",
                config: capability_config.clone(),
                result: json!({}),
            },
        )
        .await?;

        let patch_text = match patches::generate_staged_patch_text(Path::new(&workspace_path)) {
            Ok(value) => value,
            Err(err) => {
                let details = format!("{:#}", err);
                let summary = "Could not read the staged changes from this work unit";
                let result = json!({
                    "ok": false,
                    "error_type": "staged_patch_generation_failed",
                    "summary": summary,
                    "work_unit_id": work_unit_id,
                    "workspace_path": workspace_path,
                    "details": details
                });
                append_git_patch_payload_event(
                    state,
                    GitPatchPayloadEvent {
                        run_id,
                        step,
                        stage_execution_id: stage_execution_id.as_deref(),
                        capability_invocation_id: capability_invocation_id.as_str(),
                        level: "error",
                        kind: "git_patch_payload_failed",
                        message: summary,
                        config: capability_config,
                        result: result.clone(),
                    },
                )
                .await?;
                capability_results.push(json!({
                    "key": "git_patch_payload",
                    "ok": false,
                    "result": result
                }));
                failed.push(json!({
                    "patch": patch,
                    "error": format!("failed to generate staged patch text: {:#}", err)
                }));
                break;
            }
        };

        if patch_text.trim().is_empty() {
            let result = json!({
                "ok": true,
                "summary": "No staged changes were present in this integration input",
                "work_unit_id": work_unit_id,
                "workspace_path": workspace_path,
                "patch_bytes": 0,
                "empty_patch": true
            });
            append_git_patch_payload_event(
                state,
                GitPatchPayloadEvent {
                    run_id,
                    step,
                    stage_execution_id: stage_execution_id.as_deref(),
                    capability_invocation_id: capability_invocation_id.as_str(),
                    level: "info",
                    kind: "git_patch_payload_completed",
                    message: "git patch payload completed",
                    config: capability_config,
                    result: result.clone(),
                },
            )
            .await?;
            capability_results.push(json!({
                "key": "git_patch_payload",
                "ok": true,
                "result": result
            }));
            applied.push(json!({
                "work_unit_id": work_unit_id,
                "feature_id": feature_id,
                "workspace_path": workspace_path,
                "workflow_run_id": workflow_run_id,
                "patch_id": null,
                "empty_patch": true
            }));
            continue;
        }

        match patches::apply_patch_text(Path::new(repo_ref), &patch_text) {
            Ok(()) => {
                let patch_hash = patches::patch_content_hash(&patch_text);
                let base_commit = patches::current_head(Path::new(&workspace_path))?;

                pending_patches.push(PendingIntegrationPatch {
                    id: Uuid::new_v4().to_string(),
                    source_work_unit_id: work_unit_id.clone(),
                    source_kind: patch.kind,
                    source_workflow_run_id: workflow_run_id.clone(),
                    feature_id: feature_id.clone(),
                    workspace_path: workspace_path.clone(),
                    base_commit,
                    patch_text: patch_text.clone(),
                    patch_hash,
                    patch_bytes: patch_text.len() as i64,
                });

                let result = json!({
                    "ok": true,
                    "summary": "Staged changes merged into the integration workspace",
                    "work_unit_id": work_unit_id,
                    "workspace_path": workspace_path,
                    "patch_bytes": patch_text.len()
                });
                append_git_patch_payload_event(
                    state,
                    GitPatchPayloadEvent {
                        run_id,
                        step,
                        stage_execution_id: stage_execution_id.as_deref(),
                        capability_invocation_id: capability_invocation_id.as_str(),
                        level: "info",
                        kind: "git_patch_payload_completed",
                        message: "git patch payload completed",
                        config: capability_config,
                        result: result.clone(),
                    },
                )
                .await?;
                capability_results.push(json!({
                    "key": "git_patch_payload",
                    "ok": true,
                    "result": result
                }));
                applied.push(json!({
                    "work_unit_id": work_unit_id,
                    "feature_id": feature_id,
                    "kind": patch.kind,
                    "workspace_path": workspace_path,
                    "workflow_run_id": workflow_run_id,
                    "patch_bytes": patch_text.len()
                }));
            }
            Err(err) => {
                let details = format!("{:#}", err);
                let failed_files = patch_apply_failed_files(&details);
                let summary = patch_apply_failure_summary(&failed_files);
                let error_type = if failed_files.is_empty() {
                    "patch_apply_failed"
                } else {
                    "patch_conflict"
                };
                let result = json!({
                    "ok": false,
                    "error_type": error_type,
                    "summary": summary,
                    "work_unit_id": work_unit_id,
                    "workspace_path": workspace_path,
                    "failed_files": failed_files,
                    "details": details
                });
                append_git_patch_payload_event(
                    state,
                    GitPatchPayloadEvent {
                        run_id,
                        step,
                        stage_execution_id: stage_execution_id.as_deref(),
                        capability_invocation_id: capability_invocation_id.as_str(),
                        level: "error",
                        kind: "git_patch_payload_failed",
                        message: result
                            .get("summary")
                            .and_then(Value::as_str)
                            .unwrap_or("Patch could not be merged"),
                        config: capability_config,
                        result: result.clone(),
                    },
                )
                .await?;
                capability_results.push(json!({
                    "key": "git_patch_payload",
                    "ok": false,
                    "result": result.clone()
                }));
                failed.push(result);
                break;
            }
        }
    }

    let ok = failed.is_empty();
    let status = if ok { "merged" } else { "merge_failed" };
    let final_patch = if ok {
        Some(
            persist_successful_integration_merge(
                state,
                supervisor_run_id.as_deref(),
                run_id,
                repo_ref,
                &pending_patches,
                &applied,
            )
            .await?,
        )
    } else {
        None
    };
    local_state["merge_patches"] = json!({
        "status": status,
        "source": "supervisor_integration_pool",
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

struct GitPatchPayloadEvent<'a> {
    run_id: Uuid,
    step: &'a WorkflowStepDefinition,
    stage_execution_id: Option<&'a str>,
    capability_invocation_id: &'a str,
    level: &'a str,
    kind: &'a str,
    message: &'a str,
    config: Value,
    result: Value,
}

async fn append_git_patch_payload_event(
    state: &AppState,
    event: GitPatchPayloadEvent<'_>,
) -> Result<()> {
    engine::append_engine_event(
        state,
        event.run_id,
        Some(event.step.id.as_str()),
        event.level,
        event.kind,
        event.message,
        json!({
            "capability": "git_patch_payload",
            "config": event.config,
            "ok": event.result.get("ok").and_then(Value::as_bool),
            "result": event.result,
            "event_meta": engine::event_meta(
                event.stage_execution_id,
                Some(event.capability_invocation_id),
                None,
                false,
            )
        }),
    )
    .await
}

#[derive(Debug, Clone)]
struct PendingIntegrationPatch {
    id: String,
    source_work_unit_id: String,
    source_kind: SupervisorWorkPoolKind,
    source_workflow_run_id: Option<String>,
    feature_id: Option<String>,
    workspace_path: String,
    base_commit: Option<String>,
    patch_text: String,
    patch_hash: String,
    patch_bytes: i64,
}

async fn persist_successful_integration_merge(
    state: &AppState,
    supervisor_run_id: Option<&str>,
    integration_workflow_run_id: Uuid,
    integration_repo_path: &str,
    patches_to_persist: &[PendingIntegrationPatch],
    applied: &[Value],
) -> Result<Value> {
    let supervisor_run_id = supervisor_run_id
        .ok_or_else(|| anyhow!("merge_patches requires supervisor_run_id to persist integration results"))?;
    let final_patch_text = patches::generate_patch_text(Path::new(integration_repo_path))?;
    let final_patch_hash = patches::patch_content_hash(&final_patch_text);
    let now = Utc::now().to_rfc3339();
    let final_patch_ref = format!(
        "supervisor_work_units:{}:context_json.final_patch_text",
        supervisor_run_id
    );

    let mut tx = state.db.begin().await?;

    let integration_work_unit_id = sqlx::query_scalar::<_, String>(
        "SELECT id FROM supervisor_work_units WHERE supervisor_run_id = ? AND kind = 'integration' AND archived_at IS NULL LIMIT 1",
    )
    .bind(supervisor_run_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| anyhow!("supervisor integration work unit is missing"))?;

    let mut persisted_inputs = Vec::with_capacity(patches_to_persist.len());
    for patch in patches_to_persist {
        sqlx::query(
            r#"
            INSERT INTO supervisor_integration_patches (
                id,
                supervisor_run_id,
                integration_work_unit_id,
                integration_workflow_run_id,
                source_work_unit_id,
                source_kind,
                source_workflow_run_id,
                feature_id,
                workspace_path,
                base_commit,
                patch_text,
                patch_hash,
                patch_bytes,
                created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&patch.id)
        .bind(supervisor_run_id)
        .bind(&integration_work_unit_id)
        .bind(integration_workflow_run_id.to_string())
        .bind(&patch.source_work_unit_id)
        .bind(patch.source_kind.as_str())
        .bind(&patch.source_workflow_run_id)
        .bind(&patch.feature_id)
        .bind(&patch.workspace_path)
        .bind(&patch.base_commit)
        .bind(&patch.patch_text)
        .bind(&patch.patch_hash)
        .bind(patch.patch_bytes)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE supervisor_work_units SET patch_id = ?, updated_at = ? WHERE supervisor_run_id = ? AND id = ? AND archived_at IS NULL",
        )
        .bind(&patch.id)
        .bind(&now)
        .bind(supervisor_run_id)
        .bind(&patch.source_work_unit_id)
        .execute(&mut *tx)
        .await?;

        persisted_inputs.push(json!({
            "patch_id": patch.id,
            "work_unit_id": patch.source_work_unit_id,
            "kind": patch.source_kind,
            "feature_id": patch.feature_id,
            "workflow_run_id": patch.source_workflow_run_id,
            "workspace_path": patch.workspace_path,
            "base_commit": patch.base_commit,
            "patch_hash": patch.patch_hash,
            "patch_bytes": patch.patch_bytes
        }));
    }

    let report = json!({
        "ok": true,
        "status": "merged",
        "source": "supervisor_integration_pool",
        "integration_work_unit_id": integration_work_unit_id,
        "integration_workflow_run_id": integration_workflow_run_id,
        "integration_path": integration_repo_path,
        "final_patch_ref": final_patch_ref,
        "final_patch_hash": final_patch_hash,
        "final_patch_bytes": final_patch_text.len(),
        "inputs": persisted_inputs,
        "applied": applied,
        "persisted_at": now
    });

    sqlx::query(
        r#"
        UPDATE supervisor_work_units
        SET integration_path = COALESCE(NULLIF(integration_path, ''), ?),
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
        WHERE id = ?
          AND supervisor_run_id = ?
          AND archived_at IS NULL
        "#,
    )
    .bind(integration_repo_path)
    .bind(&final_patch_ref)
    .bind(&final_patch_text)
    .bind(&final_patch_hash)
    .bind(final_patch_text.len() as i64)
    .bind(serde_json::to_string(&report)?)
    .bind(integration_repo_path)
    .bind(&now)
    .bind(&integration_work_unit_id)
    .bind(supervisor_run_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query("UPDATE supervisor_runs SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(supervisor_run_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(report)
}

