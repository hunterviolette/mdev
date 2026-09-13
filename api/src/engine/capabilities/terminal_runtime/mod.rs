use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration as StdDuration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    sync::{Mutex, RwLock},
    time::{sleep, Duration},
};
use uuid::Uuid;

use crate::engine::runtime_tools::{
    TerminalCommandMode,
    TerminalCommandSpec,
    TerminalSequenceSpec,
    TerminalShell,
};

const MAX_CAPTURE_BYTES: usize = 1_000_000;
const COMPLETED_RETENTION: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalExecutionOwner {
    pub run_id: String,
    pub step_id: String,
    pub capability: String,
    pub service_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalCommandResult {
    pub execution_id: String,
    pub command_id: String,
    pub label: String,
    pub command: String,
    pub working_directory: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: i64,
    pub owner: TerminalExecutionOwner,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSequenceResult {
    pub ok: bool,
    pub commands: Vec<TerminalCommandResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub execution_id: String,
    pub pid: Option<u32>,
    pub command_id: String,
    pub label: String,
    pub command: String,
    pub arguments: Vec<String>,
    pub working_directory: String,
    pub environment_keys: Vec<String>,
    pub environment: HashMap<String, String>,
    pub mode: TerminalCommandMode,
    pub status: String,
    pub exit_code: Option<i32>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    pub owner: TerminalExecutionOwner,
}

struct ManagedProcess {
    record: RwLock<ProcessRecord>,
    child: Mutex<Option<Child>>,
}

#[derive(Clone, Default)]
pub struct ProcessRegistry {
    processes: Arc<RwLock<HashMap<String, Arc<ManagedProcess>>>>,
}

#[cfg(target_os = "windows")]
async fn terminate_windows_process_tree(pid: u32) -> Result<()> {
    let output = Command::new("taskkill")
        .args(["/PID", pid.to_string().as_str(), "/T", "/F"])
        .output()
        .await
        .with_context(|| format!("failed to run taskkill for process tree {}", pid))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let message = format!("{} {}", stdout, stderr).to_ascii_lowercase();

    if message.contains("not found")
        || message.contains("no running instance")
        || message.contains("process with pid")
    {
        return Ok(());
    }

    bail!(
        "taskkill failed for process tree {}: {}{}",
        pid,
        stdout.trim(),
        stderr.trim()
    )
}

impl ProcessRegistry {
    pub async fn list(&self) -> Vec<ProcessRecord> {
        let managed = self
            .processes
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut records = Vec::with_capacity(managed.len());
        for process in managed {
            records.push(process.record.read().await.clone());
        }
        records.sort_by(|left, right| right.started_at.cmp(&left.started_at));
        records
    }

    pub async fn get(&self, execution_id: &str) -> Option<ProcessRecord> {
        let process = self.processes.read().await.get(execution_id).cloned()?;
        let record = process.record.read().await.clone();
        Some(record)
    }

    pub async fn terminate(&self, execution_id: &str, force: bool) -> Result<ProcessRecord> {
        let process = self
            .processes
            .read()
            .await
            .get(execution_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown process execution id '{}'", execution_id))?;

        let pid = {
            let mut record = process.record.write().await;
            if !is_active_status(record.status.as_str()) {
                return Ok(record.clone());
            }
            record.status = if force {
                "killing".to_string()
            } else {
                "stopping".to_string()
            };
            record.pid
        };

        #[cfg(target_os = "windows")]
        {
            if let Some(pid) = pid {
                terminate_windows_process_tree(pid).await?;
            }
        }

        let mut child_slot = process.child.lock().await;
        if let Some(child) = child_slot.as_mut() {
            #[cfg(not(target_os = "windows"))]
            child
                .kill()
                .await
                .with_context(|| format!("failed to terminate process {}", execution_id))?;

            let _ = child.wait().await;
        }
        child_slot.take();
        drop(child_slot);

        let mut record = process.record.write().await;
        record.status = if force {
            "killed".to_string()
        } else {
            "stopped".to_string()
        };
        record.finished_at = Some(Utc::now().to_rfc3339());
        Ok(record.clone())
    }

    pub async fn terminate_run(&self, run_id: &str, force: bool) -> Vec<Result<ProcessRecord>> {
        let ids = self
            .list()
            .await
            .into_iter()
            .filter(|record| record.owner.run_id == run_id && is_active_status(record.status.as_str()))
            .map(|record| record.execution_id)
            .collect::<Vec<_>>();

        let mut results = Vec::with_capacity(ids.len());
        for execution_id in ids {
            results.push(self.terminate(execution_id.as_str(), force).await);
        }
        results
    }

    pub async fn terminate_deployment(
        &self,
        run_id: &str,
        step_id: &str,
        force: bool,
    ) -> Vec<Result<ProcessRecord>> {
        let ids = self
            .list()
            .await
            .into_iter()
            .filter(|record| {
                record.owner.run_id == run_id
                    && record.owner.step_id == step_id
                    && is_active_status(record.status.as_str())
            })
            .map(|record| record.execution_id)
            .collect::<Vec<_>>();

        let mut results = Vec::with_capacity(ids.len());
        for execution_id in ids {
            results.push(self.terminate(execution_id.as_str(), force).await);
        }
        results
    }

    pub async fn terminate_all(&self, force: bool) -> Vec<Result<ProcessRecord>> {
        let ids = self
            .list()
            .await
            .into_iter()
            .filter(|record| is_active_status(record.status.as_str()))
            .map(|record| record.execution_id)
            .collect::<Vec<_>>();

        let mut results = Vec::with_capacity(ids.len());
        for execution_id in ids {
            results.push(self.terminate(execution_id.as_str(), force).await);
        }
        results
    }

    pub async fn remove_completed(&self) -> usize {
        let records = self.list().await;
        let removable = records
            .into_iter()
            .filter(|record| !is_active_status(record.status.as_str()))
            .map(|record| record.execution_id)
            .collect::<Vec<_>>();

        let mut processes = self.processes.write().await;
        let mut removed = 0;
        for execution_id in removable {
            if processes.remove(execution_id.as_str()).is_some() {
                removed += 1;
            }
        }
        removed
    }

    async fn insert(&self, process: Arc<ManagedProcess>) {
        let execution_id = process.record.read().await.execution_id.clone();
        let mut processes = self.processes.write().await;
        processes.insert(execution_id, process);

        if processes.len() > COMPLETED_RETENTION {
            let mut completed = Vec::new();
            for (id, process) in processes.iter() {
                let record = process.record.try_read();
                if let Ok(record) = record {
                    if !is_active_status(record.status.as_str()) {
                        completed.push((id.clone(), record.started_at.clone()));
                    }
                }
            }
            completed.sort_by(|left, right| left.1.cmp(&right.1));
            let excess = processes.len().saturating_sub(COMPLETED_RETENTION);
            for (id, _) in completed.into_iter().take(excess) {
                processes.remove(id.as_str());
            }
        }
    }
}

pub async fn run_sequence(
    registry: &ProcessRegistry,
    workspace_root: &Path,
    sequence: &TerminalSequenceSpec,
    owner: &TerminalExecutionOwner,
) -> Result<TerminalSequenceResult> {
    let mut commands = Vec::new();
    let mut ok = true;

    for command in &sequence.commands {
        if command.mode != TerminalCommandMode::Run {
            bail!("terminal sequence command '{}' must use run mode", command.id);
        }

        let result = run_command(registry, workspace_root, command, owner.clone()).await?;
        let succeeded = result.status == "succeeded";
        ok &= succeeded;
        let continue_on_error = command.continue_on_error;
        commands.push(result);

        if !succeeded && sequence.stop_on_failure && !continue_on_error {
            break;
        }
    }

    Ok(TerminalSequenceResult { ok, commands })
}

pub async fn run_command(
    registry: &ProcessRegistry,
    workspace_root: &Path,
    spec: &TerminalCommandSpec,
    owner: TerminalExecutionOwner,
) -> Result<TerminalCommandResult> {
    let execution_id = spawn_registered(registry, workspace_root, spec, owner).await?;
    wait_for_completion(registry, execution_id.as_str(), spec.timeout_seconds).await
}

pub async fn start_service(
    registry: &ProcessRegistry,
    workspace_root: &Path,
    spec: &TerminalCommandSpec,
    owner: TerminalExecutionOwner,
) -> Result<ProcessRecord> {
    if spec.mode != TerminalCommandMode::Service {
        bail!("service command '{}' must use service mode", spec.id);
    }

    let execution_id = spawn_registered(registry, workspace_root, spec, owner).await?;
    registry
        .get(execution_id.as_str())
        .await
        .ok_or_else(|| anyhow!("spawned service was not registered"))
}

async fn spawn_registered(
    registry: &ProcessRegistry,
    workspace_root: &Path,
    spec: &TerminalCommandSpec,
    owner: TerminalExecutionOwner,
) -> Result<String> {
    validate_command(spec)?;
    let workspace_root = workspace_root
        .canonicalize()
        .with_context(|| format!("failed to resolve workspace root {}", workspace_root.display()))?;
    let working_directory = resolve_working_directory(&workspace_root, spec.working_directory.as_str())?;
    let execution_id = Uuid::new_v4().to_string();
    let mut command = build_command(spec);

    command
        .current_dir(&working_directory)
        .envs(spec.environment.iter())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);

    let mut child = command.spawn().context("terminal command failed to start")?;
    let pid = child.id();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let process = Arc::new(ManagedProcess {
        record: RwLock::new(ProcessRecord {
            execution_id: execution_id.clone(),
            pid,
            command_id: spec.id.clone(),
            label: spec.label.clone(),
            command: display_command(spec),
            arguments: spec.arguments.clone(),
            working_directory: working_directory.to_string_lossy().to_string(),
            environment_keys: spec.environment.keys().cloned().collect(),
            environment: spec.environment.iter().map(|(key, value)| (key.clone(), value.clone())).collect(),
            mode: spec.mode.clone(),
            status: "running".to_string(),
            exit_code: None,
            started_at: Utc::now().to_rfc3339(),
            finished_at: None,
            duration_ms: None,
            stdout: String::new(),
            stderr: String::new(),
            owner,
        }),
        child: Mutex::new(Some(child)),
    });

    registry.insert(process.clone()).await;

    if let Some(stdout) = stdout {
        let process = process.clone();
        tokio::spawn(async move {
            capture_stream(stdout, process, ProcessOutputStream::Stdout).await;
        });
    }

    if let Some(stderr) = stderr {
        let process = process.clone();
        tokio::spawn(async move {
            capture_stream(stderr, process, ProcessOutputStream::Stderr).await;
        });
    }

    let watcher = process.clone();
    tokio::spawn(async move {
        let started = Instant::now();
        loop {
            let status = {
                let mut child_slot = watcher.child.lock().await;
                match child_slot.as_mut() {
                    Some(child) => child.try_wait(),
                    None => return,
                }
            };

            match status {
                Ok(Some(exit)) => {
                    let mut record = watcher.record.write().await;
                    if is_active_status(record.status.as_str()) {
                        record.status = if exit.success() {
                            "succeeded".to_string()
                        } else {
                            "failed".to_string()
                        };
                    }
                    record.exit_code = exit.code();
                    record.finished_at = Some(Utc::now().to_rfc3339());
                    record.duration_ms = Some(
                        i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
                    );
                    watcher.child.lock().await.take();
                    return;
                }
                Ok(None) => sleep(Duration::from_millis(100)).await,
                Err(error) => {
                    let mut record = watcher.record.write().await;
                    record.status = "failed".to_string();
                    record.stderr = append_bounded(
                        record.stderr.as_str(),
                        format!("process wait error: {}", error).as_str(),
                    );
                    record.finished_at = Some(Utc::now().to_rfc3339());
                    record.duration_ms = Some(
                        i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
                    );
                    return;
                }
            }
        }
    });

    Ok(execution_id)
}

async fn wait_for_completion(
    registry: &ProcessRegistry,
    execution_id: &str,
    timeout_seconds: Option<u64>,
) -> Result<TerminalCommandResult> {
    let started = Instant::now();
    let timeout = timeout_seconds
        .filter(|value| *value > 0)
        .map(StdDuration::from_secs);

    loop {
        let record = registry
            .get(execution_id)
            .await
            .ok_or_else(|| anyhow!("process '{}' disappeared from registry", execution_id))?;

        if !is_active_status(record.status.as_str()) {
            return Ok(result_from_record(record));
        }

        if timeout
            .map(|limit| started.elapsed() >= limit)
            .unwrap_or(false)
        {
            let record = registry.terminate(execution_id, true).await?;
            let mut result = result_from_record(record);
            result.status = "timed_out".to_string();
            result.stderr = append_bounded(
                result.stderr.as_str(),
                "Terminal command timed out.",
            );
            return Ok(result);
        }

        sleep(Duration::from_millis(100)).await;
    }
}

fn result_from_record(record: ProcessRecord) -> TerminalCommandResult {
    TerminalCommandResult {
        execution_id: record.execution_id,
        command_id: record.command_id,
        label: record.label,
        command: record.command,
        working_directory: record.working_directory,
        status: record.status,
        exit_code: record.exit_code,
        stdout: record.stdout,
        stderr: record.stderr,
        duration_ms: record.duration_ms.unwrap_or_default(),
        owner: record.owner,
    }
}

#[derive(Clone, Copy)]
enum ProcessOutputStream {
    Stdout,
    Stderr,
}

async fn capture_stream<R>(
    mut reader: R,
    process: Arc<ManagedProcess>,
    stream: ProcessOutputStream,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 8192];

    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => {
                let chunk = String::from_utf8_lossy(&buffer[..read]);
                let mut record = process.record.write().await;
                match stream {
                    ProcessOutputStream::Stdout => {
                        record.stdout = append_chunk_bounded(record.stdout.as_str(), chunk.as_ref());
                    }
                    ProcessOutputStream::Stderr => {
                        record.stderr = append_chunk_bounded(record.stderr.as_str(), chunk.as_ref());
                    }
                }
            }
            Err(error) => {
                let mut record = process.record.write().await;
                record.stderr = append_bounded(
                    record.stderr.as_str(),
                    format!("stream read error: {}", error).as_str(),
                );
                break;
            }
        }
    }
}

fn append_chunk_bounded(existing: &str, addition: &str) -> String {
    let mut combined = String::with_capacity(existing.len() + addition.len());
    combined.push_str(existing);
    combined.push_str(addition);

    if combined.len() <= MAX_CAPTURE_BYTES {
        return combined;
    }

    let mut start = combined.len() - MAX_CAPTURE_BYTES;
    while !combined.is_char_boundary(start) {
        start += 1;
    }
    combined[start..].to_string()
}

fn append_bounded(existing: &str, addition: &str) -> String {
    let combined = if existing.trim().is_empty() {
        addition.to_string()
    } else {
        format!("{}\n{}", existing, addition)
    };
    if combined.len() <= MAX_CAPTURE_BYTES {
        combined
    } else {
        let mut start = combined.len() - MAX_CAPTURE_BYTES;
        while !combined.is_char_boundary(start) {
            start += 1;
        }
        combined[start..].to_string()
    }
}

fn is_active_status(status: &str) -> bool {
    matches!(status, "starting" | "running" | "stopping" | "killing")
}

fn validate_command(spec: &TerminalCommandSpec) -> Result<()> {
    if spec.id.trim().is_empty() {
        bail!("terminal command id is required");
    }
    if spec.command.trim().is_empty() {
        bail!("terminal command '{}' is empty", spec.id);
    }
    Ok(())
}

fn resolve_working_directory(workspace_root: &Path, configured: &str) -> Result<PathBuf> {
    let requested = PathBuf::from(configured.trim());
    if requested.is_absolute() {
        bail!("terminal working directory must be relative to the workspace root");
    }

    let resolved = workspace_root
        .join(if configured.trim().is_empty() {
            "."
        } else {
            configured.trim()
        })
        .canonicalize()
        .context("failed to resolve terminal working directory")?;

    if !resolved.starts_with(workspace_root) {
        bail!("terminal working directory escapes the workspace root");
    }

    Ok(resolved)
}

fn build_command(spec: &TerminalCommandSpec) -> Command {
    match spec.shell {
        TerminalShell::Direct => {
            let mut command = Command::new(spec.command.trim());
            command.args(&spec.arguments);
            command
        }
        TerminalShell::Cmd => shell_command("cmd", &["/C"], spec),
        TerminalShell::PowerShell => shell_command(
            "powershell",
            &["-NoProfile", "-NonInteractive", "-Command"],
            spec,
        ),
        TerminalShell::Sh => shell_command("sh", &["-lc"], spec),
        TerminalShell::Bash => shell_command("bash", &["-lc"], spec),
        TerminalShell::System => system_shell_command(spec),
    }
}

fn shell_command(program: &str, prefix: &[&str], spec: &TerminalCommandSpec) -> Command {
    let mut command = Command::new(program);
    command.args(prefix);
    command.arg(display_command(spec));
    command
}

#[cfg(target_os = "windows")]
fn system_shell_command(spec: &TerminalCommandSpec) -> Command {
    shell_command("cmd", &["/C"], spec)
}

#[cfg(not(target_os = "windows"))]
fn system_shell_command(spec: &TerminalCommandSpec) -> Command {
    shell_command("sh", &["-lc"], spec)
}

fn display_command(spec: &TerminalCommandSpec) -> String {
    if spec.arguments.is_empty() {
        spec.command.trim().to_string()
    } else {
        format!("{} {}", spec.command.trim(), spec.arguments.join(" "))
    }
}
