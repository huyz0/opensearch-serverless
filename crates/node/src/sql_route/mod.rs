//! `/_plugins/_sql`: SQL parsed by `oss-query`, pushed into the search path
//! (ADR 0082, spec sql-select).
//!
//! The service reads the statement, checks every index name it extracts
//! against the caller's permissions before any search runs, lowers a `SELECT`
//! to a search body and writes the answer in the requested format. It makes
//! no object-store request of its own: a statement costs the search it
//! pushes.

mod guard;
mod handle;
mod lister;
mod mount;
mod names;
mod routes;

use std::sync::Arc;

use oss_rest::RestError;
use serde_json::Value;

pub use guard::NameGuard;
pub use lister::DescriptorLister;
pub(crate) use mount::mount;
pub use routes::routes;

/// What the node supplies to a statement: mappings and searches.
pub trait SqlBackend: Send + Sync {
    /// The mapping body (`{"properties": ...}`) of the index or alias `name`.
    ///
    /// # Errors
    ///
    /// A refusal as the REST layer answers it, or a search error body.
    fn mapping(&self, name: &str) -> Result<Value, SqlFailure>;

    /// Runs the search `body` over `names` and returns its response bytes.
    ///
    /// # Errors
    ///
    /// A refusal as the REST layer answers it, or a search error body.
    fn search(&self, names: &[String], body: &Value) -> Result<Vec<u8>, SqlFailure>;

    /// As [`Self::search`], giving up with an error between shards once
    /// `stop` answers true. A backend that cannot stop early runs to the end.
    ///
    /// # Errors
    ///
    /// As [`Self::search`]; a search that stopped is an error.
    fn search_until(
        &self,
        names: &[String],
        body: &Value,
        _stop: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<u8>, SqlFailure> {
        self.search(names, body)
    }
}

/// Why a backend call gave no answer.
#[derive(Debug)]
pub enum SqlFailure {
    /// An error the HTTP edge answers.
    Rest(RestError),
    /// A search error: its status and JSON body.
    Body(u16, Vec<u8>),
}

/// Decides whether a caller may read indices.
pub trait SqlGuard: Send + Sync {
    /// `Ok` when `principal` may run `action` on every one of `names`, else the
    /// body of the refusal (403).
    ///
    /// # Errors
    ///
    /// The JSON body of the refusal.
    fn allow(&self, principal: Option<&str>, action: &str, names: &[&str]) -> Result<(), String>;
}

/// A guard for a node with security off: everything is allowed.
pub struct AllowAll;

impl SqlGuard for AllowAll {
    fn allow(&self, _: Option<&str>, _: &str, _: &[&str]) -> Result<(), String> {
        Ok(())
    }
}

/// The bounds a statement runs under.
#[derive(Debug, Clone, Copy)]
pub struct SqlLimits {
    /// Whether the endpoint answers.
    pub enabled: bool,
    /// Whether the PPL endpoint answers.
    pub ppl_enabled: bool,
    /// The most rows a statement returns.
    pub size_limit: usize,
    /// The largest request body.
    pub max_request_bytes: usize,
    /// The most groups a grouped statement reads across its pages.
    pub max_buckets: usize,
}

/// The names an index pattern's literal prefix stands for: the one listing `SHOW TABLES` and
/// `DESCRIBE` over a pattern read.
pub trait NameLister: Send + Sync {
    /// The names that begin with `prefix`, at most `cap`.
    ///
    /// # Errors
    ///
    /// A refusal as the REST layer answers it; more than `cap` names is one.
    fn names_with_prefix(&self, prefix: &str, cap: usize) -> Result<Vec<String>, SqlFailure>;
}

/// The SQL endpoint's service.
pub struct SqlService {
    backend: Arc<dyn SqlBackend>,
    guard: Arc<dyn SqlGuard>,
    limits: SqlLimits,
    lister: Option<Arc<dyn NameLister>>,
}

impl SqlService {
    /// A service over `backend`, checking names with `guard`.
    #[must_use]
    pub fn new(backend: Arc<dyn SqlBackend>, guard: Arc<dyn SqlGuard>, limits: SqlLimits) -> Self {
        Self {
            backend,
            guard,
            limits,
            lister: None,
        }
    }

    /// The service reading the names a pattern stands for from `lister`.
    #[must_use]
    pub fn with_lister(mut self, lister: Arc<dyn NameLister>) -> Self {
        self.lister = Some(lister);
        self
    }
}
