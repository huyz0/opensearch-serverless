//! `ignore_unmapped` over a real shard; compiled only under `#[cfg(test)]`.

use lucene_analysis::Analyzer;

use super::tests::l2;
use crate::dsl::parse_chunks;
use crate::search::{Hits, ShardError, search};

fn run(query: &str) -> Result<Hits, ShardError> {
    let shard = l2();
    let body = format!(r#"{{"query":{query},"size":100}}"#);
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(&shard, "kn", &source, &Analyzer::standard(None))
}

fn on(field: &str, params: &str) -> String {
    format!(r#"{{"knn":{{"{field}":{{"vector":[1,2,3],{params}}}}}}}"#)
}

#[test]
fn an_unmapped_field_matches_nothing_when_ignored_and_fails_when_not() {
    for mode in [r#""k":3"#, r#""min_score":0.5"#, r#""max_distance":2.0"#] {
        let hits = run(&on("nope", &format!(r#"{mode},"ignore_unmapped":true"#))).expect("ok");
        assert_eq!(hits.total, 0, "{mode}");
        let e =
            run(&on("nope", &format!(r#"{mode},"ignore_unmapped":false"#))).expect_err("refused");
        assert!(
            e.failure.reason.contains("is not knn_vector type"),
            "{}",
            e.failure.reason
        );
    }
}

#[test]
fn a_mapped_field_that_is_not_a_vector_still_fails_when_unmapped_is_ignored() {
    let e = run(&on("tag", r#""k":3,"ignore_unmapped":true"#)).expect_err("refused");
    assert!(
        e.failure.reason.contains("is not knn_vector type"),
        "{}",
        e.failure.reason
    );
}

#[test]
fn an_ignored_unmapped_clause_leaves_the_rest_of_the_bool_to_answer() {
    let q = format!(
        r#"{{"bool":{{"must":[{{"match_all":{{}}}}],"should":[{}]}}}}"#,
        on("nope", r#""k":3,"ignore_unmapped":true"#)
    );
    let all = run(r#"{"match_all":{}}"#).expect("ok").total;
    assert!(all > 0);
    assert_eq!(run(&q).expect("ok").total, all);
}

#[test]
fn a_mapped_vector_field_is_searched_whatever_ignore_unmapped_says() {
    let hits = run(&on("v", r#""k":3,"ignore_unmapped":true"#)).expect("ok");
    assert_eq!(hits.total, 3);
}
