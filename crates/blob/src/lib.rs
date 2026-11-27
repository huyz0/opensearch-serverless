//! Object-store access for every node role.
//!
//! Every S3 call the system makes goes through this crate, and every attempt
//! is counted by [`Meter`]. No other crate may depend on an S3 client
//! (`scripts/check-s3-access.py`). The [`BlobStore`] trait is the interface;
//! calls block and bodies stream through caller-provided buffers
//! (docs/adr/0005-io-model.md, docs/adr/0004-bounded-memory-and-streaming.md).
//! [`S3Store`] implements it over S3's REST API, and the in-process
//! [`emulator`] behind the `emulator` feature, with an S3 HTTP front end for
//! testing the client. [`FsStore`] implements it over a directory, for single-host nodes and
//! tests (`serverless.store.type = fs`, [`settings`]). [`PooledStore`] streams bodies
//! through the node's buffer pool and answers [`BlobError::Busy`] when it is
//! empty. [`Rounds`], behind the `emulator` feature, wraps a store to measure
//! a path's round-trip depth for the request-budget gates.

mod children;
pub mod chunk;
pub mod cost;
#[cfg(feature = "emulator")]
pub mod emulator;
pub mod error;
pub mod export_store;
pub mod fs;
pub mod guard;
mod http_date;
pub mod meter;
pub mod op;
pub mod pooled;
pub mod read_only;
pub mod register;
#[cfg(feature = "emulator")]
mod rounds;
pub mod s3;
pub mod settings;
mod shared;
pub mod source_roots;
pub mod store;
pub mod usage;
mod usage_store;

pub use chunk::{ChunkSink, ChunkSource, SliceSource};
#[cfg(feature = "emulator")]
pub use emulator::Emulator;
pub use error::{BlobError, BlobResult};
pub use export_store::ExportStore;
pub use fs::FsStore;
pub use meter::Meter;
pub use op::{PriceClass, S3Op};
pub use pooled::PooledStore;
pub use read_only::ReadOnlyStore;
pub use register::{RegisterRead, SwapOutcome};
#[cfg(feature = "emulator")]
pub use rounds::Rounds;
pub use s3::retry::RetryPolicy;
pub use s3::sigv4::Credentials;
pub use s3::{S3Config, S3Store};
pub use settings::StoreSettings;
pub use shared::Shared;
pub use source_roots::{RootError, SourceRoots};
pub use store::{
    BlobStore, ByteRange, CreateOutcome, GetOutcome, ListPage, MAX_DELETE_KEYS, MAX_KEY_BYTES,
    MAX_LIST_KEYS, ObjectMeta,
};
pub use usage_store::UsageStore;
