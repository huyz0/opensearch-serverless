//! The types of the job registry: limits, who a job belongs to, what a running
//! search is told, and how a read answers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use oss_mem::Pool;

/// The wall clock, in milliseconds; passed in so a test drives it.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// What each entry adds to its reservation for its id and state.
pub const ENTRY_OVERHEAD: usize = 1 << 10;

/// The bounds of the registry (docs/specs/async-search, "Switch").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Jobs whose thread is alive, a cancelled one included.
    pub max_running: usize,
    /// The same for one tenant.
    pub max_running_per_tenant: usize,
    /// Entries kept, running or finished.
    pub max_kept: usize,
    /// Bytes of kept results.
    pub max_kept_bytes: usize,
    /// The most bytes of one result.
    pub max_response_bytes: usize,
    /// The longest `keep_alive`, in milliseconds.
    pub max_keep_alive_ms: u64,
    /// The longest a search may run, in milliseconds.
    pub max_running_time_ms: u64,
    /// The memory pool entries are reserved from: the plugin pool for a
    /// plugin's jobs, the query pool for the node's own routes (the plugin
    /// pool is empty on a node that runs no extension).
    pub pool: Pool,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_running: 8,
            max_running_per_tenant: 2,
            max_kept: 64,
            max_kept_bytes: 32 << 20,
            max_response_bytes: 1 << 20,
            max_keep_alive_ms: 3_600_000,
            max_running_time_ms: 600_000,
            pool: Pool::Extensions,
        }
    }
}

impl Limits {
    /// The keep-alive a request that names none gets: 12 hours, or the
    /// maximum when that is smaller.
    pub fn default_keep_alive_ms(&self) -> u64 {
        self.max_keep_alive_ms.min(43_200_000)
    }
}

/// Whose a job is: the host's, from the authenticated request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Who {
    /// The tenant.
    pub tenant: String,
    /// The user within it.
    pub user: String,
}

/// What a running search is told, and asks.
pub struct Ctx {
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) deadline: u64,
    pub(super) clock: Clock,
    pub(super) max_bytes: usize,
}

impl Ctx {
    /// Whether the search should stop at its next stage: it was cancelled or
    /// its running time is spent.
    pub fn should_stop(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || (self.clock)() >= self.deadline
    }

    /// The most bytes the result may have; the search stops writing at it.
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }
}

/// Why a search did not give a result.
#[derive(Debug)]
pub enum Failure {
    /// It saw [`Ctx::should_stop`] and stopped.
    Stopped,
    /// It wrote past [`Ctx::max_bytes`].
    TooLarge,
    /// It failed; the error body as the plugin renders it.
    Error(String),
}

/// The work of a job: a search the host builds; a plugin supplies no code.
pub type Work = Box<dyn FnOnce(&Ctx) -> Result<Vec<u8>, Failure> + Send>;

/// Where a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Its search is running.
    Running,
    /// It finished with a result.
    Succeeded,
    /// It finished without one.
    Failed,
}

/// A read of a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// The id, `<node>:<number>`.
    pub id: String,
    /// The state.
    pub state: State,
    /// When it started, in milliseconds.
    pub start: u64,
    /// When it expires, in milliseconds.
    pub expiration: u64,
    /// The result, once succeeded.
    pub body: Option<Vec<u8>>,
    /// The error type and body, once failed.
    pub error: Option<(&'static str, String)>,
    /// What a read of the running job shows.
    pub note: String,
}

/// A refusal, with the HTTP status it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    /// 429: a limit or the memory pool.
    Rejected(&'static str),
    /// 404: no such job for this caller.
    NotFound,
    /// 400: a parameter.
    BadRequest(String),
}

impl JobError {
    /// The HTTP status.
    pub fn status(&self) -> u16 {
        match self {
            Self::Rejected(_) => 429,
            Self::NotFound => 404,
            Self::BadRequest(_) => 400,
        }
    }
}

/// Counts for this node, in stock's names, and the extras apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Jobs whose search is running.
    pub running_current: u64,
    /// Submits accepted.
    pub submitted: u64,
    /// Searches that finished with a result.
    pub search_completed: u64,
    /// Searches that failed.
    pub search_failed: u64,
    /// Submits refused for a limit.
    pub rejected: u64,
    /// Jobs cancelled by a delete.
    pub cancelled: u64,
    /// Finished entries dropped at their expiry.
    pub expired: u64,
    /// Finished entries dropped for room.
    pub evicted: u64,
    /// Bytes reserved for entries.
    pub held_bytes: u64,
}
