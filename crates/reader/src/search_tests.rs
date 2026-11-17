//! Tests of the search module; compiled only under `#[cfg(test)]`.

use super::*;

#[test]
fn a_count_past_the_tracking_limit_is_reported_at_the_limit() {
    let exact = TotalHitsRelation::EqualTo;
    let at_least = TotalHitsRelation::GreaterThanOrEqualTo;
    assert_eq!(reported(0, exact), (0, true));
    assert_eq!(reported(TRACK_TOTAL_HITS, exact), (TRACK_TOTAL_HITS, true));
    assert_eq!(
        reported(TRACK_TOTAL_HITS, at_least),
        (TRACK_TOTAL_HITS, false)
    );
    assert_eq!(
        reported(TRACK_TOTAL_HITS + 1, exact),
        (TRACK_TOTAL_HITS, false)
    );
    assert_eq!(reported(12_000, at_least), (TRACK_TOTAL_HITS, false));
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
