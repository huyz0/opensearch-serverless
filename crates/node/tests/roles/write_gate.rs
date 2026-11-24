//! docs/specs/tenant-write-pacing: a writer role asks its write gate before it
//! applies a group.

#![cfg(feature = "writer")]

use std::sync::Arc;

use oss_writer::bulk::ItemResult;

use super::writer_role::{bulk, docs, open, puts, rig, written};

// docs/specs/tenant-write-pacing AC-3, AC-4
#[test]
fn a_group_the_write_gate_refuses_is_neither_applied_nor_logged_and_asks_for_a_retry() {
    let r = rig();
    open(&r, "logs", 1);
    let refuse = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (flag, log) = (Arc::clone(&refuse), Arc::clone(&asked));
    assert!(r.role.set_write_gate(Arc::new(move |index, docs| {
        log.lock().expect("lock").push((index.to_owned(), docs));
        if flag.load(std::sync::atomic::Ordering::SeqCst) {
            Err(2_500)
        } else {
            Ok(0)
        }
    })));
    assert!(!r.role.set_write_gate(Arc::new(|_, _| Ok(0))), "one gate");
    assert_eq!(r.role.retry_after_secs(), None, "nothing refused yet");
    let before = puts(&r);
    let items = bulk(
        &r,
        "logs",
        &docs(["a".to_owned(), "b".to_owned()].into_iter()),
    );
    assert_eq!(items.len(), 2);
    for item in &items {
        let ItemResult::Failed { status, error } = &item.result else {
            panic!("refused");
        };
        assert_eq!(*status, 429);
        assert!(
            error.reason().contains("over its cost budget"),
            "{}",
            error.reason()
        );
    }
    assert_eq!(puts(&r), before, "nothing logged");
    assert_eq!(*asked.lock().expect("lock"), [("logs".to_owned(), 2)]);
    assert_eq!(r.role.retry_after_secs(), Some(3), "2.5 s rounds up");
    // A write the gate lets through clears the hint and is applied.
    refuse.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        written(&bulk(&r, "logs", &docs(std::iter::once("c".to_owned())))),
        1
    );
    assert_eq!(r.role.retry_after_secs(), None);
}

// docs/specs/tenant-write-pacing AC-3
#[test]
fn a_write_the_gate_holds_waits_before_it_is_applied() {
    let r = rig();
    open(&r, "logs", 1);
    assert!(r.role.set_write_gate(Arc::new(|_, _| Ok(150))));
    let began = std::time::Instant::now();
    assert_eq!(
        written(&bulk(&r, "logs", &docs(std::iter::once("a".to_owned())))),
        1
    );
    assert!(began.elapsed() >= std::time::Duration::from_millis(150));
}
