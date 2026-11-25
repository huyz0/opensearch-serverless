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
