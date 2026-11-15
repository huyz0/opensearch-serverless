//! The metadata plane on registers.
//!
//! What names exist and what they hold lives in the object store, one
//! register a name, and changes only by compare-and-swap, as in the Java
//! shell's `metadata` package: no cluster manager, no lock and no leader.
//! Rust and Java nodes on one bucket see the same names.
//!
//! - [`validate_index_or_alias`]: what a name may be before it becomes a key.
//! - [`DescriptorStore`]: create, read, update and delete an index's
//!   descriptor register, with tombstones keeping a deleted name's
//!   generations moving.
//! - [`DeleteMode::probe`]: whether a store's conditional delete can be
//!   trusted, asked of the store itself.
//! - [`DescriptorStore::sweep_tombstones`]: removing tombstones once their
//!   quarantine has passed, found from their hourly markers.
//! - [`DescriptorStore::list_page`] and [`DescriptorStore::names_with_prefix`]:
//!   the one enumeration of names, a bounded page at a time, and what an
//!   index pattern matches, at a cost set by its cap.

mod descriptors;
mod error;
mod listing;
mod names;
mod probe;
mod sweep;

pub use descriptors::{
    BUCKET_MILLIS, DEFAULT_QUARANTINE_MILLIS, DeleteMode, DescriptorStore, REAP_DEADLINE_MILLIS,
    REAP_HANDOFF_MILLIS, Resolution, Target,
};
pub use error::MetaError;
pub use names::{MAX_NAME_BYTES, NameError, NameRule, validate_index_or_alias, validate_prefix};
pub use probe::PROBE_PREFIX;
pub use sweep::{MARKERS_PREFIX, Sweep, Swept};
