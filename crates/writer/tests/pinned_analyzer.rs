//! A shard indexes a field with the module its mapping pins, and refuses
//! the write rather than fall back to the built-in analyzer.

use std::sync::Arc;

use lucene_search::directory_reader::DirectoryReader;
use lucene_store::ByteBuffersDirectory;
use oss_ext::analyzer::{Analyze, Token};
use oss_ext::{CallError, Status};
use oss_writer::index::knn::open_writer;
use oss_writer::index::{Expect, Refused, Shard};
use oss_writer::mapping::{Mapping, ParseOptions};

const MAPPING: &[u8] = br#"{"properties":{"t":{"type":"text","analyzer":"ext_shout"}}}"#;

struct Shout;

impl Analyze for Shout {
    fn analyze(&self, text: &str) -> Result<Vec<Token>, CallError> {
        Ok(vec![Token {
            term: text.to_uppercase(),
            start: 0,
            end: u32::try_from(text.encode_utf16().count()).expect("small"),
            increment: 1,
        }])
    }
}

struct Broken;

impl Analyze for Broken {
    fn analyze(&self, _: &str) -> Result<Vec<Token>, CallError> {
        Err(CallError::from_status(Status::Limit, b"fuel"))
    }
}

fn shard<'d>(dir: &'d ByteBuffersDirectory, module: Arc<dyn Analyze>) -> Shard<'d> {
    let mapping = Mapping::parse_pinned(MAPPING, ParseOptions::default()).expect("mapping");
    let (writer, derived) = open_writer(dir, &mapping).expect("writer");
    Shard::new(dir, writer, mapping, 1, Box::new(|| 0))
        .expect("shard")
        .with_derived_source(derived)
        .with_module("ext_shout", module)
}

// docs/specs/extension-analyzer AC-10
#[test]
fn a_pinned_field_is_indexed_with_the_modules_terms() {
    let dir = ByteBuffersDirectory::new();
    let mut shard = shard(&dir, Arc::new(Shout));
    shard
        .index("1", br#"{"t":"Hello World"}"#, false, &Expect::default())
        .expect("indexed");
    shard.refresh().expect("refresh");
    let reader = DirectoryReader::open(&dir).expect("open");
    let freq = |term: &str| {
        reader.segment_readers()[0]
            .doc_freq("t", term.as_bytes())
            .expect("freq")
    };
    assert_eq!(freq("HELLO WORLD"), 1);
    assert_eq!(freq("hello"), 0);
}

// docs/specs/extension-analyzer AC-8: no result built without the module
#[test]
fn a_module_that_fails_refuses_the_write() {
    let dir = ByteBuffersDirectory::new();
    let mut shard = shard(&dir, Arc::new(Broken));
    let e = shard
        .index("1", br#"{"t":"Hello"}"#, false, &Expect::default())
        .expect_err("refused");
    let Refused::Mapping(e) = e else {
        panic!("{e:?}")
    };
    assert_eq!(e.ty, "extension_failed", "a limit is the input's, 400");
}

struct Failing(fn() -> CallError);

impl Analyze for Failing {
    fn analyze(&self, _: &str) -> Result<Vec<Token>, CallError> {
        Err((self.0)())
    }
}

// docs/specs/extension-analyzer AC-2, AC-16
#[test]
fn a_failure_is_typed_by_what_a_caller_can_do() {
    for (make, ty) in [
        (CallError::busy as fn() -> CallError, "extension_busy"),
        (|| CallError::malformed(9), "extension_malformed"),
        (|| CallError::trap("unreachable"), "extension_failed"),
    ] {
        let dir = ByteBuffersDirectory::new();
        let mut shard = shard(&dir, Arc::new(Failing(make)));
        let Err(Refused::Mapping(e)) =
            shard.index("1", br#"{"t":"Hello"}"#, false, &Expect::default())
        else {
            panic!("refused")
        };
        assert_eq!(e.ty, ty);
    }
}

/// One token a word, upper-cased, with the word's own offsets.
struct Words;

impl Analyze for Words {
    fn analyze(&self, text: &str) -> Result<Vec<Token>, CallError> {
        let mut out = Vec::new();
        let mut at = 0_u32;
        for word in text.split(' ') {
            let n = u32::try_from(word.encode_utf16().count()).expect("small");
            if n > 0 {
                out.push(Token {
                    term: word.to_uppercase(),
                    start: at,
                    end: at.saturating_add(n),
                    increment: 1,
                });
            }
            at = at.saturating_add(n).saturating_add(1);
        }
        Ok(out)
    }
}

const SOURCES: [&str; 4] = [
    r#"{"t":"the quick brown fox"}"#,
    r#"{"t":"jumps over the lazy dog"}"#,
    r#"{"t":"quick quick quick"}"#,
    r#"{"t":"the end"}"#,
];

fn terms_of(dir: &ByteBuffersDirectory) -> Vec<(String, i32)> {
    let reader = DirectoryReader::open(dir).expect("open");
    let seg = &reader.segment_readers()[0];
    [
        "THE", "QUICK", "BROWN", "FOX", "JUMPS", "OVER", "LAZY", "DOG", "END", "the", "quick",
    ]
    .iter()
    .map(|t| {
        (
            (*t).to_owned(),
            seg.doc_freq("t", t.as_bytes()).expect("freq"),
        )
    })
    .collect()
}

// docs/specs/extension-analyzer AC-12: replay reproduces what the live write indexed
#[test]
fn a_replay_indexes_the_same_terms_as_the_live_write() {
    let live_dir = ByteBuffersDirectory::new();
    let mut live = shard(&live_dir, Arc::new(Words));
    for (n, source) in SOURCES.iter().enumerate() {
        live.index(
            &format!("d{n}"),
            source.as_bytes(),
            false,
            &Expect::default(),
        )
        .expect("indexed");
    }
    live.refresh().expect("refresh");

    let replay_dir = ByteBuffersDirectory::new();
    let mut replayed = shard(&replay_dir, Arc::new(Words));
    for (n, source) in SOURCES.iter().enumerate() {
        let seq = i64::try_from(n).expect("small");
        let record =
            oss_format::WalRecord::index(format!("d{n}"), (*source).to_owned(), seq, 1, seq + 1);
        assert!(replayed.replay(&record).expect("replayed"));
    }
    replayed.refresh().expect("refresh");

    let (a, b) = (terms_of(&live_dir), terms_of(&replay_dir));
    assert_eq!(a, b, "the same documents frequency for every term");
    assert_eq!(a.iter().find(|t| t.0 == "QUICK").expect("term").1, 2);
    assert_eq!(a.iter().find(|t| t.0 == "quick").expect("term").1, 0);
}
