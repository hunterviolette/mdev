use std::{path::Path, path::PathBuf, process::Command};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult};

pub async fn execute(
    ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    _config: Value,
) -> Result<CapabilityResult> {
    let commands = ctx
        .step
        .execution_logic
        .get("compile_checks")
        .and_then(|v| v.get("commands"))
        .cloned()
        .unwrap_or_else(|| json!([]));

    let result = execute_terminal_command(
        PathBuf::from(ctx.repo_ref).as_path(),
        commands,
    )?;

    Ok(CapabilityResult {
        ok: result.get("ok").and_then(Value::as_bool).unwrap_or(false),
        capability: "compile_commands".to_string(),
        payload: result,
        follow_ups: CapabilityInvocationRequest::None,
    })
}

fn execute_terminal_command(repo: &Path, commands: Value) -> Result<Value> {
    let rows = commands.as_array().cloned().unwrap_or_default();
    let mut results = Vec::new();
    let mut ok = true;

    for item in rows {
        let command = item.get("command").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let label = item.get("label").and_then(Value::as_str).unwrap_or(command.as_str()).to_string();
        if command.is_empty() {
            continue;
        }

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
