//! docs/specs/extension-analyzer AC-13: the fast ingest plan never reads a
//! field an index pins an analyzer for; the complete path takes it, with the
//! module.

mod fast_common;

use std::sync::Arc;

use fast_common::side;
use lucene_store::ByteBuffersDirectory;
use oss_doc::fast::FastIngest;
use oss_doc::lucene::uid;
use oss_doc::mapping::{Mapping, ParseOptions};
use oss_ext::CallError;
use oss_ext::analyzer::{Analyze, Token};

struct Probe;

impl Analyze for Probe {
    fn analyze(&self, text: &str) -> Result<Vec<Token>, CallError> {
        Ok(vec![Token {
            term: "probe".to_owned(),
            start: 0,
            end: u32::try_from(text.encode_utf16().count()).expect("small"),
            increment: 1,
        }])
    }
}

const PLAIN: &str = r#"{"properties":{"t":{"type":"text"}}}"#;
const PINNED: &str = r#"{"properties":{"t":{"type":"text","analyzer":"ext_a"}}}"#;
const DOC: &str = r#"{"t":"some words here"}"#;

/// Whether the plan reads the second document of `mapping`, after the first
/// has numbered its fields through the complete path.
fn plan_reads(mapping: &str, pinned: bool) -> bool {
    let dir = ByteBuffersDirectory::new();
    let mut s = side(&dir, PLAIN);
    if pinned {
        s.mapping = Mapping::parse_pinned(mapping.as_bytes(), ParseOptions::default()).expect("m");
        s.indexer = s.indexer.with_module("ext_a", Arc::new(Probe));
    }
    s.slow("a", 0, DOC.as_bytes()).expect("the complete path");
    let key = uid("b").expect("uid");
    FastIngest::new()
        .prepare(&s.indexer, &s.mapping, DOC.as_bytes(), &key)
        .is_some()
}

// AC-13 (the tail overlay's fast plan)
#[test]
fn the_fast_plan_reads_a_plain_text_field_and_declines_a_pinned_one() {
    assert!(plan_reads(PLAIN, false), "the control: the plan reads `t`");
    assert!(!plan_reads(PINNED, true), "the plan declines a pinned `t`");
}
