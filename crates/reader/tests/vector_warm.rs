//! When the vector files of a shard are read in: at open (the engine's
//! default) or at the first `knn` search (`oss_reader::vector_warm`), over a
//! 5-segment index of vectors on the emulator.
//!
//! The engine's warm switch is process-wide, so every test here takes the
//! lock and runs against an index of its own.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use lucene_analysis::Analyzer;
use lucene_codecs::field_infos::VectorSimilarityFunction;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, Emulator, PooledStore, Rounds, S3Op, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_reader::dsl::parse_chunks;
use oss_reader::search::search;
use oss_reader::{OpenShard, open_shard, vector_warm};
use oss_testkit::index::{Spec, blobs, spec_vector};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const DOCS: usize = 16_000;
const DIM: usize = 128;
const SEGMENT: usize = 8_000;

type Store = Arc<PooledStore<Rounds<Emulator>>>;

fn spec() -> Spec {
    Spec::new("kn", DOCS)
        .segments_of(SEGMENT)
        .with_vectors(VectorSimilarityFunction::Euclidean)
        .with_vector_dimension(DIM)
}

/// The index's blobs, built once: a build costs more than a search.
fn built() -> &'static (Vec<(String, Vec<u8>)>, usize) {
    static BUILT: std::sync::OnceLock<(Vec<(String, Vec<u8>)>, usize)> = std::sync::OnceLock::new();
    BUILT.get_or_init(|| {
        let all: Vec<(String, Vec<u8>)> = blobs("", &spec()).into_iter().collect();
        let bytes = all.iter().map(|(_, b)| b.len()).sum();
        (all, bytes)
    })
}

fn store() -> (Store, usize) {
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::Transport, 64 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 64).expect("fits");
    let store = Arc::new(PooledStore::new(
        Rounds::new(Emulator::new(1 << 30, 1)),
        buffers,
    ));
    let (all, bytes) = built();
    for (key, blob) in all {
        store.put(key, &mut SliceSource::new(blob)).expect("put");
    }
    (store, *bytes)
}

/// What one cold open and the searches after it cost.
#[derive(Debug)]
struct Cost {
    open_bytes: u64,
    open_requests: u64,
    first_bytes: u64,
    first_requests: u64,
    /// Round trips of the open and first search together.
    depth: usize,
    second_requests: u64,
    hits: Vec<(i32, Option<u32>)>,
    index_bytes: usize,
}

fn measure(at_open: bool, body: &str) -> Cost {
    vector_warm::set_at_open(at_open);
    let (store, index_bytes) = store();
    let rounds = store.store();
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    rounds.reset(Duration::from_millis(5));
    let (b0, r0) = (rounds.meter().bytes_read(), rounds.meter().total());
    let shard: OpenShard<_> = open_shard(store.clone(), cache, "", "kn", 0).expect("opens");
    let (b1, r1) = (rounds.meter().bytes_read(), rounds.meter().total());
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let analyzer = Analyzer::standard(None);
    let hits = search(&shard, "kn", &source, &analyzer).expect("search");
    let (b2, r2) = (rounds.meter().bytes_read(), rounds.meter().total());
    let depth = rounds.depth();
    search(&shard, "kn", &source, &analyzer).expect("search");
    let r3 = rounds.meter().total();
    let _ = S3Op::ALL;
    Cost {
        open_bytes: b1 - b0,
        open_requests: r1 - r0,
        first_bytes: b2 - b1,
        first_requests: r2 - r1,
        depth,
        second_requests: r3 - r2,
        hits: hits
            .hits
            .iter()
            .map(|h| (h.doc, h.score.map(f32::to_bits)))
            .collect(),
        index_bytes,
    }
}

fn knn_body() -> String {
    let vector: Vec<String> = spec_vector(&spec(), 1)
        .expect("vector")
        .iter()
        .map(f32::to_string)
        .collect();
    format!(
        r#"{{"query":{{"knn":{{"v":{{"vector":[{}],"k":10}}}}}},"size":10}}"#,
        vector.join(",")
    )
}

const TERM: &str = r#"{"query":{"term":{"tag":"t3"}},"size":10}"#;

fn needs_demand_paging() -> bool {
    let on = lucene_store::page_cache::demand_paging_available();
    if !on {
        eprintln!("no userfaultfd here: every file is read whole at open, nothing to defer");
    }
    on
}

// docs/specs/reader-vector-warm AC-1
#[test]
fn the_warm_at_open_reads_every_vector_file_at_open() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !needs_demand_paging() {
        return;
    }
    let old = measure(true, TERM);
    assert!(
        old.open_bytes as usize * 10 >= old.index_bytes * 9,
        "{old:?}"
    );
}

// docs/specs/reader-vector-warm AC-2
#[test]
fn a_term_search_of_a_vector_index_reads_no_vector_file() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !needs_demand_paging() {
        return;
    }
    let new = measure(false, TERM);
    let read = new.open_bytes + new.first_bytes;
    assert!(read as usize * 4 < new.index_bytes, "{new:?}");
}

// docs/specs/reader-vector-warm AC-3, AC-4
#[test]
fn a_knn_search_reads_the_same_bytes_in_no_more_requests_or_round_trips() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !needs_demand_paging() {
        return;
    }
    let body = knn_body();
    let old = measure(true, &body);
    let new = measure(false, &body);
    eprintln!("old {old:?}\nnew {new:?}");
    let (old_bytes, new_bytes) = (
        old.open_bytes + old.first_bytes,
        new.open_bytes + new.first_bytes,
    );
    assert!(
        new_bytes * 100 <= old_bytes * 102,
        "bytes {new_bytes} > {old_bytes}\nold {old:?}\nnew {new:?}"
    );
    assert!(
        new.open_requests + new.first_requests <= old.open_requests + old.first_requests,
        "requests\nold {old:?}\nnew {new:?}"
    );
    assert!(new.depth <= old.depth, "depth\nold {old:?}\nnew {new:?}");
    assert_eq!(new.second_requests, 0, "{new:?}");
    assert_eq!(new.hits, old.hits);
    assert_eq!(measure(false, TERM).hits, measure(true, TERM).hits);
}
