//! Open prefetch: the files a cold open reads whole, fetched together.
//!
//! The engine opens a commit by reading its small and pinned files one
//! after another (the commit file, each segment's `.si`, field infos and
//! metadata). [`plan()`] picks those files from the commit's file table and
//! merges neighbours of one blob into ranges; [`Primed::fetch`] reads the
//! ranges in one round; [`crate::EngineStore`] then serves the engine's reads
//! from the bytes until the open returns. See
//! [the spec](../../../../docs/specs/reader-open-prefetch/SPEC.md).

use std::sync::Arc;

mod gate;
mod plan;
mod primed;

#[cfg(test)]
mod gate_tests;
#[cfg(test)]
mod plan_tests;
#[cfg(test)]
mod tests;

pub use gate::{Gate, Grant};
pub use plan::{Member, Range, plan};
pub use primed::Primed;

/// How the open's files are prefetched.
#[derive(Clone, Debug)]
pub struct OpenPrefetch {
    /// Whether to prefetch at all; off, an open reads as it always did.
    pub enabled: bool,
    /// Files of one blob at most this many bytes apart are one range.
    pub gap_bytes: u64,
    /// The most bytes of files picked for one open.
    pub max_bytes: u64,
    /// The bytes and threads every open of the node shares; a range it
    /// refuses is left to the engine.
    pub gate: Arc<Gate>,
}

impl Default for OpenPrefetch {
    fn default() -> Self {
        Self {
            enabled: true,
            gap_bytes: 32 << 10,
            max_bytes: 8 << 20,
            gate: Gate::node(),
        }
    }
}

impl OpenPrefetch {
    /// No prefetch: the open reads file by file.
    pub fn off() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }
}

/// The most ranges fetched at once.
pub(crate) const MAX_CONCURRENT: usize = 16;
