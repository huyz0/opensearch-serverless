//! Document framing: one, concatenated, and NDJSON, with byte ranges.
// ARITH: test inputs are small and fixed; an overflow would be a test bug, and debug builds panic on it.
#![allow(missing_docs, clippy::arithmetic_side_effects)]

mod common;

use chunkjson::Framing;
use common::{config, parse_chunked};
use serde_json::json;

#[test]
fn ndjson_documents_end_with_their_byte_ranges() {
    let body = b"{\"index\":{\"_id\":\"1\"}}\r\n\n{\"title\":\"a\"}\n  \n7\ntrue\n\"s\"";
    for sizes in [&[1][..], &[usize::MAX], &[3, 5, 7]] {
        let b = parse_chunked(body, sizes, config(Framing::Ndjson)).unwrap();
        assert_eq!(
            b.docs,
            vec![
                json!({"index": {"_id": "1"}}),
                json!({"title": "a"}),
                json!(7),
                json!(true),
                json!("s")
            ]
        );
        let texts: Vec<&[u8]> = b
            .ranges
            .iter()
            .map(|&(s, e)| &body[s as usize..e as usize])
            .collect();
        assert_eq!(
            texts,
            vec![
                &b"{\"index\":{\"_id\":\"1\"}}"[..],
                b"{\"title\":\"a\"}",
                b"7",
                b"true",
                b"\"s\""
            ]
        );
    }
}

#[test]
fn ndjson_and_concatenated_accept_empty_input() {
    for framing in [Framing::Ndjson, Framing::Concatenated] {
        for body in [&b""[..], b"\n\n", b" \r\n"] {
            assert!(
                parse_chunked(body, &[1], config(framing))
                    .unwrap()
                    .docs
                    .is_empty()
            );
        }
    }
}

#[test]
fn concatenated_documents_may_span_lines_and_share_one() {
    let body = b"{\n  \"a\": [1,\n 2]\n}\n{\"b\":null} [] 3 \"x\"";
    let b = parse_chunked(body, &[2], config(Framing::Concatenated)).unwrap();
    assert_eq!(
        b.docs,
        vec![
            json!({"a": [1, 2]}),
            json!({"b": null}),
            json!([]),
            json!(3),
            json!("x")
        ]
    );
    assert_eq!(b.ranges[0], (0, 18));
    assert_eq!(b.ranges[3], (33, 34));
}

#[test]
fn single_document_range_excludes_whitespace() {
    let b = parse_chunked(b"  {\"a\":1}  ", &[1], config(Framing::Single)).unwrap();
    assert_eq!(b.ranges, vec![(2, 9)]);
}

#[test]
fn a_string_splits_into_fragments_and_escapes_decode() {
    let body = r#"{"k\"ey":"ab\ncdé\/","e":""}"#.as_bytes();
    let b = parse_chunked(body, &[usize::MAX], config(Framing::Single)).unwrap();
    assert_eq!(b.docs, vec![json!({"k\"ey": "ab\ncdé/", "e": ""})]);
    let bytewise = parse_chunked(body, &[1], config(Framing::Single)).unwrap();
    assert_eq!(bytewise.docs, b.docs);
    assert!(
        bytewise.longest_fragment <= 2,
        "a fragment never outgrows its slice"
    );
}
