//! Whether a node takes new work: ready once it listens, draining once an
//! operator says so.
//!
//! Ported from the shell's `SearchReadiness`. A warming node, one still
//! taking its peers' hot pages, and a draining node both keep their shards and
//! their `ingest` role; only the `search` role leaves the lease, so no
//! coordinator ranks either as a reader until warming ends.

#[cfg(all(feature = "reader", feature = "writer"))]
mod routes;

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};

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
#[derive(Debug)]
pub struct Readiness {
    draining: AtomicBool,
    warming: AtomicBool,
    /// When the lease first said `search`, epoch milliseconds; 0 until then.
    search_since: AtomicI64,
    /// The joiner's own ramp, 0 to 1000; -1 until it says one.
    ramp: AtomicI32,
    /// Whether the lease says the slow start's fields; off, a lease is as it always was.
    stamps_ramp: bool,
}

impl Default for Readiness {
    fn default() -> Self {
        Self {
            draining: AtomicBool::new(false),
            warming: AtomicBool::new(false),
            search_since: AtomicI64::new(0),
            ramp: AtomicI32::new(-1),
            stamps_ramp: false,
        }
    }
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
            warming: AtomicBool::new(true),
            ..Self::default()
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

    /// A node whose lease says `search_since` and its ramp, for the slow start.
    #[must_use]
    pub fn stamping_ramp(mut self, on: bool) -> Self {
        self.stamps_ramp = on;
        self
    }

    /// Sets the ramp the lease publishes (the slow start's own judgement of
    /// how warm the node is, 0 to 1000).
    pub fn set_ramp(&self, permille: i32) {
        self.ramp.store(permille.clamp(0, 1000), Ordering::Release);
    }

    /// Takes the `search` role out of `lease` once the node drains, and
    /// says since when a node that keeps it has held it, and its ramp.
    pub fn stamp(&self, lease: &mut NodeLease) {
        if self.stamps_ramp && self.ready() && lease.roles().iter().any(|r| r == ROLE_SEARCH) {
            let _ = self.search_since.compare_exchange(
                0,
                oss_cluster::placement::now_millis().max(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            lease.search_since = Some(self.search_since.load(Ordering::Acquire));
            let ramp = self.ramp.load(Ordering::Acquire);
            lease.set_ramp_permille((ramp >= 0).then_some(ramp));
        }
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
        self.router = routes(self.router, readiness, settle, self.events.clone())
            .map_err(AssembleError::Route)?;
        Ok(self)
    }
}

#[cfg(test)]
mod tests;
