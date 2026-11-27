//! The forwarded shard search against the bytes the Java shell writes
//! (`fixtures/search-shard/java-3.10.0.txt`).

use oss_transport::actions::search_shard::*;
use oss_transport::error::WireError;
use oss_transport::stream::Output;

const FIXTURES: &str = include_str!("../../../fixtures/search-shard/java-3.10.0.txt");

fn bytes(name: &str) -> Vec<u8> {
    let hex = FIXTURES
        .lines()
        .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
        .unwrap_or_else(|| panic!("no fixture {name}"));
    hex.as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).expect("ascii"), 16).expect("hex"))
        .collect()
}

fn read(name: &str) -> Result<Request, Refusal> {
    Request::read(&bytes(&format!("request.{name}")), &Bounds::DEFAULT)
}

fn node(query: Query) -> Node {
    Node {
        query,
        boost: 1.0,
        name: None,
    }
}

fn s(text: &str) -> String {
    text.to_owned()
}

#[test]
fn match_all_reads_with_the_request_around_it() {
    let r = read("match_all").expect("reads");
    assert_eq!(r.index, "idx");
    assert_eq!(r.shard, 2);
    assert_eq!(r.index_uuid.as_deref(), Some("uuid-1"));
    assert_eq!(r.now_millis, 1_700_000_000_000);
    assert_eq!(r.source.query, Some(node(Query::MatchAll)));
    assert_eq!((r.source.from, r.source.size), (-1, -1));
}

#[test]
fn a_request_with_no_query_has_none() {
    assert_eq!(read("no_query").expect("reads").source.query, None);
}

#[test]
fn term_keeps_its_boost_name_and_value() {
    let r = read("term").expect("reads");
    assert_eq!(
        r.source.query,
        Some(Node {
            query: Query::Term {
                field: s("n"),
                value: Value::Str(s("a")),
                case_insensitive: false
            },
            boost: 2.0,
            name: Some(s("q")),
        })
    );
    let long = read("term_long").expect("reads");
    assert!(matches!(
        long.source.query,
        Some(Node {
            query: Query::Term {
                value: Value::Long(7),
                ..
            },
            ..
        })
    ));
}

#[test]
fn terms_range_ids_and_exists_read() {
    assert_eq!(
        read("terms").expect("reads").source.query,
        Some(node(Query::Terms {
            field: s("tag"),
            values: vec![Value::Str(s("a")), Value::Str(s("b"))]
        }))
    );
    assert_eq!(
        read("range").expect("reads").source.query,
        Some(node(Query::Range {
            field: s("n"),
            from: Value::Long(3),
            to: Value::Long(9),
            include_lower: true,
            include_upper: false,
            time_zone: None,
            format: None,
            relation: None
        }))
    );
    assert_eq!(
        read("ids").expect("reads").source.query,
        Some(node(Query::Ids(vec![s("a"), s("b"), s("c")])))
    );
    assert_eq!(
        read("exists").expect("reads").source.query,
        Some(node(Query::Exists(s("f"))))
    );
}

#[test]
fn a_range_keeps_its_format_and_a_missing_bound() {
    let r = read("range_format").expect("reads");
    let Some(Node {
        query:
            Query::Range {
                from,
                to,
                include_lower,
                format,
                ..
            },
        ..
    }) = r.source.query
    else {
        panic!("a range")
    };
    assert_eq!(from, Value::Str(s("now-1h")));
    assert_eq!(to, Value::Null);
    assert!(!include_lower);
    assert_eq!(format.as_deref(), Some("epoch_millis"));
}

#[test]
fn bool_keeps_every_clause_and_minimum_should_match() {
    let r = read("bool").expect("reads");
    let Some(Node {
        query:
            Query::Bool {
                must,
                must_not,
                should,
                filter,
                adjust_pure_negative,
                minimum_should_match,
            },
        ..
    }) = r.source.query
    else {
        panic!("a bool")
    };
    assert_eq!(
        (must.len(), must_not.len(), should.len(), filter.len()),
        (1, 1, 1, 1)
    );
    assert!(adjust_pure_negative);
    assert_eq!(minimum_should_match.as_deref(), Some("1"));
    assert_eq!(must_not[0].query, Query::Ids(vec![s("1"), s("2")]));
}

#[test]
fn the_options_of_a_search_are_read() {
    let src = read("options").expect("reads").source;
    assert_eq!((src.from, src.size), (3, 5));
    assert_eq!(src.track_total_hits_up_to, Some(100));
    assert_eq!(src.min_score, Some(0.5));
    assert_eq!(src.terminate_after, 7);
    assert_eq!(src.timeout_millis, Some(2_000));
}

#[test]
fn a_sort_is_refused_by_name() {
    assert_eq!(read("sorted"), Err(Refusal::Unsupported("sort")));
}

#[test]
fn the_first_unsupported_item_is_named_and_nothing_after_it_is_read() {
    // The recording's `terms` aggregation is one this node runs, and the
    // `from` after it is read.
    let ok = read("aggs_then_from").expect("reads");
    assert_eq!((ok.source.from, ok.source.aggregations.len()), (4, 1));
    // The same bytes with the type renamed to `other`, five letters as well:
    // an aggregation this node does not run comes first, and the `from` after
    // it is never reached.
    let mut wire = bytes("request.aggs_then_from");
    let at = wire
        .windows(5)
        .position(|w| w == b"terms")
        .expect("the type");
    wire[at..at + 5].copy_from_slice(b"other");
    assert_eq!(
        Request::read(&wire, &Bounds::DEFAULT),
        Err(Refusal::Unsupported("an aggregation of another type"))
    );
}

#[test]
fn a_query_this_node_does_not_run_is_refused_by_its_name() {
    assert_eq!(read("match"), Err(Refusal::UnsupportedQuery(s("match"))));
}

#[test]
fn a_request_over_a_limit_is_refused_and_one_at_it_is_not() {
    let whole = bytes("request.match_all");
    let at = Bounds {
        bytes: whole.len(),
        ..Bounds::DEFAULT
    };
    assert!(Request::read(&whole, &at).is_ok());
    let under = Bounds {
        bytes: whole.len() - 1,
        ..Bounds::DEFAULT
    };
    assert_eq!(
        Request::read(&whole, &under),
        Err(Refusal::TooLarge("request bytes"))
    );

    let bool_bytes = bytes("request.bool");
    // The bool holds three levels at most: itself and one clause.
    let deep = |depth| Bounds {
        query: Limits {
            depth,
            clauses: 1_024,
        },
        ..Bounds::DEFAULT
    };
    assert!(Request::read(&bool_bytes, &deep(2)).is_ok());
    assert_eq!(
        Request::read(&bool_bytes, &deep(1)),
        Err(Refusal::TooLarge("query nesting"))
    );
    let few = |clauses| Bounds {
        query: Limits { depth: 16, clauses },
        ..Bounds::DEFAULT
    };
    assert!(Request::read(&bool_bytes, &few(5)).is_ok());
    assert_eq!(
        Request::read(&bool_bytes, &few(4)),
        Err(Refusal::TooLarge("query clauses"))
    );

    let options = bytes("request.options");
    let window = |window| Bounds {
        window,
        ..Bounds::DEFAULT
    };
    assert!(Request::read(&options, &window(8)).is_ok());
    assert_eq!(
        Request::read(&options, &window(7)),
        Err(Refusal::TooLarge("from plus size"))
    );
}

#[test]
fn truncated_bytes_are_a_wire_error_not_a_panic() {
    let whole = bytes("request.bool");
    for cut in 0..whole.len() {
        assert!(
            Request::read(&whole[..cut], &Bounds::DEFAULT).is_err(),
            "cut at {cut}"
        );
    }
}

fn written(hits: &[Hit<'_>], total: i64) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut out = Output::new(&mut buf, MAX_BYTES);
    Response {
        total,
        total_is_lower_bound: false,
        max_score: f32::NAN,
        timed_out: false,
        terminated_early: None,
    }
    .write(hits, &[], &mut out)
    .expect("writes");
    buf
}

#[test]
fn an_empty_answer_is_what_the_java_shell_writes() {
    assert_eq!(written(&[], 0), bytes("response.empty"));
}

#[test]
fn an_answer_with_hits_is_what_the_java_shell_writes_byte_for_byte() {
    let hits = [
        Hit {
            id: "a",
            score: 1.5,
            version: 3,
            seq_no: 7,
            primary_term: 2,
            source: b"{\"n\":1}",
        },
        Hit {
            id: "b",
            score: 0.25,
            version: 3,
            seq_no: 8,
            primary_term: 2,
            source: b"{\"n\":2}",
        },
    ];
    assert_eq!(written(&hits, 42), bytes("response.two_hits"));
}

#[test]
fn an_answer_past_the_limit_is_refused_whole() {
    let mut buf = Vec::new();
    let mut out = Output::new(&mut buf, 16);
    let hit = Hit {
        id: "a",
        score: 1.0,
        version: 1,
        seq_no: 0,
        primary_term: 1,
        source: &[b'x'; 64],
    };
    let answer = Response {
        total: 1,
        total_is_lower_bound: false,
        max_score: 1.0,
        timed_out: false,
        terminated_early: None,
    };
    assert!(matches!(
        answer.write(&[hit], &[], &mut out),
        Err(WireError::TooLarge { .. })
    ));
}

fn request_bytes(request: &Request) -> Result<Vec<u8>, Refusal> {
    let mut buf = Vec::new();
    request.write(&mut Output::new(&mut buf, 1 << 20))?;
    Ok(buf)
}

#[test]
fn a_request_the_java_shell_wrote_is_written_back_byte_for_byte() {
    // Every fixture whose query the writer encodes (match-all, or none): the
    // bytes are the Java writer's, the options one included.
    let mut equal = Vec::new();
    for name in ["match_all", "no_query", "options"] {
        let request = read(name).expect("reads");
        assert_eq!(
            request_bytes(&request).expect("writes"),
            bytes(&format!("request.{name}")),
            "request.{name}"
        );
        equal.push(name);
    }
    assert_eq!(equal.len(), 3);
}

#[test]
fn a_request_with_another_query_is_refused_by_the_writer() {
    let request = read("term").expect("reads");
    assert!(matches!(
        request_bytes(&request),
        Err(Refusal::UnsupportedQuery(_))
    ));
}

#[test]
fn a_named_match_all_is_refused_by_the_writer() {
    // The writer writes the name as absent, so a name would be dropped.
    let mut request = read("match_all").expect("reads");
    request.source.query.as_mut().expect("a query").name = Some(s("q"));
    assert!(matches!(
        request_bytes(&request),
        Err(Refusal::UnsupportedQuery(_))
    ));
}

#[test]
fn a_written_request_reads_back_the_same() {
    let mut request = read("no_query").expect("reads");
    request.index = s("logs");
    request.shard = 3;
    request.index_uuid = None;
    request.now_millis = 42;
    request.source.size = 17;
    request.source.track_total_hits_up_to = Some(1000);
    let back =
        Request::read(&request_bytes(&request).expect("writes"), &Bounds::DEFAULT).expect("reads");
    assert_eq!(back, request);
}
