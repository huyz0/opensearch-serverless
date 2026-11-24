//! Whether a node takes new work: ready once it listens, draining once an
//! operator says so.
//!
//! Ported from the shell's `SearchReadiness` (without its warming state, which
//! needs the hot-set copy). A draining node keeps its shards and its `ingest`
//! role; only the `search` role leaves its lease, so no coordinator ranks it
//! as a reader.

mod routes;

use std::sync::atomic::{AtomicBool, Ordering};

use oss_cluster::placement::ROLE_SEARCH;
use oss_format::NodeLease;

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
}

impl Readiness {
    /// A ready node.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the node takes new work.
    #[must_use]
    pub fn ready(&self) -> bool {
        !self.draining.load(Ordering::Acquire)
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
