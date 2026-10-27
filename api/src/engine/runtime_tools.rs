use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

fn default_working_directory() -> String {
    ".".to_string()
}

fn default_port_start() -> u16 {
    24000
}

fn default_port_end() -> u16 {
    24999
}

fn default_shutdown_grace_seconds() -> u64 {
    5
}

fn default_readiness_timeout_seconds() -> u64 {
    60
}

fn default_hostname_template() -> String {
    "{workflow}.qa.localhost".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalShell {
    System,
    Direct,
    Cmd,
    PowerShell,
    Sh,
    Bash,
}

impl Default for TerminalShell {
    fn default() -> Self {
        Self::System
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalCommandMode {
    Run,
    Service,
}

impl Default for TerminalCommandMode {
    fn default() -> Self {
        Self::Run
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalCommandSpec {
    pub id: String,
    pub label: String,
    pub command: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default = "default_working_directory")]
    pub working_directory: String,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub shell: TerminalShell,
    #[serde(default)]
    pub mode: TerminalCommandMode,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub continue_on_error: bool,
}

impl Default for TerminalCommandSpec {
    fn default() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            command: String::new(),
            arguments: Vec::new(),
            working_directory: default_working_directory(),
            environment: BTreeMap::new(),
            shell: TerminalShell::System,
            mode: TerminalCommandMode::Run,
            timeout_seconds: None,
            continue_on_error: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSequenceStrategy {
    #[default]
    Series,
    Parallel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSequenceSpec {
    #[serde(default)]
    pub strategy: TerminalSequenceStrategy,
    #[serde(default)]
    pub commands: Vec<TerminalCommandSpec>,
    #[serde(default = "default_true")]
    pub stop_on_failure: bool,
}

impl Default for TerminalSequenceSpec {
    fn default() -> Self {
        Self {
            strategy: TerminalSequenceStrategy::Series,
            commands: Vec::new(),
            stop_on_failure: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DependencyEcosystem {
    Node,
    Cargo,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DependencyMismatchDisposition {
    OperatorCheckpoint,
    SkipStage,
    ContinueTrustedWithWarning,
}

impl Default for DependencyMismatchDisposition {
    fn default() -> Self {
        Self::OperatorCheckpoint
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DependencyCheckpointDisposition {
    CreateIsolatedDependencies,
    ContinueTrustedWithWarning,
    SkipStage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrustedDependencyArtifactSpec {
    NodeModules {
        path: String,
        #[serde(default = "default_true")]
        read_only: bool,
    },
    Cargo {
        cargo_home: Option<String>,
        target_directory: Option<String>,
        #[serde(default)]
        share_target: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IsolatedDependencySpec {
    pub storage_path: String,
    #[serde(default)]
    pub seed_from_trusted: bool,
    #[serde(default)]
    pub install: TerminalSequenceSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyMismatchPolicy {
    #[serde(default)]
    pub disposition: DependencyMismatchDisposition,
    #[serde(default)]
    pub allowed_dispositions: Vec<DependencyCheckpointDisposition>,
}

impl Default for DependencyMismatchPolicy {
    fn default() -> Self {
        Self {
            disposition: DependencyMismatchDisposition::OperatorCheckpoint,
            allowed_dispositions: vec![
                DependencyCheckpointDisposition::CreateIsolatedDependencies,
                DependencyCheckpointDisposition::ContinueTrustedWithWarning,
                DependencyCheckpointDisposition::SkipStage,
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyProviderSpec {
    pub id: String,
    pub label: String,
    pub ecosystem: DependencyEcosystem,
    #[serde(default = "default_working_directory")]
    pub root: String,
    #[serde(default)]
    pub manifests: Vec<String>,
    #[serde(default)]
    pub trusted_artifact: Option<TrustedDependencyArtifactSpec>,
    #[serde(default)]
    pub isolated: IsolatedDependencySpec,
    #[serde(default)]
    pub mismatch: DependencyMismatchPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SharedDependenciesConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub providers: Vec<DependencyProviderSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortRangeSpec {
    #[serde(default = "default_port_start")]
    pub start: u16,
    #[serde(default = "default_port_end")]
    pub end: u16,
}

impl Default for PortRangeSpec {
    fn default() -> Self {
        Self {
            start: default_port_start(),
            end: default_port_end(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QaServicePortSpec {
    pub environment_variable: String,
    #[serde(default)]
    pub preferred: Option<u16>,
}

impl Default for QaServicePortSpec {
    fn default() -> Self {
        Self {
            environment_variable: "PORT".to_string(),
            preferred: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QaReadinessSpec {
    None,
    Http {
        path: String,
        #[serde(default)]
        expected_status: Option<u16>,
        #[serde(default = "default_readiness_timeout_seconds")]
        timeout_seconds: u64,
    },
    Tcp {
        #[serde(default = "default_readiness_timeout_seconds")]
        timeout_seconds: u64,
    },
    Log {
        pattern: String,
        #[serde(default = "default_readiness_timeout_seconds")]
        timeout_seconds: u64,
    },
}

impl Default for QaReadinessSpec {
    fn default() -> Self {
        Self::Http {
            path: "/".to_string(),
            expected_status: Some(200),
            timeout_seconds: default_readiness_timeout_seconds(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QaServiceSpec {
    pub id: String,
    pub label: String,
    pub command: TerminalCommandSpec,
    #[serde(default)]
    pub port: QaServicePortSpec,
    #[serde(default)]
    pub readiness: QaReadinessSpec,
    #[serde(default)]
    pub public: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QaDirectoryMode {
    Copy,
    Symlink,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QaSymlinkBehavior {
    Reject,
    Preserve,
    #[default]
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QaDirectoryMapping {
    pub source: String,
    pub destination: String,
    pub mode: QaDirectoryMode,
    #[serde(default)]
    pub replace_existing: bool,
    #[serde(default)]
    pub existing_destination: Option<String>,
    #[serde(default)]
    pub exclude_directories: Vec<String>,
    #[serde(default)]
    pub symlink_behavior: QaSymlinkBehavior,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QaEnvironmentSpec {
    #[serde(default)]
    pub directory_mappings: Vec<QaDirectoryMapping>,
    #[serde(default)]
    pub port_range: PortRangeSpec,
    #[serde(default = "default_hostname_template")]
    pub hostname_template: String,
    #[serde(default)]
    pub prepare: TerminalSequenceSpec,
    #[serde(default = "default_qa_services")]
    pub services: Vec<QaServiceSpec>,
    #[serde(default = "default_shutdown_grace_seconds")]
    pub shutdown_grace_seconds: u64,
}

fn default_qa_services() -> Vec<QaServiceSpec> {
    let mut api_environment = BTreeMap::new();
    api_environment.insert(
        "WORKFLOW_API_HOST".to_string(),
        "127.0.0.1".to_string(),
    );

    vec![
        QaServiceSpec {
            id: "api".to_string(),
            label: "API".to_string(),
            command: TerminalCommandSpec {
                id: "deploy-qa-api".to_string(),
                label: "cargo run".to_string(),
                command: "cargo run".to_string(),
                arguments: Vec::new(),
                working_directory: "api".to_string(),
                environment: api_environment,
                shell: TerminalShell::System,
                mode: TerminalCommandMode::Service,
                timeout_seconds: None,
                continue_on_error: false,
            },
            port: QaServicePortSpec {
                environment_variable: "WORKFLOW_API_PORT".to_string(),
                preferred: None,
            },
            readiness: QaReadinessSpec::Http {
                path: "/api/health".to_string(),
                expected_status: Some(200),
                timeout_seconds: 120,
            },
            public: false,
        },
    ]
}

impl Default for QaEnvironmentSpec {
    fn default() -> Self {
        Self {
            directory_mappings: Vec::new(),
            port_range: PortRangeSpec::default(),
            hostname_template: default_hostname_template(),
            prepare: TerminalSequenceSpec::default(),
            services: default_qa_services(),
            shutdown_grace_seconds: default_shutdown_grace_seconds(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompileStageSpec {
    #[serde(default)]
    pub dependency_providers: Vec<String>,
    #[serde(default)]
    pub commands: TerminalSequenceSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QaStageSpec {
    #[serde(default)]
    pub environment: QaEnvironmentSpec,
}
