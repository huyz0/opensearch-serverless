//! Whether a node takes new work: ready once it listens, draining once an
//! operator says so.
//!
//! Ported from the shell's `SearchReadiness`. A warming node, one still
//! taking its peers' hot pages, and a draining node both keep their shards and
//! their `ingest` role; only the `search` role leaves the lease, so no
//! coordinator ranks either as a reader until warming ends.

#[cfg(all(feature = "reader", feature = "writer"))]
mod routes;

use std::sync::atomic::{AtomicBool, Ordering};

use oss_cluster::placement::ROLE_SEARCH;
use oss_format::NodeLease;

#[cfg(all(feature = "reader", feature = "writer"))]
pub use routes::routes;

#[cfg(all(feature = "reader", feature = "writer"))]
use std::sync::Arc;
#[cfg(all(feature = "reader", feature = "writer"))]
use std::time::Duration;

#[cfg(all(feature = "reader", feature = "writer"))]
use crate::assemble::{AssembleError, Assembled};

/// A node's readiness, shared by its endpoints and its lease writer.
#[derive(Debug, Default)]
pub struct Readiness {
    draining: AtomicBool,
    warming: AtomicBool,
}

impl Readiness {
    /// A ready node.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A node that is not ready until [`Readiness::finish_warming`].
    #[must_use]
    pub fn warming() -> Self {
        Self {
            draining: AtomicBool::new(false),
            warming: AtomicBool::new(true),
        }
    }

    /// Whether the node takes new work.
    #[must_use]
    pub fn ready(&self) -> bool {
        !self.draining.load(Ordering::Acquire) && !self.warming.load(Ordering::Acquire)
    }

    /// What the node is doing: `draining`, `warming` or `ready`. A drain
    /// wins over warming.
    #[must_use]
    pub fn state(&self) -> &'static str {
        if self.draining.load(Ordering::Acquire) {
            "draining"
        } else if self.warming.load(Ordering::Acquire) {
            "warming"
        } else {
            "ready"
        }
    }

    /// Ends warming; the node is ready unless it drains.
    pub fn finish_warming(&self) {
        self.warming.store(false, Ordering::Release);
    }

    /// Starts draining; whether this call did (false when already draining).
    pub fn start_draining(&self) -> bool {
        !self.draining.swap(true, Ordering::AcqRel)
    }

    /// Takes the `search` role out of `lease` once the node drains.
    pub fn stamp(&self, lease: &mut NodeLease) {
        if self.ready() || !lease.roles().iter().any(|r| r == ROLE_SEARCH) {
            return;
        }
        let kept: Vec<String> = lease
            .roles()
            .iter()
            .filter(|r| r.as_str() != ROLE_SEARCH)
            .cloned()
            .collect();
        lease.set_roles(kept);
    }
}

#[cfg(all(feature = "reader", feature = "writer"))]
impl Assembled {
    /// Registers `GET /_serverless/ready` and `POST /_serverless/drain` over
    /// `readiness`; a drain answers after `settle`.
    ///
    /// # Errors
    ///
    /// [`AssembleError::Route`] when either route is registered twice.
    pub fn with_lifecycle(
        mut self,
        readiness: Arc<Readiness>,
        settle: Duration,
    ) -> Result<Self, AssembleError> {
        self.router = routes(self.router, readiness, settle).map_err(AssembleError::Route)?;
        Ok(self)
    }
}

#[cfg(test)]
mod tests;
