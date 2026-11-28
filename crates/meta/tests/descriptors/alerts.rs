//! docs/specs/alerting/SPEC.md, slice 6: a monitor's alert register over the
//! three backends, with its request counts.

use oss_blob::BlobStore;
use oss_blob::S3Op::{Get, Put};
use oss_format::MAX_ALERT_SET_BYTES;
use oss_meta::{Alerts, MetaError};
use oss_testkit::budget;

use crate::{BASE, Backend, Kind, all, counted};

fn alerts(b: &Backend, monitor: &str) -> Alerts {
    Alerts::new(&b.store, BASE, "acme", monitor).unwrap()
}

fn set(text: &str) -> Result<Option<Vec<u8>>, MetaError> {
    Ok(Some(text.as_bytes().to_vec()))
}

#[test]
fn a_monitor_with_no_alert_reads_empty_and_a_change_creates_then_swaps() {
    for b in all() {
        let a = alerts(&b, "m1");
        assert!(a.read().unwrap().is_empty(), "{:?}", b.kind);
        assert!(
            a.change(|cur| {
                assert!(cur.is_empty());
                set("{\"alerts\":[1]}")
            })
            .unwrap()
        );
        assert_eq!(a.read().unwrap(), b"{\"alerts\":[1]}", "{:?}", b.kind);
        assert!(
            a.change(|cur| {
                assert_eq!(cur, b"{\"alerts\":[1]}");
                set("{\"alerts\":[1,2]}")
            })
            .unwrap()
        );
        assert_eq!(a.read().unwrap(), b"{\"alerts\":[1,2]}", "{:?}", b.kind);
        assert!(alerts(&b, "m2").read().unwrap().is_empty());
    }
}

#[test]
fn an_edit_that_returns_nothing_writes_nothing() {
    for b in all() {
        if b.kind == Kind::Fs {
            continue;
        }
        let a = alerts(&b, "m1");
        let (written, sent) = counted(b.meter(), || a.change(|_| Ok(None)).unwrap());
        assert!(!written);
        assert_eq!(sent, [(Get, 1)], "{:?}", b.kind);
        let (_, sent) = counted(b.meter(), || a.change(|_| set("{}")).unwrap());
        assert_eq!(sent, [(Get, 1), (Put, 1)], "{:?}", b.kind);
        if b.kind == Kind::Emulator {
            budget::check(
                "alerting.alerts_change.requests",
                2,
                "a change of an alert set",
            );
            let (_, read) = counted(b.meter(), || a.read().unwrap());
            budget::check(
                "alerting.alerts_read.requests",
                read.iter().map(|(_, n)| n).sum(),
                "a read of an alert set",
            );
        }
    }
}

#[test]
fn a_set_over_its_bound_is_refused_and_the_old_set_stays() {
    for b in all() {
        let a = alerts(&b, "m1");
        a.change(|_| set("{}")).unwrap();
        let big = vec![b'a'; MAX_ALERT_SET_BYTES + 1];
        let err = a.change(|_| Ok(Some(big.clone()))).unwrap_err();
        assert!(matches!(err, MetaError::Format(_)), "{err:?}");
        assert_eq!(a.read().unwrap(), b"{}", "{:?}", b.kind);
    }
}

#[test]
fn a_swap_lost_to_another_writer_reads_again() {
    for b in all() {
        let a = alerts(&b, "m1");
        a.change(|_| set("{\"n\":0}")).unwrap();
        let rival = alerts(&b, "m1");
        let mut first = true;
        a.change(|cur| {
            if std::mem::take(&mut first) {
                rival.change(|_| set("{\"n\":1}")).unwrap();
            }
            assert!(cur == b"{\"n\":0}" || cur == b"{\"n\":1}");
            set("{\"n\":2}")
        })
        .unwrap();
        assert_eq!(a.read().unwrap(), b"{\"n\":2}", "{:?}", b.kind);
    }
}

#[test]
fn a_tenant_and_a_monitor_have_their_own_register() {
    for b in all() {
        alerts(&b, "m1").change(|_| set("{\"m\":1}")).unwrap();
        let other = Alerts::new(&b.store, BASE, "other", "m1").unwrap();
        assert!(other.read().unwrap().is_empty());
        assert!(Alerts::new(&b.store, BASE, "acme", "a/b").is_err());
        let _: &dyn BlobStore = &*b.store;
    }
}
