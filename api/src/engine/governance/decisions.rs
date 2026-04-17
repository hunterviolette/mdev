use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::GovernanceScope;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMutation {
    pub scope: GovernanceScope,
    #[serde(default)]
    pub patch: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityInjection {
    pub capability: String,
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GovernanceDecision {
    Continue,
    MutateContext { mutation: ContextMutation },
    InjectCapability { capability: CapabilityInjection },
    RequireApproval { reason: String },
    Retry { reason: String },
    Pause { reason: String },
}
