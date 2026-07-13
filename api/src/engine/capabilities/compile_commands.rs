use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::engine::runtime_tools::{
    TerminalCommandMode,
    TerminalCommandSpec,
    TerminalSequenceSpec,
    TerminalShell,
};

use super::{
    registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult},
    terminal_runtime::{self, TerminalExecutionOwner},
};

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let mut sequence = resolve_compile_sequence(ctx, config)?;
    let shared_environment = shared_dependency_environment(prior_results);

    for command in &mut sequence.commands {
        for (key, value) in &shared_environment {
            command
                .environment
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }
    let execution_mode = ctx
        .local_state
        .get("execution")
        .and_then(|value| value.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or("manual");
    let repo_ref = ctx
        .local_state
        .get("resources")
        .and_then(|value| value.get("repo"))
        .and_then(|value| value.get("repo_ref"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(ctx.repo_ref);

    let result = execute_compile_commands(
        &ctx.state.process_registry,
        PathBuf::from(repo_ref),
        sequence,
        execution_mode,
        TerminalExecutionOwner {
            run_id: ctx.run_id.to_string(),
            step_id: ctx.step.id.clone(),
            capability: "compile_commands".to_string(),
            service_id: None,
        },
    )
    .await?;

    Ok(CapabilityResult {
        ok: result.get("ok").and_then(Value::as_bool).unwrap_or(false),
        capability: "compile_commands".to_string(),
        payload: result,
        follow_ups: CapabilityInvocationRequest::None,
    })
}

fn shared_dependency_environment(
    prior_results: &[CapabilityResult],
) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();

    let Some(result) = prior_results
        .iter()
        .rev()
        .find(|result| result.capability == "shared_dependencies")
    else {
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

fn resolve_compile_sequence(
    ctx: &CapabilityContext<'_>,
    config: Value,
) -> Result<TerminalSequenceSpec> {
    if let Some(compile) = ctx.step.execution.compile.as_ref() {
        if !compile.commands.commands.is_empty() {
            return Ok(compile.commands.clone());
        }
    }

    let commands = resolve_legacy_compile_commands(
        config,
        ctx.local_state,
        ctx.step.execution.compile_checks.clone(),
        ctx.step.execution_logic.clone(),
    );

    terminal_sequence_from_value(commands)
}

fn resolve_legacy_compile_commands(
    config: Value,
    local_state: &Value,
    step_compile_checks: Value,
    execution_logic: Value,
) -> Value {
    non_empty_commands(config.get("commands").cloned())
        .or_else(|| commands_text_to_rows(config.get("commands_text")))
        .or_else(|| {
            non_empty_commands(
                local_state
                    .get("capabilities")
                    .and_then(|value| value.get("compile_commands"))
                    .and_then(|value| value.get("commands"))
                    .cloned(),
            )
        })
        .or_else(|| {
            commands_text_to_rows(
                local_state
                    .get("capabilities")
                    .and_then(|value| value.get("compile_commands"))
                    .and_then(|value| value.get("commands_text")),
            )
        })
        .or_else(|| {
            non_empty_commands(
                local_state
                    .get("execution")
                    .and_then(|value| value.get("compile_checks"))
                    .and_then(|value| value.get("commands"))
                    .cloned(),
            )
        })
        .or_else(|| {
            commands_text_to_rows(
                local_state
                    .get("execution")
                    .and_then(|value| value.get("compile_checks"))
                    .and_then(|value| value.get("commands_text")),
            )
        })
        .or_else(|| non_empty_commands(step_compile_checks.get("commands").cloned()))
        .or_else(|| commands_text_to_rows(step_compile_checks.get("commands_text")))
        .or_else(|| {
            non_empty_commands(
                local_state
                    .get("execution_logic")
                    .and_then(|value| value.get("compile_checks"))
                    .and_then(|value| value.get("commands"))
                    .cloned(),
            )
        })
        .or_else(|| {
            commands_text_to_rows(
                local_state
                    .get("execution_logic")
                    .and_then(|value| value.get("compile_checks"))
                    .and_then(|value| value.get("commands_text")),
            )
        })
        .or_else(|| {
            non_empty_commands(
                execution_logic
                    .get("compile_checks")
                    .and_then(|value| value.get("commands"))
                    .cloned(),
            )
        })
        .or_else(|| {
            commands_text_to_rows(
                execution_logic
                    .get("compile_checks")
                    .and_then(|value| value.get("commands_text")),
            )
        })
        .unwrap_or_else(|| json!([]))
}

fn terminal_sequence_from_value(commands: Value) -> Result<TerminalSequenceSpec> {
    let rows = match commands {
        Value::Array(rows) => rows,
        Value::String(command) if !command.trim().is_empty() => {
            vec![Value::String(command)]
        }
        Value::Null => Vec::new(),
        other => bail!("compile commands must be an array or string, received {}", other),
    };

    let commands = rows
        .into_iter()
        .enumerate()
        .map(|(index, value)| terminal_command_from_value(index, value))
        .collect::<Result<Vec<_>>>()?;

    Ok(TerminalSequenceSpec {
        commands,
        stop_on_failure: true,
    })
}

fn terminal_command_from_value(
    index: usize,
    value: Value,
) -> Result<TerminalCommandSpec> {
    match value {
        Value::String(command) => {
            let command = command.trim();
            if command.is_empty() {
                bail!("compile command {} is empty", index + 1);
            }

            Ok(TerminalCommandSpec {
                id: format!("compile-command-{}", index + 1),
                label: command.to_string(),
                command: command.to_string(),
                arguments: Vec::new(),
                working_directory: ".".to_string(),
                environment: BTreeMap::new(),
                shell: TerminalShell::System,
                mode: TerminalCommandMode::Run,
                timeout_seconds: None,
                continue_on_error: false,
            })
        }
        Value::Object(mut object) => {
            object
                .entry("id".to_string())
                .or_insert_with(|| Value::String(format!("compile-command-{}", index + 1)));
            let default_label = object
                .get("command")
                .cloned()
                .unwrap_or_else(|| Value::String(format!("Compile command {}", index + 1)));
            object
                .entry("label".to_string())
                .or_insert(default_label);
            object
                .entry("working_directory".to_string())
                .or_insert_with(|| Value::String(".".to_string()));
            object
                .entry("environment".to_string())
                .or_insert_with(|| json!({}));
            object
                .entry("shell".to_string())
                .or_insert_with(|| Value::String("system".to_string()));
            object
                .entry("mode".to_string())
                .or_insert_with(|| Value::String("run".to_string()));
            object
                .entry("continue_on_error".to_string())
                .or_insert_with(|| Value::Bool(false));

            let spec = serde_json::from_value::<TerminalCommandSpec>(Value::Object(object))
                .with_context(|| format!("invalid compile command {}", index + 1))?;

            if spec.mode != TerminalCommandMode::Run {
                bail!("compile command '{}' must use run mode", spec.id);
            }

            Ok(spec)
        }
        _ => bail!("compile command {} must be a string or object", index + 1),
    }
}

fn non_empty_commands(commands: Option<Value>) -> Option<Value> {
    match commands {
        Some(Value::Array(rows)) if !rows.is_empty() => Some(Value::Array(rows)),
        Some(Value::String(command)) if !command.trim().is_empty() => {
            Some(json!([command.trim()]))
        }
        _ => None,
    }
}

fn commands_text_to_rows(value: Option<&Value>) -> Option<Value> {
    let text = value.and_then(Value::as_str)?.trim();
    if text.is_empty() {
        return None;
    }

    let rows = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|command| Value::String(command.to_string()))
        .collect::<Vec<_>>();

    if rows.is_empty() {
        None
    } else {
        Some(Value::Array(rows))
    }
}

async fn execute_compile_commands(
    registry: &terminal_runtime::ProcessRegistry,
    repo: PathBuf,
    sequence: TerminalSequenceSpec,
    execution_mode: &str,
    owner: TerminalExecutionOwner,
) -> Result<Value> {
    if sequence.commands.is_empty() {
        let automatic = execution_mode.eq_ignore_ascii_case("automatic");
        return Ok(json!({
            "ok": automatic,
            "results": [],
            "message": if automatic {
                "No compile commands configured. Autonomous run should pause at compile."
            } else {
                "No compile commands configured."
            },
            "no_commands_configured": true,
            "skipped": automatic
        }));
    }

    let sequence_result = terminal_runtime::run_sequence(
        registry,
        repo.as_path(),
        &sequence,
        &owner,
    )
    .await?;

    let results = sequence_result
        .commands
        .into_iter()
        .map(|result| {
            json!({
                "execution_id": result.execution_id,
                "command_id": result.command_id,
                "label": result.label,
                "command": result.command,
                "working_directory": result.working_directory,
                "status": result.exit_code,
                "execution_status": result.status,
                "stdout": result.stdout,
                "stderr": result.stderr,
                "duration_ms": result.duration_ms,
                "timed_out": result.status == "timed_out"
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "ok": sequence_result.ok,
        "results": results
    }))
}
