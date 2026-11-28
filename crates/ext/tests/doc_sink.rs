//! The `index.docs` seam (docs/specs/plugin-system, Host API 1.0): its
//! bytes are bounded and strict, and the backend routes it to a sink.

use std::sync::{Arc, Mutex};

use oss_ext::doc_sink::{
    Doc, DocResult, DocSink, DocSinkBackend, MAX_CALL_BYTES, MAX_DOCS, MAX_NAME_BYTES,
    MAX_SOURCE_BYTES, decode_call, decode_results, encode_call, encode_results,
};
use oss_ext::host::{Backend, CallHost, Host, HostError, NoBackend, Who};
use oss_ext::rest::CodecError;
use oss_ext::{Budget, Capability};

fn doc(id: Option<&str>, source: &str) -> Doc {
    Doc {
        id: id.map(str::to_owned),
        source: source.as_bytes().to_vec(),
    }
}

#[test]
fn a_call_round_trips_and_has_golden_bytes() {
    let docs = vec![doc(Some("a"), "{}"), doc(None, "{\"x\":1}")];
    let bytes = encode_call("logs", &docs);
    let mut want = vec![1, 4, 0];
    want.extend_from_slice(b"logs");
    want.extend_from_slice(&[2, 0]);
    want.extend_from_slice(&[1, 1, 0, b'a', 2, 0, 0, 0, b'{', b'}']);
    want.extend_from_slice(&[0, 7, 0, 0, 0]);
    want.extend_from_slice(b"{\"x\":1}");
    assert_eq!(bytes, want);
    assert_eq!(decode_call(&bytes), Ok(("logs".to_owned(), docs)));
}

#[test]
fn an_answer_round_trips_and_has_golden_bytes() {
    let results = vec![
        DocResult {
            id: "a".into(),
            status: 201,
        },
        DocResult {
            id: "b".into(),
            status: 409,
        },
    ];
    let bytes = encode_results(&results);
    assert_eq!(
        bytes,
        vec![1, 2, 0, 1, 0, b'a', 201, 0, 1, 0, b'b', 0x99, 0x01]
    );
    assert_eq!(decode_results(&bytes), Ok(results));
}

#[test]
fn malformed_calls_are_refused() {
    let good = encode_call("i", &[doc(Some("a"), "{}")]);
    let mut v = good.clone();
    v[0] = 2;
    assert_eq!(decode_call(&v), Err(CodecError::Version));
    assert_eq!(decode_call(&[]), Err(CodecError::Short));
    assert_eq!(decode_call(&good[..good.len() - 1]), Err(CodecError::Short));
    let mut t = good.clone();
    t.push(0);
    assert_eq!(decode_call(&t), Err(CodecError::Trailing));
    let mut flag = good.clone();
    flag[6] = 7;
    assert_eq!(decode_call(&flag), Err(CodecError::Value));
    let mut text = good;
    text[3] = 0xff;
    assert_eq!(decode_call(&text), Err(CodecError::Text));
    assert_eq!(decode_results(&[1]), Err(CodecError::Short));
    assert_eq!(decode_results(&[2, 0, 0]), Err(CodecError::Version));
    assert_eq!(decode_results(&[1, 0, 0, 9]), Err(CodecError::Trailing));
}

#[test]
fn each_bound_is_inclusive() {
    let many = |n: usize| encode_call("i", &vec![doc(None, "{}"); n]);
    assert!(decode_call(&many(MAX_DOCS)).is_ok(), "1000 documents");
    assert_eq!(decode_call(&many(MAX_DOCS + 1)), Err(CodecError::Bound));
    let sized = |n: usize| {
        encode_call(
            "i",
            &[Doc {
                id: None,
                source: vec![b' '; n],
            }],
        )
    };
    assert!(
        decode_call(&sized(MAX_SOURCE_BYTES)).is_ok(),
        "one source at the ceiling"
    );
    assert_eq!(
        decode_call(&sized(MAX_SOURCE_BYTES + 1)),
        Err(CodecError::Bound)
    );
    let named = |n: usize| encode_call(&"n".repeat(n), &[]);
    assert!(
        decode_call(&named(MAX_NAME_BYTES)).is_ok(),
        "a name at the ceiling"
    );
    assert_eq!(
        decode_call(&named(MAX_NAME_BYTES + 1)),
        Err(CodecError::Bound)
    );
    let id = |n: usize| encode_call("i", &[doc(Some(&"d".repeat(n)), "{}")]);
    assert!(
        decode_call(&id(MAX_NAME_BYTES)).is_ok(),
        "an id at the ceiling"
    );
    assert_eq!(decode_call(&id(MAX_NAME_BYTES + 1)), Err(CodecError::Bound));
    // Eight sources of exactly 1 MiB are the whole call; one byte more is over.
    let whole = |extra: usize| {
        let mut docs = vec![
            Doc {
                id: None,
                source: vec![b' '; MAX_SOURCE_BYTES]
            };
            MAX_CALL_BYTES / MAX_SOURCE_BYTES
        ];
        docs.push(Doc {
            id: None,
            source: vec![b' '; extra],
        });
        encode_call("i", &docs)
    };
    assert!(decode_call(&whole(0)).is_ok(), "exactly the call ceiling");
    assert_eq!(decode_call(&whole(1)), Err(CodecError::Bound));
}

struct Sink(Mutex<Vec<(String, usize)>>);

impl DocSink for Sink {
    fn index(&self, index: &str, docs: &[Doc]) -> Result<Vec<DocResult>, HostError> {
        self.0
            .lock()
            .expect("log")
            .push((index.to_owned(), docs.len()));
        Ok(docs
            .iter()
            .enumerate()
            .map(|(i, d)| DocResult {
                id: d.id.clone().unwrap_or_else(|| format!("gen{i}")),
                status: 201,
            })
            .collect())
    }
}

struct Rest;

impl Backend for Rest {
    fn call(&self, _who: &Who<'_>, op: &str, _input: &[u8]) -> Result<Vec<u8>, HostError> {
        Ok(format!("rest:{op}").into_bytes())
    }
}

#[test]
fn the_backend_serves_index_docs_from_the_sink_and_the_rest_from_its_neighbour() {
    let sink = Arc::new(Sink(Mutex::new(Vec::new())));
    let backend = DocSinkBackend::new(sink.clone(), Arc::new(Rest));
    let grants = [Capability::IndexWrite, Capability::Identity];
    let who = Who {
        plugin: "p",
        tenant: "t",
        grants: &grants,
    };
    let budget = Budget {
        index_ops: 2,
        ..Budget::default()
    };
    let host = CallHost::new(who, budget, &backend);
    let call = encode_call("alerts", &[doc(Some("a"), "{}"), doc(None, "{}")]);
    let out = host.call("index.docs", &call, 1 << 16).expect("indexes");
    assert_eq!(
        decode_results(&out),
        Ok(vec![
            DocResult {
                id: "a".into(),
                status: 201
            },
            DocResult {
                id: "gen1".into(),
                status: 201
            },
        ])
    );
    assert_eq!(*sink.0.lock().expect("log"), vec![("alerts".to_owned(), 2)]);
    assert_eq!(
        host.call("identity", b"", 64),
        Ok(b"rest:identity".to_vec())
    );
    assert_eq!(
        host.call("index.docs", b"junk", 64),
        Err(HostError::Invalid)
    );
    assert_eq!(
        host.call("index.docs", &call, 1 << 16).map(|_| ()),
        Err(HostError::Budget),
        "the budget is two"
    );
}

#[test]
fn index_docs_needs_its_capability_and_a_budget() {
    let sink = Arc::new(Sink(Mutex::new(Vec::new())));
    let backend = DocSinkBackend::new(sink.clone(), Arc::new(NoBackend));
    let call = encode_call("i", &[doc(None, "{}")]);
    let none: [Capability; 0] = [];
    let denied = CallHost::new(
        Who {
            plugin: "p",
            tenant: "",
            grants: &none,
        },
        Budget {
            index_ops: 9,
            ..Budget::default()
        },
        &backend,
    );
    assert_eq!(denied.call("index.docs", &call, 64), Err(HostError::Denied));
    let grants = [Capability::IndexWrite];
    let zero = CallHost::new(
        Who {
            plugin: "p",
            tenant: "",
            grants: &grants,
        },
        Budget::default(),
        &backend,
    );
    assert_eq!(zero.call("index.docs", &call, 64), Err(HostError::Budget));
    assert!(
        sink.0.lock().expect("log").is_empty(),
        "a refused call never reaches the sink"
    );
}
