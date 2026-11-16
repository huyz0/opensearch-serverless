//! Alias records read and written as the Java's `AliasRecord` reads and
//! writes them (docs/specs/alias-records/SPEC.md, AC-1 and AC-2).

mod common;

use std::borrow::Cow;

use common::alias_fixture;
use oss_format::{AliasMutation, AliasRecord, FormatError, NameRecord};

fn written(record: &AliasRecord<'_>) -> String {
    let mut buf = vec![0u8; 4096];
    let n = record.write(&mut buf).unwrap();
    buf.truncate(n);
    String::from_utf8(buf).unwrap()
}

fn show(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

#[test]
fn every_case_reads_and_writes_as_the_javas() {
    let cases = alias_fixture("cases.txt");
    let parsed = alias_fixture("parsed.txt");
    assert_eq!(parsed.len(), cases.len() * 2);
    assert!(cases.len() >= 35);
    for (case, java) in cases.iter().zip(parsed.chunks(2)) {
        let rust = AliasRecord::parse(case.as_bytes());
        if java[0] == "refused" {
            assert!(
                matches!(rust, Err(FormatError::Corrupt { .. })),
                "{case}: {rust:?}"
            );
            continue;
        }
        let rust = rust.unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_eq!(written(&rust), java[0], "{case}");
        let derived = format!(
            "{}\t{}\t{}\t{}\t{}",
            show(rust.write_index()),
            show(rust.writes_to()),
            rust.creation_pending(),
            rust.mutation_pending(),
            rust.protects_hint("b")
        );
        assert_eq!(derived, java[1], "{case}");
        // What the Java writes reads back and writes the same.
        let again = AliasRecord::parse(java[0].as_bytes()).unwrap();
        assert_eq!(written(&again), java[0], "{case}");
    }
}

#[test]
fn values_the_java_coerces_are_refused() {
    let cases = alias_fixture("strict.txt");
    let parsed = alias_fixture("strict-parsed.txt");
    assert_eq!(parsed.len(), cases.len() * 2);
    for (case, java) in cases.iter().zip(parsed.chunks(2)) {
        // The Java reads each, coercing a type it never writes.
        assert_ne!(java[0], "refused", "{case}");
        assert!(
            matches!(
                AliasRecord::parse(case.as_bytes()),
                Err(FormatError::Corrupt { .. })
            ),
            "{case}"
        );
    }
}

#[test]
fn a_built_record_writes_the_javas_bytes() {
    let mut record = AliasRecord::new("logs", &["a", "b"]);
    assert_eq!(written(&record), r#"{"alias":"logs","indices":["a","b"]}"#);
    assert!(!record.mutation_pending() && !record.creation_pending());
    assert_eq!(record.writes_to(), None);
    // A write member that is not a member is not written.
    record.write_member = Some(Cow::Borrowed("c"));
    assert_eq!(record.write_index(), None);
    assert_eq!(written(&record), r#"{"alias":"logs","indices":["a","b"]}"#);
    record.write_member = Some(Cow::Borrowed("b"));
    record.mutation = Some(AliasMutation {
        until: 9,
        creating: false,
        additions: vec![Cow::Borrowed("c")],
    });
    assert_eq!(
        written(&record),
        r#"{"alias":"logs","indices":["a","b"],"write_index":"b","mutation":{"until":9,"creating":false,"additions":["c"]}}"#
    );
    assert!(record.mutation_pending() && !record.creation_pending());
    assert!(record.protects_hint("a") && record.protects_hint("c"));
    assert!(!record.protects_hint("d"));
    // A data stream writes its fields, and no write member.
    let mut stream = AliasRecord::new("ds", &["ds-1", "ds-2"]);
    stream.data_stream = true;
    stream.generation = 2;
    stream.write_member = Some(Cow::Borrowed("ds-1"));
    assert_eq!(
        written(&stream),
        r#"{"alias":"ds","indices":["ds-1","ds-2"],"data_stream":true,"generation":2,"timestamp_field":null}"#
    );
    assert_eq!(stream.writes_to(), Some("ds-2"));
}

#[test]
fn a_repeated_field_is_refused_as_jackson_refuses_it() {
    for value in [
        &br#"{"alias":"x","indices":["a"],"indices":["b"]}"#[..],
        br#"{"alias":"x","indices":["a"],"write_index":"a","write_index":"a"}"#,
        br#"{"alias":"x","indices":["a"],"mutation":{"until":5,"creating":true,"creating":true,"additions":[]}}"#,
        br#"{"alias":"x","indices":["a"],"mutation":{"until":5,"creating":true,"additions":[],"additions":[]}}"#,
    ] {
        assert!(
            matches!(AliasRecord::parse(value), Err(FormatError::Corrupt { .. })),
            "{}",
            String::from_utf8_lossy(value)
        );
    }
}

#[test]
fn a_repeated_unknown_field_is_kept_where_jackson_refuses_it() {
    // The one way this reads more than the Java: no node writes a field it
    // does not know, let alone twice.
    for value in [
        &br#"{"alias":"x","indices":["a"],"other":1,"other":2}"#[..],
        br#"{"alias":"x","indices":["a"],"mutation":{"until":5,"creating":true,"additions":[],"k":1,"k":2}}"#,
    ] {
        assert!(AliasRecord::parse(value).is_ok());
    }
}

#[test]
fn a_write_member_the_java_drops_is_not_kept() {
    for value in [
        &br#"{"alias":"x","indices":["a","b"],"write_index":"c"}"#[..],
        br#"{"alias":"x","indices":["a"],"data_stream":true,"write_index":"a"}"#,
    ] {
        assert_eq!(AliasRecord::parse(value).unwrap().write_member, None);
    }
    let kept = AliasRecord::parse(br#"{"alias":"x","indices":["a","b"],"write_index":"b"}"#);
    assert_eq!(kept.unwrap().write_member.as_deref(), Some("b"));
}

#[test]
fn a_record_the_java_cannot_read_back_is_not_written() {
    let mut buf = [0u8; 256];
    let empty = AliasRecord::new("x", &[]);
    assert!(matches!(
        empty.write(&mut buf),
        Err(FormatError::Corrupt { .. })
    ));
    for (until, added) in [(0, "b"), (-1, "b"), (5, "")] {
        let mut record = AliasRecord::new("x", &["a"]);
        record.mutation = Some(AliasMutation {
            until,
            creating: true,
            additions: vec![Cow::Borrowed(added)],
        });
        assert!(
            matches!(record.write(&mut buf), Err(FormatError::Corrupt { .. })),
            "{until} {added:?}"
        );
    }
    let long = AliasRecord::new("x", &["a"]);
    assert!(matches!(
        long.write(&mut [0u8; 8]),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn a_name_register_holding_an_alias_reads_as_one() {
    let value = br#"{"alias":"logs","indices":["a"],"mutation":{"until":5,"creating":true,"additions":[]}}"#;
    let NameRecord::Alias(alias) = NameRecord::parse(value).unwrap() else {
        panic!("not an alias");
    };
    assert_eq!(alias.name, "logs");
    assert!(alias.creation_pending());
    assert!(matches!(
        NameRecord::parse(br#"{"alias":"logs","indices":[]}"#),
        Err(FormatError::Corrupt { .. })
    ));
}

#[test]
fn a_listing_reads_only_index_records() {
    let index = NameRecord::index_only(br#"{"name":"a","uuid":"u","number_of_shards":2}"#)
        .unwrap()
        .expect("an index record");
    assert_eq!((index.name.as_ref(), index.number_of_shards), ("a", 2));
    for skipped in [
        &br#"{"tombstone":true,"deleted_at":5}"#[..],
        br#"{"alias":"logs","indices":["a"]}"#,
        // A malformed alias is recognised, not decoded.
        br#"{"alias":7,"indices":[]}"#,
    ] {
        assert_eq!(NameRecord::index_only(skipped).unwrap(), None);
    }
    assert!(NameRecord::index_only(br#"{"name":"a"}"#).is_err());
}
