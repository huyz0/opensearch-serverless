//! The fast ingest path against the complete path: for each document the documents they give
//! are equal, and a document the plan declines is the complete path's alone.

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use lucene_index::index_writer::{ExplicitDocument, IndexWriter, InvertedField};
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_doc::document;
use oss_doc::fast::FastIngest;
use oss_doc::lucene::{Indexer, Meta, uid};
use oss_doc::mapping::Mapping;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

/// A writer and the indexer numbering for it.
struct Side<'d> {
    writer: IndexWriter<'d>,
    indexer: Indexer,
    mapping: Mapping,
}

fn side<'d>(dir: &'d ByteBuffersDirectory, mapping: &str) -> Side<'d> {
    let mut writer = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    writer.enable_explicit_documents().expect("explicit");
    Side {
        writer,
        indexer: Indexer::new(),
        mapping: Mapping::parse(mapping.as_bytes()).expect("mapping"),
    }
}

impl Side<'_> {
    /// The complete path.
    fn slow(&mut self, id: &str, seq: i64, source: &[u8]) -> Option<ExplicitDocument> {
        let parsed = document::parse(&self.mapping, id, source).ok()?;
        if let Some(grown) = &parsed.grown {
            self.mapping = grown.clone();
        }
        let meta = Meta {
            id,
            seq_no: seq,
            primary_term: 1,
            version: 1,
            source,
        };
        Some(
            self.indexer
                .document(&mut self.writer, &self.mapping, &parsed, &meta)
                .expect("document"),
        )
    }
}

/// One entry list, ordered by field number (entries of one field keep their order).
fn by_number<T: Clone>(v: &[T], number: impl Fn(&T) -> i32) -> Vec<T> {
    let mut out = v.to_vec();
    out.sort_by_key(|e| number(e));
    out
}

/// A field's terms in term order; a term without positions is a document-only occurrence, whose
/// frequency the segment does not keep.
fn terms_of(f: &InvertedField) -> Vec<String> {
    let mut out: Vec<String> = f
        .terms
        .iter()
        .map(|t| {
            let freq = if t.positions.is_empty() { 0 } else { t.freq };
            format!(
                "{:?} freq {freq} pos {:?} off {:?} pay {}",
                t.term,
                t.positions,
                t.offsets,
                t.payloads.len()
            )
        })
        .collect();
    out.sort();
    out
}

fn canonical(d: &ExplicitDocument) -> String {
    let stored = by_number(&d.stored, |s| s.field_number);
    let inv = by_number(&d.fields.inverted, |s| s.field_number);
    let dv = by_number(&d.fields.doc_values, |s| s.field_number);
    let pt = by_number(&d.fields.points, |s| s.field_number);
    let inverted: Vec<String> = inv
        .iter()
        .map(|f| format!("{} norm {:?} {:?}", f.field_number, f.norm, terms_of(f)))
        .collect();
    let mut registered = d.fields.registered.clone();
    registered.sort_unstable();
    format!(
        "stored {stored:?}\ninverted {inverted:?}\ndv {dv:?}\npoints {pt:?}\nregistered {registered:?}"
    )
}

/// Indexes `sources` on both sides; whether each was read by the plan.
fn run(mapping: &str, sources: &[&str]) -> Vec<bool> {
    let (d1, d2) = (ByteBuffersDirectory::new(), ByteBuffersDirectory::new());
    let (mut slow, mut fast) = (side(&d1, mapping), side(&d2, mapping));
    let mut path = FastIngest::new();
    let mut taken = Vec::new();
    for (i, source) in sources.iter().enumerate() {
        let (id, seq) = (format!("id{i}"), i as i64);
        let key = uid(&id).expect("uid");
        let Some(want) = slow.slow(&id, seq, source.as_bytes()) else {
            // The complete path refuses it: the plan must leave it to the complete path.
            assert!(
                path.prepare(&fast.indexer, &fast.mapping, source.as_bytes(), &key)
                    .is_none(),
                "document {i} is an error and the plan read it: {source}"
            );
            taken.push(false);
            continue;
        };
        let got = match path.prepare(&fast.indexer, &fast.mapping, source.as_bytes(), &key) {
            Some(f) => {
                taken.push(true);
                f.finish(seq, 1, 1).pop().expect("one document")
            }
            None => {
                taken.push(false);
                let before = fast.mapping.clone();
                let d = fast
                    .slow(&id, seq, source.as_bytes())
                    .expect("same as slow");
                if fast.mapping != before {
                    path.invalidate();
                }
                d
            }
        };
        let (got, want) = (canonical(&got), canonical(&want));
        if got != want {
            let first = got.lines().zip(want.lines()).position(|(a, b)| a != b);
            panic!(
                "document {i}: {source}\nfirst differing section: {:?}\nfast: {}\nslow: {}",
                first.map(|l| got.lines().nth(l).map(|x| &x[..x.len().min(600)])),
                got.lines().nth(first.unwrap_or(0)).unwrap_or(""),
                want.lines().nth(first.unwrap_or(0)).unwrap_or("")
            );
        }
    }
    taken
}

const LOGS: &str = r#"{"properties":{
  "@timestamp":{"type":"date"},"level":{"type":"keyword"},"host":{"type":"keyword"},
  "status":{"type":"integer"},"bytes":{"type":"long"},"message":{"type":"text"}}}"#;

fn log(i: usize) -> String {
    format!(
        r#"{{"@timestamp":"2026-11-20T08:{:02}:{:02}.000Z","level":"info","host":"web-{}","status":{},"bytes":{},"message":"GET /api/v1/items/{} returned ok after {} ms"}}"#,
        (i / 60) % 60,
        i % 60,
        i % 50,
        200 + (i % 5),
        i * 13,
        i,
        i % 900
    )
}

#[test]
fn log_documents_equal_the_complete_path() {
    let docs: Vec<String> = (0..40).map(log).collect();
    let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
    let taken = run(LOGS, &refs);
    // The first document numbers the fields; the rest are read by the plan.
    assert!(!taken[0]);
    assert!(taken[1..].iter().all(|&t| t), "{taken:?}");
}

const WIDE: &str = r#"{"properties":{
  "t":{"type":"text","fields":{"raw":{"type":"keyword","ignore_above":5}}},
  "u":{"type":"text","position_increment_gap":10,"norms":false,"index_options":"offsets"},
  "k":{"type":"keyword"}, "kd":{"type":"keyword","doc_values":false},
  "l":{"type":"long"}, "i":{"type":"integer"}, "f":{"type":"float"}, "d":{"type":"double"},
  "b":{"type":"boolean"}, "dt":{"type":"date"}, "ln":{"type":"long","index":false},
  "o":{"properties":{"a":{"type":"keyword"},"n":{"type":"long"},"in":{"properties":{"x":{"type":"text"}}}}}
}}"#;

const WIDE_DOCS: &[&str] = &[
    // The first document numbers the fields the later ones use.
    r#"{"t":"Hello World","u":"a b","k":"x","kd":"y","l":1,"i":2,"f":1.5,"d":2.25,"b":true,"dt":"2026-01-02T03:04:05Z","ln":7,"o":{"a":"p","n":3,"in":{"x":"deep text"}}}"#,
    r#"{"t":"Hello World","u":"a b","k":"x","kd":"y","l":1,"i":2,"f":1.5,"d":2.25,"b":false,"dt":"2026-01-02T03:04:05Z","ln":7,"o":{"a":"p","n":3,"in":{"x":"deep text"}}}"#,
    // Arrays of scalars and of objects, nulls, an empty object, repeated values.
    r#"{"t":["one two","three"],"k":["a","b","a"],"l":[1,2],"o":{"a":["q","r"],"n":null}}"#,
    r#"{"t":"","k":"","l":0,"b":false,"o":{}}"#,
    r#"{"t":null,"k":null,"l":null}"#,
    r#"{}"#,
    // Edge numbers, long strings, escapes, unicode.
    r#"{"l":9223372036854775807,"i":-2147483648,"f":1e10,"d":-0.0,"k":"caf\u00e9 \ud83d\ude00","t":"Line\nbreak \"quoted\""}"#,
    r#"{"raw":1,"t":"abcdefgh"}"#,
    // Coercions and strings for numbers, which the plan leaves to the complete path.
    r#"{"l":"12","i":"3","d":"1.5","b":"true","dt":1700000000000}"#,
    r#"{"l":1.0,"i":2.9,"f":"x1"}"#,
    // A field no mapping names, a repeated key, an array of arrays, odd whitespace.
    r#"{"zzz":1,"k":"q"}"#,
    r#"{"k":"a","k":"b"}"#,
    r#"{"l":[[1,2],[3]]}"#,
    "  {\n\t\"k\" :\r\n \"ws\" }  ",
    r#"{"k":"a","o":{"a":"b","a":"c"}}"#,
    // A keyword over ignore_above of its multi-field, and a number past its type.
    r#"{"t":"toolongvalue"}"#,
    r#"{"i":3000000000}"#,
    r#"{"b":"yes"}"#,
    // Trailing junk and bad JSON are the complete path's error, so they stay out of this list.
];

#[test]
fn a_wide_mapping_equals_the_complete_path() {
    let taken = run(WIDE, WIDE_DOCS);
    // The fast path must take a good share, or the comparison proves nothing.
    assert!(taken.iter().filter(|&&t| t).count() >= 5, "{taken:?}");
}

#[test]
fn dynamic_fields_are_read_by_the_plan_once_the_complete_path_has_mapped_them() {
    let docs = [
        r#"{"k":"a"}"#,
        r#"{"k":"b","n":5,"s":"some words"}"#,
        r#"{"k":"c","n":6,"s":"more words","f":1.5,"b":true}"#,
        r#"{"k":"d","n":7,"s":"other words","f":2.5,"b":false}"#,
        r#"{"k":"e","n":8,"s":"words again","f":0.5,"b":true,"extra":"new"}"#,
        r#"{"k":"f","n":9,"s":"and again","f":3.5,"b":true,"extra":"newer"}"#,
    ];
    let taken = run(r#"{"properties":{"k":{"type":"keyword"}}}"#, &docs);
    // Each new field set is the complete path's, then the plan reads the next of its shape.
    assert_eq!(taken, [false, false, false, true, false, true]);
}

/// A tiny deterministic generator, so the corpus is the same on every run.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % n
    }
}

fn pick<'a>(r: &mut Lcg, pool: &[&'a str]) -> &'a str {
    pool[r.next(pool.len())]
}

#[test]
fn generated_documents_equal_the_complete_path() {
    let words = [
        "alpha",
        "Beta",
        "gamma-delta",
        "e\u{301}clair",
        "日本語",
        "x",
        "",
        "a b  c",
        "O'Neil",
        "3.14",
        "snake_case",
        "UPPER lower",
        "tab\\there",
    ];
    let keywords = ["a", "b", "ab", "abcdef", "", "日本", "a b", "Z"];
    let ints = ["0", "1", "-1", "2147483647", "-2147483648", "42", "7"];
    let longs = [
        "0",
        "9223372036854775807",
        "-9223372036854775808",
        "123456789012",
        "5",
    ];
    let floats = ["0.0", "-0.0", "1.5", "3.4028235e38", "1e-45", "2", "0.1"];
    let dates = [
        "\"2026-01-02T03:04:05Z\"",
        "\"2026-01-02\"",
        "\"2026-01-02T03:04:05.678+01:00\"",
        "1700000000000",
        "\"1969-12-31T23:59:59.999Z\"",
    ];
    let bools = ["true", "false"];
    let mut r = Lcg(7);
    let mut docs = Vec::new();
    for _ in 0..3000 {
        let mut f: Vec<String> = Vec::new();
        let mut add = |name: &str, value: String| f.push(format!("\"{name}\":{value}"));
        for (name, pool) in [
            ("t", &words[..]),
            ("k", &keywords[..]),
            ("kd", &keywords[..]),
        ] {
            match r.next(5) {
                0 => {}
                1 => add(name, "null".into()),
                2 => add(
                    name,
                    format!(
                        "[\"{}\",\"{}\"]",
                        pick(&mut r, pool).replace('\\', "\\\\"),
                        pick(&mut r, pool).replace('\\', "\\\\")
                    ),
                ),
                _ => add(
                    name,
                    format!("\"{}\"", pick(&mut r, pool).replace('\\', "\\\\")),
                ),
            }
        }
        for (name, pool) in [
            ("i", &ints[..]),
            ("l", &longs[..]),
            ("f", &floats[..]),
            ("d", &floats[..]),
        ] {
            match r.next(4) {
                0 => {}
                1 => add(name, pick(&mut r, pool).to_owned()),
                2 => add(
                    name,
                    format!("[{},{}]", pick(&mut r, pool), pick(&mut r, pool)),
                ),
                _ => add(name, pick(&mut r, pool).to_owned()),
            }
        }
        if r.next(2) == 0 {
            add("dt", pick(&mut r, &dates).to_owned());
        }
        if r.next(2) == 0 {
            add("b", pick(&mut r, &bools).to_owned());
        }
        if r.next(3) == 0 {
            add(
                "o",
                format!(
                    "{{\"a\":\"{}\",\"n\":{}}}",
                    pick(&mut r, &keywords),
                    pick(&mut r, &longs)
                ),
            );
        }
        docs.push(format!("{{{}}}", f.join(",")));
    }
    let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
    let taken = run(WIDE, &refs);
    let n = taken.iter().filter(|&&t| t).count();
    eprintln!("generated: {n} of {} read by the plan", taken.len());
    assert!(n > 50, "too few taken");
}

#[test]
fn the_plan_is_compiled_when_fields_or_the_mapping_change_and_not_per_document() {
    let (d1, d2) = (ByteBuffersDirectory::new(), ByteBuffersDirectory::new());
    let (mut slow, mut fast) = (side(&d1, LOGS), side(&d2, LOGS));
    let mut path = FastIngest::new();
    for i in 0..200 {
        let (id, source) = (format!("id{i}"), log(i));
        let key = uid(&id).expect("uid");
        slow.slow(&id, i as i64, source.as_bytes());
        if path
            .prepare(&fast.indexer, &fast.mapping, source.as_bytes(), &key)
            .is_none()
        {
            fast.slow(&id, i as i64, source.as_bytes());
        }
    }
    // One compile before any field is numbered, one after the first document numbered them all.
    assert_eq!(path.compiles(), 2);
}

/// A mapping whose only field the plan cannot read: every document with it is the complete
/// path's, and equal to it.
fn left_alone(field: &str, value: &str) {
    let mapping = format!(r#"{{"properties":{{{field}}}}}"#);
    let doc = format!(r#"{{"f":{value}}}"#);
    let docs = [doc.as_str(); 4];
    let taken = run(&mapping, &docs);
    assert!(taken.iter().all(|&t| !t), "{field}: {taken:?}");
}

#[test]
fn fields_the_plan_cannot_read_stay_with_the_complete_path() {
    left_alone(r#""f":{"type":"byte"}"#, "5");
    left_alone(r#""f":{"type":"short"}"#, "5");
    left_alone(r#""f":{"type":"keyword","store":true}"#, r#""v""#);
    left_alone(r#""f":{"type":"keyword","norms":true}"#, r#""v""#);
    left_alone(
        r#""f":{"type":"keyword","index_options":"freqs"}"#,
        r#""v""#,
    );
    left_alone(r#""f":{"type":"keyword","doc_values":false}"#, r#""v""#);
    left_alone(r#""f":{"type":"keyword","null_value":"NV"}"#, r#""v""#);
    left_alone(r#""f":{"type":"text","store":true}"#, r#""v w""#);
    left_alone(
        r#""f":{"type":"date","format":"epoch_second"}"#,
        "1700000000",
    );
    left_alone(r#""f":{"type":"long","ignore_malformed":true}"#, r#""x""#);
    left_alone(
        r#""f":{"type":"boolean","index":false,"doc_values":false}"#,
        "true",
    );
}

#[test]
fn fields_the_plan_reads_with_their_parameters_equal_the_complete_path() {
    for (field, value) in [
        (r#""f":{"type":"keyword","ignore_above":3}"#, r#""abcdef""#),
        (r#""f":{"type":"keyword","ignore_above":3}"#, r#""abc""#),
        (r#""f":{"type":"keyword","index":false}"#, r#""abc""#),
        (r#""f":{"type":"text","norms":false}"#, r#""one two""#),
        (
            r#""f":{"type":"text","index_options":"freqs"}"#,
            r#""one one two""#,
        ),
        (
            r#""f":{"type":"text","index_options":"docs"}"#,
            r#""one one two""#,
        ),
        (
            r#""f":{"type":"text","index_options":"offsets"}"#,
            r#""one one two""#,
        ),
        (
            r#""f":{"type":"text","position_increment_gap":0}"#,
            r#"["a b","c d"]"#,
        ),
        (r#""f":{"type":"long","doc_values":false}"#, "5"),
        (r#""f":{"type":"double","index":false}"#, "5.5"),
        (r#""f":{"type":"boolean"}"#, "true"),
        (
            r#""f":{"properties":{"a":{"properties":{"b":{"type":"keyword"}}}}}"#,
            r#"{"a":{"b":"deep"}}"#,
        ),
    ] {
        let mapping = format!(r#"{{"properties":{{{field}}}}}"#);
        let doc = format!(r#"{{"f":{value}}}"#);
        let taken = run(&mapping, &[doc.as_str(); 5]);
        // The first document numbers the field; the plan reads the rest, so each parameter is
        // one the plan carries (a declined field would trivially equal the complete path).
        // Left to the complete path: a value `ignore_above` drops indexes nothing, so the field
        // is never numbered; text without norms and a long without doc values are not carried.
        let declined = value == r#""abcdef""#
            || field.contains(r#""norms":false"#)
            || field.contains(r#""type":"long","doc_values":false"#);
        assert!(
            declined || taken[1..].iter().all(|&t| t),
            "{field} {value}: {taken:?}"
        );
    }
}

#[test]
fn a_source_over_the_size_bound_stays_with_the_complete_path() {
    let words = |n: usize| "word ".repeat(n / 5);
    let small = format!(r#"{{"message":"{}"}}"#, words(900_000));
    let large = format!(r#"{{"message":"{}"}}"#, words(1_100_000));
    let mapping = r#"{"properties":{"message":{"type":"text"}}}"#;
    let taken = run(mapping, &[&small, &small, &large, &large]);
    // The first numbers the field; the second is under the bound and read by the plan; the
    // large ones are too big for the plan, whose memory is about twice the source's.
    assert_eq!(taken, [false, true, false, false]);
}

#[test]
fn a_source_of_exactly_the_size_bound_is_read_by_the_plan_and_one_byte_more_is_not() {
    // `{"message":"` and `"}` are 14 bytes; the rest is one-letter words.
    let of_len = |n: usize| {
        let pad = n - 14;
        format!(
            r#"{{"message":"{}{}"}}"#,
            "a ".repeat(pad / 2),
            "a".repeat(pad % 2)
        )
    };
    let (at, over) = (of_len(1 << 20), of_len((1 << 20) + 1));
    assert_eq!((at.len(), over.len()), (1 << 20, (1 << 20) + 1));
    let mapping = r#"{"properties":{"message":{"type":"text"}}}"#;
    let taken = run(mapping, &[&at, &at, &over]);
    assert_eq!(taken, [false, true, false]);
}

#[test]
fn a_fast_document_weighs_what_it_holds_and_invalidating_recompiles_the_plan() {
    let (d1, d2) = (ByteBuffersDirectory::new(), ByteBuffersDirectory::new());
    let (mut slow, fast) = (side(&d1, LOGS), side(&d2, LOGS));
    let _ = fast;
    let first = log(0);
    slow.slow("id0", 0, first.as_bytes());
    let mut path = FastIngest::new();
    let key = uid("id1").expect("uid");
    let small = path
        .prepare(&slow.indexer, &slow.mapping, log(1).as_bytes(), &key)
        .expect("read by the plan");
    let long = format!(
        r#"{{"@timestamp":"2026-11-20T08:00:00.000Z","level":"info","host":"h","status":200,"bytes":1,"message":"{}"}}"#,
        "word ".repeat(400)
    );
    let big = path
        .prepare(&slow.indexer, &slow.mapping, long.as_bytes(), &key)
        .expect("read by the plan");
    assert!(small.weight() > 1, "{}", small.weight());
    assert!(
        big.weight() > small.weight() + 1000,
        "{} {}",
        big.weight(),
        small.weight()
    );
    let compiles = path.compiles();
    path.prepare(&slow.indexer, &slow.mapping, log(2).as_bytes(), &key)
        .expect("read by the plan");
    assert_eq!(path.compiles(), compiles);
    path.invalidate();
    path.prepare(&slow.indexer, &slow.mapping, log(3).as_bytes(), &key)
        .expect("read by the plan");
    assert_eq!(path.compiles(), compiles + 1);
}

#[test]
fn a_text_with_no_token_is_numbered_but_written_nowhere_alone_or_beside_others() {
    let mapping = r#"{"properties":{"t":{"type":"text"},"k":{"type":"keyword"}}}"#;
    let docs = [
        r#"{"t":"a b","k":"x"}"#,
        r#"{"t":"","k":"x"}"#,
        r#"{"t":""}"#,
        r#"{"t":"  "}"#,
        r#"{"t":"c","k":"y"}"#,
        r#"{"t":"","k":""}"#,
    ];
    let taken = run(mapping, &docs);
    assert!(taken[1..].iter().filter(|&&t| t).count() >= 3, "{taken:?}");
}
