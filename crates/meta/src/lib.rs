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

mod descriptors;
mod error;
mod names;

pub use descriptors::{DeleteMode, DescriptorStore, REAP_HANDOFF_MILLIS, Resolution, Target};
pub use error::MetaError;
pub use names::{MAX_NAME_BYTES, NameError, NameRule, validate_index_or_alias};
