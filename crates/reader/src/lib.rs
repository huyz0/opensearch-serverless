//! The search role: serves queries from immutable published segment blobs.
//!
//! A reader holds no durable state. It finds a shard's latest commit with
//! two GETs and no LIST ([`ShardFiles`]), and opens it with lucene-rust,
//! whose page cache reads each file by ranged GETs of the shell blob that
//! holds it ([`EngineStore`]). It never writes to the object store. Tail
//! records pushed by the writer and the search endpoint come next
//! (docs/roadmap.md).

pub mod aggs;
pub mod date;
pub mod dsl;
pub mod engine_store;
pub mod error;
pub mod failure;
pub mod fetch;
pub mod lucene_query;
pub mod mapping;
pub mod numbers;
pub mod open_cost;
pub mod response;
pub mod search;
mod search_sort;
pub mod shard_files;

use std::sync::Arc;

use lucene_search::directory_reader::DirectoryReader;
use lucene_store::CachedBlobDirectory;
use lucene_store::page_cache::PageCache;
use oss_blob::{BlobStore, PooledStore};

pub use engine_store::EngineStore;
pub use error::ReaderError;
pub use shard_files::{FileRange, IndexMeta, MAX_REGISTER_BYTES, ShardFiles};

/// One shard's latest published commit, open for search.
pub struct OpenShard<S: BlobStore + 'static> {
    /// The directory the reader's files come from; it outlives the reader.
    pub directory: CachedBlobDirectory<EngineStore<S>>,
    /// The engine's reader over the commit's segments.
    pub reader: DirectoryReader,
    /// The index's uuid, shard count and fields.
    pub meta: IndexMeta,
}

impl<S: BlobStore + 'static> std::fmt::Debug for OpenShard<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenShard")
            .field("max_doc", &self.reader.max_doc())
            .field("num_docs", &self.reader.num_docs())
            .finish()
    }
}

/// Opens shard `shard` of `index` under `base`: two GETs to find the
/// commit, then the engine's ranged reads through `cache`.
pub fn open_shard<S: BlobStore + 'static>(
    store: Arc<PooledStore<S>>,
    cache: Arc<PageCache>,
    base: &str,
    index: &str,
    shard: u32,
) -> Result<OpenShard<S>, ReaderError> {
    let (files, meta) = ShardFiles::open_with_meta(&store, base, index, shard)?;
    open_located(store, cache, base, index, shard, files, meta)
}

/// Opens the commit [`ShardFiles::open_with_meta`] found for shard `shard`
/// of `index` under `base`, with the engine's ranged reads through `cache`.
/// What it keeps in the heap is at most [`open_cost::open_bytes`] of the
/// same files.
pub fn open_located<S: BlobStore + 'static>(
    store: Arc<PooledStore<S>>,
    cache: Arc<PageCache>,
    base: &str,
    index: &str,
    shard: u32,
    files: ShardFiles,
    meta: IndexMeta,
) -> Result<OpenShard<S>, ReaderError> {
    // ALLOC: one namespace per open shard.
    let namespace = format!("{base}/{index}/{shard}/t={}", files.term);
    let directory = CachedBlobDirectory::new(EngineStore::new(store, files), cache, &namespace);
    let reader = DirectoryReader::open(&directory)?;
    Ok(OpenShard {
        directory,
        reader,
        meta,
    })
}
