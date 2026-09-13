use std::net::SocketAddr;

use anyhow::{Context, Result};

pub fn required_env(name: &str) -> Result<String> {
    std::env::var(name)
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| default_env(name).map(str::to_string))
        .with_context(|| format!("{name} is not set"))
}

fn default_env(name: &str) -> Option<&'static str> {
    match name {
        "WORKFLOW_API_HOST" => Some("127.0.0.1"),
        "WORKFLOW_API_PORT" => Some("8788"),
        "WORKFLOW_WEB_HOST" => Some("127.0.0.1"),
        "WORKFLOW_WEB_PORT" => Some("5173"),
        "WORKFLOW_BROWSER_CDP_HOST" => Some("127.0.0.1"),
        "WORKFLOW_BROWSER_CDP_PORT" => Some("9222"),
        "WORKFLOW_BROWSER_BRIDGE_HOST" => Some("127.0.0.1"),
        "WORKFLOW_BROWSER_BRIDGE_PORT" => Some("8789"),
        _ => None,
    }
}

pub fn env_host_port(host_key: &str, port_key: &str) -> Result<String> {
    Ok(format!("{}:{}", required_env(host_key)?, required_env(port_key)?))
}

pub fn env_http_url(host_key: &str, port_key: &str) -> Result<String> {
    Ok(format!("http://{}", env_host_port(host_key, port_key)?))
}

pub fn workflow_api_bind_addr() -> Result<SocketAddr> {
    env_host_port("WORKFLOW_API_HOST", "WORKFLOW_API_PORT")?
        .parse()
        .context("invalid WORKFLOW_API_HOST/WORKFLOW_API_PORT")
}

pub fn default_browser_cdp_url() -> Result<String> {
    env_http_url("WORKFLOW_BROWSER_CDP_HOST", "WORKFLOW_BROWSER_CDP_PORT")
}

pub fn default_browser_bridge_url() -> Result<String> {
    env_http_url("WORKFLOW_BROWSER_BRIDGE_HOST", "WORKFLOW_BROWSER_BRIDGE_PORT")
}
