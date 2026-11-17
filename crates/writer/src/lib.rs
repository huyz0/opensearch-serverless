//! The indexing role: makes writes durable and publishes segments.
//!
//! A writer appends acknowledged writes to its node's single write log (one
//! log per node, never per shard), fenced so a paused or replaced writer
//! cannot overwrite a successor. It builds segments, uploads each as one
//! immutable blob, and carries commit metadata inside uploads it already
//! makes rather than in extra PUTs.

pub mod document;
pub mod index;
pub mod log;
pub mod lucene;
pub mod mapping;
