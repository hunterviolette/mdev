use serde::{Deserialize, Serialize};

pub mod changeset;
pub mod sync;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GatewayMode {
    ChangeSet,
    Sync,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncMode {
    Entire,
    Tree,
    Diff,
}
