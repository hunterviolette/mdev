use std::time::Instant;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{app_state::AppState, engine::{append_engine_event, event_meta}, models::WorkflowStepDefinition};

use super::{changeset_schema, compile_commands, context_export, gateway_model, inference};

#[derive(Debug, Clone)]
pub struct StageCapabilityPolicy {
    pub entrypoint: &'static str,
    pub allowed_invocations: &'static [&'static str],
}

#[derive(Clone)]
pub struct CapabilityContext<'a> {
    pub state: &'a AppState,
    pub run_id: Uuid,
    pub repo_ref: &'a str,
    pub step: &'a WorkflowStepDefinition,
    pub local_state: &'a Value,
}

#[derive(Debug, Clone)]
pub struct CapabilityInvocation {
    pub capability: String,
    pub config: Value,
}

#[derive(Debug, Clone)]
pub enum CapabilityInvocationRequest {
    None,
    One(CapabilityInvocation),
    Many(Vec<CapabilityInvocation>),
}

#[derive(Debug, Clone)]
pub struct CapabilityResult {
    pub ok: bool,
    pub capability: String,
    pub payload: Value,
    pub follow_ups: CapabilityInvocationRequest,
}

pub fn stage_capability_policy(step: &WorkflowStepDefinition) -> Result<StageCapabilityPolicy> {
    match step.step_type.as_str() {
        "design" => Ok(StageCapabilityPolicy {
            entrypoint: "inference",
            allowed_invocations: &["context_export"],
        }),
        "code" => Ok(StageCapabilityPolicy {
            entrypoint: "inference",
            allowed_invocations: &[
                "context_export",
                "changeset_schema",
                "gateway_model/changeset",
            ],
        }),
        "compile" => Ok(StageCapabilityPolicy {
            entrypoint: "compile_commands",
            allowed_invocations: &[],
        }),
        "review" => Ok(StageCapabilityPolicy {
            entrypoint: "inference",
            allowed_invocations: &[],
        }),
        other => Err(anyhow!("unsupported step_type for capability policy: {}", other)),
    }
}

fn ensure_allowed(policy: &StageCapabilityPolicy, capability: &str) -> Result<()> {
    if capability == policy.entrypoint || policy.allowed_invocations.iter().any(|item| *item == capability) {
        return Ok(());
    }
    Err(anyhow!(
        "capability '{}' is not allowed for stage entrypoint '{}'",
        capability,
        policy.entrypoint
    ))
}

pub async fn execute_root_capability(ctx: CapabilityContext<'_>) -> Result<Vec<CapabilityResult>> {
    let policy = stage_capability_policy(ctx.step)?;
    let root = CapabilityInvocation {
        capability: policy.entrypoint.to_string(),
        config: json!({}),
    };
    execute_capability_chain(ctx, &policy, vec![root]).await
}

pub async fn execute_capability_invocations(
    ctx: CapabilityContext<'_>,
    queue: Vec<CapabilityInvocation>,
) -> Result<Vec<CapabilityResult>> {
    let policy = stage_capability_policy(ctx.step)?;
    execute_capability_chain(ctx, &policy, queue).await
}

async fn execute_capability_chain(
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
        queue.remove(0);
        ensure_allowed(policy, invocation.capability.as_str())?;

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
                "capability": invocation.capability,
                "config": invocation.config,
                "event_meta": event_meta(stage_execution_id.as_deref(), Some(capability_invocation_id.as_str()), None, false)
            }),
        )
        .await?;

        let result = match dispatch(&ctx, policy, &results, invocation.clone()).await {
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

        append_engine_event(
            ctx.state,
            ctx.run_id,
            Some(ctx.step.id.as_str()),
            if result.ok { "info" } else { "error" },
            &format!("{}_completed", result.capability),
            &format!("{} completed", result.capability.replace('_', " ")),
            json!({
                "capability": result.capability,
                "ok": result.ok,
                "duration_ms": i64::try_from(capability_started_at.elapsed().as_millis()).unwrap_or(i64::MAX),
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

        let follow_ups = follow_up_vec(&result.follow_ups)
            .into_iter()
            .filter(|item| !existing_capabilities.contains(&item.capability))
            .collect::<Vec<_>>();

        queue.extend(follow_ups);
        results.push(result);
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

async fn dispatch(
    ctx: &CapabilityContext<'_>,
    _policy: &StageCapabilityPolicy,
    prior_results: &[CapabilityResult],
    invocation: CapabilityInvocation,
) -> Result<CapabilityResult> {
    match invocation.capability.as_str() {
        "inference" => inference::execute(ctx, prior_results, invocation.config).await,
        "context_export" => context_export::execute(ctx, prior_results, invocation.config).await,
        "changeset_schema" => changeset_schema::execute(ctx, prior_results, invocation.config).await,
        "gateway_model/changeset" => gateway_model::changeset::execute(ctx, prior_results, invocation.config).await,
        "compile_commands" => compile_commands::execute(ctx, prior_results, invocation.config).await,
        other => Err(anyhow!("unknown capability '{}'", other)),
    }
}

pub fn find_result<'a>(results: &'a [CapabilityResult], capability: &str) -> Option<&'a CapabilityResult> {
    results.iter().find(|item| item.capability == capability)
}
