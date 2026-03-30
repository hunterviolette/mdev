use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone)]
pub struct WorkflowApiClient {
    base_url: String,
    client: Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowRunDto {
    pub id: String,
    pub template_id: Option<String>,
    pub status: String,
    pub current_step_id: Option<String>,
    pub title: String,
    pub repo_ref: String,
    pub context: Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowEventDto {
    pub id: String,
    pub run_id: String,
    pub step_id: Option<String>,
    pub level: String,
    pub kind: String,
    pub message: String,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateRunBody {
    pub template_id: Option<String>,
    pub title: String,
    pub repo_ref: String,
    pub context: Value,
}

impl WorkflowApiClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            client: Client::new(),
        }
    }

    pub fn create_run(&self, body: CreateRunBody) -> Result<WorkflowRunDto> {
        self.client
            .post(format!("{}/api/workflow-runs", self.base_url))
            .json(&body)
            .send()
            .context("failed to create workflow run")?
            .error_for_status()
            .context("workflow run creation failed")?
            .json()
            .context("failed to decode workflow run")
    }

    pub fn invoke_context_export(
        &self,
        run_id: &str,
        step_id: Option<&str>,
        payload: Value,
    ) -> Result<Value> {
        self.client
            .post(format!(
                "{}/api/workflow-runs/{}/capabilities/context-export",
                self.base_url, run_id
            ))
            .json(&json!({
                "step_id": step_id,
                "payload": payload,
            }))
            .send()
            .context("failed to invoke context export")?
            .error_for_status()
            .context("context export invocation failed")?
            .json()
            .context("failed to decode context export response")
    }

    pub fn invoke_payload_gateway(
        &self,
        run_id: &str,
        step_id: Option<&str>,
        payload: Value,
    ) -> Result<Value> {
        self.client
            .post(format!(
                "{}/api/workflow-runs/{}/capabilities/payload-gateway",
                self.base_url, run_id
            ))
            .json(&json!({
                "step_id": step_id,
                "payload": payload,
            }))
            .send()
            .context("failed to invoke payload gateway")?
            .error_for_status()
            .context("payload gateway invocation failed")?
            .json()
            .context("failed to decode payload gateway response")
    }

    pub fn list_run_events(&self, run_id: &str) -> Result<Vec<WorkflowEventDto>> {
        self.client
            .get(format!("{}/api/workflow-runs/{}/events", self.base_url, run_id))
            .send()
            .context("failed to fetch workflow events")?
            .error_for_status()
            .context("workflow events request failed")?
            .json()
            .context("failed to decode workflow events")
    }
}

pub fn repo_ref_from_repo_path(repo: &std::path::Path) -> String {
    repo.to_string_lossy().replace('\\', "/")
}

pub fn ensure_run_for_repo(
    api: &WorkflowApiClient,
    repo: &std::path::Path,
    title: &str,
) -> Result<String> {
    let run = api.create_run(CreateRunBody {
        template_id: None,
        title: title.to_string(),
        repo_ref: repo_ref_from_repo_path(repo),
        context: json!({
            "repo_path": repo_ref_from_repo_path(repo),
            "source": "egui",
        }),
    })?;

    if run.id.trim().is_empty() {
        return Err(anyhow!("workflow api returned empty run id"));
    }

    Ok(run.id)
}
