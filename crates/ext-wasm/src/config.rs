//! The limits the host sets; a module's descriptor may only lower them.

use std::sync::Arc;

/// Where a module's `log` lines go: the module's name and the cleaned text.
pub type LogSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Host defaults and the log sink.
#[derive(Clone)]
pub struct HostConfig {
    /// Instructions a call may run.
    pub fuel: u64,
    /// Bytes of linear memory a call may hold.
    pub memory_bytes: u64,
    /// Milliseconds a call may take (the epoch ticks every 10 ms).
    pub deadline_ms: u64,
    /// Calls at once for one module.
    pub concurrency: u32,
    /// Calls at once for one tenant of one module.
    pub tenant_concurrency: u32,
    /// Bytes of native stack a call may use.
    pub stack_bytes: usize,
    /// The sink for `log`, after the per-message and per-second caps.
    pub log: LogSink,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            fuel: 50_000_000,
            memory_bytes: 64 * 1024 * 1024,
            deadline_ms: 500,
            concurrency: 8,
            tenant_concurrency: 2,
            stack_bytes: 1024 * 1024,
            log: Arc::new(|_, _| {}),
        }
    }
}
