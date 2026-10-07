use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use super::session::{InferenceSession, ResolvedInferenceRoute};
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

pub async fn materialize_run_bindings(
    state: &AppState,
    run: &WorkflowRun,
) -> Result<()> {
    let existing_bindings = session::bindings_for_run(&state.db, run.id).await?;
    let mut new_sessions = std::collections::BTreeMap::<String, String>::new();

    for step in &run.definition.steps {
        if !crate::engine::stages::stage_supports_capability(step, "inference") {
            continue;
        }

        let route = session::resolve_inference_route_from_run(run, step)?;

        if let Some(session_id) = existing_bindings.get(&step.step_type) {
            if route.existing_session_id.is_none() {
                new_sessions
                    .entry(route.name.clone())
                    .or_insert_with(|| session_id.clone());
            }
            continue;
        }

        if let Some(session_id) = route.existing_session_id.as_deref() {
            let selected = session::get_session(&state.db, session_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("configured inference session '{}' was not found", session_id))?;

            if selected.repo_ref != run.repo_ref {
                anyhow::bail!("configured inference session '{}' belongs to a different repository", session_id);
            }

            if selected.status == "archived" {
                anyhow::bail!("configured inference session '{}' is archived", session_id);
            }

            session::bind_session(&state.db, run.id, &step.step_type, session_id).await?;
            continue;
        }

        if let Some(session_id) = new_sessions.get(&route.name) {
            session::bind_session(&state.db, run.id, &step.step_type, session_id).await?;
            continue;
        }

        let created = session::create_session(
            &state.db,
            &run.repo_ref,
            run.id,
            &route,
        )
        .await?;

        new_sessions.insert(route.name.clone(), created.id);
    }

    Ok(())
}

pub async fn backfill_run_bindings_on_startup(state: &AppState) -> Result<usize> {
    let run_ids = sqlx::query_scalar::<_, String>(
        "SELECT id FROM workflow_runs WHERE status NOT IN ('success', 'cancelled') ORDER BY created_at ASC",
    )
    .fetch_all(&state.db)
    .await?;

    let mut backfilled = 0usize;

    for run_id in run_ids {
        let run_id = uuid::Uuid::parse_str(&run_id)?;
        let run = crate::engine::load_run(state, run_id).await?;
        let before = session::bindings_for_run(&state.db, run_id).await?.len();

        match materialize_run_bindings(state, &run).await {
            Ok(()) => {
                let after = session::bindings_for_run(&state.db, run_id).await?.len();
                backfilled += after.saturating_sub(before);
            }
            Err(error) => {
                tracing::warn!(
                    run_id = %run_id,
                    error = %format!("{:#}", error),
                    "failed to backfill workflow inference bindings"
                );
            }
        }
    }

    Ok(backfilled)
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
                ctx.repo_ref,
                ctx.run_id,
                route,
            )
            .await?;

            crate::engine::automation::apply_inference_session_transition(
                ctx.state,
                ctx.run_id,
                ctx.step,
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

                logical_session = session::create_session(
                    &ctx.state.db,
                    ctx.repo_ref,
                    ctx.run_id,
                    route,
                )
                .await?;
                creating = true;

                crate::engine::automation::apply_inference_session_transition(
                    ctx.state,
                    ctx.run_id,
                    ctx.step,
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
