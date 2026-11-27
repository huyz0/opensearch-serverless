use oss_transport::actions::search_shard::{Bounds, Request};

use super::convert::{ConvertError, search_source};

const FIXTURES: &str = include_str!("../../../../fixtures/search-shard/java-3.10.0.txt");

fn read(name: &str) -> oss_transport::actions::search_shard::Source {
    let hex = FIXTURES
        .lines()
        .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
        .expect("a fixture");
    let bytes: Vec<u8> = hex
        .as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16).expect("hex"))
        .collect();
    Request::read(&bytes, &Bounds::DEFAULT)
        .expect("a request")
        .source
}

fn source(name: &str) -> oss_reader::dsl::SearchSource {
    search_source(&read(name)).expect("a search source")
}

#[test]
fn every_fixture_request_becomes_a_source_the_reader_accepts_or_is_refused_as_named() {
    let mut accepted = 0;
    for name in [
        "match_all",
        "no_query",
        "term",
        "term_long",
        "terms",
        "range",
        "range_format",
        "bool",
        "ids",
        "exists",
        "options",
    ] {
        match search_source(&read(&format!("request.{name}"))) {
            Ok(s) => {
                assert!(s.version && s.seq_no_primary_term, "{name}");
                accepted += 1;
            }
            Err(ConvertError::Named) => {}
            // The reader does not run a range's `format` yet.
            Err(ConvertError::Parse(_)) if name == "range_format" => {}
            Err(e) => panic!("{name}: {e:?}"),
        }
    }
    assert!(accepted >= 6, "{accepted}");
}

#[test]
fn the_options_of_a_request_reach_the_source() {
    let s = source("request.options");
    assert_eq!((s.from, s.size), (3, 5));
    assert!(s.min_score.is_some());
    assert!(s.terminate_after > 0);
}

#[test]
fn a_named_query_is_refused_for_the_answer_cannot_say_which_matched() {
    let mut named = read("request.term");
    if let Some(node) = named.query.as_mut() {
        node.name = Some("q".to_owned());
    }
    assert!(matches!(search_source(&named), Err(ConvertError::Named)));
}

#[test]
fn the_aggregations_of_a_request_reach_the_source() {
    use oss_transport::actions::search_shard::{AggKind, AggRequest};
    let mut with = read("request.match_all");
    with.aggregations = vec![
        AggRequest {
            name: "hi".to_owned(),
            kind: AggKind::Max,
            field: "n".to_owned(),
            precision_threshold: None,
            percentiles: None,
        },
        AggRequest {
            name: "c".to_owned(),
            kind: AggKind::ValueCount,
            field: "tag".to_owned(),
            precision_threshold: None,
            percentiles: None,
        },
    ];
    let s = search_source(&with).expect("a search source");
    let names: Vec<(&str, &str)> = s
        .aggs
        .iter()
        .map(|a| (a.name.as_str(), a.field.as_str()))
        .collect();
    assert_eq!(names, [("hi", "n"), ("c", "tag")]);
}

#[test]
fn a_precision_threshold_reaches_the_source() {
    use oss_transport::actions::search_shard::{AggKind, AggRequest};
    let mut with = read("request.match_all");
    with.aggregations = vec![AggRequest {
        name: "u".to_owned(),
        kind: AggKind::Cardinality,
        field: "tag".to_owned(),
        precision_threshold: Some(100),
        percentiles: None,
    }];
    let s = search_source(&with).expect("a search source");
    assert_eq!(s.aggs[0].cardinality.precision_threshold, Some(100));
}

#[test]
fn percentile_keys_and_options_reach_the_source() {
    use oss_transport::actions::search_shard::{AggKind, AggRequest, PercentilesRequest};
    let mut with = read("request.match_all");
    let ask = |name: &str, kind, keys: &[f64]| AggRequest {
        name: name.to_owned(),
        kind,
        field: "n".to_owned(),
        precision_threshold: None,
        percentiles: Some(PercentilesRequest {
            keys: keys.to_vec(),
            keyed: false,
            compression: Some(200.0),
        }),
    };
    with.aggregations = vec![
        ask("p", AggKind::Percentiles, &[50.0, 99.5]),
        ask("r", AggKind::PercentileRanks, &[10.0]),
    ];
    let s = search_source(&with).expect("a search source");
    let got: Vec<_> = s
        .aggs
        .iter()
        .map(|a| {
            (
                a.percentiles.keys.clone(),
                a.percentiles.keyed,
                a.percentiles.compression,
            )
        })
        .collect();
    assert_eq!(
        got,
        [(vec![50.0, 99.5], false, 200.0), (vec![10.0], false, 200.0)]
    );
}
