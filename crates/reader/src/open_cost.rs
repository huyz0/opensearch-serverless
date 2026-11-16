//! What an open shard keeps in the heap, estimated before it is opened from
//! what locating it read, so the bytes are reserved before they are
//! allocated ([ADR 0014](../../../docs/adr/0014-open-indexes-pool.md)).
//!
//! The engine's `CachedBlobDirectory` reads a file whole into the heap when
//! its name is pinned ([`PageCache::is_pinned_name`]), when it fits one
//! page, or when the cache can neither demand-fill pages nor spill the file
//! to its disk tier; it pages every other file outside the heap. On top of
//! the files read whole, the engine keeps state per segment, per field of a
//! segment and per file. Each constant here is at least 1.35 times what was
//! measured with the counting allocator on indexes the engine wrote (from
//! 10 to 200,000 documents, 1 to 200 segments, up to 100 fields), after
//! the open and a mix of searches, and a test holds the estimate between
//! the measured heap and eight times it.

use lucene_store::page_cache::PageCache;

use crate::shard_files::{IndexMeta, ShardFiles};

/// The fixed part of an open shard: its reader, directory and map entry
/// (measured about 15 KiB after the process's first open).
pub const SHARD_BYTES: usize = 32 << 10;

/// What the engine keeps for each segment (measured about 11 KiB).
pub const SEGMENT_BYTES: usize = 16 << 10;

/// What the engine keeps for each field of each segment (measured about
/// 2.2 KiB for a keyword or text field).
pub const FIELD_BYTES: usize = 3 << 10;

/// What the directory keeps for each file it opened (measured under
/// 0.6 KiB).
pub const FILE_BYTES: usize = 1 << 10;

/// The fields a segment holds besides the mapping's: OpenSearch's metadata
/// fields (`_id`, `_source`, `_seq_no`, `_primary_term`, `_version`,
/// `_field_names`, `_routing`, `_ignored` and their kin), counted
/// generously.
pub const META_FIELDS: usize = 16;

/// How the cache brings files in, as the engine decides it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paging {
    /// The cache's page size.
    pub page_size: u64,
    /// Whether a file over one page is demand-filled outside the heap.
    pub demand: bool,
    /// The smallest file streamed to the disk tier when not demand-filled;
    /// `None` when nothing spills.
    pub spill_min: Option<u64>,
}

impl Paging {
    /// How `cache` brings files in on this host.
    pub fn of(cache: &PageCache) -> Self {
        let config = cache.config();
        let page_size = u64::try_from(config.page_size).unwrap_or(u64::MAX);
        Self {
            page_size,
            demand: config.demand_paging && pager_supports(config.page_size),
            spill_min: if cache.has_disk() {
                config.spill_min_bytes
            } else {
                None
            },
        }
    }

    /// Whether a file of `length` bytes, not pinned, is read whole into the
    /// heap.
    pub fn reads_whole(&self, length: u64) -> bool {
        if length <= self.page_size {
            return true;
        }
        if self.demand {
            return false;
        }
        self.spill_min.is_none_or(|min| length < min)
    }
}

/// Whether the process's `userfaultfd` pager can demand-fill pages of
/// `page_size` bytes.
#[cfg(target_os = "linux")]
fn pager_supports(page_size: usize) -> bool {
    lucene_store::paged_input::Pager::global().is_some_and(|p| p.supports(page_size))
}

/// Whether the process's `userfaultfd` pager can demand-fill pages of
/// `page_size` bytes: never off Linux.
#[cfg(not(target_os = "linux"))]
fn pager_supports(_page_size: usize) -> bool {
    false
}

/// The bytes an open of `files` keeps in the heap, at most, when its files
/// come through `cache`.
pub fn open_bytes(files: &ShardFiles, meta: &IndexMeta, cache: &PageCache) -> usize {
    estimate(files, meta, &Paging::of(cache), &|name| {
        cache.is_pinned_name(name)
    })
}

/// [`open_bytes`] with the cache's paging and pinning given.
pub fn estimate(
    files: &ShardFiles,
    meta: &IndexMeta,
    paging: &Paging,
    pinned: &dyn Fn(&str) -> bool,
) -> usize {
    let segments = files.names().filter(|n| n.ends_with(".si")).count();
    let fields = meta.mapping.len().saturating_add(META_FIELDS);
    let whole = files
        .names()
        .filter_map(|name| files.get(name).map(|f| (name, f.length)))
        .filter(|(name, length)| pinned(name) || paging.reads_whole(*length))
        .map(|(_, length)| usize::try_from(length).unwrap_or(usize::MAX))
        .fold(0usize, usize::saturating_add);
    SHARD_BYTES
        .saturating_add(
            segments
                .saturating_mul(SEGMENT_BYTES.saturating_add(fields.saturating_mul(FIELD_BYTES))),
        )
        .saturating_add(files.len().saturating_mul(FILE_BYTES))
        .saturating_add(whole)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::mapping::Mapping;
    use crate::shard_files::FileRange;

    const PAGE: u64 = 256 << 10;

    fn files(list: &[(&str, u64)]) -> ShardFiles {
        let map: BTreeMap<String, FileRange> = list
            .iter()
            .map(|(name, length)| {
                (
                    (*name).to_owned(),
                    FileRange {
                        key: format!("k/{name}"),
                        offset: 0,
                        length: *length,
                    },
                )
            })
            .collect();
        ShardFiles::of(1, map)
    }

    fn meta(mapping: &str) -> IndexMeta {
        IndexMeta {
            uuid: "u".to_owned(),
            number_of_shards: 1,
            mapping: Mapping::from_escaped(mapping.as_bytes()).expect("mapping"),
        }
    }

    fn paging(demand: bool, spill_min: Option<u64>) -> Paging {
        Paging {
            page_size: PAGE,
            demand,
            spill_min,
        }
    }

    #[test]
    fn the_constants_are_the_adrs() {
        // ADR 0014's numbers, each at least 1.35 times what was measured;
        // `the_estimate_bounds_what_an_open_retains` holds them to the heap.
        assert_eq!(
            (
                SHARD_BYTES,
                SEGMENT_BYTES,
                FIELD_BYTES,
                FILE_BYTES,
                META_FIELDS
            ),
            (32 * 1024, 16 * 1024, 3 * 1024, 1024, 16)
        );
    }

    #[test]
    fn a_file_is_whole_when_it_fits_a_page_or_cannot_be_paged() {
        let demand = paging(true, None);
        assert!(demand.reads_whole(PAGE));
        assert!(!demand.reads_whole(PAGE + 1));
        let heap = paging(false, None);
        assert!(heap.reads_whole(PAGE + 1));
        assert!(heap.reads_whole(u64::MAX));
        let spill = paging(false, Some(1 << 20));
        assert!(spill.reads_whole(PAGE + 1));
        assert!(spill.reads_whole((1 << 20) - 1));
        assert!(!spill.reads_whole(1 << 20));
    }

    #[test]
    fn the_estimate_adds_the_shard_segments_fields_files_and_whole_files() {
        let big = PAGE + 1;
        let commit = files(&[
            ("_0.si", 500),
            ("_0.cfs", big),
            ("_1.si", 400),
            ("_1.tip", big),
            ("_1.fdt", big),
            ("segments_3", 200),
        ]);
        let meta =
            meta(r#"{\"properties\":{\"a\":{\"type\":\"keyword\"},\"b\":{\"type\":\"long\"}}}"#);
        assert_eq!(meta.mapping.len(), 2);
        let pinned = |name: &str| name.ends_with(".tip");
        let fixed =
            SHARD_BYTES + 2 * (SEGMENT_BYTES + (2 + META_FIELDS) * FIELD_BYTES) + 6 * FILE_BYTES;
        let small = 500 + 400 + 200;
        let tip = usize::try_from(big).expect("fits");
        assert_eq!(
            estimate(&commit, &meta, &paging(true, None), &pinned),
            fixed + small + tip,
            "a pinned file is whole whatever its size; a paged one is not"
        );
        assert_eq!(
            estimate(&commit, &meta, &paging(false, None), &pinned),
            fixed + small + 3 * tip,
            "with no paging every file is whole"
        );
        assert_eq!(
            estimate(&files(&[]), &meta, &paging(true, None), &pinned),
            SHARD_BYTES,
        );
    }

    #[test]
    fn a_memory_only_cache_pages_on_this_host_as_the_engine_does() {
        let cache =
            PageCache::new(lucene_store::page_cache::PageCacheConfig::default()).expect("cache");
        let paging = Paging::of(&cache);
        assert_eq!(paging.page_size, PAGE);
        assert_eq!(paging.spill_min, None, "no disk tier, nothing spills");
        assert_eq!(paging.demand, pager_supports(256 << 10));
        let off = PageCache::new(lucene_store::page_cache::PageCacheConfig {
            demand_paging: false,
            ..lucene_store::page_cache::PageCacheConfig::default()
        })
        .expect("cache");
        assert!(!Paging::of(&off).demand);
        let commit = files(&[("_0.si", 10)]);
        let meta = meta("{}");
        assert_eq!(
            open_bytes(&commit, &meta, &cache),
            SHARD_BYTES + SEGMENT_BYTES + META_FIELDS * FIELD_BYTES + FILE_BYTES + 10
        );
    }
}
