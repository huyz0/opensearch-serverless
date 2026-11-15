//! The tokenizer against serde_json as the oracle, under random chunking.
// ARITH: test inputs are small and fixed; an overflow would be a test bug, and debug builds panic on it.
#![allow(missing_docs, clippy::arithmetic_side_effects)]

mod common;

use common::parse_one;
use proptest::prelude::*;
use serde_json::{Map, Number, Value};

fn leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        any::<f64>().prop_filter_map("finite", |f| Number::from_f64(f).map(Value::Number)),
        any::<String>().prop_map(Value::String),
        "[\\x00-\\x1f\"\\\\/a-zé€😀]{0,12}".prop_map(Value::String),
    ]
}

fn value() -> impl Strategy<Value = Value> {
    leaf().prop_recursive(6, 64, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            prop::collection::vec((any::<String>(), inner), 0..8)
                .prop_map(|kv| Value::Object(kv.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

fn sizes() -> impl Strategy<Value = Vec<usize>> {
    prop_oneof![
        Just(vec![1]),
        Just(vec![usize::MAX]),
        prop::collection::vec(1usize..40, 1..16)
    ]
}

/// Bytes worth inserting: structure, escapes, UTF-8 leads and continuations.
const INTERESTING: &[u8] = b"{}[]:,\"\\/ubfnrt0123456789-+.eE \t\n\r\x00\x1f\x7f\xc3\xa9\xe2\x82\xac\xf0\x9f\xed\xa0\xbf\xff";

#[derive(Debug, Clone)]
enum Edit {
    Replace(usize, u8),
    Insert(usize, u8),
    Delete(usize),
}

fn edits() -> impl Strategy<Value = Vec<Edit>> {
    let byte = prop::sample::select(INTERESTING);
    let one = prop_oneof![
        (any::<usize>(), byte.clone()).prop_map(|(i, b)| Edit::Replace(i, b)),
        (any::<usize>(), byte).prop_map(|(i, b)| Edit::Insert(i, b)),
        any::<usize>().prop_map(Edit::Delete),
    ];
    prop::collection::vec(one, 1..4)
}

fn apply(mut bytes: Vec<u8>, edits: &[Edit]) -> Vec<u8> {
    for e in edits {
        let n = bytes.len();
        match *e {
            Edit::Replace(i, b) if n > 0 => bytes[i % n] = b,
            Edit::Insert(i, b) => bytes.insert(i % (n + 1), b),
            Edit::Delete(i) if n > 0 => {
                bytes.remove(i % n);
            }
            _ => {}
        }
    }
    bytes
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    #[test]
    fn events_rebuild_what_serde_json_parses(v in value(), pretty in any::<bool>(), sizes in sizes()) {
        let text = if pretty { serde_json::to_vec_pretty(&v) } else { serde_json::to_vec(&v) }.unwrap();
        let expected: Value = serde_json::from_slice(&text).unwrap();
        prop_assert_eq!(parse_one(&text, &sizes).unwrap(), expected);
    }

    #[test]
    fn accepts_exactly_what_serde_json_accepts(v in value(), edits in edits(), sizes in sizes()) {
        let bytes = apply(serde_json::to_vec(&v).unwrap(), &edits);
        let oracle = serde_json::from_slice::<Value>(&bytes);
        if let Err(e) = &oracle {
            prop_assume!(!e.to_string().contains("number out of range"));
        }
        let ours = parse_one(&bytes, &sizes);
        prop_assert_eq!(ours.is_ok(), oracle.is_ok(), "ours {:?} oracle {:?} input {:?}", ours, oracle, String::from_utf8_lossy(&bytes));
        if let (Ok(a), Ok(b)) = (ours, oracle) {
            prop_assert_eq!(a, b);
        }
    }

    #[test]
    fn arbitrary_short_inputs_agree_with_serde_json(bytes in prop::collection::vec(prop::sample::select(INTERESTING), 0..24)) {
        let oracle = serde_json::from_slice::<Value>(&bytes);
        if let Err(e) = &oracle {
            prop_assume!(!e.to_string().contains("number out of range"));
        }
        let ours = parse_one(&bytes, &[1]);
        prop_assert_eq!(ours.is_ok(), oracle.is_ok(), "ours {:?} oracle {:?}", ours, oracle);
    }
}

#[test]
fn numbers_outside_f64_range_pass_through_as_text() {
    assert!(serde_json::from_slice::<Value>(b"1e400").is_err());
    let mut tok = chunkjson::Tokenizer::new(chunkjson::Config::default());
    let input = b"1e400";
    let (event, used) = tok.next(input, true).unwrap();
    assert_eq!(event, chunkjson::Event::Number("1e400"));
    assert_eq!(used, 5);
}
