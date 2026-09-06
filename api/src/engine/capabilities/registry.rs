use std::time::Instant;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    engine::{
        append_engine_event,
        ensure_engine_root,
        event_meta,
        governance,
        load_run,
        orchestration_inputs::{
            AttachmentRole,
            OrchestrationInputLifecycle,
            OrchestrationInputPayload,
            OrchestrationInputScope,
        },
        persist_context,
        set_run_status,
    },
    models::{RunStatus, StageExecutionNodeKind, WorkflowStepDefinition},
};

use super::{capability_enabled, changeset, compile_commands, context_export, git_patch_payload, inference, operator_checkpoint::{self, OperatorInputResponse}, planner, qa_environment, repo_sync, review_validation, sap, shared_dependencies};

#[derive(Debug, Clone)]
pub struct StageCapabilityPolicy {
    pub entrypoint: String,
    pub allowed_invocations: Vec<String>,
}

#[derive(Clone)]
pub struct CapabilityContext<'a> {
    pub state: &'a AppState,
    pub run_id: Uuid,
    pub repo_ref: &'a str,
    pub step: &'a WorkflowStepDefinition,
    pub local_state: &'a Value,
    pub cancellation: CancellationToken,
    pub capability_invocation_id: Option<String>,
}

impl CapabilityContext<'_> {
    pub fn ensure_active(&self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(anyhow!("workflow execution was cancelled"));
        }
        Ok(())
    }

    pub async fn request_user_input(
        &self,
        message: impl Into<String>,
        recommended_disposition: impl Into<String>,
        available_dispositions: Vec<String>,
    ) -> Result<OperatorInputResponse> {
        self.ensure_active()?;

        let message = message.into();
        let recommended_disposition = recommended_disposition.into();
        let receiver = self
            .state
            .operator_inputs
            .register(self.run_id)
            .ok_or_else(|| anyhow!("workflow already has an active operator input request"))?;

        let mut run = load_run(self.state, self.run_id).await?;
        let stage_execution_id = self
            .local_state
            .get("_stage_execution_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        {
            let root = ensure_engine_root(&mut run.context);
            let run_state = root
                .entry("run_state".to_string())
                .or_insert_with(|| json!({}));
            let run_state = run_state
                .as_object_mut()
                .ok_or_else(|| anyhow!("run_state must be object"))?;

            run_state.insert(
                "blocked_on".to_string(),
                json!({
                    "kind": "operator_checkpoint",
                    "capability": "operator_checkpoint",
                    "process_session_id": self.state.process_session_id(),
                    "stage_id": self.step.id,
                    "stage_type": self.step.step_type,
                    "stage_execution_id": stage_execution_id,
                    "capability_invocation_id": self.capability_invocation_id,
                    "recommended_disposition": recommended_disposition,
                    "available_dispositions": available_dispositions,
                    "next_step_id": self.step.id,
                    "message": message
                }),
            );
        }

        if let Err(error) = persist_context(self.state, self.run_id, &run.context).await {
            self.state.operator_inputs.clear(self.run_id);
            return Err(error);
        }

        if let Err(error) = set_run_status(
            self.state,
            self.run_id,
            RunStatus::Running,
            Some(self.step.id.as_str()),
        )
        .await
        {
            self.state.operator_inputs.clear(self.run_id);
            return Err(error);
        }

        append_engine_event(
            self.state,
            self.run_id,
            Some(self.step.id.as_str()),
            "info",
            "capability_execution_state_changed",
            message.as_str(),
            json!({
                "capability": "operator_checkpoint",
                "execution_state": "awaiting_user_input",
                "message": message,
                "recommended_disposition": recommended_disposition,
                "available_dispositions": available_dispositions,
                "run_context": run.context,
                "status": "running",
                "current_step_id": self.step.id,
                "event_meta": event_meta(
                    Some(stage_execution_id.as_str()),
                    self.capability_invocation_id.as_deref(),
                    None,
                    false
                )
            }),
        )
        .await?;

        let response = tokio::select! {
            _ = self.cancellation.cancelled() => {
                self.state.operator_inputs.clear(self.run_id);
                Err(anyhow!("workflow execution was cancelled"))
            }
            response = receiver => {
                response.map_err(|_| anyhow!("operator input request was cancelled"))
            }
        };

        let mut run = load_run(self.state, self.run_id).await?;
        {
            let root = ensure_engine_root(&mut run.context);
            if let Some(run_state) = root.get_mut("run_state").and_then(Value::as_object_mut) {
                run_state.remove("blocked_on");
            }
        }
        persist_context(self.state, self.run_id, &run.context).await?;

        if response.is_ok() {
            set_run_status(
                self.state,
                self.run_id,
                RunStatus::Running,
                Some(self.step.id.as_str()),
            )
            .await?;
        }

        response
    }

    pub fn provide_prompt_text(
        &self,
        source: impl Into<String>,
        label: impl Into<String>,
        text: impl Into<String>,
    ) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }

        self.state.orchestration_inputs.publish(
            self.run_id,
            OrchestrationInputScope::Run,
            OrchestrationInputLifecycle::SingleUse,
            500,
            OrchestrationInputPayload::PromptContribution {
                text,
                source: Some(source.into()),
                label: Some(label.into()),
            },
        );
    }

    pub fn provide_prompt_attachment(
        &self,
        path: impl Into<String>,
        filename: impl Into<String>,
        media_type: Option<String>,
        role: AttachmentRole,
    ) {
        let path = path.into();
        if path.trim().is_empty() {
            return;
        }

        self.state.orchestration_inputs.publish(
            self.run_id,
            OrchestrationInputScope::Run,
            OrchestrationInputLifecycle::SingleUse,
            500,
            OrchestrationInputPayload::Attachment {
                path,
                filename: filename.into(),
                media_type,
                role,
            },
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityInvocation {
    pub capability: String,
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CapabilityInvocationRequest {
    None,
    One(CapabilityInvocation),
    Many(Vec<CapabilityInvocation>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityResult {
    pub ok: bool,
    pub capability: String,
    pub payload: Value,
    pub follow_ups: CapabilityInvocationRequest,
}

fn stage_capability_policy_from_queue(queue: &[CapabilityInvocation]) -> Result<StageCapabilityPolicy> {
    let capabilities = queue
        .iter()
        .map(|invocation| invocation.capability.clone())
        .collect::<Vec<_>>();

    let entrypoint = capabilities
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("stage capability queue does not define any invocations"))?;

    Ok(StageCapabilityPolicy {
        entrypoint,
        allowed_invocations: capabilities,
    })
}

pub fn stage_capability_policy(step: &WorkflowStepDefinition) -> Result<StageCapabilityPolicy> {
    let mut capabilities: Vec<String> = step
        .execution_plan
        .iter()
        .filter(|node| node.enabled && node.kind == StageExecutionNodeKind::Capability)
        .map(|node| node.key.clone())
        .collect();

    if capabilities.is_empty() {
        capabilities = step
            .capabilities
            .iter()
            .filter(|binding| binding.enabled)
            .map(|binding| binding.capability.clone())
            .collect();
    }

    let entrypoint = capabilities
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("stage '{}' does not define any enabled capabilities", step.id))?;

    Ok(StageCapabilityPolicy {
        entrypoint,
        allowed_invocations: capabilities,
    })
}

fn ensure_allowed(policy: &StageCapabilityPolicy, capability: &str) -> Result<()> {
    if capability == "planner_apply"
        || capability == "operator_checkpoint"
        || capability == "repo_sync_changeset"
    {
        return Ok(());
    }

    if capability == policy.entrypoint || policy.allowed_invocations.iter().any(|item| item == capability) {
        return Ok(());
    }
    Err(anyhow!(
        "capability '{}' is not present in the stage capability plan rooted at '{}'",
        capability,
        policy.entrypoint
    ))
}

pub async fn execute_root_capability(ctx: CapabilityContext<'_>) -> Result<Vec<CapabilityResult>> {
    let policy = stage_capability_policy(ctx.step)?;
    let root = CapabilityInvocation {
        capability: policy.entrypoint.clone(),
        config: json!({}),
    };
    execute_capability_chain(ctx, &policy, vec![root]).await
}

pub async fn execute_capability_invocations(
    ctx: CapabilityContext<'_>,
    queue: Vec<CapabilityInvocation>,
) -> Result<Vec<CapabilityResult>> {
    if queue.is_empty() {
        return Ok(Vec::new());
    }

    let policy = stage_capability_policy_from_queue(&queue)?;
    execute_capability_chain(ctx, &policy, queue).await
}

pub(crate) async fn execute_capability_chain(
    ctx: CapabilityContext<'_>,
    policy: &StageCapabilityPolicy,
    mut queue: Vec<CapabilityInvocation>,
) -> Result<Vec<CapabilityResult>> {
    let mut results = Vec::new();
    let stage_execution_id = ctx
        .local_state
        .get("_stage_execution_id")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    while let Some(invocation) = queue.first().cloned() {
        ctx.ensure_active()?;
        queue.remove(0);
        ensure_allowed(policy, invocation.capability.as_str())?;

        let mut governance_run = load_run(ctx.state, ctx.run_id).await?;
        let before_decisions = governance::before_capability(
            ctx.state,
            ctx.run_id,
            &governance_run,
            ctx.step,
            stage_execution_id.as_deref(),
            &invocation,
            &results,
        )
        .await?;
        governance::apply_context_mutations(
            &mut governance_run,
            &before_decisions,
            Some(ctx.step.id.as_str()),
            Some(invocation.capability.as_str()),
        )?;
        if !before_decisions.is_empty() {
            persist_context(ctx.state, ctx.run_id, &governance_run.context).await?;
        }
        for injected in governance::injected_capabilities(&before_decisions).into_iter().rev() {
            if injected.capability != invocation.capability && !queue.iter().any(|item| item.capability == injected.capability) {
                queue.insert(0, injected);
            }
        }

        let capability_invocation_id = Uuid::new_v4().to_string();
        let capability_started_at = Instant::now();

        tracing::info!(
            run_id = %ctx.run_id,
            step_id = %ctx.step.id,
            capability = %invocation.capability,
            capability_invocation_id = %capability_invocation_id,
            "capability dispatch starting"
        );

        append_engine_event(
            ctx.state,
            ctx.run_id,
            Some(ctx.step.id.as_str()),
            "info",
            &format!("{}_started", invocation.capability),
            &format!("{} started", invocation.capability.replace('_', " ")),
            json!({
        // governor hooks can be added here later for before_capability / after_capability if needed.

                "capability": invocation.capability,
                "config": invocation.config,
                "event_meta": event_meta(stage_execution_id.as_deref(), Some(capability_invocation_id.as_str()), None, false)
            }),
        )
        .await?;

        let dispatch_result = tokio::select! {
            biased;
            _ = ctx.cancellation.cancelled() => {
                let duration_ms = i64::try_from(capability_started_at.elapsed().as_millis()).unwrap_or(i64::MAX);
                append_engine_event(
                    ctx.state,
                    ctx.run_id,
                    Some(ctx.step.id.as_str()),
                    "error",
                    &format!("{}_failed", invocation.capability),
                    &format!("{} cancelled", invocation.capability.replace('_', " ")),
                    json!({
                        "capability": invocation.capability,
                        "config": invocation.config,
                        "error": "workflow execution was cancelled",
                        "cancelled": true,
                        "duration_ms": duration_ms,
                        "event_meta": event_meta(stage_execution_id.as_deref(), Some(capability_invocation_id.as_str()), None, false)
                    }),
                )
                .await?;
                return Err(anyhow!("workflow execution was cancelled"));
            }
            result = async {
                let invocation_ctx = CapabilityContext {
                    capability_invocation_id: Some(capability_invocation_id.clone()),
                    ..ctx.clone()
                };
                dispatch(&invocation_ctx, policy, &results, invocation.clone()).await
            } => result,
        };

        ctx.ensure_active()?;

        let mut result = match dispatch_result {
            Ok(result) => {
                tracing::info!(
                    run_id = %ctx.run_id,
                    step_id = %ctx.step.id,
                    capability = %invocation.capability,
                    capability_invocation_id = %capability_invocation_id,
                    ok = result.ok,
                    duration_ms = i64::try_from(capability_started_at.elapsed().as_millis()).unwrap_or(i64::MAX),
                    "capability dispatch returned"
                );
                result
            }
            Err(err) => {
                tracing::error!(
                    run_id = %ctx.run_id,
                    step_id = %ctx.step.id,
                    capability = %invocation.capability,
                    capability_invocation_id = %capability_invocation_id,
                    duration_ms = i64::try_from(capability_started_at.elapsed().as_millis()).unwrap_or(i64::MAX),
                    error = %err,
                    "capability dispatch returned error"
                );

                append_engine_event(
                    ctx.state,
                    ctx.run_id,
                    Some(ctx.step.id.as_str()),
                    "error",
                    &format!("{}_failed", invocation.capability),
                    &format!("{} failed", invocation.capability.replace('_', " ")),
                    json!({
                        "capability": invocation.capability,
                        "config": invocation.config,
                        "error": err.to_string(),
                        "duration_ms": i64::try_from(capability_started_at.elapsed().as_millis()).unwrap_or(i64::MAX),
                        "event_meta": event_meta(stage_execution_id.as_deref(), Some(capability_invocation_id.as_str()), None, false)
                    }),
                )
                .await?;

                CapabilityResult {
                    ok: false,
                    capability: invocation.capability.clone(),
                    payload: json!({
                        "ok": false,
                        "summary": format!("Capability '{}' failed during execution.", invocation.capability),
                        "error": err.to_string(),
                        "config": invocation.config,
                    }),
                    follow_ups: CapabilityInvocationRequest::None,
                }
            }
        };

        if let Some(obj) = result.payload.as_object_mut() {
            obj.insert("_stage_execution_id".to_string(), Value::String(stage_execution_id.clone().unwrap_or_default()));
            obj.insert("_capability_invocation_id".to_string(), Value::String(capability_invocation_id.clone()));
        }

        ctx.ensure_active()?;

        let mut governance_run = load_run(ctx.state, ctx.run_id).await?;
        let after_decisions = governance::after_capability(
            ctx.state,
            ctx.run_id,
            &governance_run,
            ctx.step,
            stage_execution_id.as_deref(),
            &result,
            &results,
        )
        .await?;
        governance::apply_context_mutations(
            &mut governance_run,
            &after_decisions,
            Some(ctx.step.id.as_str()),
            Some(result.capability.as_str()),
        )?;
        if !after_decisions.is_empty() {
            persist_context(ctx.state, ctx.run_id, &governance_run.context).await?;
        }
        let governance_pause_requested = governance::pause_message(&after_decisions).is_some();
        let governance_follow_ups = if governance_pause_requested {
            Vec::new()
        } else {
            governance::injected_capabilities(&after_decisions)
        };

        if let Some(payload) = result.payload.as_object_mut() {
            payload.insert(
                "execution_state".to_string(),
                Value::String("completed".to_string()),
            );
        }

        let capability_event_kind = format!("{}_completed", result.capability);
        let capability_event_message = format!("{} completed", result.capability.replace('_', " "));

        append_engine_event(
            ctx.state,
            ctx.run_id,
            Some(ctx.step.id.as_str()),
            if result.ok { "info" } else { "error" },
            capability_event_kind.as_str(),
            capability_event_message.as_str(),
            json!({
                "capability": result.capability,
                "ok": result.ok,
                "execution_state": "completed",
                "duration_ms": json!(i64::try_from(capability_started_at.elapsed().as_millis()).unwrap_or(i64::MAX)),
                "result": result.payload,
                "event_meta": event_meta(stage_execution_id.as_deref(), Some(capability_invocation_id.as_str()), None, false)
            }),
        )
        .await?;

        tracing::info!(
            run_id = %ctx.run_id,
            step_id = %ctx.step.id,
            capability = %result.capability,
            capability_invocation_id = %capability_invocation_id,
            ok = result.ok,
            follow_up_count = follow_up_vec(&result.follow_ups).len(),
            "capability result recorded"
        );

        let existing_capabilities: std::collections::HashSet<String> = queue
            .iter()
            .map(|item| item.capability.clone())
            .chain(results.iter().map(|item| item.capability.clone()))
            .collect();

        let changeset_applied_actions = result.capability == "changeset"
            && result
                .payload
                .get("stats")
                .and_then(|stats| stats.get("successful_actions"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0;

        let requested_follow_ups = follow_up_vec(&result.follow_ups);
        let capability_follow_ups = if result.ok || changeset_applied_actions {
            requested_follow_ups
        } else {
            requested_follow_ups
                .into_iter()
                .filter(|item| item.capability == "operator_checkpoint")
                .collect()
        };

        let mut follow_ups = capability_follow_ups
            .into_iter()
            .chain(governance_follow_ups.into_iter())
            .filter(|item| !existing_capabilities.contains(&item.capability))
            .collect::<Vec<_>>();

        if result.capability == "inference" {
            if let Some(config) = planner_apply_config(&ctx)? {
                if !existing_capabilities.contains("planner_apply")
                    && !queue.iter().any(|item| item.capability == "planner_apply")
                {
                    follow_ups.push(CapabilityInvocation {
                        capability: "planner_apply".to_string(),
                        config,
                    });
                }
            }
        }

        let checkpoint_stops_chain = result.capability == "operator_checkpoint"
            && matches!(
                result.payload.get("disposition").and_then(Value::as_str),
                Some("pause_error" | "select_stage")
            );

        queue.extend(follow_ups);
        results.push(result);
        if governance_pause_requested || checkpoint_stops_chain {
            break;
        }
    }

    Ok(results)
}

fn follow_up_vec(req: &CapabilityInvocationRequest) -> Vec<CapabilityInvocation> {
    match req {
        CapabilityInvocationRequest::None => Vec::new(),
        CapabilityInvocationRequest::One(item) => vec![item.clone()],
        CapabilityInvocationRequest::Many(items) => items.clone(),
    }
}

fn planner_apply_config(ctx: &CapabilityContext<'_>) -> Result<Option<Value>> {
    if !crate::engine::stages::stage_supports_capability(ctx.step, "planner_apply")
        || !capability_enabled(ctx.local_state, "planner_apply", false)
    {
        return Ok(None);
    }

    let state = planner::PlannerCapabilityState::from_global_state(ctx.local_state)?;

    if !state.auto_apply_armed {
        return Ok(None);
    }

    Ok(Some(serde_json::to_value(state.binding()?)?))
}

async fn dispatch(
    ctx: &CapabilityContext<'_>,
    _policy: &StageCapabilityPolicy,
    prior_results: &[CapabilityResult],
    invocation: CapabilityInvocation,
) -> Result<CapabilityResult> {
    match invocation.capability.as_str() {
        "inference" => inference::execute(ctx, prior_results, invocation.config).await,
        "context_export" => context_export::execute(ctx, prior_results, invocation.config).await,
        "changeset_schema" => changeset::schema::execute(ctx, prior_results, invocation.config).await,
        "changeset" => changeset::apply::execute(ctx, prior_results, invocation.config).await,
        "repo_sync_changeset" => repo_sync::execute(ctx, prior_results, invocation.config).await,
        "planner_apply" => planner::apply::execute(ctx, prior_results, invocation.config).await,
        "compile_commands" => compile_commands::execute(ctx, prior_results, invocation.config).await,
        "shared_dependencies" => shared_dependencies::execute(ctx, prior_results, invocation.config).await,
        "qa_environment" => qa_environment::execute(ctx, prior_results, invocation.config).await,
        "git_patch_payload" => git_patch_payload::execute(ctx, prior_results, invocation.config).await,
        "review_validation" => review_validation::execute(ctx, prior_results, invocation.config).await,
        "operator_checkpoint" => operator_checkpoint::execute(ctx, prior_results, invocation.config).await,
        "sap/import" => sap::import::execute(ctx, prior_results, invocation.config).await,
        "sap/export" => sap::export::execute(ctx, prior_results, invocation.config).await,
        other => Err(anyhow!("unknown capability '{}'", other)),
    }
}

pub fn find_result<'a>(results: &'a [CapabilityResult], capability: &str) -> Option<&'a CapabilityResult> {
    results.iter().find(|item| item.capability == capability)
}
