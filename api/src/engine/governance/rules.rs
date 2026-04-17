use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{GovernanceContext, GovernanceDecision, GovernanceHook, GovernanceScope};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceRule {
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub hooks: Vec<GovernanceHook>,
    #[serde(default)]
    pub scopes: Vec<GovernanceScope>,
    #[serde(default)]
    pub when: Value,
    #[serde(default)]
    pub decisions: Vec<GovernanceDecision>,
}

impl GovernanceRule {
    pub fn matches(&self, hook: GovernanceHook, scope: GovernanceScope, ctx: &GovernanceContext<'_>) -> bool {
        self.enabled
            && self.hooks.iter().any(|item| *item == hook)
            && self.scopes.iter().any(|item| *item == scope)
            && matches_when(&self.when, ctx)
    }
}

fn default_true() -> bool {
    true
}

fn matches_when(when: &Value, ctx: &GovernanceContext<'_>) -> bool {
    let Some(obj) = when.as_object() else {
        return true;
    };

    if let Some(step_id) = obj.get("step_id").and_then(Value::as_str) {
        if ctx.step.map(|step| step.id.as_str()) != Some(step_id) {
            return false;
        }
    }

    if let Some(capability) = obj.get("capability").and_then(Value::as_str) {
        if ctx.capability.map(|item| item.capability.as_str()) != Some(capability)
            && ctx.capability_result.map(|item| item.capability.as_str()) != Some(capability)
        {
            return false;
        }
    }

    true
}
