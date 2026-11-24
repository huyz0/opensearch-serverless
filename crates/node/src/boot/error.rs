//! Why a node did not start.

use crate::assemble::AssembleError;

/// Why a node did not start.
#[derive(Debug)]
pub enum BootError {
    /// The configuration does not fit: the budget or the store.
    Config(String),
    /// The store, the lease or a head could not be reached.
    Cluster(String),
    /// The roles could not be assembled.
    Assemble(AssembleError),
    /// The listener could not be bound.
    Bind(String),
}

impl std::fmt::Display for BootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(e) => write!(f, "configuration: {e}"),
            Self::Cluster(e) => write!(f, "cluster: {e}"),
            Self::Assemble(e) => write!(f, "assembly: {e}"),
            Self::Bind(e) => write!(f, "bind: {e}"),
        }
    }
}

impl std::error::Error for BootError {}
