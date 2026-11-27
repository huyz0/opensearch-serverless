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
use lucene_search::{BooleanQuery, Clause};

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

#[test]
fn a_nested_query_scores_the_fields_of_its_child_query() {
    use lucene_search::join::{
        BitSetProducer, QueryBitSetProducer, ScoreMode, ToParentBlockJoinQuery,
    };
    use std::sync::Arc;
    let parents: Arc<dyn BitSetProducer> = Arc::new(QueryBitSetProducer::new(must(term("tag"))));
    let nested: Clause = ToParentBlockJoinQuery::new(
        Clause::Boolean(Box::new(must(term("c.w")))),
        parents,
        ScoreMode::Avg,
    )
    .into();
    assert_eq!(scored_fields(&must(nested)), vec!["c.w".to_owned()]);
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

#[test]
fn terminate_after_keeps_the_first_matches_in_index_order() {
    let shard = text_shard();
    let plain = run(&shard, "").expect("searches");
    assert_eq!(plain.terminated_early, None);
    let mut by_doc: Vec<_> = plain.hits.iter().map(|h| (h.doc, h.score)).collect();
    by_doc.sort_by_key(|(doc, _)| *doc);
    let first_two = &by_doc[..2];
    let cut = run(&shard, r#","terminate_after":2"#).expect("searches");
    assert_eq!(cut.terminated_early, Some(true));
    assert_eq!((cut.total, cut.exact), (2, true));
    let mut kept: Vec<_> = cut.hits.iter().map(|h| (h.doc, h.score)).collect();
    kept.sort_by_key(|(doc, _)| *doc);
    assert_eq!(kept, first_two);
    // Ranked by score as an ordinary search ranks them, with no sort values.
    assert!(cut.hits.windows(2).all(|w| w[0].score >= w[1].score));
    assert!(cut.hits.iter().all(|h| h.sort.is_empty()));
    assert_eq!(cut.max_score, cut.hits[0].score);
}

/// Stock marks a size 0 search with an aggregation `terminated_early`, whatever
/// else it holds, unless it has a `post_filter` or `min_score`.
#[test]
fn a_size_zero_search_with_an_aggregation_is_terminated_early_as_stock_marks_it() {
    let shard = text_shard();
    let agg = r#""aggs":{"m":{"max":{"field":"n"}}}"#;
    let early = |body: String| {
        let source = parse_chunks([body.as_bytes()]).expect("parses");
        search(&shard, "idx", &source, &Analyzer::standard(None))
            .expect("searches")
            .terminated_early
    };
    assert_eq!(early(format!(r#"{{"size":0,{agg}}}"#)), Some(true));
    // A term that matches nothing is still collected; a query that cannot be
    // counted from the index is not.
    let none = format!(r#"{{"size":0,"query":{{"term":{{"x0":"zzzzzz"}}}},{agg}}}"#);
    assert_eq!(early(none), Some(true));
    let two = format!(r#"{{"size":0,"query":{{"match":{{"x0":"w1 u1"}}}},{agg}}}"#);
    assert_eq!(early(two), None);
    // A pipeline aggregation alone counts as an aggregation.
    let pipe = r#"{"size":0,"aggs":{"t":{"terms":{"field":"n"}},"p":{"max_bucket":{"buckets_path":"t>_count"}}}}"#;
    assert_eq!(early(pipe.to_owned()), Some(true));
    // Not without an aggregation, not with hits asked for, not past a post filter.
    assert_eq!(early(r#"{"size":0}"#.to_owned()), None);
    assert_eq!(early(format!(r#"{{"size":1,{agg}}}"#)), None);
    let filtered = format!(r#"{{"size":0,"post_filter":{{"match_all":{{}}}},{agg}}}"#);
    assert_eq!(early(filtered), None);
}

#[test]
fn terminate_after_past_the_matches_collects_them_all_and_does_not_stop() {
    let shard = text_shard();
    let plain = run(&shard, "").expect("searches");
    let all = run(&shard, r#","terminate_after":1000"#).expect("searches");
    assert_eq!(all.terminated_early, Some(false));
    assert_eq!(all.total, plain.total);
    let docs = |h: &Hits| h.hits.iter().map(|h| (h.doc, h.score)).collect::<Vec<_>>();
    assert_eq!(docs(&all), docs(&plain));
    // Zero is off, as OpenSearch's default.
    let off = run(&shard, r#","terminate_after":0"#).expect("searches");
    assert_eq!(off.terminated_early, None);
    assert_eq!(docs(&off), docs(&plain));
}

#[test]
fn terminate_after_parses_a_non_negative_number() {
    let n = parse_chunks([br#"{"terminate_after":"7"}"#.as_slice()]).expect("parses");
    assert_eq!(n.terminate_after, 7);
    assert_eq!(SearchSource::default().terminate_after, 0);
    let e = parse_chunks([br#"{"terminate_after":-1}"#.as_slice()]).expect_err("negative");
    assert!(
        format!("{e:?}").contains("terminateAfter must be > 0"),
        "{e:?}"
    );
    assert!(parse_chunks([br#"{"terminate_after":null}"#.as_slice()]).is_err());
}

#[test]
fn terminate_after_is_refused_where_it_cannot_be_served() {
    let shard = text_shard();
    for (rest, what) in [
        (r#","terminate_after":2,"size":0"#, "size"),
        (r#","terminate_after":2,"min_score":0.1"#, "min_score"),
        (
            r#","terminate_after":2,"track_scores":true"#,
            "track_scores",
        ),
        (r#","terminate_after":2,"sort":[{"n":"asc"}]"#, "sort"),
        (
            r#","terminate_after":2,"track_total_hits":1"#,
            "track_total_hits",
        ),
        (
            r#","terminate_after":2,"aggs":{"a":{"max":{"field":"n"}}}"#,
            "aggregations",
        ),
    ] {
        let reason = refused(run(&shard, rest));
        assert!(
            reason.contains("[terminate_after]") && reason.contains(what),
            "{rest}: {reason}"
        );
    }
    for query in [
        r#"{"match_all":{}}"#,
        r#"{"bool":{"filter":[{"term":{"x0":"w1"}}]}}"#,
    ] {
        let body = format!(r#"{{"query":{query},"terminate_after":2}}"#);
        let source = parse_chunks([body.as_bytes()]).expect("parses");
        let reason = refused(search(&shard, "idx", &source, &Analyzer::standard(None)));
        assert!(reason.contains("[terminate_after]"), "{query}: {reason}");
    }
}

fn search_body(shard: &OpenShard<Emulator>, body: &str) -> Result<Hits, ShardError> {
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "idx", &source, &Analyzer::standard(None))
}

fn docs_and_scores(h: &Hits) -> Vec<(i32, Option<f32>)> {
    h.hits.iter().map(|h| (h.doc, h.score)).collect()
}

#[test]
fn post_filter_narrows_the_hits_and_total_without_changing_scores() {
    let shard = text_shard();
    let plain = run(&shard, "").expect("searches");
    let filtered = run(&shard, r#","post_filter":{"match":{"x0":"w1"}}"#).expect("searches");
    let as_filter = search_body(
        &shard,
        r#"{"query":{"bool":{"must":{"match":{"x0":"w1 u1"}},"filter":{"match":{"x0":"w1"}}}}}"#,
    )
    .expect("searches");
    // The same hits, scores and total as the query with the filter inside it.
    assert_eq!(docs_and_scores(&filtered), docs_and_scores(&as_filter));
    assert_eq!((filtered.total, filtered.exact), (as_filter.total, true));
    assert_eq!(filtered.max_score, as_filter.max_score);
    // It narrows, and what it keeps scored as in the plain search.
    assert!(!filtered.hits.is_empty());
    assert!(filtered.hits.len() < plain.hits.len());
    for hit in &filtered.hits {
        let at = plain.hits.iter().find(|p| p.doc == hit.doc).expect("a hit");
        assert_eq!(hit.score, at.score);
    }
    // A filter nothing satisfies leaves nothing.
    let none = run(&shard, r#","post_filter":{"match":{"x0":"nosuchword"}}"#).expect("searches");
    assert!(none.hits.is_empty());
    assert_eq!(none.total, 0);
}

#[test]
fn post_filter_leaves_the_aggregations_to_the_whole_query() {
    let shard = text_shard();
    let aggs = r#","aggs":{"top":{"max":{"field":"n"}},"n":{"value_count":{"field":"n"}}}"#;
    let plain = run(&shard, aggs).expect("searches");
    let filtered = run(
        &shard,
        &format!(r#"{aggs},"post_filter":{{"match":{{"x0":"w1"}}}}"#),
    )
    .expect("searches");
    assert!(filtered.hits.len() < plain.hits.len());
    assert_eq!(
        format!("{:?}", filtered.aggregations),
        format!("{:?}", plain.aggregations)
    );
}

#[test]
fn post_filter_parses_one_query_and_is_refused_where_it_cannot_be_served() {
    let on = parse_chunks([br#"{"post_filter":{"match_all":{}}}"#.as_slice()]).expect("parses");
    assert!(on.post_filter.is_some());
    assert!(SearchSource::default().post_filter.is_none());
    assert!(parse_chunks([br#"{"post_filter":{"nosuch":{}}}"#.as_slice()]).is_err());
    let shard = text_shard();
    let filter = r#","post_filter":{"match_all":{}}"#;
    for (rest, what) in [
        (r#","min_score":0.1"#, "min_score"),
        (r#","terminate_after":2"#, "terminate_after"),
        (r#","explain":true"#, "explain"),
        (
            r#","track_scores":true,"sort":[{"n":"asc"}]"#,
            "track_scores",
        ),
    ] {
        let reason = refused(run(&shard, &format!("{rest}{filter}")));
        assert!(
            reason.contains("[post_filter]") && reason.contains(what),
            "{rest}: {reason}"
        );
    }
}

#[path = "search_decimal_tests.rs"]
mod decimal_tests;

#[path = "search_half_tests.rs"]
mod half_tests;

#[path = "search_scaled_tests.rs"]
mod scaled_tests;
