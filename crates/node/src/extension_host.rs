//! The one WASM host of a node: every user of modules (the upload route,
//! the pin policy, the writer's opener and the search service) compiles in
//! the same engine.

use std::sync::OnceLock;

use oss_ext_wasm::{Host, HostConfig};

/// The node's host, made on first use with the default limits.
pub fn host() -> Host {
    static HOST: OnceLock<Host> = OnceLock::new();
    HOST.get_or_init(|| Host::new(HostConfig::default()))
        .clone()
}
