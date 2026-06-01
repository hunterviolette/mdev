use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::BTreeSet;
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::time::Duration;

use super::super::super::registry::CapabilityContext;

const DEFAULT_START: u16 = 9222;

pub async fn allocate_cdp_url_for_session(
    ctx: &CapabilityContext<'_>,
    session_name: &str,
    current: &str,
) -> Result<String> {
    let run = crate::engine::load_run(ctx.state, ctx.run_id).await?;
    let global_state = run
        .context
        .get("workflow_engine")
        .and_then(|v| v.get("global_state"))
        .or_else(|| run.context.get("engine").and_then(|v| v.get("global_state")))
        .or_else(|| run.context.get("global_state"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    let inference = global_state
        .get("capabilities")
        .and_then(|v| v.get("inference"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    let host = cdp_host(current).unwrap_or_else(|| "127.0.0.1".to_string());
    let process_session_id = ctx.state.process_session_id().to_string();

    if let Some(existing) = current_session_runtime_cdp_url(&inference, session_name, process_session_id.as_str()) {
        if cdp_reachable(existing.as_str()) {
            return Ok(existing);
        }
    }

    if let Some(shared) = shared_profile_reachable_cdp_url(&inference, session_name) {
        return Ok(shared);
    }

    let reserved_ports = reserved_ports_for_other_sessions(&inference, session_name, process_session_id.as_str());
    let (start, end) = configured_range();

    for port in start..=end {
        if reserved_ports.contains(&port) {
            continue;
        }
        if port_is_available(&host, port) {
            return Ok(format!("http://{}:{}", host, port));
        }
    }

    Err(anyhow!("no available browser inference debug port in range {}-{}", start, end))
}

pub fn allocate_cdp_url(current: &str) -> Result<String> {
    let host = cdp_host(current).unwrap_or_else(|| "127.0.0.1".to_string());
    let (start, end) = configured_range();

    for port in start..=end {
        if port_is_available(&host, port) {
            return Ok(format!("http://{}:{}", host, port));
        }
    }

    Err(anyhow!("no available browser inference debug port in range {}-{}", start, end))
}

fn current_session_runtime_cdp_url(inference: &Value, session_name: &str, process_session_id: &str) -> Option<String> {
    let session = inference
        .get("sessions")
        .and_then(Value::as_object)
        .and_then(|sessions| sessions.get(session_name))?;
    let runtime = session.get("runtime").and_then(Value::as_object)?;
    let runtime_process_session_id = runtime
        .get("process_session_id")
        .and_then(Value::as_str)
        .unwrap_or("");

    if !runtime_process_session_id.is_empty() && runtime_process_session_id != process_session_id {
        return None;
    }

    runtime
        .get("cdp_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn shared_profile_reachable_cdp_url(inference: &Value, session_name: &str) -> Option<String> {
    let sessions = inference.get("sessions").and_then(Value::as_object)?;
    let current_session = sessions.get(session_name)?;
    let current_profile_key = browser_credential_profile_key(current_session);

    for (name, session) in sessions {
        if name == session_name {
            continue;
        }

        if browser_credential_profile_key(session) != current_profile_key {
            continue;
        }

        let runtime = session.get("runtime").and_then(Value::as_object)?;
        let cdp_url = runtime
            .get("cdp_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?;

        if cdp_reachable(cdp_url) {
            return Some(cdp_url.to_string());
        }
    }

    None
}

fn browser_credential_profile_key(session: &Value) -> String {
    let browser = session.get("browser").and_then(Value::as_object);
    let user_data_dir = browser
        .and_then(|browser| browser.get("user_data_dir"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .replace('\\', "/")
        .to_lowercase();

    if user_data_dir.is_empty() {
        "default-user-data-dir".to_string()
    } else {
        user_data_dir
    }
}

fn reserved_ports_for_other_sessions(inference: &Value, session_name: &str, process_session_id: &str) -> BTreeSet<u16> {
    let mut reserved = BTreeSet::new();
    let Some(sessions) = inference.get("sessions").and_then(Value::as_object) else {
        return reserved;
    };

    for (name, session) in sessions {
        if name == session_name {
            continue;
        }

        let runtime = session.get("runtime").and_then(Value::as_object);
        let same_process = runtime
            .and_then(|runtime| runtime.get("process_session_id"))
            .and_then(Value::as_str)
            .map(|value| value == process_session_id)
            .unwrap_or(false);

        if !same_process {
            continue;
        }

        if let Some(port) = runtime
            .and_then(|runtime| runtime.get("debug_port"))
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<u16>().ok())
        {
            reserved.insert(port);
        }

        if let Some(port) = runtime
            .and_then(|runtime| runtime.get("cdp_url"))
            .and_then(Value::as_str)
            .and_then(cdp_port)
        {
            reserved.insert(port);
        }
    }

    reserved
}

fn configured_range() -> (u16, u16) {
    let start = std::env::var("WORKFLOW_BROWSER_CDP_PORT_RANGE_START")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_START);
    let size = std::env::var("WORKFLOW_BROWSER_CDP_PORT_RANGE_SIZE")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(100)
        .clamp(1, 1000);
    let end = start.saturating_add(size - 1).min(u16::MAX);
    (start, end)
}

fn cdp_reachable(cdp_url: &str) -> bool {
    let raw = cdp_url.trim();
    if raw.is_empty() {
        return false;
    }

    let no_scheme = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))
        .unwrap_or(raw);
    let host_port = no_scheme.split('/').next().unwrap_or(no_scheme);

    match host_port.to_socket_addrs() {
        Ok(mut addrs) => addrs.any(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()),
        Err(_) => false,
    }
}

fn cdp_host(current: &str) -> Option<String> {
    current
        .trim()
        .strip_prefix("http://")
        .or_else(|| current.trim().strip_prefix("https://"))
        .unwrap_or(current.trim())
        .split('/')
        .next()
        .and_then(|host_port| host_port.rsplit_once(':').map(|(host, _)| host.to_string()))
        .filter(|host| !host.trim().is_empty())
}

fn cdp_port(current: &str) -> Option<u16> {
    current
        .trim()
        .strip_prefix("http://")
        .or_else(|| current.trim().strip_prefix("https://"))
        .unwrap_or(current.trim())
        .split('/')
        .next()
        .and_then(|host_port| host_port.rsplit_once(':').map(|(_, port)| port))
        .and_then(|port| port.parse::<u16>().ok())
}

fn port_is_available(host: &str, port: u16) -> bool {
    TcpListener::bind((host, port)).is_ok()
}
