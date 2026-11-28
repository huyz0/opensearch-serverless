//! docs/specs/alerting/SPEC.md, slice 2: a tenant's monitors over the three
//! backends, with exact request counts and the registry's follow-through.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_blob::S3Op::{Delete, Get, Put};
use oss_format::{MAX_MONITORS, MonitorEntry, MonitorIndex, monitor_index_key};
use oss_meta::{AlertRegistry, MetaError, Monitors};
use oss_testkit::budget;

use crate::probe::Quirky;
use crate::{BASE, Backend, Kind, all, counted};

fn monitors(b: &Backend, tenant: &str) -> Monitors {
    Monitors::new(&b.store, BASE, tenant).unwrap()
}

fn registry(b: &Backend) -> AlertRegistry {
    AlertRegistry::new(std::sync::Arc::clone(&b.store), BASE).unwrap()
}

fn put(m: &Monitors, id: &str, body: &str, enabled: bool) {
    m.put(
        id,
        body.as_bytes(),
        enabled,
        "period:1:MINUTES",
        1_000,
        false,
    )
    .unwrap();
}

fn read(m: &Monitors, id: &str) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    m.get(id, &mut buf)
        .unwrap()
        .map(|(_, n)| String::from_utf8(buf[..n].to_vec()).unwrap())
}

#[test]
fn a_monitor_is_put_read_replaced_and_deleted() {
    for b in all() {
        let m = monitors(&b, "acme");
        assert_eq!(read(&m, "m1"), None);
        let first = m
            .put("m1", b"{\"a\":1}", false, "period:1:MINUTES", 5, false)
            .unwrap();
        assert!(!first.replaced);
        assert_eq!(first.version, 1);
        assert_eq!(read(&m, "m1").unwrap(), "{\"a\":1}");
        let second = m
            .put("m1", b"{\"a\":2}", false, "period:5:MINUTES", 6, false)
            .unwrap();
        assert!(second.replaced);
        assert_eq!(second.version, 2);
        let mut buf = vec![0u8; 64];
        let (entry, n) = m.get("m1", &mut buf).unwrap().unwrap();
        assert_eq!(&buf[..n], b"{\"a\":2}");
        assert_eq!(
            (entry.schedule.as_str(), entry.updated_ms),
            ("period:5:MINUTES", 6)
        );
        assert!(m.delete("m1").unwrap());
        assert!(!m.delete("m1").unwrap());
        assert_eq!(read(&m, "m1"), None);
        assert!(m.index().unwrap().is_empty());
    }
}

#[test]
fn a_replaced_or_deleted_body_leaves_no_blob_behind() {
    for b in all() {
        let m = monitors(&b, "acme");
        put(&m, "m1", "one", false);
        let (_, sent) = counted(b.meter(), || put(&m, "m1", "two", false));
        assert!(
            sent.iter().any(|(op, _)| *op == Delete),
            "{:?} {sent:?}",
            b.kind
        );
        let (_, sent) = counted(b.meter(), || {
            m.delete("m1").unwrap();
        });
        assert!(
            sent.iter().any(|(op, _)| *op == Delete),
            "{:?} {sent:?}",
            b.kind
        );
    }
}

#[test]
fn require_absent_refuses_a_present_id_and_leaves_no_blob() {
    for b in all() {
        let m = monitors(&b, "acme");
        put(&m, "m1", "one", false);
        let out = m.put("m1", b"two", false, "p", 1, true);
        assert!(matches!(out, Err(MetaError::AlreadyExists)), "{:?}", b.kind);
        assert_eq!(read(&m, "m1").unwrap(), "one");
    }
}

#[test]
fn a_put_of_a_disabled_monitor_costs_one_read_and_two_writes() {
    for b in all() {
        let m = monitors(&b, "acme");
        let (_, sent) = counted(b.meter(), || put(&m, "m1", "x", false));
        if b.kind == Kind::Fs {
            continue;
        }
        // Read the index, write the body, create the index; the tenant has
        // no enabled monitor, so the registry is asked only to remove (the
        // shard read finds no entry and stops).
        let own = [(Get, 2), (Put, 2)];
        assert_eq!(sent, own, "{:?}", b.kind);
        if b.kind == Kind::Emulator {
            budget::check(
                "alerting.monitor_put.requests",
                sent.iter().map(|(_, n)| n).sum(),
                "a put",
            );
        }
    }
}

#[test]
fn an_enabled_put_registers_the_tenant_with_one_shard_read_and_swap() {
    for b in all() {
        let m = monitors(&b, "acme");
        let r = registry(&b);
        assert!(!r.is_registered("acme").unwrap());
        let (_, sent) = counted(b.meter(), || put(&m, "m1", "x", true));
        assert!(r.is_registered("acme").unwrap(), "{:?}", b.kind);
        if b.kind == Kind::Emulator {
            // index read, body put, index create, shard read, shard create.
            assert_eq!(sent, [(Get, 2), (Put, 3)], "{:?}", b.kind);
        }
    }
}

#[test]
fn deleting_the_last_enabled_monitor_removes_the_tenant_but_not_while_another_is_enabled() {
    for b in all() {
        let m = monitors(&b, "acme");
        let r = registry(&b);
        put(&m, "m1", "x", true);
        put(&m, "m2", "y", true);
        assert!(m.delete("m1").unwrap());
        assert!(r.is_registered("acme").unwrap(), "{:?}", b.kind);
        assert!(m.delete("m2").unwrap());
        assert!(!r.is_registered("acme").unwrap(), "{:?}", b.kind);
    }
}

#[test]
fn disabling_the_last_enabled_monitor_removes_the_tenant_and_enabling_adds_it_back() {
    for b in all() {
        let m = monitors(&b, "acme");
        let r = registry(&b);
        put(&m, "m1", "x", true);
        put(&m, "m1", "x", false);
        assert!(!r.is_registered("acme").unwrap(), "{:?}", b.kind);
        put(&m, "m1", "x", true);
        assert!(r.is_registered("acme").unwrap(), "{:?}", b.kind);
        // A disabled monitor beside an enabled one keeps the tenant.
        put(&m, "m2", "y", false);
        assert!(r.is_registered("acme").unwrap(), "{:?}", b.kind);
    }
}

#[test]
fn tenants_keep_their_own_monitors_and_share_a_shard() {
    for b in all() {
        let a = monitors(&b, "acme");
        let z = monitors(&b, "zenith");
        put(&a, "m1", "a-body", true);
        put(&z, "m1", "z-body", true);
        assert_eq!(read(&a, "m1").unwrap(), "a-body");
        assert_eq!(read(&z, "m1").unwrap(), "z-body");
        let r = registry(&b);
        assert!(r.is_registered("acme").unwrap() && r.is_registered("zenith").unwrap());
    }
}

#[test]
fn a_body_over_its_bound_or_an_invalid_id_sends_nothing() {
    for b in all() {
        let m = monitors(&b, "acme");
        let (_, sent) = counted(b.meter(), || {
            let big = vec![b'x'; oss_format::MAX_MONITOR_BYTES + 1];
            assert!(matches!(
                m.put("m1", &big, true, "p", 1, false),
                Err(MetaError::InvalidArgument(_))
            ));
        });
        assert!(sent.is_empty(), "{:?}", b.kind);
        assert!(Monitors::new(&b.store, BASE, "a/b").is_err());
        assert!(Monitors::new(&b.store, "no-slash", "acme").is_err());
    }
}

#[test]
fn a_bad_id_leaves_no_body_behind() {
    for b in all() {
        let m = monitors(&b, "acme");
        let out = m.put("a/b", b"x", true, "p", 1, false);
        assert!(matches!(out, Err(MetaError::Format(_))), "{:?}", b.kind);
        assert!(m.index().unwrap().is_empty());
    }
}

#[test]
fn a_body_that_does_not_fit_the_callers_buffer_is_an_error() {
    let b = &all()[0];
    let m = monitors(b, "acme");
    put(&m, "m1", "0123456789", false);
    assert!(m.get("m1", &mut [0u8; 4]).is_err());
}

#[test]
fn the_index_names_every_monitor_with_its_flag_and_schedule() {
    for b in all() {
        let m = monitors(&b, "acme");
        put(&m, "m1", "x", true);
        put(&m, "m2", "y", false);
        let index = m.index().unwrap();
        assert_eq!(index.len(), 2, "{:?}", b.kind);
        assert!(index.any_enabled());
        assert!(index.get("m1").unwrap().enabled && !index.get("m2").unwrap().enabled);
        assert_eq!(index.get("m2").unwrap().schedule, "period:1:MINUTES");
        assert_eq!(index.version, 2);
    }
}

#[test]
fn a_body_of_exactly_the_bound_is_stored() {
    let b = &all()[0];
    let m = monitors(b, "acme");
    let body = vec![b'x'; oss_format::MAX_MONITOR_BYTES];
    m.put("m1", &body, false, "p", 1, false).unwrap();
    let mut buf = vec![0u8; oss_format::MAX_MONITOR_BYTES];
    let (_, n) = m.get("m1", &mut buf).unwrap().unwrap();
    assert_eq!(n, oss_format::MAX_MONITOR_BYTES);
}

#[test]
fn a_disabled_put_beside_an_enabled_monitor_does_not_touch_the_registry() {
    for b in all() {
        if b.kind == Kind::Fs {
            continue;
        }
        let m = monitors(&b, "acme");
        put(&m, "m1", "x", true);
        let (_, sent) = counted(b.meter(), || put(&m, "m2", "y", false));
        // Index read, body write, index swap: no shard read, no index re-read.
        assert_eq!(sent, [(Get, 1), (Put, 2)], "{:?}", b.kind);
    }
}

fn emulator_budget(b: &Backend, key: &str, sent: &[(oss_blob::S3Op, u64)], what: &str) {
    if b.kind == Kind::Emulator {
        budget::check(key, sent.iter().map(|(_, n)| n).sum(), what);
    }
}

#[test]
fn a_replacing_put_a_delete_and_a_get_have_exact_request_counts() {
    for b in all() {
        if b.kind == Kind::Fs {
            continue;
        }
        let m = monitors(&b, "acme");
        put(&m, "m1", "one", false);
        // Index read, new body, index swap, old body's delete, then the
        // registry's removal check (one shard read).
        let (_, sent) = counted(b.meter(), || put(&m, "m1", "two", false));
        assert_eq!(sent, [(Get, 2), (Put, 2), (Delete, 1)], "{:?}", b.kind);
        emulator_budget(&b, "alerting.monitor_replace.requests", &sent, "a replace");
        let mut buf = vec![0u8; 64];
        let (_, sent) = counted(b.meter(), || {
            m.get("m1", &mut buf).unwrap();
        });
        assert_eq!(sent, [(Get, 2)], "{:?}", b.kind);
        emulator_budget(&b, "alerting.monitor_get.requests", &sent, "a get");
        // Index read and swap, the body's delete, the shard read.
        let (_, sent) = counted(b.meter(), || {
            m.delete("m1").unwrap();
        });
        assert_eq!(sent, [(Get, 2), (Put, 1), (Delete, 1)], "{:?}", b.kind);
        emulator_budget(&b, "alerting.monitor_delete.requests", &sent, "a delete");
    }
}

/// Another writer's enabled put lands after the delete's index swap and
/// before its shard read: the removal's question sees it and the tenant
/// stays registered.
#[test]
fn a_put_landing_before_the_removals_shard_read_keeps_the_tenant() {
    let key = monitor_index_key(BASE, "acme").unwrap();
    let store = Arc::new(Quirky::hooked(move |inner| {
        let mut buf = vec![0u8; oss_format::MAX_MONITOR_INDEX_BYTES];
        let read = inner.read_register(&key, &mut buf).unwrap().unwrap();
        let mut index = MonitorIndex::parse(&buf[..read.len]).unwrap();
        index
            .insert(
                "m2",
                MonitorEntry {
                    content: "theirs".to_owned(),
                    enabled: true,
                    schedule: "period:1:MINUTES".to_owned(),
                    updated_ms: 1,
                },
            )
            .unwrap();
        let n = index.write(&mut buf).unwrap();
        inner
            .swap_register(&key, read.generation, &buf[..n])
            .unwrap();
    }));
    let dynamic: Arc<dyn BlobStore> = Arc::clone(&store) as Arc<dyn BlobStore>;
    let m = Monitors::new(&dynamic, BASE, "acme").unwrap();
    let r = AlertRegistry::new(Arc::clone(&dynamic), BASE).unwrap();
    put(&m, "m1", "x", true);
    assert!(r.is_registered("acme").unwrap());
    // Requests of the delete: index read, index swap, body delete, then the
    // registry's shard read, where the hook runs first.
    store.arm(4);
    assert!(m.delete("m1").unwrap());
    assert!(m.index().unwrap().get("m2").unwrap().enabled);
    assert!(r.is_registered("acme").unwrap());
}

#[test]
fn a_full_index_refuses_a_new_monitor_and_leaves_no_body() {
    let b = &all()[0];
    let m = monitors(b, "acme");
    for i in 0..MAX_MONITORS {
        put(&m, &format!("m{i}"), "x", false);
    }
    let out = m.put("extra", b"x", false, "p", 1, false);
    assert!(matches!(out, Err(MetaError::Format(_))), "{out:?}");
    assert_eq!(m.index().unwrap().len(), MAX_MONITORS);
    // A replacement of an existing id still lands.
    m.put("m0", b"y", false, "p", 1, false).unwrap();
}
