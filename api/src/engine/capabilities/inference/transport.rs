use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use super::session::{InferenceSession, ResolvedInferenceRoute};
use super::accounting::ReportedUsage;
use super::super::registry::CapabilityContext;
use super::{api, browser, prompting::ModelInput, session, InferenceTransport};
use crate::{
    app_state::AppState,
    models::{WorkflowRun, WorkflowStepDefinition},
};

pub struct InferenceTransportExecution {
    pub text: String,
    pub metadata: Value,
    pub transport_state: Value,
    pub reported_usage: Option<ReportedUsage>,
}

#[async_trait]
pub trait InferenceTransportAdapter: Send + Sync {
    async fn session_available(
        &self,
        state: &AppState,
        route: &ResolvedInferenceRoute,
        session: &InferenceSession,
    ) -> Result<bool>;

    fn is_session_unavailable_error(&self, _error: &anyhow::Error) -> bool {
        false
    }

    async fn create_session(
        &self,
        ctx: &CapabilityContext<'_>,
        route: &ResolvedInferenceRoute,
        input: &ModelInput,
    ) -> Result<InferenceTransportExecution>;

    async fn continue_session(
        &self,
        ctx: &CapabilityContext<'_>,
        route: &ResolvedInferenceRoute,
        session: &InferenceSession,
        input: &ModelInput,
    ) -> Result<InferenceTransportExecution>;
}

fn inference_configs_equal(
    left: &super::InferenceConfig,
    right: &super::InferenceConfig,
) -> Result<bool> {
    Ok(serde_json::to_value(left)? == serde_json::to_value(right)?)
}

pub async fn materialize_run_bindings(
    state: &AppState,
    run: &WorkflowRun,
) -> Result<()> {
    let existing_bindings = session::bindings_for_run(&state.db, run.id).await?;
    let session_repo_ref = session::repository_scope_for_run(run);
    let mut new_sessions = std::collections::BTreeMap::<String, String>::new();
    let mut created_session = false;

    for step in &run.definition.steps {
        if !crate::engine::stages::stage_supports_capability(step, "inference") {
            continue;
        }

        let route = session::resolve_inference_route_from_run(run, step)?;

        if let Some(session_id) = existing_bindings.get(&step.step_type) {
            if route.existing_session_id.is_none() {
                if let Some(bound) = session::get_session(&state.db, session_id).await? {
                    if bound.repo_ref == session_repo_ref
                        && bound.status != "archived"
                        && inference_configs_equal(&bound.config, &route.config)?
                    {
                        new_sessions.entry(route.name.clone()).or_insert_with(|| session_id.clone());
                        continue;
                    }
                }
            }
        }

        if let Some(session_id) = route.existing_session_id.as_deref() {
            let selected = session::get_session(&state.db, session_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("configured inference session '{}' was not found", session_id))?;

            if selected.repo_ref != session_repo_ref {
                anyhow::bail!("configured inference session '{}' belongs to a different repository", session_id);
            }

            if selected.status == "archived" {
                anyhow::bail!("configured inference session '{}' is archived", session_id);
            }

            if existing_bindings.get(&step.step_type).map(String::as_str) != Some(session_id) {
                session::bind_session(&state.db, run.id, &step.step_type, session_id).await?;
            }
            continue;
        }

        if let Some(session_id) = new_sessions.get(&route.name) {
            session::bind_session(&state.db, run.id, &step.step_type, session_id).await?;
            continue;
        }

        if let Some(session_id) = existing_bindings.get(&step.step_type) {
            let selected = session::get_session(&state.db, session_id).await?;
            if let Some(selected) = selected {
                if selected.repo_ref == session_repo_ref
                    && selected.status != "archived"
                    && inference_configs_equal(&selected.config, &route.config)?
                {
                    new_sessions.insert(route.name.clone(), selected.id);
                    continue;
                }
            }
        }

        let created = session::create_session(
            &state.db,
            &session_repo_ref,
            run.id,
            &route,
        )
        .await?;

        new_sessions.insert(route.name.clone(), created.id);
        created_session = true;
    }

    if created_session {
        crate::engine::automation::apply_inference_session_transition(state, run.id).await?;
    }

    Ok(())
}

pub async fn reconcile_stage_session(
    state: &AppState,
    run: &WorkflowRun,
    step: &WorkflowStepDefinition,
) -> Result<bool> {
    if !crate::engine::stages::stage_supports_capability(step, "inference") {
        return Ok(false);
    }

    let route = session::resolve_inference_route_from_run(run, step)?;
    let session_repo_ref = session::repository_scope_for_run(run);
    let current = session::load_bound_session(&state.db, run.id, &step.step_type).await?;

    if let Some(session_id) = route.existing_session_id.as_deref() {
        let selected = session::get_session(&state.db, session_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("configured inference session '{}' was not found", session_id))?;

        if selected.repo_ref != session_repo_ref {
            anyhow::bail!("configured inference session '{}' belongs to a different repository", session_id);
        }

        if selected.status == "archived" {
            anyhow::bail!("configured inference session '{}' is archived", session_id);
        }

        if current.as_ref().map(|bound| bound.id.as_str()) != Some(session_id) {
            session::bind_session(&state.db, run.id, &step.step_type, session_id).await?;
            return Ok(true);
        }

        if selected.status == "pending" {
            return Ok(false);
        }

        let mut effective_route = route.clone();
        effective_route.config = selected.config.clone();
        let adapter = adapter_for_transport(&effective_route.config.transport);
        let available = selected.status == "active"
            && adapter.session_available(state, &effective_route, &selected).await?;

        if available {
            return Ok(false);
        }

        let created = session::create_session(&state.db, &session_repo_ref, run.id, &effective_route).await?;
        session::rebind_run_session(&state.db, run.id, &selected.id, &created.id).await?;
        crate::engine::automation::apply_inference_session_transition(state, run.id).await?;
        return Ok(true);
    }

    if let Some(bound) = current.as_ref() {
        if bound.repo_ref == session_repo_ref && bound.status != "archived" && inference_configs_equal(&bound.config, &route.config)? {
            if bound.status == "pending" {
                return Ok(false);
            }

            let adapter = adapter_for_transport(&route.config.transport);
            if bound.status == "active" && adapter.session_available(state, &route, bound).await? {
                return Ok(false);
            }
        }
    }

    let bindings = session::bindings_for_run(&state.db, run.id).await?;
    for candidate in &run.definition.steps {
        if candidate.step_type == step.step_type
            || !crate::engine::stages::stage_supports_capability(candidate, "inference")
        {
            continue;
        }

        let candidate_route = session::resolve_inference_route_from_run(run, candidate)?;
        if candidate_route.existing_session_id.is_some()
            || candidate_route.name != route.name
            || !inference_configs_equal(&candidate_route.config, &route.config)?
        {
            continue;
        }

        let Some(candidate_id) = bindings.get(&candidate.step_type) else {
            continue;
        };
        let Some(candidate_session) = session::get_session(&state.db, candidate_id).await? else {
            continue;
        };

        if candidate_session.repo_ref == session_repo_ref
            && candidate_session.status != "archived"
            && inference_configs_equal(&candidate_session.config, &route.config)?
        {
            session::bind_session(&state.db, run.id, &step.step_type, &candidate_session.id).await?;
            return Ok(true);
        }
    }

    let created = session::create_session(&state.db, &session_repo_ref, run.id, &route).await?;
    crate::engine::automation::apply_inference_session_transition(state, run.id).await?;
    let _ = created;
    Ok(true)
}

pub struct Inference;

impl Inference {
    pub fn new() -> Self {
        Self
    }

    pub async fn execute(
        &self,
        ctx: &CapabilityContext<'_>,
        route: &ResolvedInferenceRoute,
        input: &ModelInput,
    ) -> Result<Value> {
        let existing = session::bound_session(ctx, &ctx.step.step_type)
            .await?
            .ok_or_else(|| anyhow::anyhow!(
                "inference stage '{}' does not have a materialized session binding",
                ctx.step.step_type
            ))?;
        let session_repo_ref = existing.repo_ref.clone();

        let mut effective_route = route.clone();
        effective_route.config = existing.config.clone();

        let route = &effective_route;
        let adapter = adapter_for_transport(&route.config.transport);
        let available = existing.status == "active"
            && adapter
                .session_available(ctx.state, route, &existing)
                .await?;

        let (mut logical_session, mut creating) = if available {
            (existing, false)
        } else if existing.status == "pending" {
            (existing, true)
        } else {
            session::archive_session(&ctx.state.db, &existing.id).await?;

            let created = session::create_session(
                &ctx.state.db,
                &session_repo_ref,
                ctx.run_id,
                route,
            )
            .await?;

            session::rebind_run_session(
                &ctx.state.db,
                ctx.run_id,
                &existing.id,
                &created.id,
            )
            .await?;

            crate::engine::automation::apply_inference_session_transition(
                ctx.state,
                ctx.run_id,
            )
            .await?;

            (created, true)
        };

        let execution = if creating {
            adapter.create_session(ctx, route, input).await
        } else {
            adapter
                .continue_session(ctx, route, &logical_session, input)
                .await
        };

        let execution = match execution {
            Ok(execution) => execution,
            Err(error) if !creating && adapter.is_session_unavailable_error(&error) => {
                session::archive_session(&ctx.state.db, &logical_session.id).await?;

                let previous_session_id = logical_session.id.clone();
                logical_session = session::create_session(
                    &ctx.state.db,
                    &session_repo_ref,
                    ctx.run_id,
                    route,
                )
                .await?;

                session::rebind_run_session(
                    &ctx.state.db,
                    ctx.run_id,
                    &previous_session_id,
                    &logical_session.id,
                )
                .await?;

                creating = true;

                crate::engine::automation::apply_inference_session_transition(
                    ctx.state,
                    ctx.run_id,
                )
                .await?;

                match adapter.create_session(ctx, route, input).await {
                    Ok(execution) => execution,
                    Err(error) => {
                        let _ = session::mark_session_failed(
                            &ctx.state.db,
                            &logical_session.id,
                            &error,
                        )
                        .await;
                        return Err(error);
                    }
                }
            }
            Err(error) => {
                if creating {
                    let _ = session::mark_session_failed(
                        &ctx.state.db,
                        &logical_session.id,
                        &error,
                    )
                    .await;
                }
                return Err(error);
            }
        };

        let logical_session = session::activate_session(
            &ctx.state.db,
            &logical_session.id,
            execution.transport_state.clone(),
        )
        .await?;

        let mut payload = json!({
            "ok": true,
            "text": execution.text
        });

        if let (Some(payload_object), Some(metadata)) = (
            payload.as_object_mut(),
            execution.metadata.as_object(),
        ) {
            for (key, value) in metadata {
                payload_object.insert(key.clone(), value.clone());
            }
        }

        if let Some(object) = payload.as_object_mut() {
            object.insert(
                "inference_session".to_string(),
                json!({
                    "id": logical_session.id,
                    "repo_ref": logical_session.repo_ref,
                    "title": logical_session.title,
                    "transport": logical_session.transport,
                    "status": logical_session.status,
                    "created_at": logical_session.created_at,
                    "updated_at": logical_session.updated_at,
                    "last_used_at": logical_session.last_used_at
                }),
            );
        }

        let provider = payload.get("provider").and_then(Value::as_str).unwrap_or("browser").to_string();
        let model = payload.get("model").and_then(Value::as_str).unwrap_or("").to_string();
        let transport = match route.config.transport {
            super::InferenceTransport::Api => "api",
            super::InferenceTransport::Browser => "browser",
        };
        let invocation_id = super::persistence::record_invocation(
            &ctx.state.db,
            ctx,
            &logical_session.id,
            transport,
            &provider,
            &model,
            input,
            &payload,
            execution.reported_usage.as_ref(),
            &execution.text,
        ).await?;
        if let Some(object) = payload.as_object_mut() {
            object.insert("inference_invocation_id".to_string(), Value::String(invocation_id));
        }

        Ok(payload)
    }
}

fn adapter_for_transport(
    transport: &InferenceTransport,
) -> &'static dyn InferenceTransportAdapter {
    match transport {
        InferenceTransport::Browser => &browser::BROWSER_INFERENCE_TRANSPORT,
        InferenceTransport::Api => &api::API_INFERENCE_TRANSPORT,
    }
}
