//! A searcher over the commit and the overlay (docs/specs/reader-overlay-view).

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use crate::common;

use std::collections::HashMap;

use lucene_search::field_norms::FieldNorms;
use lucene_search::index_searcher::IndexSearcher;
use lucene_search::query::{TermInSetQuery, TermQuery};
use lucene_search::{BooleanQuery, Clause};
use oss_doc::lucene::uid;
use oss_doc::mapping::Mapping;
use oss_format::WalRecord;
use oss_reader::overlay::{Masks, Overlay, combine, find_copies};

fn mapping() -> Mapping {
    Mapping::parse(br#"{"properties":{"name":{"type":"keyword"},"body":{"type":"text"}}}"#)
        .expect("mapping")
}

fn source(name: &str, body: &str) -> String {
    format!(r#"{{"name":"{name}","body":"{body}"}}"#)
}

fn index(id: &str, name: &str, body: &str, seq: i64) -> WalRecord<'static> {
    WalRecord::index(id.to_owned(), source(name, body), seq, 1, seq + 1)
}

fn term(field: &str, value: &str) -> BooleanQuery {
    BooleanQuery {
        filter: vec![Clause::Term(TermQuery::new(field, value.as_bytes()))],
        ..BooleanQuery::default()
    }
}

fn ids_query(ids: &[&str]) -> BooleanQuery {
    let terms: Vec<Vec<u8>> = ids.iter().map(|i| uid(i).expect("uid")).collect();
    BooleanQuery {
        filter: vec![Clause::TermInSet(TermInSetQuery::new("_id", terms))],
        ..BooleanQuery::default()
    }
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// Counts of `queries` over the commit `batches` (minus `deleted`) with `records` pushed above it.
fn counts(
    batches: &[&[(&str, &str, i64)]],
    deleted: &[&str],
    records: &[WalRecord<'_>],
    queries: &[BooleanQuery],
) -> Vec<u64> {
    let m = mapping();
    let (_dir, reader) = common::commit_without(&m, batches, deleted);
    let candidates = oss_reader::overlay::candidate_ids(records, 0).expect("ids");
    let copies = find_copies(&reader, &candidates).expect("copies");
    let overlay = Overlay::build_over(&m, records, 0, usize::MAX, &copies).expect("overlay");
    let masks = Masks::new(&reader, overlay.hidden(&copies));
    let commit = reader.open_segments().expect("commit segments");
    let overlay_opened = overlay
        .reader()
        .map(|r| r.open_segments().expect("overlay segments"));
    let segments = combine(&commit, &masks, overlay_opened.as_ref(), reader.max_doc());
    let none: Vec<Option<&HashMap<String, FieldNorms<'_>>>> = vec![None; segments.len()];
    let searcher = IndexSearcher::new(&segments, &none).expect("searcher");
    queries
        .iter()
        .map(|q| searcher.count(q).expect("count"))
        .collect()
}

const A: &str = r#"{"name":"a","body":"alpha"}"#;
const B: &str = r#"{"name":"b","body":"beta"}"#;
const C: &str = r#"{"name":"c","body":"gamma"}"#;
const D: &str = r#"{"name":"d","body":"delta"}"#;

#[test]
fn the_overlay_replaces_and_deletes_commit_documents() {
    let batches: &[&[(&str, &str, i64)]] =
        &[&[("a", A, 1), ("b", B, 2)], &[("c", C, 3), ("d", D, 4)]];
    let records = [
        index("b", "b2", "beta two", 10),
        WalRecord::deletion("c".to_owned(), 11, 1, 12),
        index("e", "e", "epsilon", 12),
    ];
    let got = counts(
        batches,
        &[],
        &records,
        &[
            ids_query(&["a", "b", "c", "d", "e"]),
            term("name", "b"),
            term("name", "b2"),
            term("name", "c"),
            term("name", "e"),
            term("name", "a"),
            term("name", "d"),
        ],
    );
    // a, b (new), d and e are live; c and the old b are hidden.
    assert_eq!(got, [4, 0, 1, 0, 1, 1, 1]);
}

#[test]
fn with_no_overlay_the_commit_answers_as_before() {
    let batches: &[&[(&str, &str, i64)]] = &[&[("a", A, 1), ("b", B, 2)]];
    let got = counts(
        batches,
        &[],
        &[],
        &[ids_query(&["a", "b"]), term("name", "a")],
    );
    assert_eq!(got, [2, 1]);
}

#[test]
fn a_mask_keeps_the_commits_own_deletions() {
    let batches: &[&[(&str, &str, i64)]] = &[&[("a", A, 1), ("b", B, 2), ("c", C, 3)]];
    // The commit deleted a; the overlay replaces b. a must stay gone.
    let records = [index("b", "b2", "beta", 10)];
    let got = counts(
        batches,
        &["a"],
        &records,
        &[
            ids_query(&["a", "b", "c"]),
            term("name", "a"),
            term("name", "b2"),
            term("name", "c"),
        ],
    );
    assert_eq!(got, [2, 0, 1, 1]);
}

#[test]
fn a_scored_text_query_reads_both_sides() {
    let m = mapping();
    let batches: &[&[(&str, &str, i64)]] = &[&[
        ("a", r#"{"name":"a","body":"quick brown fox"}"#, 1),
        ("b", r#"{"name":"b","body":"lazy dog"}"#, 2),
    ]];
    let (_dir, reader) = common::commit_of(&m, batches);
    let records = [index("n", "n", "quick fox jumps", 10)];
    let copies = find_copies(&reader, &ids(&["n"])).expect("copies");
    let overlay = Overlay::build_over(&m, &records, 0, usize::MAX, &copies).expect("overlay");
    let masks = Masks::new(&reader, overlay.hidden(&copies));
    let commit = reader.open_segments().expect("commit segments");
    let overlay_opened = overlay
        .reader()
        .map(|r| r.open_segments().expect("overlay segments"));
    let segments = combine(&commit, &masks, overlay_opened.as_ref(), reader.max_doc());
    let fields = vec!["body".to_owned()];
    let mut owned = reader.field_norms_by_field(&fields).expect("commit norms");
    owned.extend(overlay.norms(&fields).expect("overlay norms"));
    let norms: Vec<Option<&HashMap<String, FieldNorms<'_>>>> = owned.iter().map(Some).collect();
    let searcher = IndexSearcher::new(&segments, &norms).expect("searcher");
    let query = BooleanQuery {
        should: vec![Clause::Term(TermQuery::new("body", b"fox".to_vec()))],
        ..BooleanQuery::default()
    };
    let top = searcher.search(&query, 10).expect("search");
    assert_eq!(
        top.score_docs.len(),
        2,
        "the commit's fox and the overlay's"
    );
    assert!(top.score_docs.iter().all(|d| d.score > 0.0), "{top:?}");
}

#[test]
fn overlay_documents_are_numbered_after_the_commits() {
    let m = mapping();
    let batches: &[&[(&str, &str, i64)]] = &[&[("a", A, 1), ("b", B, 2)], &[("c", C, 3)]];
    let (_dir, reader) = common::commit_of(&m, batches);
    let records = [index("n", "n", "new", 10)];
    let overlay = Overlay::build_over(
        &m,
        &records,
        0,
        usize::MAX,
        &find_copies(&reader, &[]).expect("none"),
    )
    .expect("overlay");
    let masks = Masks::new(&reader, []);
    let commit = reader.open_segments().expect("commit segments");
    let overlay_opened = overlay
        .reader()
        .map(|r| r.open_segments().expect("overlay segments"));
    let segments = combine(&commit, &masks, overlay_opened.as_ref(), reader.max_doc());
    assert_eq!(segments.len(), 3, "two commit segments and the overlay");
    let last = segments.last().expect("overlay segment");
    assert_eq!(last.doc_base, reader.max_doc());
    assert_eq!(reader.max_doc(), 3);
    let none: Vec<Option<&HashMap<String, FieldNorms<'_>>>> = vec![None; segments.len()];
    let searcher = IndexSearcher::new(&segments, &none).expect("searcher");
    let top = searcher.search(&term("name", "n"), 5).expect("search");
    assert_eq!(top.score_docs[0].doc, 3);
}

#[test]
fn masks_ignore_what_the_commit_does_not_hold_and_an_empty_overlay_has_no_norms() {
    let m = mapping();
    let batches: &[&[(&str, &str, i64)]] = &[&[("a", A, 1), ("b", B, 2)]];
    let (_dir, reader) = common::commit_of(&m, batches);
    // A segment, a negative document and a document past the end that the commit lacks.
    let masks = Masks::new(&reader, [(9, 0), (0, -1), (0, 999)]);
    let commit = reader.open_segments().expect("commit segments");
    let segments = combine(&commit, &masks, None, reader.max_doc());
    let none: Vec<Option<&HashMap<String, FieldNorms<'_>>>> = vec![None; segments.len()];
    let searcher = IndexSearcher::new(&segments, &none).expect("searcher");
    assert_eq!(searcher.count(&ids_query(&["a", "b"])).expect("count"), 2);
    let empty = Overlay::build_over(
        &m,
        &[],
        0,
        usize::MAX,
        &find_copies(&reader, &[]).expect("none"),
    )
    .expect("overlay");
    assert!(empty.norms(&["body".to_owned()]).expect("norms").is_empty());
    let one = Overlay::build_over(
        &m,
        &[index("n", "n", "text", 5)],
        0,
        usize::MAX,
        &find_copies(&reader, &[]).expect("none"),
    )
    .expect("overlay");
    assert_eq!(one.norms(&["body".to_owned()]).expect("norms").len(), 1);
}
