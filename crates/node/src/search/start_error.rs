//! Why a reader node did not start.

use oss_rest::RouteError;

/// Why a reader node did not start.
#[derive(Debug)]
pub enum StartError {
    /// The page cache could not be set up.
    Cache(String),
    /// The budget gives the `open_indexes` pool nothing, so no index could
    /// ever open.
    NoOpenIndexes,
    /// A route could not be registered.
    Route(RouteError),
    /// The HTTP server did not start.
    Server(oss_rest::StartError),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::Cache(e) => write!(f, "page cache: {e}"),
            StartError::NoOpenIndexes => f.write_str("the open_indexes pool is empty"),
            StartError::Route(e) => write!(f, "route: {e:?}"),
            StartError::Server(e) => write!(f, "server: {e:?}"),
        }
    }
}

impl std::error::Error for StartError {}
