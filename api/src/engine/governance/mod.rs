pub mod context;
pub mod decisions;
pub mod evaluate;
pub mod hooks;
pub mod rules;
pub mod scopes;
pub mod signals;

pub use context::{apply_context_mutations, ensure_governance_slots, GovernanceContext};
pub use decisions::{CapabilityInjection, ContextMutation, GovernanceDecision};
pub use evaluate::{
    after_capability,
    after_stage,
    before_capability,
    before_stage,
    injected_capabilities,
    pause_message,
};
pub use hooks::GovernanceHook;
pub use rules::GovernanceRule;
pub use scopes::GovernanceScope;
