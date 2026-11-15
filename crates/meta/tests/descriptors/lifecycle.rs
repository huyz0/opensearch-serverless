//! AC-6 and AC-8: what each operation does, on every backend.

use std::borrow::Cow;

use oss_blob::SwapOutcome;
use oss_blob::register::ABSENT_GENERATION;
use oss_format::{FormatError, IndexRecord, Tombstone};
use oss_meta::{DeleteMode, MetaError, NameRule, REAP_HANDOFF_MILLIS, Target};

use crate::{BASE, all, counted};

const NOW: i64 = 1_763_164_800_000;

fn record(name: &'static str, uuid: &'static str) -> IndexRecord<'static> {
    IndexRecord {
        mapping: Some(Cow::Borrowed(r#"{"properties":{"m":{"type":"text"}}}"#)),
        ..IndexRecord::new(name, uuid, 2)
    }
}

/// A tombstone's bytes.
fn tombstone(deleted_at: i64, reaping_at: i64) -> Vec<u8> {
    let mut buf = [0u8; 128];
    let n = Tombstone {
        deleted_at,
        reaping_at,
    }
    .write(&mut buf)
    .unwrap();
    buf[..n].to_vec()
}

#[test]
fn create_then_get_returns_the_record_and_a_second_create_is_refused() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let mut buf = vec![0u8; 4096];
        let g = d.create(&record("logs", "u1"), NOW, &mut buf).unwrap();
        assert_eq!(
            d.generation_of("logs", &mut buf).unwrap(),
            g,
            "{:?}",
            b.kind
        );
        assert_eq!(
            d.get("logs", &mut buf).unwrap(),
            Some(record("logs", "u1")),
            "{:?}",
            b.kind
        );
        assert!(
            matches!(
                d.create(&record("logs", "u2"), NOW, &mut buf),
                Err(MetaError::AlreadyExists)
            ),
            "{:?}",
            b.kind
        );
        assert_eq!(
            d.get("logs", &mut buf).unwrap().unwrap().uuid,
            "u1",
            "{:?} unchanged",
            b.kind
        );
        assert_eq!(d.get("other", &mut buf).unwrap(), None, "{:?}", b.kind);
        assert_eq!(
            d.generation_of("other", &mut buf).unwrap(),
            ABSENT_GENERATION
        );
    }
}

#[test]
fn update_applies_only_from_the_current_generation() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let mut buf = vec![0u8; 4096];
        let g = d.create(&record("logs", "u1"), NOW, &mut buf).unwrap();
        let moved = IndexRecord {
            mapping_version: 2,
            ..record("logs", "u1")
        };
        let g2 = d.update(&moved, g, &mut buf).unwrap();
        assert_eq!(g2, Some(g + 1), "{:?}", b.kind);
        let stale = IndexRecord {
            mapping_version: 9,
            ..record("logs", "u1")
        };
        assert_eq!(d.update(&stale, g, &mut buf).unwrap(), None, "{:?}", b.kind);
        assert_eq!(
            d.get("logs", &mut buf).unwrap(),
            Some(moved),
            "{:?}",
            b.kind
        );
        // No register to update.
        assert_eq!(
            d.update(&record("none", "u"), g, &mut buf).unwrap(),
            None,
            "{:?}",
            b.kind
        );
    }
}

#[test]
fn a_deleted_name_is_absent_and_can_be_created_again() {
    for mode in [DeleteMode::Tombstone, DeleteMode::Conditional] {
        for b in all() {
            let d = b.descriptors(mode);
            let mut buf = vec![0u8; 4096];
            let g = d.create(&record("logs", "u1"), NOW, &mut buf).unwrap();
            assert!(
                !d.delete_if_unchanged("logs", g + 1, NOW).unwrap(),
                "{:?} {mode:?} stale",
                b.kind
            );
            assert!(
                d.get("logs", &mut buf).unwrap().is_some(),
                "{:?} {mode:?} kept",
                b.kind
            );
            assert!(
                d.delete_if_unchanged("logs", g, NOW).unwrap(),
                "{:?} {mode:?}",
                b.kind
            );
            assert_eq!(
                d.get("logs", &mut buf).unwrap(),
                None,
                "{:?} {mode:?}",
                b.kind
            );
            let resolved = d.resolve_raw("logs", &mut buf).unwrap();
            assert_eq!(resolved.target, Target::Absent);
            let resolved = resolved.generation;
            let marker = b
                .store
                .head(&format!("{BASE}tombstones/2025111500/logs"))
                .unwrap();
            let g2 = d.create(&record("logs", "u2"), NOW, &mut buf).unwrap();
            match mode {
                DeleteMode::Tombstone => {
                    assert_eq!(resolved, g + 1, "{:?} the tombstone's", b.kind);
                    assert_eq!(marker.map(|m| m.len), Some(0), "{:?} the marker", b.kind);
                    assert_eq!(g2, g + 2, "{:?} generations keep counting", b.kind);
                }
                DeleteMode::Conditional => {
                    assert_eq!(resolved, ABSENT_GENERATION, "{:?}", b.kind);
                    assert_eq!(marker, None, "{:?} no marker", b.kind);
                    assert_ne!(g2, g, "{:?} a fresh generation", b.kind);
                }
            }
            assert_eq!(
                d.get("logs", &mut buf).unwrap().unwrap().uuid,
                "u2",
                "{:?}",
                b.kind
            );
            // A swap from before the delete cannot land on the new index.
            assert_eq!(d.update(&record("logs", "u1"), g, &mut buf).unwrap(), None);
        }
    }
}

#[test]
fn a_name_a_sweep_claimed_under_a_minute_ago_is_refused() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let key = format!("{BASE}indices/logs");
        let claimed = NOW - REAP_HANDOFF_MILLIS;
        b.store
            .create_register(&key, &tombstone(1, claimed + 1))
            .unwrap();
        let mut buf = vec![0u8; 4096];
        assert!(
            matches!(
                d.create(&record("logs", "u1"), NOW, &mut buf),
                Err(MetaError::BeingReclaimed)
            ),
            "{:?}",
            b.kind
        );
        // A minute after the claim, the name is free.
        b.store.delete(&key).unwrap();
        b.store
            .create_register(&key, &tombstone(1, claimed))
            .unwrap();
        d.create(&record("logs", "u1"), NOW, &mut buf).unwrap();
        assert!(d.get("logs", &mut buf).unwrap().is_some(), "{:?}", b.kind);
    }
}

#[test]
fn an_alias_is_not_an_index_and_holds_its_name() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let alias = br#"{"name":"al","alias":{"indices":["logs"]}}"#;
        let g = match b
            .store
            .create_register(&format!("{BASE}indices/al"), alias)
            .unwrap()
        {
            SwapOutcome::Applied(g) => g,
            other => panic!("{other:?}"),
        };
        let mut buf = vec![0u8; 4096];
        assert_eq!(d.get("al", &mut buf).unwrap(), None, "{:?}", b.kind);
        let resolved = d.resolve_raw("al", &mut buf).unwrap();
        assert_eq!(resolved.generation, g);
        assert_eq!(resolved.target, Target::Alias(alias));
        assert!(matches!(
            d.create(&record("al", "u"), NOW, &mut buf),
            Err(MetaError::AlreadyExists)
        ));
        d.create(&record("logs", "u"), NOW, &mut buf).unwrap();
        let resolved = d.resolve_raw("logs", &mut buf).unwrap();
        assert_eq!(resolved.target, Target::Index(record("logs", "u")));
    }
}

#[test]
fn delete_is_idempotent() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let mut buf = vec![0u8; 4096];
        d.create(&record("logs", "u"), NOW, &mut buf).unwrap();
        d.delete("logs").unwrap();
        d.delete("logs").unwrap();
        assert_eq!(
            d.resolve_raw("logs", &mut buf).unwrap().target,
            Target::Absent
        );
    }
}

#[test]
fn a_malformed_register_is_an_error_not_absent() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        b.store
            .create_register(&format!("{BASE}indices/bad"), br#"{"name":"bad""#)
            .unwrap();
        b.store
            .create_register(
                &format!("{BASE}indices/worse"),
                br#"{"tombstone":true,"reaping_at":"x"}"#,
            )
            .unwrap();
        let mut buf = vec![0u8; 4096];
        assert!(matches!(
            d.get("bad", &mut buf),
            Err(MetaError::Format(FormatError::Corrupt { .. }))
        ));
        assert!(matches!(
            d.create(&record("bad", "u"), NOW, &mut buf),
            Err(MetaError::AlreadyExists)
        ));
        assert!(matches!(
            d.create(&record("worse", "u"), NOW, &mut buf),
            Err(MetaError::Format(FormatError::Corrupt { .. }))
        ));
    }
}

// AC-5 at the store: refused before any request.
#[test]
fn an_invalid_name_sends_nothing() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let mut buf = vec![0u8; 4096];
        let (results, n) = counted(b.meter(), || {
            [
                d.create(&record("../x", "u"), NOW, &mut buf).map(drop),
                d.get("Logs", &mut buf).map(drop),
                d.resolve_raw("_x", &mut buf).map(drop),
                d.generation_of("a#b", &mut buf).map(drop),
            ]
        });
        assert_eq!(n, [], "{:?}", b.kind);
        let rules: Vec<_> = results
            .into_iter()
            .map(|r| match r {
                Err(MetaError::InvalidName(e)) => e.rule,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            rules,
            [
                NameRule::FileNameChar,
                NameRule::Uppercase,
                NameRule::Start,
                NameRule::Separator,
            ]
        );
    }
}

// AC-8
#[test]
fn a_short_buffer_is_refused_with_the_length_needed_and_sends_nothing() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let r = record("logs", "u");
        let mut full = vec![0u8; 4096];
        let needed = r.write(&mut full).unwrap();
        let mut short = vec![0u8; needed - 1];
        let (out, n) = counted(b.meter(), || d.create(&r, NOW, &mut short));
        assert_eq!(n, [], "{:?}", b.kind);
        assert!(matches!(
            out,
            Err(MetaError::Format(FormatError::TooLarge { actual, .. })) if actual == needed as u64
        ));
        let g = d.create(&r, NOW, &mut full).unwrap();
        let (out, n) = counted(b.meter(), || d.update(&r, g, &mut short));
        assert_eq!(n, [], "{:?}", b.kind);
        assert!(matches!(
            out,
            Err(MetaError::Format(FormatError::TooLarge { .. }))
        ));
        // A read too long for the buffer is refused by the store, not cut.
        assert!(
            matches!(d.get("logs", &mut short), Err(MetaError::Blob(_))),
            "{:?}",
            b.kind
        );
    }
}

#[test]
fn a_delete_time_without_a_bucket_is_refused_before_any_request() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let mut buf = vec![0u8; 4096];
        let g = d.create(&record("logs", "u"), NOW, &mut buf).unwrap();
        let (out, n) = counted(b.meter(), || d.delete_if_unchanged("logs", g, -1));
        assert_eq!(n, [], "{:?}", b.kind);
        assert!(matches!(out, Err(MetaError::Format(_))));
    }
}

#[test]
fn a_base_must_end_in_a_slash() {
    let b = &all()[0];
    assert!(
        oss_meta::DescriptorStore::new(std::sync::Arc::clone(&b.store), "d", DeleteMode::Tombstone)
            .is_err()
    );
    assert!(
        oss_meta::DescriptorStore::new(std::sync::Arc::clone(&b.store), "", DeleteMode::Tombstone)
            .is_ok()
    );
}

#[test]
fn a_name_a_later_rule_refuses_can_still_be_updated_and_deleted() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let key = format!("{BASE}indices/a#b");
        let mut buf = vec![0u8; 4096];
        let n = IndexRecord::new("a#b", "u", 1).write(&mut buf).unwrap();
        let g = match b.store.create_register(&key, &buf[..n]).unwrap() {
            SwapOutcome::Applied(g) => g,
            other => panic!("{other:?}"),
        };
        let g = d
            .update(&IndexRecord::new("a#b", "u", 2), g, &mut buf)
            .unwrap()
            .unwrap();
        assert!(
            d.delete_if_unchanged("a#b", g, NOW).unwrap(),
            "{:?}",
            b.kind
        );
        d.delete("a#b").unwrap();
        assert!(
            matches!(b.store.read_register(&key, &mut buf), Ok(None)),
            "{:?}",
            b.kind
        );
        // Still one key segment: a path is refused.
        assert!(matches!(d.delete("../x"), Err(MetaError::Format(_))));
    }
}

#[test]
fn a_taken_name_longer_than_the_buffer_is_taken_not_an_error() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let big = "x".repeat(20_000);
        let mut large = vec![0u8; 32_768];
        let held = IndexRecord {
            mapping: Some(Cow::Borrowed(&big)),
            ..IndexRecord::new("logs", "u1", 1)
        };
        d.create(&held, NOW, &mut large).unwrap();
        let mut small = vec![0u8; 256];
        let (out, n) = counted(b.meter(), || {
            d.create(&record("logs", "u2"), NOW, &mut small)
        });
        assert!(
            matches!(out, Err(MetaError::AlreadyExists)),
            "{:?} {out:?}",
            b.kind
        );
        assert_eq!(
            n,
            [(oss_blob::S3Op::Get, 1), (oss_blob::S3Op::Put, 1)],
            "{:?}",
            b.kind
        );
    }
}
