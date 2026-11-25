//! `knn` queries over a shard of three segments; compiled only under
//! `#[cfg(test)]`.

use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_codecs::field_infos::VectorSimilarityFunction;
use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs, doc_tag, doc_vector};

use crate::dsl::parse_chunks;
use crate::search::{Hits, ShardError, search};
use crate::{OpenShard, open_shard};

const DOCS: usize = 40;

fn shard(similarity: VectorSimilarityFunction) -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    let spec = Spec::new("kn", DOCS)
        .segments_of(15)
        .with_vectors(similarity);
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", "kn", 0).expect("opens")
}

fn run(shard: &OpenShard<Emulator>, query: &str, rest: &str) -> Result<Hits, ShardError> {
    let body = format!(r#"{{"query":{query}{rest}}}"#);
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "kn", &source, &Analyzer::standard(None))
}

fn knn(params: &str) -> String {
    format!(r#"{{"knn":{{"v":{{{params}}}}}}}"#)
}

fn l2_score(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d: f32 = a.iter().zip(&b).map(|(x, y)| (x - y) * (x - y)).sum();
    1.0 / (1.0 + d)
}

/// The best `k` scores of the documents with a vector, best first.
fn best(target: [f32; 3], k: usize, keep: impl Fn(usize) -> bool) -> Vec<f32> {
    let mut scores: Vec<f32> = (0..DOCS)
        .filter(|&i| keep(i))
        .filter_map(|i| doc_vector(i).map(|v| l2_score(v, target)))
        .collect();
    scores.sort_by(|a, b| b.total_cmp(a));
    scores.truncate(k);
    scores
}

fn scores(hits: &Hits) -> Vec<f32> {
    hits.hits.iter().map(|h| h.score.expect("scored")).collect()
}

fn docs(hits: &Hits) -> Vec<i32> {
    let mut docs: Vec<i32> = hits.hits.iter().map(|h| h.doc).collect();
    docs.sort_unstable();
    docs
}

fn l2() -> OpenShard<Emulator> {
    shard(VectorSimilarityFunction::Euclidean)
}

#[test]
fn the_best_k_of_all_three_segments_come_back_best_first_with_their_scores() {
    let shard = l2();
    let hits = run(&shard, &knn(r#""vector":[3,2,1],"k":6"#), r#","size":20"#).expect("searches");
    assert_eq!(hits.total, 6);
    assert!(hits.exact);
    assert_eq!(scores(&hits), best([3.0, 2.0, 1.0], 6, |_| true));
    // Each hit's score is its own vector's.
    for h in &hits.hits {
        let v = doc_vector(usize::try_from(h.doc).expect("doc")).expect("has a vector");
        assert_eq!(h.score, Some(l2_score(v, [3.0, 2.0, 1.0])));
    }
    assert_eq!(hits.max_score, hits.hits[0].score);
}

#[test]
fn a_k_past_the_vectors_returns_every_document_that_has_one() {
    let shard = l2();
    let hits = run(
        &shard,
        &knn(r#""vector":[0,0,0],"k":100"#),
        r#","size":100"#,
    )
    .expect("ok");
    let with: Vec<i32> = (0..DOCS)
        .filter(|&i| doc_vector(i).is_some())
        .map(|i| i32::try_from(i).expect("fits"))
        .collect();
    assert_eq!(docs(&hits), with);
    assert_eq!(hits.total, u64::try_from(with.len()).expect("fits"));
}

#[test]
fn size_and_from_page_the_k_hits_and_the_total_stays_k() {
    let shard = l2();
    let q = knn(r#""vector":[3,2,1],"k":6"#);
    let all = run(&shard, &q, r#","size":6"#).expect("ok");
    let page = run(&shard, &q, r#","size":2,"from":1"#).expect("ok");
    assert_eq!(page.total, 6);
    assert_eq!(scores(&page), scores(&all)[1..3].to_vec());
}

#[test]
fn a_filter_limits_the_candidates_before_the_k_are_taken() {
    let shard = l2();
    let tag = doc_tag(3);
    let q = knn(&format!(
        r#""vector":[3,2,1],"k":2,"filter":{{"term":{{"tag":"{tag}"}}}}"#
    ));
    let hits = run(&shard, &q, "").expect("ok");
    // Documents 3, 19 and 35 carry the tag; two of the three come back.
    assert_eq!(hits.hits.len(), 2);
    assert_eq!(
        scores(&hits),
        best([3.0, 2.0, 1.0], 2, |i| i % 16 == 3),
        "the best of the tagged documents"
    );
    let none = knn(r#""vector":[3,2,1],"k":2,"filter":{"term":{"tag":"zzz"}}"#);
    assert_eq!(run(&shard, &none, "").expect("ok").total, 0);
}

#[test]
fn a_filter_on_points_is_read_from_the_segments_points() {
    let shard = l2();
    let q = knn(r#""vector":[3,2,1],"k":50,"filter":{"range":{"n":{"gte":10,"lt":20}}}"#);
    let hits = run(&shard, &q, r#","size":50"#).expect("ok");
    let want: Vec<i32> = (10..20)
        .filter(|&i| doc_vector(i).is_some())
        .map(|i| i32::try_from(i).expect("fits"))
        .collect();
    assert_eq!(docs(&hits), want);
}

#[test]
fn a_boost_multiplies_and_a_bool_combines_the_knn_hits_with_other_clauses() {
    let shard = l2();
    let plain = run(&shard, &knn(r#""vector":[3,2,1],"k":3"#), "").expect("ok");
    let boosted = run(&shard, &knn(r#""vector":[3,2,1],"k":3,"boost":2"#), "").expect("ok");
    let doubled: Vec<f32> = scores(&plain).iter().map(|s| s * 2.0).collect();
    assert_eq!(scores(&boosted), doubled);
    // The knn hits and a term's hits, in one `should`.
    let tag = doc_tag(0);
    let q = format!(
        r#"{{"bool":{{"should":[{},{{"term":{{"tag":"{tag}"}}}}]}}}}"#,
        knn(r#""vector":[3,2,1],"k":3"#)
    );
    let both = run(&shard, &q, r#","size":50"#).expect("ok");
    let mut want: Vec<i32> = plain.hits.iter().map(|h| h.doc).collect();
    want.extend([0, 16, 32]);
    want.sort_unstable();
    want.dedup();
    assert_eq!(docs(&both), want);
    // The knn hits as the `must` of a bool with a filter outside it.
    let q = format!(
        r#"{{"bool":{{"must":[{}],"filter":[{{"term":{{"tag":"zzz"}}}}]}}}}"#,
        knn(r#""vector":[3,2,1],"k":3"#)
    );
    assert_eq!(run(&shard, &q, "").expect("ok").total, 0);
}

#[test]
fn each_space_scores_as_the_plugin_does() {
    let q = knn(r#""vector":[3,2,1],"k":40"#);
    let at = |hits: &Hits, doc: i32| {
        hits.hits
            .iter()
            .find(|h| h.doc == doc)
            .and_then(|h| h.score)
            .expect("a hit")
    };
    let v = |i: usize| doc_vector(i).expect("vector");
    let target = [3.0f32, 2.0, 1.0];
    let dot = |a: [f32; 3]| a.iter().zip(&target).map(|(x, y)| x * y).sum::<f32>();
    let norm = |a: [f32; 3]| a.iter().map(|x| x * x).sum::<f32>().sqrt();

    let cos = shard(VectorSimilarityFunction::Cosine);
    let hits = run(&cos, &q, r#","size":40"#).expect("ok");
    let cosine = dot(v(1)) / (norm(v(1)) * norm(target));
    assert!((at(&hits, 1) - (1.0 + cosine) / 2.0).abs() < 1e-6);

    let ip = shard(VectorSimilarityFunction::MaximumInnerProduct);
    let hits = run(&ip, &q, r#","size":40"#).expect("ok");
    // A positive product scores one plus it, a negative one 1 / (1 - it).
    assert!((at(&hits, 1) - (dot(v(1)) + 1.0)).abs() < 1e-6);
    assert!(hits.hits.iter().all(|h| h.score.is_some_and(|s| s > 0.0)));
}

fn refusal(r: Result<Hits, ShardError>) -> String {
    let e = r.expect_err("fails");
    assert_eq!(e.status, 400);
    assert_eq!(e.failure.kind, "query_shard_exception");
    e.failure.reason
}

#[test]
fn a_field_that_is_not_a_vector_and_a_vector_of_the_wrong_length_fail_as_the_plugin_does() {
    let shard = l2();
    let on = |field: &str, vector: &str| {
        refusal(run(
            &shard,
            &format!(r#"{{"knn":{{"{field}":{{"vector":{vector},"k":2}}}}}}"#),
            "",
        ))
    };
    assert_eq!(
        on("tag", "[1,2,3]"),
        "failed to create query: Field 'tag' is not knn_vector type."
    );
    assert_eq!(
        on("nope", "[1,2,3]"),
        "failed to create query: Field 'nope' is not knn_vector type."
    );
    assert_eq!(
        on("v", "[1,2]"),
        "failed to create query: Query vector has invalid dimension: 2. Dimension should be: 3"
    );
    assert_eq!(
        on("v", "[1,2,3,4]"),
        "failed to create query: Query vector has invalid dimension: 4. Dimension should be: 3"
    );
}

#[test]
fn a_knn_query_where_it_is_not_run_is_refused() {
    let shard = l2();
    let q = knn(r#""vector":[3,2,1],"k":2"#);
    let post = run(
        &shard,
        r#"{"match_all":{}}"#,
        &format!(r#","post_filter":{q}"#),
    );
    let e = post.expect_err("refused");
    assert!(
        e.failure.reason.contains("[knn] query here"),
        "{}",
        e.failure.reason
    );
}

#[test]
fn a_query_without_knn_runs_no_vector_search() {
    assert!(
        crate::knn::nodes(
            parse_chunks([br#"{"query":{"match_all":{}}}"#.as_slice()])
                .expect("parses")
                .query
                .as_ref()
                .expect("query")
        )
        .is_empty()
    );
    let shard = l2();
    assert_eq!(
        run(&shard, r#"{"match_all":{}}"#, "").expect("ok").total,
        40
    );
}

#[test]
fn a_tree_built_without_the_searchs_answers_refuses_the_knn_node() {
    use crate::lucene_query::{QueryContext, to_lucene};
    use crate::mapping::Mapping;
    let mapping = Mapping::from_json(r#"{"properties":{"v":{"type":"knn_vector","dimension":3}}}"#)
        .expect("mapping");
    let analyzer = Analyzer::standard(None);
    let cx = QueryContext {
        mapping: &mapping,
        analyzer: &analyzer,
        max_doc: 1,
        knn: None,
        pinned: None,
        now_millis: None,
    };
    let source =
        parse_chunks([format!(r#"{{"query":{}}}"#, knn(r#""vector":[1,2,3],"k":1"#)).as_bytes()])
            .expect("parses");
    let f = to_lucene(source.query.as_ref().expect("query"), &cx).expect_err("refused");
    assert_eq!(
        f.reason,
        "the [knn] query here is not supported by this node yet"
    );
    // A node the search did not run is refused even when others were.
    let empty = crate::knn::KnnResults::default();
    let cx = QueryContext {
        knn: Some(&empty),
        pinned: None,
        ..cx
    };
    assert!(to_lucene(source.query.as_ref().expect("query"), &cx).is_err());
}

#[test]
fn a_mapping_reads_a_vector_fields_dimension_and_only_its() {
    use crate::mapping::{FieldType, Mapping};
    let m = Mapping::from_json(
        r#"{"properties":{"v":{"type":"knn_vector","dimension":4},"w":{"type":"knn_vector"},
        "k":{"type":"keyword","dimension":9}}}"#,
    )
    .expect("mapping");
    assert_eq!(m.get("v").and_then(|f| f.knn.map(|k| k.dimension)), Some(4));
    assert_eq!(m.get("w").and_then(|f| f.knn.map(|k| k.dimension)), None);
    assert_eq!(m.get("k").and_then(|f| f.knn.map(|k| k.dimension)), None);
    assert!(matches!(
        m.get("v").map(|f| &f.ty),
        Some(FieldType::Unsupported(_))
    ));
}

fn all_scores(similarity: VectorSimilarityFunction, target: [f32; 3]) -> Vec<(i32, f32)> {
    let dot = |a: [f32; 3], b: [f32; 3]| a.iter().zip(&b).map(|(x, y)| x * y).sum::<f32>();
    (0..DOCS)
        .filter_map(|i| {
            let v = doc_vector(i)?;
            let score = match similarity {
                VectorSimilarityFunction::Euclidean => l2_score(v, target),
                VectorSimilarityFunction::Cosine => {
                    let norm = |a: [f32; 3]| dot(a, a).sqrt();
                    let cos = if norm(v) == 0.0 {
                        0.0
                    } else {
                        dot(v, target) / (norm(v) * norm(target))
                    };
                    (1.0 + cos) / 2.0
                }
                _ => {
                    let ip = dot(v, target);
                    if ip < 0.0 { 1.0 / (1.0 - ip) } else { ip + 1.0 }
                }
            };
            Some((i32::try_from(i).expect("fits"), score))
        })
        .collect()
}

/// The documents scoring at least `at`, away from every score by more than rounding.
fn at_least(similarity: VectorSimilarityFunction, target: [f32; 3], at: f32) -> Vec<i32> {
    let all = all_scores(similarity, target);
    assert!(
        all.iter().all(|(_, s)| (s - at).abs() > 1e-4),
        "{at} sits on a score"
    );
    all.into_iter()
        .filter(|(_, s)| *s >= at)
        .map(|(d, _)| d)
        .collect()
}

fn radial(similarity: VectorSimilarityFunction, params: &str) -> Vec<i32> {
    let shard = shard(similarity);
    let q = knn(&format!(r#""vector":[3,2,1],{params}"#));
    docs(&run(&shard, &q, r#","size":100"#).expect("searches"))
}

#[test]
fn a_min_score_is_the_lucene_score_a_document_must_reach_in_every_space() {
    use VectorSimilarityFunction::{Cosine, Euclidean, MaximumInnerProduct};
    let target = [3.0, 2.0, 1.0];
    for (similarity, at) in [(Euclidean, 0.16), (Cosine, 0.8), (MaximumInnerProduct, 6.5)] {
        let want = at_least(similarity, target, at);
        assert!(
            !want.is_empty() && want.len() < 32,
            "{similarity:?} {}",
            want.len()
        );
        assert_eq!(
            radial(similarity, &format!(r#""min_score":{at}"#)),
            want,
            "{similarity:?}"
        );
    }
}

#[test]
fn a_max_distance_is_translated_to_a_score_per_space() {
    use VectorSimilarityFunction::{Cosine, Euclidean, MaximumInnerProduct};
    let target = [3.0, 2.0, 1.0];
    // L2: squared distance at most d, so a score of at least 1 / (1 + d).
    assert_eq!(
        radial(Euclidean, r#""max_distance":5.5"#),
        at_least(Euclidean, target, 1.0 / 6.5)
    );
    // Cosine: the distance is one minus the cosine, the score half of two minus it.
    assert_eq!(
        radial(Cosine, r#""max_distance":0.3"#),
        at_least(Cosine, target, 0.85)
    );
    // Inner product: the product a document must reach, scored as the space scores it.
    assert_eq!(
        radial(MaximumInnerProduct, r#""max_distance":9.5"#),
        at_least(MaximumInnerProduct, target, 10.5)
    );
    assert_eq!(
        radial(MaximumInnerProduct, r#""max_distance":-2.5"#).len(),
        32
    );
}

#[test]
fn a_radial_filter_limits_the_documents_and_a_boost_scales_them() {
    let shard = l2();
    let tag = doc_tag(3);
    let q = knn(&format!(
        r#""vector":[3,2,1],"min_score":0.01,"boost":3,"filter":{{"term":{{"tag":"{tag}"}}}}"#
    ));
    let hits = run(&shard, &q, r#","size":100"#).expect("ok");
    assert_eq!(docs(&hits), vec![3, 35], "19 has no vector");
    let v = doc_vector(3).expect("vector");
    let first = hits
        .hits
        .iter()
        .find(|h| h.doc == 3)
        .and_then(|h| h.score)
        .expect("hit");
    assert_eq!(first, l2_score(v, [3.0, 2.0, 1.0]) * 3.0);
}

#[test]
fn the_space_refuses_a_threshold_it_cannot_score() {
    let on = |similarity, params: &str| {
        refusal(run(
            &shard(similarity),
            &knn(&format!(r#""vector":[3,2,1],{params}"#)),
            "",
        ))
    };
    use VectorSimilarityFunction::{Cosine, Euclidean, MaximumInnerProduct};
    assert_eq!(
        on(Euclidean, r#""max_distance":-1"#),
        "failed to create query: [knn] requires distance to be non-negative for space type: L2"
    );
    assert_eq!(
        on(Cosine, r#""max_distance":-1"#),
        "failed to create query: [knn] requires distance to be non-negative for space type: COSINESIMIL"
    );
    assert_eq!(
        on(Euclidean, r#""min_score":1.5"#),
        "failed to create query: [knn] requires score to be in the range [0, 1] for space type: L2"
    );
    assert_eq!(
        on(Cosine, r#""min_score":1.5"#),
        "failed to create query: [knn] requires score to be in the range [0, 1] for space type: COSINESIMIL"
    );
    // The inner product's scores are not bounded by one, nor its distances by zero.
    let shard = shard(MaximumInnerProduct);
    for params in [r#""min_score":1.5"#, r#""max_distance":-1"#] {
        let q = knn(&format!(r#""vector":[3,2,1],{params}"#));
        assert!(run(&shard, &q, "").is_ok(), "{params}");
    }
}

#[test]
fn a_radial_query_on_a_field_that_is_not_a_vector_fails_as_the_plugin_does() {
    let shard = l2();
    let q = r#"{"knn":{"tag":{"vector":[1,2,3],"min_score":0.5}}}"#;
    assert_eq!(
        refusal(run(&shard, q, "")),
        "failed to create query: Field 'tag' is not knn_vector type."
    );
}

#[test]
fn a_space_is_read_from_the_method_or_the_field_and_defaults_to_l2() {
    use crate::mapping::{Mapping, Space};
    let m = Mapping::from_json(
        r#"{"properties":{"a":{"type":"knn_vector","dimension":2,"method":{"engine":"lucene","space_type":"cosinesimil","parameters":{"m":8}}},
        "b":{"type":"knn_vector","dimension":2,"space_type":"innerproduct"},
        "c":{"type":"knn_vector","dimension":2},
        "d":{"type":"knn_vector","dimension":2,"method":{"name":"hnsw"}}}}"#,
    )
    .expect("mapping");
    let space = |f: &str| m.get(f).and_then(|f| f.knn).map(|k| k.space);
    assert_eq!(space("a"), Some(Space::CosineSimil));
    assert_eq!(space("b"), Some(Space::InnerProduct));
    assert_eq!(space("c"), Some(Space::L2));
    assert_eq!(space("d"), Some(Space::L2));
    assert_eq!(
        (
            Space::L2.name(),
            Space::CosineSimil.name(),
            Space::InnerProduct.name()
        ),
        ("L2", "COSINESIMIL", "INNER_PRODUCT")
    );
}
