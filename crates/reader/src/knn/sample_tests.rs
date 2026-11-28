//! The vector sample's tests; compiled only under `#[cfg(test)]`.

use std::sync::Arc;

use lucene_codecs::field_infos::VectorSimilarityFunction;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs, doc_vector};

use super::sample::{Reservoir, sample_shard};
use crate::{OpenShard, open_shard};

const DOCS: usize = 40;

fn shard() -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    let spec = Spec::new("kn", DOCS)
        .segments_of(15)
        .with_vectors(VectorSimilarityFunction::Euclidean);
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", "kn", 0).expect("opens")
}

fn rows(sample: &[f32]) -> Vec<[f32; 3]> {
    sample.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()
}

#[test]
fn a_reservoir_keeps_everything_while_it_has_room() {
    let mut r = Reservoir::new(2, 4, 1);
    for i in 0..3 {
        r.offer(&[i as f32, 0.0]);
    }
    assert_eq!(r.seen(), 3);
    assert_eq!(r.into_sample(), [0.0, 0.0, 1.0, 0.0, 2.0, 0.0]);
}

#[test]
fn a_full_reservoir_holds_its_cap_and_draws_from_all_it_saw() {
    let mut r = Reservoir::new(1, 10, 9);
    for i in 0..1000 {
        r.offer(&[i as f32]);
    }
    assert_eq!(r.seen(), 1000);
    let sample = r.into_sample();
    assert_eq!(sample.len(), 10);
    assert!(sample.iter().any(|&v| v >= 500.0), "{sample:?}");
    let mut sorted = sample.clone();
    sorted.sort_by(f32::total_cmp);
    sorted.dedup();
    assert_eq!(sorted.len(), 10, "a vector was kept twice");
}

#[test]
fn the_same_seed_draws_the_same_sample() {
    let draw = |seed| {
        let mut r = Reservoir::new(1, 5, seed);
        (0..200).for_each(|i| r.offer(&[i as f32]));
        r.into_sample()
    };
    assert_eq!(draw(3), draw(3));
    assert_ne!(draw(3), draw(4));
}

#[test]
fn every_vector_of_a_shard_is_offered_once_across_its_segments() {
    let shard = shard();
    let mut r = Reservoir::new(3, 100, 1);
    sample_shard(&shard, "v", &mut r).unwrap();
    let want: Vec<[f32; 3]> = (0..DOCS).filter_map(doc_vector).collect();
    assert_eq!(r.seen(), want.len() as u64);
    let mut got = rows(&r.into_sample());
    let mut want = want;
    got.sort_by(|a, b| a.partial_cmp(b).unwrap());
    want.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(got, want);
}

#[test]
fn a_cap_below_the_vector_count_keeps_that_many() {
    let shard = shard();
    let mut r = Reservoir::new(3, 7, 1);
    sample_shard(&shard, "v", &mut r).unwrap();
    assert_eq!(r.into_sample().len(), 7 * 3);
}

#[test]
fn a_wrong_dimension_and_an_unknown_field_are_told_apart() {
    let shard = shard();
    let mut wrong = Reservoir::new(5, 10, 1);
    let err = sample_shard(&shard, "v", &mut wrong).unwrap_err();
    assert!(err.reason.contains("dimension"), "{}", err.reason);
    let mut none = Reservoir::new(3, 10, 1);
    sample_shard(&shard, "nope", &mut none).unwrap();
    assert_eq!(none.seen(), 0);
}
