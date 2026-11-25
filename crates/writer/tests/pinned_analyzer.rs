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
