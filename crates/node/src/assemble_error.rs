//! Why a node was not assembled.

use oss_rest::RouteError;

/// Why a node was not assembled.
#[derive(Debug)]
pub enum AssembleError {
    /// The budget gives the `open_indexes` pool nothing, so no index could
    /// ever open.
    NoOpenIndexes,
    /// The page cache or the store's buffers could not be set up.
    Memory(String),
    /// A store refused the deployment's base.
    Store(String),
    /// A route is registered twice.
    Route(RouteError),
    /// The HTTP server did not start.
    Server(oss_rest::StartError),
}

impl std::fmt::Display for AssembleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssembleError::NoOpenIndexes => f.write_str("the open_indexes pool is empty"),
            AssembleError::Memory(e) => write!(f, "memory: {e}"),
            AssembleError::Store(e) => write!(f, "store: {e}"),
            AssembleError::Route(e) => write!(f, "route: {e:?}"),
            AssembleError::Server(e) => write!(f, "server: {e:?}"),
        }
    }
}

impl std::error::Error for AssembleError {}
