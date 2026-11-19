//! docs/specs/coordinator-top-hits-reduce: a top_hits merges across shards.

use oss_coordinator::reduce::hits::{MergeError, SortSpec};
use oss_coordinator::reduce::top_hits::reduce_top_hits;
use oss_reader::aggs::metric::top_hits::TopHitsResult;
use oss_reader::search::{Hit, SortValue};

fn scored(doc: i32, score: f32) -> Hit {
    Hit {
        doc,
        score: Some(score),
        sort: Vec::new(),
    }
}

fn keyed(doc: i32, value: SortValue) -> Hit {
    Hit {
        doc,
        score: None,
        sort: vec![value],
    }
}

/// A shard's result: `all` ranked hits, the first `from` of them `before`.
fn shard(total: u64, max: Option<f32>, from: usize, all: Vec<Hit>) -> TopHitsResult {
    let mut before = all;
    let hits = before.split_off(from.min(before.len()));
    TopHitsResult {
        total,
        max_score: max,
        before,
        hits,
        ..TopHitsResult::empty()
    }
}

#[test]
fn the_page_is_the_best_hits_of_every_shard_after_from() {
    let a = shard(
        9,
        Some(9.0),
        1,
        vec![scored(1, 9.0), scored(2, 5.0), scored(3, 1.0)],
    );
    let b = shard(4, Some(7.0), 1, vec![scored(7, 7.0), scored(8, 6.0)]);
    let merged = reduce_top_hits(&[a, b], 1, 2, None).expect("merges");
    // Merged order: a0 (9), b0 (7), b1 (6), a1 (5), a2 (1); skip one, take two.
    assert_eq!(merged.page, [(1, 0), (1, 1)]);
}

#[test]
fn a_hit_a_shard_ranked_ahead_of_from_can_be_in_the_page() {
    let a = shard(2, Some(2.0), 1, vec![scored(1, 2.0), scored(2, 1.0)]);
    let b = shard(1, Some(9.0), 1, vec![scored(5, 9.0)]);
    // Merged: b0 (9), a0 (2), a1 (1); from 1, size 1 is a0, a shard's `before`.
    let merged = reduce_top_hits(&[a, b], 1, 1, None).expect("merges");
    assert_eq!(merged.page, [(0, 0)]);
}

#[test]
fn a_field_sort_follows_its_direction_and_a_tie_goes_to_the_lower_shard() {
    let sorts = Some(vec![SortSpec {
        descending: true,
        missing_first: false,
    }]);
    let a = shard(
        2,
        None,
        0,
        vec![keyed(1, SortValue::Long(5)), keyed(2, SortValue::Long(1))],
    );
    let b = shard(
        2,
        None,
        0,
        vec![keyed(3, SortValue::Long(5)), keyed(4, SortValue::Long(9))],
    );
    // b's hits are listed in its own order here, not re-sorted by the merge input.
    let merged = reduce_top_hits(&[a, b], 0, 4, sorts).expect("merges");
    assert_eq!(merged.page, [(1, 1), (0, 0), (1, 0), (0, 1)]);
}

#[test]
fn a_missing_value_goes_where_the_sort_puts_it() {
    let hit = |doc, v: Option<&[u8]>| keyed(doc, SortValue::Term(v.map(<[u8]>::to_vec)));
    let a = shard(2, None, 0, vec![hit(1, Some(b"x")), hit(2, None)]);
    let b = shard(1, None, 0, vec![hit(3, Some(b"a"))]);
    let last = Some(vec![SortSpec {
        descending: false,
        missing_first: false,
    }]);
    let merged = reduce_top_hits(&[a.clone(), b.clone()], 0, 3, last).expect("merges");
    assert_eq!(merged.page, [(1, 0), (0, 0), (0, 1)]);
    let first = Some(vec![SortSpec {
        descending: false,
        missing_first: true,
    }]);
    let merged = reduce_top_hits(&[a, b], 0, 3, first).expect("merges");
    assert_eq!(merged.page, [(0, 1), (1, 0), (0, 0)]);
}

#[test]
fn the_total_is_summed_and_the_best_score_is_the_highest() {
    let a = shard(7, Some(2.5), 0, vec![scored(1, 2.5)]);
    let b = shard(5, Some(4.0), 0, vec![scored(2, 4.0)]);
    let c = shard(3, None, 0, Vec::new());
    let merged = reduce_top_hits(&[a, b, c], 0, 3, None).expect("merges");
    assert_eq!((merged.total, merged.max_score), (15, Some(4.0)));
    let none = reduce_top_hits(&[shard(2, None, 0, Vec::new())], 0, 3, None).expect("merges");
    assert_eq!((none.total, none.max_score), (2, None));
}

#[test]
fn sort_values_of_different_kinds_are_refused() {
    let sorts = Some(vec![SortSpec {
        descending: false,
        missing_first: false,
    }]);
    let a = shard(1, None, 0, vec![keyed(1, SortValue::Long(1))]);
    let b = shard(
        1,
        None,
        0,
        vec![keyed(2, SortValue::Term(Some(b"x".to_vec())))],
    );
    assert_eq!(
        reduce_top_hits(&[a, b], 0, 2, sorts).expect_err("refused"),
        MergeError::IncompatibleSortValues
    );
}

#[test]
fn no_shards_or_no_hits_give_an_empty_page() {
    let merged = reduce_top_hits(&[], 0, 3, None).expect("merges");
    assert!(merged.page.is_empty());
    assert_eq!((merged.total, merged.max_score), (0, None));
    let merged = reduce_top_hits(&[TopHitsResult::empty()], 0, 3, None).expect("merges");
    assert!(merged.page.is_empty());
}

#[test]
fn the_pile_never_holds_more_than_twice_the_window() {
    let shards: Vec<TopHitsResult> = (0..20)
        .map(|s| {
            let hits = (0..4).map(|i| scored(i, (s * 4 + i) as f32)).collect();
            shard(4, Some(1.0), 0, hits)
        })
        .collect();
    let merged = reduce_top_hits(&shards, 0, 3, None).expect("merges");
    assert_eq!(merged.page.len(), 3);
    assert!(merged.peak_held <= 2 * 3 + 4);
}

#[test]
fn the_pile_bound_counts_from_as_well_as_size() {
    let (from, size) = (2, 3);
    let shards: Vec<TopHitsResult> = (0..20)
        .map(|s| {
            let hits = (0..5).map(|i| scored(i, (s * 5 + i) as f32)).collect();
            shard(5, Some(1.0), from, hits)
        })
        .collect();
    let merged = reduce_top_hits(&shards, from, size, None).expect("merges");
    assert_eq!(merged.page.len(), size);
    assert!(merged.peak_held <= 2 * (from + size) + 5);
    // The page is the third to fifth best of 100 scores: shard 19's hits 2, 1, 0.
    assert_eq!(merged.page, [(19, 2), (19, 1), (19, 0)]);
}
