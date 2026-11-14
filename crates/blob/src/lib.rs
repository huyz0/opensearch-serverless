//! Object-store access for every node role.
//!
//! Every S3 call the system makes goes through this crate, and every call is
//! counted by [`Meter`]. No other crate may depend on an S3 client
//! (`scripts/check-s3-access.py`). The store trait and its RustFS-backed
//! implementation land here when the runtime decision is made
//! (docs/roadmap.md); until then this crate holds the request classes and the
//! meter they are counted in.

pub mod meter;
pub mod op;

pub use meter::Meter;
pub use op::{PriceClass, S3Op};
