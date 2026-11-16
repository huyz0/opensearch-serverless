//! docs/specs/alias-records/SPEC.md, AC-3 and AC-4: alias records in the
//! descriptor namespace.

use std::borrow::Cow;

use oss_blob::S3Op::{Get, GetRange, Put};
use oss_blob::register::ABSENT_GENERATION;
use oss_format::{AliasMutation, IndexRecord, Tombstone};
use oss_meta::{AliasRecord, DeleteMode, MetaError, NameRule, Target};

use crate::{BASE, all, counted};

const NOW: i64 = 1_763_164_800_000;

// A data stream as a Java node stores it (fixtures/aliases/parsed.txt).
const JAVA_STREAM: &[u8] = br#"{"alias":"ds","indices":["ds-1","ds-2"],"data_stream":true,"generation":3,"timestamp_field":"@timestamp"}"#;

fn pending(name: &'static str, members: &[&'static str], creating: bool) -> AliasRecord<'static> {
    AliasRecord {
        mutation: Some(AliasMutation {
            until: NOW,
            creating,
            additions: vec![Cow::Borrowed("added")],
        }),
        ..AliasRecord::new(name, members)
    }
}

#[test]
fn an_alias_is_created_in_one_request_and_resolves() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let mut buf = vec![0u8; 4096];
        let alias = AliasRecord::new("logs", &["logs-1", "logs-2"]);
        let (g, sent) = counted(b.meter(), || d.create_alias(&alias, NOW, &mut buf).unwrap());
        assert_eq!(sent, [(Put, 1)], "{:?}", b.kind);
        let (resolved, sent) = counted(b.meter(), || d.resolve("logs", &mut buf).unwrap());
        assert_eq!(sent, [(Get, 1)], "{:?}", b.kind);
        assert_eq!(resolved.generation, g);
        assert_eq!(resolved.target, Target::Alias(alias));
        // An alias is not an index.
        assert_eq!(d.get("logs", &mut buf).unwrap(), None);
    }
}

#[test]
fn an_alias_and_an_index_cannot_share_a_name() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let mut buf = vec![0u8; 4096];
        d.create(&IndexRecord::new("logs", "u", 1), NOW, &mut buf)
            .unwrap();
        let (got, sent) = counted(b.meter(), || {
            d.create_alias(&AliasRecord::new("logs", &["x"]), NOW, &mut buf)
        });
        assert!(matches!(got, Err(MetaError::AlreadyExists)), "{got:?}");
        // The create, then a tombstone-sized read of what took the name.
        assert_eq!(
            sent,
            b.budget(&[(Get, 1), (Put, 1)], &[(Get, 1), (Put, 1)]),
            "{:?}",
            b.kind
        );
        d.create_alias(&AliasRecord::new("al", &["logs"]), NOW, &mut buf)
            .unwrap();
        assert!(matches!(
            d.create(&IndexRecord::new("al", "u", 1), NOW, &mut buf),
            Err(MetaError::AlreadyExists)
        ));
        assert!(matches!(
            d.create_alias(&AliasRecord::new("al", &["x"]), NOW, &mut buf),
            Err(MetaError::AlreadyExists)
        ));
    }
}

#[test]
fn an_alias_takes_over_a_deleted_indexs_tombstone() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let mut tomb = [0u8; 128];
        let n = Tombstone {
            deleted_at: 1,
            reaping_at: 0,
        }
        .write(&mut tomb)
        .unwrap();
        b.store
            .create_register(&format!("{BASE}indices/logs"), &tomb[..n])
            .unwrap();
        let mut buf = vec![0u8; 4096];
        let alias = AliasRecord::new("logs", &["x"]);
        let (_, sent) = counted(b.meter(), || d.create_alias(&alias, NOW, &mut buf).unwrap());
        assert_eq!(
            sent,
            b.budget(&[(Get, 1), (Put, 2)], &[(Get, 1), (GetRange, 1), (Put, 2)]),
            "{:?}",
            b.kind
        );
        assert_eq!(
            d.resolve("logs", &mut buf).unwrap().target,
            Target::Alias(alias)
        );
    }
}

#[test]
fn an_alias_being_created_resolves_as_absent() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let mut buf = vec![0u8; 4096];
        let creating = pending("logs", &["logs-1"], true);
        let g = d.create_alias(&creating, NOW, &mut buf).unwrap();
        let resolved = d.resolve("logs", &mut buf).unwrap();
        assert_eq!((resolved.generation, resolved.target), (g, Target::Absent));
        assert_eq!(
            d.resolve_raw("logs", &mut buf).unwrap().target,
            Target::Alias(creating)
        );
        // An update in progress on a published alias still resolves to it.
        let updating = pending("other", &["logs-1"], false);
        d.create_alias(&updating, NOW, &mut buf).unwrap();
        assert_eq!(
            d.resolve("other", &mut buf).unwrap().target,
            Target::Alias(updating)
        );
        // Nothing at all.
        let none = d.resolve("none", &mut buf).unwrap();
        assert_eq!(
            (none.generation, none.target),
            (ABSENT_GENERATION, Target::Absent)
        );
    }
}

#[test]
fn an_alias_moves_by_one_swap_from_the_generation_read() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let mut buf = vec![0u8; 4096];
        let g = d
            .create_alias(&AliasRecord::new("logs", &["old"]), NOW, &mut buf)
            .unwrap();
        let moved = AliasRecord::new("logs", &["new"]);
        let (next, sent) = counted(b.meter(), || d.update_alias(&moved, g, &mut buf).unwrap());
        assert_eq!(
            sent,
            b.budget(&[(Put, 1)], &[(GetRange, 1), (Put, 1)]),
            "{:?}",
            b.kind
        );
        let next = next.unwrap();
        assert_ne!(next, g);
        assert_eq!(
            d.resolve("logs", &mut buf).unwrap().target,
            Target::Alias(moved)
        );
        // A writer that read before the move loses.
        assert_eq!(
            d.update_alias(&AliasRecord::new("logs", &["stale"]), g, &mut buf)
                .unwrap(),
            None
        );
        assert_eq!(d.resolve("logs", &mut buf).unwrap().generation, next);
    }
}

#[test]
fn a_data_stream_a_java_node_stored_resolves() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        b.store
            .create_register(&format!("{BASE}indices/ds"), JAVA_STREAM)
            .unwrap();
        let mut buf = vec![0u8; 4096];
        let Target::Alias(stream) = d.resolve("ds", &mut buf).unwrap().target else {
            panic!("not an alias");
        };
        assert!(stream.data_stream);
        assert_eq!(stream.generation, 3);
        assert_eq!(stream.writes_to(), Some("ds-2"));
        let mut out = vec![0u8; 4096];
        let n = stream.write(&mut out).unwrap();
        assert_eq!(&out[..n], JAVA_STREAM);
    }
}

#[test]
fn a_bad_name_or_record_sends_nothing() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let mut buf = vec![0u8; 4096];
        let (got, sent) = counted(b.meter(), || {
            [
                d.create_alias(&AliasRecord::new("Logs", &["x"]), NOW, &mut buf),
                d.create_alias(&AliasRecord::new("logs", &[]), NOW, &mut buf),
            ]
        });
        assert!(sent.is_empty(), "{sent:?}");
        assert!(
            matches!(
                &got[0],
                Err(MetaError::InvalidName(e)) if e.rule == NameRule::Uppercase
            ),
            "{:?}",
            got[0]
        );
        assert!(matches!(got[1], Err(MetaError::Format(_))), "{:?}", got[1]);
        let (got, sent) = counted(b.meter(), || {
            d.update_alias(&AliasRecord::new("logs", &[]), 1, &mut buf)
        });
        assert!(sent.is_empty() && matches!(got, Err(MetaError::Format(_))));
    }
}
