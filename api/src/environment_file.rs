use std::{env, fs, net::SocketAddr, path::{Path, PathBuf}};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnvironmentFileConfig {
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct EnvironmentFileStatus {
    pub path: String,
    pub effective_path: String,
    pub exists: bool,
    pub readable: bool,
    pub configured: bool,
    pub restart_required: bool,
}

fn config_path(app_root: &Path) -> PathBuf {
    crate::mdev_data_dir(app_root).join("environment-file.json")
}

pub fn read_config(app_root: &Path) -> Result<EnvironmentFileConfig> {
    let path = config_path(app_root);
    if !path.exists() {
        return Ok(EnvironmentFileConfig::default());
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&contents)
        .with_context(|| format!("invalid environment configuration in {}", path.display()))
}

pub fn effective_path(app_root: &Path, config: &EnvironmentFileConfig) -> PathBuf {
    let local_path = app_root.join(".env");

    if local_path.is_file() {
        return local_path;
    }

    if !config.path.trim().is_empty() {
        return PathBuf::from(config.path.trim());
    }

    local_path
}

pub fn status(app_root: &Path) -> Result<EnvironmentFileStatus> {
    let config = read_config(app_root)?;
    let path = effective_path(app_root, &config);
    let readable = fs::File::open(&path).is_ok();
    Ok(EnvironmentFileStatus {
        configured: !config.path.trim().is_empty(),
        path: config.path,
        effective_path: path.to_string_lossy().into_owned(),
        exists: path.is_file(),
        readable,
        restart_required: false,
    })
}

pub fn save_config(app_root: &Path, requested_path: &str) -> Result<EnvironmentFileStatus> {
    let requested_path = requested_path.trim();
    let resolved = if requested_path.is_empty() {
        String::new()
    } else {
        let path = PathBuf::from(requested_path);
        if !path.is_absolute() {
            bail!("environment file path must be absolute");
        }
        if !path.is_file() {
            bail!("environment file does not exist or is not a regular file");
        }
        fs::File::open(&path)
            .with_context(|| format!("environment file is not readable: {}", path.display()))?;
        dotenvy::from_path_iter(&path)
            .with_context(|| format!("invalid environment file: {}", path.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("invalid environment file: {}", path.display()))?;
        path.canonicalize()
            .with_context(|| format!("failed to resolve {}", path.display()))?
            .to_string_lossy()
            .into_owned()
    };

    let destination = config_path(app_root);
    let parent = destination.parent().context("configuration path has no parent")?;
    fs::create_dir_all(parent)?;
    let contents = serde_json::to_vec_pretty(&EnvironmentFileConfig { path: resolved.clone() })?;
    let temporary = destination.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temporary, contents)?;
    if let Err(error) = fs::rename(&temporary, &destination) {
        if destination.exists() {
            fs::remove_file(&destination)?;
            fs::rename(&temporary, &destination)?;
        } else {
            return Err(error.into());
        }
    }

    let mut result = status(app_root)?;
    result.restart_required = true;
    Ok(result)
}

pub fn load(app_root: &Path) -> Result<()> {
    let config = read_config(app_root)?;
    let path = effective_path(app_root, &config);
    if config.path.trim().is_empty() && !path.exists() {
        return Ok(());
    }
    if !path.is_file() {
        eprintln!("mdev environment file is unavailable: {}", path.display());
        return Ok(());
    }
    dotenvy::from_path(&path)
        .with_context(|| format!("failed to load {}", path.display()))?;
    Ok(())
}

pub fn required_env(name: &str) -> Result<String> {
    env::var(name)
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

pub fn app_root() -> Result<PathBuf> {
    let cwd = env::current_dir()?;
    crate::resolve_app_root(&cwd)
}
