use std::{
    collections::{BTreeMap, BTreeSet},
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::engine::runtime_endpoints::{NetworkExposure, RuntimeEndpointManager};

use crate::engine::{
    capabilities::{
        registry::{
            find_result,
            CapabilityContext,
            CapabilityInvocationRequest,
            CapabilityResult,
        },
        terminal_runtime::{self, ProcessRecord, TerminalExecutionOwner},
    },
    runtime_tools::{
        QaEnvironmentSpec,
        QaReadinessSpec,
        QaStageSpec,
        TerminalCommandMode,
        TerminalCommandSpec,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllocatedQaService {
    pub id: String,
    pub label: String,
    pub port: u16,
    pub environment_variable: String,
    pub public: bool,
    pub public_url: Option<String>,
    pub execution: Option<ProcessRecord>,
}

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let qa = resolve_qa_spec(ctx, config)?;
    validate_environment(&qa.environment)?;

    let repo_ref = ctx
        .local_state
        .get("resources")
        .and_then(|value| value.get("repo"))
        .and_then(|value| value.get("repo_ref"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(ctx.repo_ref);
    let workspace = PathBuf::from(repo_ref);
    let session = qa_session_id(ctx);

    let _ = ctx
        .state
        .process_registry
        .terminate_deployment(
            ctx.run_id.to_string().as_str(),
            ctx.step.id.as_str(),
            true,
        )
        .await;

    let shared_environment = shared_dependency_environment(prior_results);
    let mut prepare_sequence = qa.environment.prepare.clone();

    for command in &mut prepare_sequence.commands {
        for (key, value) in &shared_environment {
            command
                .environment
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }

    let prepare_owner = TerminalExecutionOwner {
        run_id: ctx.run_id.to_string(),
        step_id: ctx.step.id.clone(),
        capability: "qa_environment.prepare".to_string(),
        service_id: None,
    };

    let prepare = terminal_runtime::run_sequence(
        &ctx.state.process_registry,
        workspace.as_path(),
        &prepare_sequence,
        &prepare_owner,
    )
    .await?;

    if !prepare.ok {
        return Ok(CapabilityResult {
            ok: false,
            capability: "qa_environment".to_string(),
            payload: json!({
                "ok": false,
                "status": "prepare_failed",
                "session_id": session,
                "prepare": prepare,
                "services": []
            }),
            follow_ups: CapabilityInvocationRequest::None,
        });
    }

    let mut services = allocate_services(&ctx.state.runtime_endpoints, &qa.environment)?;

    let service_ports = services
        .iter()
        .map(|service| (service.id.clone(), service.port))
        .collect::<BTreeMap<_, _>>();
    let service_urls = services
        .iter()
        .map(|service| {
            (
                service.id.clone(),
                format!("http://127.0.0.1:{}", service.port),
            )
        })
        .collect::<BTreeMap<_, _>>();

    for (index, (allocated, service)) in services
        .iter_mut()
        .zip(qa.environment.services.iter())
        .enumerate()
    {
        let mut command = prepare_service_command(
            &service.command,
            workspace.as_path(),
            allocated.port,
            session.as_str(),
            service.id.as_str(),
            allocated.environment_variable.as_str(),
            allocated.public_url.as_deref(),
            index == 0,
            &service_ports,
            &service_urls,
        );

        for (key, value) in &shared_environment {
            command
                .environment
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }

        let execution = terminal_runtime::start_service(
            &ctx.state.process_registry,
            workspace.as_path(),
            &command,
            TerminalExecutionOwner {
                run_id: ctx.run_id.to_string(),
                step_id: ctx.step.id.clone(),
                capability: "qa_environment".to_string(),
                service_id: Some(service.id.clone()),
            },
        )
        .await?;

        let execution_id = execution.execution_id.clone();
        allocated.execution = Some(execution);

        let readiness = wait_for_service_readiness(
            &ctx.state.process_registry,
            execution_id.as_str(),
            service.id.as_str(),
            allocated.port,
            service.command.command.as_str(),
            &service.readiness,
        )
        .await;

        match readiness {
            Ok(current) => {
                allocated.execution = Some(current);
            }
            Err(error) => {
                let current = ctx
                    .state
                    .process_registry
                    .get(execution_id.as_str())
                    .await;

                if let Some(current) = current {
                    allocated.execution = Some(current);
                }

                let _ = ctx
                    .state
                    .process_registry
                    .terminate_deployment(
                        ctx.run_id.to_string().as_str(),
                        ctx.step.id.as_str(),
                        true,
                    )
                    .await;

                return Ok(CapabilityResult {
                    ok: false,
                    capability: "qa_environment".to_string(),
                    payload: json!({
                        "ok": false,
                        "status": "readiness_failed",
                        "session_id": session,
                        "failed_service": service.id,
                        "error": format!("{:#}", error),
                        "public_url": null,
                        "prepare": prepare,
                        "services": services,
                        "environment": qa.environment
                    }),
                    follow_ups: CapabilityInvocationRequest::None,
                });
            }
        }
    }

    let public_url = services
        .iter()
        .find_map(|service| service.public_url.clone());

    Ok(CapabilityResult {
        ok: true,
        capability: "qa_environment".to_string(),
        payload: json!({
            "ok": true,
            "status": "running",
            "session_id": session,
            "public_url": public_url,
            "prepare": prepare,
            "services": services,
            "environment": qa.environment,
            "actions": ["stop", "restart", "fail", "approve"]
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}

async fn wait_for_service_readiness(
    registry: &terminal_runtime::ProcessRegistry,
    execution_id: &str,
    service_id: &str,
    port: u16,
    command: &str,
    readiness: &QaReadinessSpec,
) -> Result<ProcessRecord> {
    let configured_timeout_seconds = match readiness {
        QaReadinessSpec::None => 0,
        QaReadinessSpec::Http {
            timeout_seconds, ..
        }
        | QaReadinessSpec::Tcp { timeout_seconds }
        | QaReadinessSpec::Log {
            timeout_seconds, ..
        } => *timeout_seconds,
    };
    let normalized_command = command.trim().to_ascii_lowercase();
    let timeout_seconds = if normalized_command == "cargo run"
        || normalized_command.starts_with("cargo run ")
    {
        configured_timeout_seconds.max(300)
    } else {
        configured_timeout_seconds
    };
    let deadline = Instant::now() + Duration::from_secs(timeout_seconds);
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;

    loop {
        let current = registry.get(execution_id).await.ok_or_else(|| {
            anyhow!(
                "QA service '{}' disappeared from the process registry during readiness checks",
                service_id
            )
        })?;

        if current.status != "running" && current.status != "starting" {
            bail!(
                "QA service '{}' exited during readiness checks with status '{}'.\ncommand: {}\nworking directory: {}\nstdout:\n{}\nstderr:\n{}",
                service_id,
                current.status,
                current.command,
                current.working_directory,
                current.stdout,
                current.stderr
            );
        }

        let ready = match readiness {
            QaReadinessSpec::None => true,
            QaReadinessSpec::Http {
                path,
                expected_status,
                ..
            } => {
                let normalized_path = if path.starts_with('/') {
                    path.clone()
                } else {
                    format!("/{}", path)
                };
                let url = format!("http://127.0.0.1:{}{}", port, normalized_path);
                match http_client.get(url).send().await {
                    Ok(response) => expected_status
                        .map(|status| response.status().as_u16() == status)
                        .unwrap_or_else(|| response.status().is_success()),
                    Err(_) => false,
                }
            }
            QaReadinessSpec::Tcp { .. } => {
                tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                    .await
                    .is_ok()
            }
            QaReadinessSpec::Log { pattern, .. } => {
                let expression = regex::Regex::new(pattern).map_err(|error| {
                    anyhow!(
                        "invalid readiness log pattern for QA service '{}': {}",
                        service_id,
                        error
                    )
                })?;
                expression.is_match(current.stdout.as_str())
                    || expression.is_match(current.stderr.as_str())
            }
        };

        if ready {
            return Ok(current);
        }

        if Instant::now() >= deadline {
            bail!(
                "QA service '{}' did not satisfy its readiness check within {} seconds.\ncommand: {}\nworking directory: {}\nstdout:\n{}\nstderr:\n{}",
                service_id,
                timeout_seconds,
                current.command,
                current.working_directory,
                current.stdout,
                current.stderr
            );
        }

        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn resolve_qa_spec(ctx: &CapabilityContext<'_>, _config: Value) -> Result<QaStageSpec> {
    let capability = ctx
        .local_state
        .get("capabilities")
        .and_then(|value| value.get("qa_environment"))
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "QA runtime capability state is missing for stage '{}'",
                ctx.step.id
            )
        })?;

    serde_json::from_value(capability)
        .map_err(|error| anyhow!("invalid QA environment configuration: {}", error))
}

fn shared_dependency_environment(
    prior_results: &[CapabilityResult],
) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();

    let Some(result) = find_result(prior_results, "shared_dependencies") else {
        return environment;
    };

    let Some(providers) = result.payload.get("providers").and_then(Value::as_array) else {
        return environment;
    };

    for provider in providers {
        let Some(values) = provider.get("environment").and_then(Value::as_object) else {
            continue;
        };

        for (key, value) in values {
            if let Some(value) = value.as_str() {
                environment.insert(key.clone(), value.to_string());
            }
        }
    }

    environment
}

fn root_scoped_service_command(command: &str, working_directory: &str) -> String {
    let directory = working_directory
        .trim()
        .trim_matches('/')
        .trim_matches('\\')
        .replace('\\', "/");

    if directory.is_empty() || directory == "." {
        return command.to_string();
    }

    let trimmed = command.trim();

    if trimmed == "cargo run" {
        return format!("cargo run --manifest-path {}/Cargo.toml", directory);
    }

    if let Some(remainder) = trimmed.strip_prefix("cargo run ") {
        return format!(
            "cargo run --manifest-path {}/Cargo.toml {}",
            directory,
            remainder
        );
    }

    if trimmed == "npm run" {
        return format!("npm --prefix {} run", directory);
    }

    if let Some(remainder) = trimmed.strip_prefix("npm run ") {
        return format!("npm --prefix {} run {}", directory, remainder);
    }

    if trimmed == "npm" {
        return format!("npm --prefix {}", directory);
    }

    if let Some(remainder) = trimmed.strip_prefix("npm ") {
        return format!("npm --prefix {} {}", directory, remainder);
    }

    command.to_string()
}

fn prepare_service_command(
    configured: &crate::engine::runtime_tools::TerminalCommandSpec,
    workspace: &Path,
    port: u16,
    deployment_id: &str,
    service_id: &str,
    configured_port_variable: &str,
    public_url: Option<&str>,
    primary_service: bool,
    service_ports: &BTreeMap<String, u16>,
    service_urls: &BTreeMap<String, String>,
) -> crate::engine::runtime_tools::TerminalCommandSpec {
    let mut command = configured.clone();
    let port_text = port.to_string();
    let workspace_text = workspace.to_string_lossy().to_string();

    command.command = replace_service_tokens(
        command.command.as_str(),
        port_text.as_str(),
        deployment_id,
        service_id,
        service_ports,
        service_urls,
    );
    command.arguments = command
        .arguments
        .iter()
        .map(|argument| {
            replace_service_tokens(
                argument,
                port_text.as_str(),
                deployment_id,
                service_id,
                service_ports,
                service_urls,
            )
        })
        .collect();
    let configured_working_directory = replace_service_tokens(
        command.working_directory.as_str(),
        port_text.as_str(),
        deployment_id,
        service_id,
        service_ports,
        service_urls,
    );
    command.command = root_scoped_service_command(
        command.command.as_str(),
        configured_working_directory.as_str(),
    );
    command.working_directory = ".".to_string();

    command.environment = command
        .environment
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                replace_service_tokens(
                    value,
                    port_text.as_str(),
                    deployment_id,
                    service_id,
                    service_ports,
                    service_urls,
                ),
            )
        })
        .collect();

    let port_variable = configured_port_variable.trim();
    if !port_variable.is_empty() {
        command
            .environment
            .insert(port_variable.to_string(), port_text.clone());
    }

    command
        .environment
        .entry("PORT".to_string())
        .or_insert_with(|| port_text.clone());
    command
        .environment
        .entry("HOST".to_string())
        .or_insert_with(|| "127.0.0.1".to_string());
    command
        .environment
        .insert("DEPLOYMENT_ID".to_string(), deployment_id.to_string());
    command
        .environment
        .insert("SERVICE_ID".to_string(), service_id.to_string());
    command
        .environment
        .insert("SERVICE_PORT".to_string(), port_text.clone());
    command.environment.insert(
        "SERVICE_PRIMARY".to_string(),
        primary_service.to_string(),
    );

    if let Some(public_url) = public_url {
        command
            .environment
            .insert("SERVICE_PUBLIC_URL".to_string(), public_url.to_string());
        command
            .environment
            .insert("MDEV_QA_PUBLIC_URL".to_string(), public_url.to_string());
    }

    command
        .environment
        .insert("MDEV_QA_SESSION".to_string(), deployment_id.to_string());
    command
        .environment
        .insert("MDEV_QA_SERVICE".to_string(), service_id.to_string());
    command
        .environment
        .insert("MDEV_QA_WORKSPACE".to_string(), workspace_text.clone());
    command
        .environment
        .insert("MDEV_REPO_ROOT".to_string(), workspace_text.clone());
    command
        .environment
        .insert("MDEV_QA_PORT".to_string(), port_text);

    if service_id == "web" {
        if let Some(api_url) = service_urls.get("api") {
            command.environment.insert(
                "VITE_API_BASE_URL".to_string(),
                format!("{}/api", api_url.trim_end_matches('/')),
            );
        }
    }

    command.mode = TerminalCommandMode::Service;
    command
}

fn replace_service_tokens(
    value: &str,
    port: &str,
    deployment_id: &str,
    service_id: &str,
    service_ports: &BTreeMap<String, u16>,
    service_urls: &BTreeMap<String, String>,
) -> String {
    let mut resolved = value
        .replace("{port}", port)
        .replace("{host}", "127.0.0.1")
        .replace("{run}", deployment_id)
        .replace("{deployment}", deployment_id)
        .replace("{service}", service_id);

    for (id, service_port) in service_ports {
        let port_text = service_port.to_string();
        let internal_url = format!("http://127.0.0.1:{}", service_port);

        resolved = resolved
            .replace(format!("{{service.{}.port}}", id).as_str(), port_text.as_str())
            .replace(
                format!("{{service.{}.internal_url}}", id).as_str(),
                internal_url.as_str(),
            )
            .replace(
                format!("{{service.{}.url}}", id).as_str(),
                internal_url.as_str(),
            );
    }

    for (id, service_url) in service_urls {
        resolved = resolved.replace(
            format!("{{service.{}.public_url}}", id).as_str(),
            service_url,
        );
    }

    resolved
}

fn validate_environment(environment: &QaEnvironmentSpec) -> Result<()> {
    if environment.port_range.start == 0 || environment.port_range.end == 0 {
        bail!("QA port range may not contain port zero");
    }

    if environment.port_range.start > environment.port_range.end {
        bail!("QA port range start must not exceed its end");
    }

    if environment.hostname_template.trim().is_empty() {
        bail!("QA hostname template is required");
    }

    for command in &environment.prepare.commands {
        if command.mode != TerminalCommandMode::Run {
            bail!("QA preparation command '{}' must use run mode", command.id);
        }
    }

    let mut service_ids = BTreeSet::new();

    for service in &environment.services {
        if service.id.trim().is_empty() {
            bail!("QA service id is required");
        }

        if !service_ids.insert(service.id.clone()) {
            bail!("duplicate QA service id '{}'", service.id);
        }

        if service.command.mode != TerminalCommandMode::Service {
            bail!("QA service '{}' command must use service mode", service.id);
        }

        if service.command.command.trim().is_empty() {
            bail!("QA service '{}' command is required", service.id);
        }

        if service.port.environment_variable.trim().is_empty() {
            bail!("QA service '{}' port environment variable is required", service.id);
        }
    }

    Ok(())
}

fn allocate_services(
    endpoints: &RuntimeEndpointManager,
    environment: &QaEnvironmentSpec,
) -> Result<Vec<AllocatedQaService>> {
    let mut allocated = Vec::<u16>::new();
    let mut services = Vec::new();

    for service in &environment.services {
        if let Some(port) = service.port.preferred {
            if port < environment.port_range.start || port > environment.port_range.end {
                bail!(
                    "preferred port {} for service '{}' is outside the QA port range",
                    port,
                    service.id
                );
            }
        }

        let endpoint = endpoints.allocate(
            NetworkExposure::Localhost,
            service.port.preferred,
            Some((environment.port_range.start, environment.port_range.end)),
            allocated.as_slice(),
        )?;
        let port = endpoint.port;
        allocated.push(port);

        let public_url = Some(format!("http://localhost:{}", port));

        services.push(AllocatedQaService {
            id: service.id.clone(),
            label: service.label.clone(),
            port,
            environment_variable: service.port.environment_variable.clone(),
            public: false,
            public_url,
            execution: None,
        });
    }

    Ok(services)
}

fn qa_session_id(ctx: &CapabilityContext<'_>) -> String {
    ctx.run_id
        .to_string()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(7)
        .collect::<String>()
        .to_ascii_lowercase()
}
