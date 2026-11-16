//! docs/specs/cluster-config/SPEC.md, AC-3 and AC-4: the cluster config
//! register.

use std::sync::Arc;

use oss_blob::S3Op::{Get, GetRange, Put};
use oss_blob::register::ABSENT_GENERATION;
use oss_blob::{BlobError, BlobStore};
use oss_meta::{ClusterConfig, ClusterSettings, MetaError, Setting, SettingsRefused};

use crate::probe::{Quirk, Quirky};
use crate::{BASE, Backend, all, counted};

// What a Java node stored after two updates (fixtures/cluster-config).
const JAVA_STORED: &str = r#"{"cluster":{"routing":{"allocation":{"enable":"all"}}},"indices":{"recovery":{"max_bytes_per_sec":"50mb"}},"search":{"max_buckets":"20000"}}"#;

fn config(b: &Backend) -> ClusterConfig {
    ClusterConfig::new(Arc::clone(&b.store), BASE).unwrap()
}

fn value(text: &str) -> Setting {
    Setting::Value(text.to_owned())
}

#[test]
fn nothing_set_reads_empty_in_one_request() {
    for b in all() {
        let c = config(&b);
        let (got, sent) = counted(b.meter(), || c.read(&mut [0u8; 1024]).unwrap());
        assert_eq!(got.generation, ABSENT_GENERATION);
        assert_eq!(got.settings, ClusterSettings::default());
        assert_eq!(sent, [(Get, 1)], "{:?}", b.kind);
    }
}

#[test]
fn updates_create_then_swap_and_merge() {
    for b in all() {
        let c = config(&b);
        let mut buf = [0u8; 1024];
        let (written, sent) = counted(b.meter(), || {
            c.update(&[("a", Some("1")), ("b", Some("2"))], &mut buf)
                .unwrap()
        });
        assert_eq!(sent, [(Get, 1), (Put, 1)], "{:?}", b.kind);
        let first = c.read(&mut buf).unwrap();
        assert_ne!(first.generation, ABSENT_GENERATION);
        assert_eq!(first.settings, written);

        let (written, sent) = counted(b.meter(), || {
            c.update(&[("b", None), ("c", Some("3"))], &mut buf)
                .unwrap()
        });
        let swap = b.budget(&[(Get, 1), (Put, 1)], &[(Get, 1), (GetRange, 1), (Put, 1)]);
        assert_eq!(sent, swap, "{:?}", b.kind);
        let second = c.read(&mut buf).unwrap();
        assert_ne!(second.generation, first.generation);
        assert_eq!(second.settings, written);
        assert_eq!(
            second.settings.settings().iter().collect::<Vec<_>>(),
            [
                (&"a".to_owned(), &value("1")),
                (&"c".to_owned(), &value("3"))
            ]
        );
    }
}

#[test]
fn a_value_a_java_node_stored_is_read_and_updated() {
    for b in all() {
        let c = config(&b);
        b.store
            .create_register(
                &format!("{BASE}cluster/config/settings"),
                JAVA_STORED.as_bytes(),
            )
            .unwrap();
        let mut buf = [0u8; 1024];
        let read = c.read(&mut buf).unwrap();
        assert_eq!(
            read.settings.get("indices.recovery.max_bytes_per_sec"),
            Some(&value("50mb"))
        );
        c.update(&[("search.max_buckets", None)], &mut buf).unwrap();
        // The flat form the Java's `flat_settings` writes for the same
        // settings, which a Java node reads back as them.
        let generation = b
            .store
            .read_register(&format!("{BASE}cluster/config/settings"), &mut buf)
            .unwrap()
            .unwrap();
        assert_eq!(
            std::str::from_utf8(&buf[..generation.len]).unwrap(),
            r#"{"cluster.routing.allocation.enable":"all","indices.recovery.max_bytes_per_sec":"50mb"}"#
        );
    }
}

#[test]
fn an_update_retries_while_others_move_the_register() {
    let store = Arc::new(Quirky::new(Quirk::ConflictSwaps(3)));
    let c = ClusterConfig::new(Arc::clone(&store) as Arc<dyn BlobStore>, BASE).unwrap();
    let mut buf = [0u8; 1024];
    let (_, sent) = counted(store.meter(), || {
        c.update(&[("a", Some("1"))], &mut buf).unwrap()
    });
    // Three lost attempts and the one that lands, the lost ones refused
    // before they reach the store, with no pause between.
    assert_eq!(sent, [(Get, 4), (Put, 1)]);
    assert_eq!(
        c.read(&mut buf).unwrap().settings.get("a"),
        Some(&value("1"))
    );

    let store = Arc::new(Quirky::new(Quirk::ConflictSwaps(u32::MAX)));
    let c = ClusterConfig::new(Arc::clone(&store) as Arc<dyn BlobStore>, BASE).unwrap();
    let started = std::time::Instant::now();
    let (got, sent) = counted(store.meter(), || c.update(&[("a", Some("1"))], &mut buf));
    assert!(matches!(got, Err(MetaError::Contended)), "{got:?}");
    assert_eq!(sent, [(Get, 10)]);
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn an_update_the_java_would_refuse_writes_nothing() {
    for b in all() {
        let c = config(&b);
        let mut buf = [0u8; 1024];
        c.update(&[("y", Some("1"))], &mut buf).unwrap();
        let before = c.read(&mut buf).unwrap();
        for asked in [
            &[("x.0", Some("p"))][..],
            &[("z", Some("s")), ("z.0", Some("q"))],
        ] {
            let (got, sent) = counted(b.meter(), || c.update(asked, &mut buf));
            assert!(
                matches!(
                    &got,
                    Err(MetaError::SettingsRefused(
                        SettingsRefused::List { .. } | SettingsRefused::Conflict { .. }
                    ))
                ),
                "{got:?}"
            );
            assert_eq!(sent, [(Get, 1)], "{:?}", b.kind);
        }
        assert_eq!(c.read(&mut buf).unwrap(), before);
    }
}

#[test]
fn a_value_too_long_for_the_buffer_is_refused() {
    for b in all() {
        let c = config(&b);
        let mut buf = [0u8; 1024];
        c.update(&[("k", Some(&"v".repeat(100)))], &mut buf)
            .unwrap();
        // Reading into a short buffer is the store's refusal.
        assert!(
            matches!(
                c.read(&mut [0u8; 16]),
                Err(MetaError::Blob(BlobError::InvalidArgument(_)))
            ),
            "{:?}",
            b.kind
        );
        // Writing a longer value than fits says how long it must be.
        let (got, sent) = counted(b.meter(), || {
            c.update(&[("j", Some(&"w".repeat(1000)))], &mut buf)
        });
        assert!(
            matches!(
                got,
                Err(MetaError::Format(oss_format::FormatError::TooLarge { .. }))
            ),
            "{got:?}"
        );
        assert_eq!(sent, [(Get, 1)], "{:?}", b.kind);
    }
}

#[test]
fn a_malformed_value_or_base_is_refused() {
    for b in all() {
        b.store
            .create_register(&format!("{BASE}cluster/config/settings"), b"[]")
            .unwrap();
        assert!(matches!(
            config(&b).read(&mut [0u8; 64]),
            Err(MetaError::Format(_))
        ));
        assert!(matches!(
            ClusterConfig::new(Arc::clone(&b.store), "no-slash"),
            Err(MetaError::Format(_))
        ));
    }
}
