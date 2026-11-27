//! An `sltr` rescore against a plain search and a plain query rescore, over
//! 5,000 documents with a 1,000-hit window and 50 features. Its own binary,
//! because the resolver is installed process-wide.
//!
//! `cargo test -p oss-reader --release --test ltr_bench -- --ignored --nocapture`
//! prints the timings (provisional: in-process emulator, one machine). The
//! request count is a plain test: after one search has warmed the shard, an
//! `sltr` rescore makes no more object-store requests than a plain search.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::cast_precision_loss)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use lucene_analysis::Analyzer;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_reader::dsl::parse_chunks;
use oss_reader::ltr::Evaluator;
use oss_reader::ltr::models::{Feature, FeatureSet, Models, ResolveError, Resolved, install};
use oss_reader::search::search;
use oss_reader::{OpenShard, open_shard};
use oss_testkit::index::{Spec, blobs};

const FEATURES: usize = 50;
const DOCS: usize = 5_000;

struct Fixed;

fn features() -> Vec<Feature> {
    (0..FEATURES)
        .map(|i| Feature {
            name: format!("f{i}"),
            params: Vec::new(),
            template: format!(r#"{{"term":{{"x0":"w{i}"}}}}"#),
        })
        .collect()
}

impl Models for Fixed {
    fn resolve(&self, _: Option<&str>, _: &str) -> Result<Arc<Resolved>, ResolveError> {
        let features = features();
        let names: Vec<String> = features.iter().map(|f| f.name.clone()).collect();
        let weights: Vec<String> = names.iter().map(|n| format!(r#""{n}":1.0"#)).collect();
        let definition = format!("{{{}}}", weights.join(","));
        let evaluator = Evaluator::parse("model/linear", &definition, &names).expect("parses");
        Ok(Arc::new(Resolved {
            evaluator,
            features,
        }))
    }

    fn feature_set(&self, _: Option<&str>, _: &str) -> Result<Arc<FeatureSet>, ResolveError> {
        Ok(Arc::new(FeatureSet {
            features: features(),
        }))
    }

    fn max_window(&self) -> usize {
        1_000
    }
}

fn shard() -> (OpenShard<Emulator>, Arc<PooledStore<Emulator>>) {
    install(Arc::new(Fixed));
    let spec = Spec::new("ltrbench", DOCS).segments_of(1_000).with_texts(1);
    let config = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = Arc::new(PooledStore::new(Emulator::new(256 << 20, 1), buffers));
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    let shard = open_shard(Arc::clone(&store), cache, "", &spec.name, 0).expect("opens");
    (shard, store)
}

const FIRST: &str = r#""size":1000,"query":{"match_all":{}}"#;

fn bodies() -> [(&'static str, String); 3] {
    [
        ("plain search", format!("{{{FIRST}}}")),
        (
            "one query rescore",
            format!(
                r#"{{{FIRST},"rescore":{{"window_size":1000,"query":{{"rescore_query":{{"term":{{"x0":"w3"}}}}}}}}}}"#
            ),
        ),
        (
            "sltr, 50 features",
            format!(
                r#"{{{FIRST},"rescore":{{"window_size":1000,"query":{{"rescore_query":{{"sltr":{{"model":"m"}}}}}}}}}}"#
            ),
        ),
    ]
}

fn run(shard: &OpenShard<Emulator>, body: &str) -> usize {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let hits = search(shard, "idx", &source, &Analyzer::standard(None)).expect("searches");
    hits.hits.len()
}

#[test]
fn an_sltr_rescore_makes_no_more_requests_than_a_plain_search_once_warm() {
    let (shard, store) = shard();
    let [(_, plain), _, (_, sltr)] = bodies();
    assert_eq!(run(&shard, &plain), 1_000);
    let before = store.store().meter().total();
    run(&shard, &plain);
    let plain_cost = store.store().meter().total() - before;
    run(&shard, &sltr);
    let before = store.store().meter().total();
    assert_eq!(run(&shard, &sltr), 1_000);
    let sltr_cost = store.store().meter().total() - before;
    assert_eq!(sltr_cost, plain_cost, "a warm sltr rescore");
}

#[test]
#[ignore = "a timing run; prints its numbers"]
fn timings() {
    let (shard, _) = shard();
    for (name, body) in bodies() {
        for _ in 0..3 {
            run(&shard, &body);
        }
        let mut each: Vec<Duration> = (0..21)
            .map(|_| {
                let t = Instant::now();
                run(&shard, &body);
                t.elapsed()
            })
            .collect();
        each.sort();
        println!(
            "{name:20} median {:>9.2?}  p90 {:>9.2?}",
            each[10], each[18]
        );
    }
}
