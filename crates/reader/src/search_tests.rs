//! Tests of the search module; compiled only under `#[cfg(test)]`.

use super::*;

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs};

use crate::dsl::parse_chunks;
use crate::{OpenShard, open_shard};

#[test]
fn a_count_past_the_tracking_limit_is_reported_at_the_limit() {
    let reported_at = |n, r| reported(n, r, TRACK_TOTAL_HITS);
    let exact = TotalHitsRelation::EqualTo;
    let at_least = TotalHitsRelation::GreaterThanOrEqualTo;
    assert_eq!(reported_at(0, exact), (0, true));
    assert_eq!(
        reported_at(TRACK_TOTAL_HITS, exact),
        (TRACK_TOTAL_HITS, true)
    );
    assert_eq!(
        reported_at(TRACK_TOTAL_HITS, at_least),
        (TRACK_TOTAL_HITS, false)
    );
    assert_eq!(
        reported_at(TRACK_TOTAL_HITS + 1, exact),
        (TRACK_TOTAL_HITS, false)
    );
    assert_eq!(reported_at(12_000, at_least), (TRACK_TOTAL_HITS, false));
}

fn points() -> Clause {
    Clause::Extended(Box::new(
        lucene_search::extended_query::ExtendedQuery::PointInSet(
            lucene_search::extended_query::PointInSetQuery::int_set("year", &[1]).expect("set"),
        ),
    ))
}

fn term(field: &str) -> Clause {
    Clause::Term(lucene_search::TermQuery::new(field, b"a".to_vec()))
}

fn must(c: Clause) -> BooleanQuery {
    BooleanQuery {
        must: vec![c],
        ..BooleanQuery::default()
    }
}

fn dis_max(clauses: Vec<Clause>) -> Clause {
    Clause::DisjunctionMax(Box::new(lucene_search::DisjunctionMaxQuery::new(
        clauses, 0.0,
    )))
}

#[test]
fn points_are_opened_only_for_a_query_that_reads_them() {
    assert!(!has_points(&must(term("tag"))));
    assert!(!has_points(&must(dis_max(vec![
        term("tag"),
        term("title")
    ]))));
    assert!(has_points(&must(points())));
    assert!(has_points(&must(dis_max(vec![term("tag"), points()]))));
    assert!(has_points(&must(Clause::Boolean(Box::new(must(points()))))));
    assert!(has_points(&must(Clause::Boost(Box::new(
        lucene_search::BoostQuery::new(points(), 2.0)
    )))));
    assert!(has_points(&must(Clause::ConstantScore(Box::new(
        lucene_search::ConstantScoreQuery::new(points(), 1.0)
    )))));
}

#[test]
fn the_scored_fields_are_each_term_s_field_once() {
    let fuzzy = Clause::Fuzzy(lucene_search::FuzzyQuery {
        field: "title".into(),
        term: b"a".to_vec(),
        max_edits: 1,
        prefix_length: 0,
        transpositions: true,
        max_expansions: 50,
    });
    assert_eq!(
        scored_fields(&must(dis_max(vec![term("tag"), fuzzy, term("tag")]))),
        vec!["tag".to_owned(), "title".to_owned()]
    );
    assert!(scored_fields(&must(points())).is_empty());
    let nested = Clause::Boolean(Box::new(must(Clause::Boost(Box::new(
        lucene_search::BoostQuery::new(term("year"), 2.0),
    )))));
    assert_eq!(scored_fields(&must(nested)), vec!["year".to_owned()]);
}

#[test]
fn a_function_score_scores_the_fields_of_its_wrapped_query() {
    use lucene_search::extended_query::ExtendedQuery;
    use lucene_search::function::FunctionScoreQuery;
    let source = lucene_search::values_source::constant(1.0);
    let wrapped = Clause::Extended(Box::new(ExtendedQuery::FunctionScore(
        FunctionScoreQuery::new(term("title"), source),
    )));
    assert_eq!(scored_fields(&must(wrapped)), vec!["title".to_owned()]);
}

fn text_shard() -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    let spec = Spec::new("ts", 30).segments_of(10).with_texts(1);
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", "ts", 0).expect("opens")
}

/// A search of the text field's two words, which four documents match with
/// two different scores, and `rest` of the body.
fn run(shard: &OpenShard<Emulator>, rest: &str) -> Result<Hits, ShardError> {
    let body = format!(r#"{{"query":{{"match":{{"x0":"w1 u1"}}}}{rest}}}"#);
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None))
}

fn refused(r: Result<Hits, ShardError>) -> String {
    let e = r.expect_err("refused");
    assert_eq!(e.status, 400);
    e.failure.reason
}

#[test]
fn track_scores_scores_a_field_sorts_hits_and_tracks_the_best() {
    let shard = text_shard();
    let plain = run(&shard, "").expect("searches");
    assert_eq!(plain.hits.len(), 4);
    let score_of = |hits: &Hits, doc: i32| {
        hits.hits
            .iter()
            .find(|h| h.doc == doc)
            .map(|h| h.score)
            .expect("a hit")
    };
    // A field sort scores nothing, and has no best score, without the key.
    let untracked = run(&shard, r#","sort":[{"n":"asc"}]"#).expect("searches");
    assert!(untracked.hits.iter().all(|h| h.score.is_none()));
    assert_eq!(untracked.max_score, None);
    // With it each hit has its score of the plain search and the best is tracked.
    let tracked = run(&shard, r#","sort":[{"n":"asc"}],"track_scores":true"#).expect("searches");
    assert_eq!(tracked.hits.len(), 4);
    for hit in &tracked.hits {
        assert!(hit.score.is_some());
        assert_eq!(hit.score, score_of(&plain, hit.doc));
    }
    assert!(plain.max_score.is_some());
    assert_eq!(tracked.max_score, plain.max_score);
    // False is the plain field sort.
    let off = run(&shard, r#","sort":[{"n":"asc"}],"track_scores":false"#).expect("searches");
    assert!(off.hits.iter().all(|h| h.score.is_none()));
    assert_eq!(off.max_score, None);
}

#[test]
fn a_tracked_field_sort_that_matches_nothing_has_no_best_score() {
    let shard = text_shard();
    let body =
        r#"{"query":{"match":{"x0":"nosuchword"}},"sort":[{"n":"asc"}],"track_scores":true}"#;
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let hits = search(&shard, "idx", &source, &Analyzer::standard(None)).expect("searches");
    assert!(hits.hits.is_empty());
    assert_eq!(hits.max_score, None);
}

#[test]
fn track_scores_is_refused_where_it_cannot_be_served() {
    let shard = text_shard();
    for sort in [r#"[{"_score":"asc"}]"#, r#"["_score",{"n":"asc"}]"#] {
        let reason = refused(run(
            &shard,
            &format!(r#","sort":{sort},"track_scores":true"#),
        ));
        assert!(reason.contains("led by the score"), "{reason}");
    }
    let range = r#"{"query":{"range":{"n":{"gte":1}}},"track_scores":true,"sort":[{"n":"asc"}]}"#;
    let source = parse_chunks([range.as_bytes()]).expect("parses");
    let reason = refused(search(&shard, "idx", &source, &Analyzer::standard(None)));
    assert!(reason.contains("range or other points query"), "{reason}");
    // A score key after a field key already scores the hits: no explanation needed.
    let keyed =
        r#"{"query":{"range":{"n":{"gte":1}}},"track_scores":true,"sort":[{"n":"asc"},"_score"]}"#;
    let source = parse_chunks([keyed.as_bytes()]).expect("parses");
    let hits = search(&shard, "idx", &source, &Analyzer::standard(None)).expect("searches");
    assert!(hits.hits.iter().all(|h| h.score.is_some()));
}

#[test]
fn track_scores_parses_as_a_boolean() {
    let on = parse_chunks([br#"{"track_scores":true}"#.as_slice()]).expect("parses");
    assert!(on.track_scores);
    let off = parse_chunks([br#"{"track_scores":"false"}"#.as_slice()]).expect("parses");
    assert!(!off.track_scores);
    assert!(parse_chunks([br#"{"track_scores":null}"#.as_slice()]).is_err());
}
