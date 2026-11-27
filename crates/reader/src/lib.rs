//! The search role: serves queries from immutable published segment blobs.
//!
//! A reader holds no durable state. It finds a shard's latest commit with
//! two GETs and no LIST ([`ShardFiles`]), and opens it with lucene-rust,
//! whose page cache reads each file by ranged GETs of the shell blob that
//! holds it ([`EngineStore`]). It never writes to the object store. Tail
//! records pushed by the writer and the search endpoint come next
//! (docs/roadmap.md).

pub mod aggs;
pub mod commit_push;
pub mod date;
pub mod date_math;
pub mod date_pattern;
pub mod date_zone;
pub mod dsl;
mod empty_commit;
pub mod engine_store;
pub mod error;
pub mod failure;
pub mod fetch;
mod frozen;
pub mod hits_total;
pub mod knn;
pub mod ltr;
pub mod lucene_query;
mod lucene_range;
pub mod mapping;
mod mapping_types;
pub mod numbers;
pub mod numbers_decimal;
pub mod open_cost;
pub mod overlay;
pub mod painless_switch;
pub mod pin_settings;
pub mod pinned;
pub mod prefetch;
pub mod rank_eval;
pub mod response;
mod result_window;
mod script_failure;
pub mod search;
mod search_collapse;
mod search_error;
mod search_inner_hits;
#[cfg(test)]
mod search_inner_hits_tests;
mod search_ltr;
pub mod search_ltr_log;
mod search_named;
mod search_nested_bytes;
mod search_options;
pub mod search_pipeline;
mod search_post_filter;
mod search_query_fields;
mod search_rescore;
mod search_sort;
pub mod stored_scripts;
pub use search_inner_hits::{InnerResult, ParentInner};
mod search_sort_nested;
mod search_terminate;
mod search_top;
#[cfg(test)]
mod settings_tests;
pub mod shard_files;
pub mod shard_ref;
mod shared_mapping;
pub mod template;
pub mod vector_warm;

use std::sync::Arc;

use lucene_search::directory_reader::DirectoryReader;
use lucene_store::CachedBlobDirectory;
use lucene_store::page_cache::PageCache;
use oss_blob::{BlobStore, PooledStore};

pub use engine_store::EngineStore;
pub use error::ReaderError;
pub use prefetch::OpenPrefetch;
pub use shard_files::{FileRange, IndexMeta, MAX_REGISTER_BYTES, ShardFiles};
pub use shared_mapping::SharedMapping;

/// One shard's latest published commit, open for search.
pub struct OpenShard<S: BlobStore + 'static> {
    /// The directory the reader's files come from; it outlives the reader.
    pub directory: Arc<CachedBlobDirectory<EngineStore<S>>>,
    /// The engine's reader over the commit's segments.
    pub reader: DirectoryReader,
    /// The index's uuid, shard count and fields.
    pub meta: IndexMeta,
    /// Which shard of the index this is.
    pub shard: u32,
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

/// [`open_shard`] for a commit the shard's writer pushed: `pushed` is the
/// commit register's value, so one GET (the descriptor) finds the commit.
pub fn open_shard_pushed<S: BlobStore + 'static>(
    store: Arc<PooledStore<S>>,
    cache: Arc<PageCache>,
    base: &str,
    index: &str,
    shard: u32,
    pushed: &[u8],
) -> Result<OpenShard<S>, ReaderError> {
    let (files, meta) = ShardFiles::open_pushed_with_meta(&store, base, index, shard, pushed)?;
    open_located(store, cache, base, index, shard, files, meta)
}

/// [`open_shard`] for the commit `view` froze of shard `shard`: one GET (the
/// descriptor), and the commit register is never read.
pub fn open_frozen_shard<S: BlobStore + 'static>(
    store: Arc<PooledStore<S>>,
    cache: Arc<PageCache>,
    base: &str,
    index: &str,
    view: &oss_format::PointInTime,
    shard: u32,
) -> Result<OpenShard<S>, ReaderError> {
    let (files, meta) = ShardFiles::open_frozen(&store, base, index, view, shard)?;
    open_located(store, cache, base, index, shard, files, meta)
}

/// The numeric doc values field holding 1 on a replaced or deleted document.
const SOFT_DELETES_FIELD: &str = "__soft_deletes";

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
    open_located_with(
        store,
        cache,
        base,
        index,
        shard,
        files,
        meta,
        &OpenPrefetch::default(),
    )
}

/// [`open_located`] with `prefetch` deciding how the files the open reads
/// whole are fetched ([`prefetch`]).
#[allow(clippy::too_many_arguments)]
pub fn open_located_with<S: BlobStore + 'static>(
    store: Arc<PooledStore<S>>,
    cache: Arc<PageCache>,
    base: &str,
    index: &str,
    shard: u32,
    files: ShardFiles,
    meta: IndexMeta,
    prefetch: &OpenPrefetch,
) -> Result<OpenShard<S>, ReaderError> {
    let primed = prefetch.enabled.then(|| {
        let page = u64::try_from(cache.config().page_size).unwrap_or(u64::MAX);
        let ranges = prefetch::plan(&files, &|n| cache.is_pinned_name(n), page, prefetch);
        prefetch::Primed::fetch(&store, &ranges)
    });
    // ALLOC: one namespace per open shard.
    let namespace = format!("{base}/{index}/{shard}/t={}", files.term);
    let engine = EngineStore::new(store, files);
    if let Some(primed) = primed {
        engine.prime(primed);
    }
    let directory = Arc::new(CachedBlobDirectory::new(engine, cache, &namespace));
    // What the writer replaced or deleted stays in the segment as a soft
    // delete; searches and counts do not see it.
    let opened = DirectoryReader::open(directory.as_ref())
        .and_then(|r| r.soft_deletes_wrapper(SOFT_DELETES_FIELD));
    directory.store().release();
    let reader = opened?;
    Ok(OpenShard {
        directory,
        reader,
        meta,
        shard,
    })
}

#[cfg(test)]
#[path = "search_nested_bytes_tests.rs"]
mod search_nested_bytes_tests;
#[cfg(test)]
#[path = "search_nested_tests.rs"]
mod search_nested_tests;
