pub mod api;
pub mod browser;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InferenceTransport {
    Api,
    Browser,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    #[serde(default = "default_profile")]
    pub profile: String,
    #[serde(default = "default_bridge_dir")]
    pub bridge_dir: String,
    #[serde(default = "default_cdp_url")]
    pub cdp_url: String,
    #[serde(default)]
    pub page_url_contains: String,
    #[serde(default)]
    pub target_url: String,
    #[serde(default)]
    pub edge_executable: String,
    #[serde(default)]
    pub user_data_dir: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default = "default_true")]
    pub auto_launch_edge: bool,
    #[serde(default = "default_response_timeout_ms")]
    pub response_timeout_ms: u64,
    #[serde(default = "default_response_poll_ms")]
    pub response_poll_ms: u64,
    #[serde(default = "default_dom_poll_ms")]
    pub dom_poll_ms: u64,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            profile: default_profile(),
            bridge_dir: default_bridge_dir(),
            cdp_url: default_cdp_url(),
            page_url_contains: String::new(),
            target_url: String::new(),
            edge_executable: String::new(),
            user_data_dir: String::new(),
            session_id: None,
            auto_launch_edge: true,
            response_timeout_ms: default_response_timeout_ms(),
            response_poll_ms: default_response_poll_ms(),
            dom_poll_ms: default_dom_poll_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceConfig {
    pub transport: InferenceTransport,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub browser: BrowserConfig,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            transport: InferenceTransport::Api,
            model: default_model(),
            conversation_id: None,
            browser: BrowserConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceResult {
    pub transport: InferenceTransport,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub browser_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserProbeResult {
    pub session_id: String,
    pub browser_connected: bool,
    pub page_open: bool,
    pub url: String,
    pub profile: String,
    pub chat_input_found: bool,
    pub chat_input_visible: bool,
    pub chat_submit_found: bool,
    pub ready: bool,
}

fn default_true() -> bool { true }
fn default_model() -> String { "gpt-4.1".to_string() }
fn default_profile() -> String { "auto".to_string() }
fn default_cdp_url() -> String { "http://127.0.0.1:9222".to_string() }
fn default_response_timeout_ms() -> u64 { 120_000 }
fn default_response_poll_ms() -> u64 { 1_000 }
fn default_dom_poll_ms() -> u64 { 1_000 }

fn default_bridge_dir() -> String {
    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors() {
            let candidate = dir.join("bridge");
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        for dir in cwd.ancestors() {
            let candidate = dir.join("bridge");
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    "bridge".to_string()
}
