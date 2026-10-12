use std::{collections::HashMap, env, fs, path::{Path, PathBuf}};

use serde::Deserialize;

#[derive(Default, Deserialize)]
struct EnvironmentFileConfig {
    #[serde(default)]
    path: String,
}

pub struct ApiEnvironment {
    pub host: String,
    pub port: u16,
}

impl ApiEnvironment {
    pub fn url(&self) -> String {
        let host = match self.host.as_str() {
            "0.0.0.0" => "127.0.0.1",
            "::" => "[::1]",
            "127.0.0.1" => "127.0.0.1",
            "localhost" => "localhost",
            value => value,
        };
        let host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        format!("http://{host}:{}", self.port)
    }
}

fn data_dir(root: &Path) -> PathBuf {
    if let Ok(value) = env::var("MDEV_DATA_DIR") {
        if !value.trim().is_empty() {
            let path = PathBuf::from(value.trim());
            return if path.is_absolute() { path } else { root.join(path) };
        }
    }

    #[cfg(windows)]
    if let Some(value) = env::var_os("LOCALAPPDATA") {
        return PathBuf::from(value).join("mdev");
    }

    #[cfg(target_os = "macos")]
    if let Some(value) = env::var_os("HOME") {
        return PathBuf::from(value).join("Library").join("Application Support").join("mdev");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(value) = env::var_os("XDG_DATA_HOME") {
            if !value.is_empty() {
                return PathBuf::from(value).join("mdev");
            }
        }
        if let Some(value) = env::var_os("HOME") {
            return PathBuf::from(value).join(".local").join("share").join("mdev");
        }
    }

    root.join(".data")
}

fn effective_path(root: &Path) -> Result<PathBuf, String> {
    let local = root.join(".env");
    if local.is_file() {
        return Ok(local);
    }

    let config_file = data_dir(root).join("environment-file.json");
    if !config_file.exists() {
        return Ok(local);
    }

    let contents = fs::read_to_string(&config_file).map_err(|e| e.to_string())?;
    let config: EnvironmentFileConfig = serde_json::from_str(&contents)
        .map_err(|e| format!("Invalid {}: {e}", config_file.display()))?;

    if config.path.trim().is_empty() {
        Ok(local)
    } else {
        Ok(PathBuf::from(config.path.trim()))
    }
}

fn read_values(root: &Path) -> Result<HashMap<String, String>, String> {
    let path = effective_path(root)?;
    if !path.is_file() {
        return Ok(HashMap::new());
    }

    dotenvy::from_path_iter(&path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?
        .map(|entry| entry.map_err(|e| e.to_string()))
        .collect::<Result<HashMap<_, _>, _>>()
}

fn value(values: &HashMap<String, String>, name: &str, default: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| values.get(name).cloned().filter(|value| !value.trim().is_empty()))
        .unwrap_or_else(|| default.to_string())
        .trim()
        .to_string()
}

pub fn api_environment(root: &Path) -> Result<ApiEnvironment, String> {
    let values = read_values(root)?;
    let host = value(&values, "WORKFLOW_API_HOST", "127.0.0.1");
    let port = value(&values, "WORKFLOW_API_PORT", "8788")
        .parse::<u16>()
        .map_err(|e| format!("Invalid WORKFLOW_API_PORT: {e}"))?;

    if port == 0 {
        return Err("WORKFLOW_API_PORT must not be zero".to_string());
    }

    Ok(ApiEnvironment { host, port })
}
