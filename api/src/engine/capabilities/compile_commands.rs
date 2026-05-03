use std::{path::Path, path::PathBuf, process::Command};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult};

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    config: Value,
) -> Result<CapabilityResult> {
    let commands = resolve_compile_commands(
        config,
        ctx.local_state,
        ctx.step.execution.compile_checks.clone(),
        ctx.step.execution_logic.clone(),
    );
    let execution_mode = ctx
        .local_state
        .get("execution")
        .and_then(|v| v.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or("manual");

    let repo_ref = ctx
        .local_state
        .get("resources")
        .and_then(|v| v.get("repo"))
        .and_then(|v| v.get("repo_ref"))
        .and_then(Value::as_str)
        .unwrap_or(ctx.repo_ref);

    let result = execute_terminal_command(
        PathBuf::from(repo_ref).as_path(),
        commands,
        execution_mode,
    )?;

    Ok(CapabilityResult {
        ok: result.get("ok").and_then(Value::as_bool).unwrap_or(false),
        capability: "compile_commands".to_string(),
        payload: result,
        follow_ups: CapabilityInvocationRequest::None,
    })
}

fn resolve_compile_commands(config: Value, local_state: &Value, step_compile_checks: Value, execution_logic: Value) -> Value {
    non_empty_commands(config.get("commands").cloned())
        .or_else(|| commands_text_to_rows(config.get("commands_text")))
        .or_else(|| {
            non_empty_commands(
                local_state
                    .get("capabilities")
                    .and_then(|v| v.get("compile_commands"))
                    .and_then(|v| v.get("commands"))
                    .cloned(),
            )
        })
        .or_else(|| {
            non_empty_commands(
                local_state
                    .get("execution")
                    .and_then(|v| v.get("compile_checks"))
                    .and_then(|v| v.get("commands"))
                    .cloned(),
            )
        })
        .or_else(|| {
            commands_text_to_rows(
                local_state
                    .get("execution")
                    .and_then(|v| v.get("compile_checks"))
                    .and_then(|v| v.get("commands_text")),
            )
        })
        .or_else(|| non_empty_commands(step_compile_checks.get("commands").cloned()))
        .or_else(|| commands_text_to_rows(step_compile_checks.get("commands_text")))
        .or_else(|| {
            non_empty_commands(
                local_state
                    .get("execution_logic")
                    .and_then(|v| v.get("compile_checks"))
                    .and_then(|v| v.get("commands"))
                    .cloned(),
            )
        })
        .or_else(|| {
            commands_text_to_rows(
                local_state
                    .get("execution_logic")
                    .and_then(|v| v.get("compile_checks"))
                    .and_then(|v| v.get("commands_text")),
            )
        })
        .or_else(|| non_empty_commands(execution_logic.get("compile_checks").and_then(|v| v.get("commands")).cloned()))
        .or_else(|| commands_text_to_rows(execution_logic.get("compile_checks").and_then(|v| v.get("commands_text"))))
        .unwrap_or_else(|| json!([]))
}

fn non_empty_commands(commands: Option<Value>) -> Option<Value> {
    match commands {
        Some(Value::Array(rows)) if !rows.is_empty() => Some(Value::Array(rows)),
        Some(Value::String(command)) if !command.trim().is_empty() => Some(json!([command.trim()])),
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

fn execute_terminal_command(repo: &Path, commands: Value, execution_mode: &str) -> Result<Value> {
    let rows = commands.as_array().cloned().unwrap_or_default();
    let mut results = Vec::new();
    let mut ok = true;
    let mut executed_any = false;

    for item in rows {
        let (command, label) = match item {
            Value::String(command) => {
                let trimmed = command.trim().to_string();
                (trimmed.clone(), trimmed)
            }
            Value::Object(obj) => {
                let command = obj
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let label = obj
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or(command.as_str())
                    .trim()
                    .to_string();
                (command, label)
            }
            _ => (String::new(), String::new()),
        };

        if command.is_empty() {
            continue;
        }

        executed_any = true;
        let output = shell_command(repo, &command)
            .with_context(|| format!("failed to run compile command '{}'", command))?;

        let status = output.status.code().unwrap_or(-1);
        if status != 0 {
            ok = false;
        }

        results.push(json!({
            "label": label,
            "command": command,
            "status": status,
            "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
            "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
        }));
    }

    if !executed_any {
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
            "skipped": automatic,
        }));
    }

    Ok(json!({
        "ok": ok,
        "results": results,
    }))
}

fn shell_command(repo: &Path, command: &str) -> Result<std::process::Output> {
    #[cfg(target_os = "windows")]
    {
        Ok(Command::new("cmd")
            .args(["/C", command])
            .current_dir(repo)
            .output()?)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(Command::new("sh")
            .args(["-lc", command])
            .current_dir(repo)
            .output()?)
    }
}
