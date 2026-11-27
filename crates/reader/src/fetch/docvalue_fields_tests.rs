//! Tests; compiled only under `#[cfg(test)]` from the parent module. The
//! messages and the key orders are OpenSearch 3.10.0's for the same input.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs, doc_keyword, doc_tag};

use super::*;
use crate::dsl::parse_chunks;
use crate::{OpenShard, open_shard};

fn parsed(body: &str) -> Vec<DocValueField> {
    parse_chunks([body.as_bytes()])
        .expect("parses")
        .docvalue_fields
}

fn refused(body: &str) -> (&'static str, String) {
    let f = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    (f.kind, f.reason)
}

fn named(field: &str) -> DocValueField {
    DocValueField {
        field: field.to_owned(),
        format: None,
    }
}

fn mapping() -> Mapping {
    Mapping::from_json(
        r#"{"properties":{"year":{"type":"integer"},"tag":{"type":"keyword"},
        "published":{"type":"date"},"n":{"type":"long"},"title":{"type":"text"},
        "spot":{"type":"geo_point"},"bare":{"type":"keyword","doc_values":false}}}"#,
    )
    .expect("mapping")
}

fn resolved(fields: &[&str]) -> Result<Vec<(String, ValueKind, usize)>, ShardError> {
    let fields: Vec<DocValueField> = fields.iter().map(|f| named(f)).collect();
    Ok(resolve(&fields, &mapping())?
        .into_iter()
        .map(|f| (f.name, f.kind, f.times))
        .collect())
}

fn reason(r: Result<impl std::fmt::Debug, ShardError>) -> (u16, String) {
    let e = r.expect_err("refused");
    (e.status, e.failure.reason)
}

#[test]
fn entries_are_names_or_objects() {
    assert_eq!(
        parsed(r#"{"docvalue_fields":["year",1,true,{"field":"tag"}]}"#),
        [named("year"), named("1"), named("true"), named("tag")]
    );
    assert_eq!(
        parsed(r#"{"docvalue_fields":[{"field":"a","format":"x"},{"format":null,"field":"b"}]}"#),
        [
            DocValueField {
                field: "a".to_owned(),
                format: Some("x".to_owned())
            },
            named("b")
        ]
    );
    assert!(parsed(r#"{"docvalue_fields":[]}"#).is_empty());
}

#[test]
fn a_bad_entry_fails_as_opensearch_fails() {
    for (body, kind, expected) in [
        (
            r#"{"docvalue_fields":[{}]}"#,
            "illegal_argument_exception",
            "Required [field]",
        ),
        (
            r#"{"docvalue_fields":[{"format":"x"}]}"#,
            "illegal_argument_exception",
            "Required [field]",
        ),
        (
            r#"{"docvalue_fields":[{"field":1}]}"#,
            "x_content_parse_exception",
            "[1:30] [docvalues_field] field doesn't support values of type: VALUE_NUMBER",
        ),
        (
            r#"{"docvalue_fields":[{"field":"year","format":1}]}"#,
            "x_content_parse_exception",
            "[1:46] [docvalues_field] format doesn't support values of type: VALUE_NUMBER",
        ),
        (
            r#"{"docvalue_fields":[{"field":null}]}"#,
            "x_content_parse_exception",
            "[1:30] [docvalues_field] field doesn't support values of type: VALUE_NULL",
        ),
        (
            r#"{"docvalue_fields":[{"field":{"a":1}}]}"#,
            "x_content_parse_exception",
            "[1:30] [docvalues_field] field doesn't support values of type: START_OBJECT",
        ),
        (
            r#"{"docvalue_fields":[{"nope":"year"}]}"#,
            "x_content_parse_exception",
            "[1:22] [docvalues_field] unknown field [nope]",
        ),
        (
            r#"{"docvalue_fields":[{"fild":"year"}]}"#,
            "x_content_parse_exception",
            "[1:22] [docvalues_field] unknown field [fild] did you mean [field]?",
        ),
        (
            r#"{"docvalue_fields":[null,"a"]}"#,
            "x_content_parse_exception",
            "[1:26] [docvalues_field] Expected START_OBJECT but was: VALUE_STRING",
        ),
        (
            r#"{"docvalue_fields":[[1]]}"#,
            "x_content_parse_exception",
            "[1:22] [docvalues_field] Expected START_OBJECT but was: VALUE_NUMBER",
        ),
        (
            r#"{"docvalue_fields":"year"}"#,
            "parsing_exception",
            "Unknown key for a VALUE_STRING in [docvalue_fields].",
        ),
    ] {
        assert_eq!(refused(body), (kind, expected.to_owned()), "{body}");
    }
}

#[test]
fn at_most_a_hundred_entries() {
    let fields = vec![named("year"); MAX_DOCVALUE_FIELDS];
    assert!(check_count(&fields).is_ok());
    let fields = vec![named("year"); MAX_DOCVALUE_FIELDS + 1];
    assert_eq!(
        reason(check_count(&fields)),
        (
            400,
            "Trying to retrieve too many docvalue_fields. Must be less than or equal to: [100] \
             but was [101]. This limit can be set by changing the \
             [index.max_docvalue_fields_search] index level setting."
                .to_owned()
        )
    );
}

#[test]
fn names_and_patterns_resolve_in_hash_map_order() {
    assert_eq!(
        resolved(&["published", "tag", "year"]).expect("resolves"),
        [
            ("year".to_owned(), ValueKind::Long, 1),
            ("published".to_owned(), ValueKind::Date, 1),
            ("tag".to_owned(), ValueKind::Term, 1)
        ]
    );
    // `published` and `tag` share a bucket: the first put comes first.
    assert_eq!(
        resolved(&["year", "tag", "published"]).expect("resolves"),
        [
            ("year".to_owned(), ValueKind::Long, 1),
            ("tag".to_owned(), ValueKind::Term, 1),
            ("published".to_owned(), ValueKind::Date, 1)
        ]
    );
    // A pattern, a name asked for again, and unmapped names.
    assert_eq!(
        resolved(&["y*", "n", "year", "nope", "nope*"]).expect("resolves"),
        [
            ("year".to_owned(), ValueKind::Long, 2),
            ("n".to_owned(), ValueKind::Long, 1)
        ]
    );
    assert!(resolved(&[]).expect("resolves").is_empty());
}

#[test]
fn a_field_without_doc_values_fails_as_opensearch_fails() {
    let text = reason(resolved(&["title"]));
    assert_eq!(text.0, 400);
    assert!(
        text.1.starts_with("Text fields are not optimised") && text.1.contains("on [title]"),
        "{}",
        text.1
    );
    assert_eq!(reason(resolved(&["ti*"])), text);
    assert_eq!(
        reason(resolved(&["bare"])),
        (
            400,
            "Can't load fielddata on [bare] because fielddata is unsupported on fields of type \
             [keyword]. Use doc values instead."
                .to_owned()
        )
    );
    for unsupported in [&["spot"][..], &["_id"], &["_seq_no*"], &["*"], &["_*"]] {
        let (status, why) = reason(resolved(unsupported));
        assert_eq!(status, 400);
        assert!(why.ends_with("is not supported by this node yet"), "{why}");
    }
    let formatted = [DocValueField {
        field: "year".to_owned(),
        format: Some("0.00".to_owned()),
    }];
    assert!(
        reason(resolve(&formatted, &mapping()))
            .1
            .ends_with("is not supported by this node yet")
    );
    let mapped = [DocValueField {
        field: "year".to_owned(),
        format: Some("use_field_mapping".to_owned()),
    }];
    assert_eq!(resolve(&mapped, &mapping()).expect("resolves").len(), 1);
}

fn shard() -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    let spec = Spec::new("dv", 20).segments_of(10).with_keywords(2);
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", "dv", 0).expect("opens")
}

fn field(name: &str, kind: ValueKind, times: usize) -> HitField {
    HitField {
        name: name.to_owned(),
        kind,
        times,
        script: None,
        source: None,
        ltr: None,
    }
}

fn written(shard: &OpenShard<Emulator>, fields: &[HitField], segment: usize, doc: i32) -> String {
    let readers = shard.reader.segment_readers();
    let mut out = Vec::new();
    write_fields(&mut out, fields, &readers[segment], doc).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

#[test]
fn a_hit_s_fields_are_written_from_its_doc_values() {
    let shard = shard();
    let fields = [
        field("k0", ValueKind::Term, 1),
        field("tag", ValueKind::Term, 2),
        field("n", ValueKind::Long, 1),
    ];
    // Document 13 is the fourth of the second segment.
    assert_eq!(
        written(&shard, &fields, 1, 3),
        format!(
            r#","fields":{{"k0":["{}"],"tag":["{}","{}"],"n":[13]}}"#,
            doc_keyword(13, 0),
            doc_tag(13),
            doc_tag(13)
        )
    );
    // A long read as a date is written in the default format.
    assert_eq!(
        written(&shard, &[field("n", ValueKind::Date, 1)], 0, 2),
        r#","fields":{"n":["1970-01-01T00:00:00.002Z"]}"#
    );
    // A field the segment does not hold has no values, and no values write
    // nothing.
    let absent = [
        field("x", ValueKind::Term, 1),
        field("y", ValueKind::Long, 1),
    ];
    assert_eq!(written(&shard, &absent, 0, 1), "");
    assert_eq!(
        written(
            &shard,
            &[absent[0].clone(), field("n", ValueKind::Long, 1)],
            0,
            1
        ),
        r#","fields":{"n":[1]}"#
    );
}

#[test]
fn the_fields_resolve_only_when_there_is_a_hit_to_fetch() {
    let shard = shard();
    let run = |query: &str| {
        let body =
            format!(r#"{{"query":{query},"docvalue_fields":[{{"field":"n","format":"0.00"}}]}}"#);
        let source = parse_chunks([body.as_bytes()]).expect("parses");
        crate::search::search(
            &shard,
            "idx",
            &source,
            &lucene_analysis::Analyzer::standard(None),
        )
        .map(|h| h.fields.len())
    };
    // No document matches, so no field is read and the format is not refused.
    assert_eq!(
        run(r#"{"term":{"tag":"no-such-tag"}}"#).map_err(|e| e.failure.reason),
        Ok(0)
    );
    let refused = run(r#"{"match_all":{}}"#).expect_err("refused");
    assert!(
        refused
            .failure
            .reason
            .ends_with("is not supported by this node yet")
    );
}

#[test]
fn the_sequence_number_and_primary_term_resolve_as_longs() {
    assert_eq!(
        resolved(&["_primary_term", "_seq_no", "_seq_no"]).expect("resolves"),
        vec![
            ("_seq_no".to_owned(), ValueKind::Long, 2),
            ("_primary_term".to_owned(), ValueKind::Long, 1),
        ]
    );
}
