//! docs/specs/extension-analyzer AC-8: an overlay indexes a pinned field
//! with the module its pin loaded, and refuses to build without it.

// ARITH: test sizes are small constants.
#![allow(clippy::arithmetic_side_effects)]

mod common;

use std::sync::Arc;

use oss_doc::mapping::Mapping;
use oss_ext::CallError;
use oss_ext::analyzer::{Analyze, Token};
use oss_format::WalRecord;
use oss_reader::overlay::{BuildError, build_ready, build_ready_with};

const MAPPING: &str =
    r#"{"properties":{"name":{"type":"keyword"},"t":{"type":"text","analyzer":"ext_a"}}}"#;

/// Every text is the one term `probe`.
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

fn record() -> WalRecord<'static> {
    WalRecord::index(
        "d".to_owned(),
        r#"{"t":"anything at all"}"#.to_owned(),
        5,
        1,
        6,
    )
}

#[test]
fn a_pinned_field_is_indexed_with_the_module() {
    let mapping = Mapping::from_descriptor_pinned(Some(MAPPING)).expect("mapping");
    let (_dir, commit) = common::commit_of(&mapping, &[&[("a", r#"{"name":"x"}"#, 1)]]);
    let modules: Vec<(String, Arc<dyn Analyze>)> = vec![("ext_a".to_owned(), Arc::new(Probe))];
    let ready = build_ready_with(&commit, MAPPING, &[record()], 4 << 20, &modules).expect("builds");
    let reader = ready.overlay().reader().expect("a reader");
    let freq = |term: &str| {
        reader.segment_readers()[0]
            .doc_freq("t", term.as_bytes())
            .expect("freq")
    };
    assert_eq!(freq("probe"), 1);
    assert_eq!(freq("anything"), 0, "never the built-in analyzer's terms");
}

#[test]
fn a_pinned_mapping_without_its_module_does_not_build() {
    let mapping = Mapping::from_descriptor_pinned(Some(MAPPING)).expect("mapping");
    let (_dir, commit) = common::commit_of(&mapping, &[&[("a", r#"{"name":"x"}"#, 1)]]);
    assert!(build_ready(&commit, MAPPING, &[record()], 4 << 20).is_err());
}

struct Failing(fn() -> CallError);

impl Analyze for Failing {
    fn analyze(&self, _: &str) -> Result<Vec<Token>, CallError> {
        Err((self.0)())
    }
}

// AC-8: a failed module call fails the build, typed by what a caller can do
#[test]
fn a_failed_module_call_fails_the_build_typed() {
    let mapping = Mapping::from_descriptor_pinned(Some(MAPPING)).expect("mapping");
    let (_dir, commit) = common::commit_of(&mapping, &[&[("a", r#"{"name":"x"}"#, 1)]]);
    let failed: fn() -> CallError = || CallError::from_status(oss_ext::Status::Limit, b"nope");
    for (fail, ty) in [
        (failed, "extension_failed"),
        (CallError::busy as fn() -> CallError, "extension_busy"),
    ] {
        let modules: Vec<(String, Arc<dyn Analyze>)> =
            vec![("ext_a".to_owned(), Arc::new(Failing(fail)))];
        let err = build_ready_with(&commit, MAPPING, &[record()], 4 << 20, &modules)
            .map(drop)
            .expect_err("does not build");
        assert!(
            matches!(&err, BuildError::Extension { id, ty: t, .. } if id == "d" && *t == ty),
            "{ty}: {err:?}"
        );
    }
}
