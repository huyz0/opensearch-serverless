//! Where a script named by `id` finds its source: the node installs one
//! resolver, which the reader's script parser asks. The trait lives in
//! `oss-types`, below every role.

pub use oss_types::stored_scripts::{ResolveError, Stored, StoredScripts, install, installed};
