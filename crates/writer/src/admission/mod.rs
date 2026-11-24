//! Which writes a node lets wait on the object store: a limit that follows
//! how the store is answering.

mod limiter;
pub mod settings;

pub use limiter::{LimiterConfig, Permit, Stats, Throttled, WriteLimiter};
