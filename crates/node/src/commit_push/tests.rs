//! The commit sink: the local take, the bounded queue.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

use oss_transport::actions::push::PushRequest;

use super::*;
use crate::push_sender::LinkError;

struct Locals(Mutex<Vec<(String, u32)>>);

impl LocalTake for Locals {
    fn take(&self, index: &str, _: &str, shard: u32, _: &[u8]) -> bool {
        self.0.lock().expect("lock").push((index.to_owned(), shard));
        true
    }
}

struct One;

impl PushTargets for One {
    fn readers(&self, _: &str, _: u32) -> Vec<String> {
        vec!["reader".to_owned()]
    }
}

/// Blocks each send until released, counting those that got through.
struct Gate {
    entered: SyncSender<()>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    sent: Arc<AtomicUsize>,
}

impl PushLink for Gate {
    fn send(&self, _: &str, _: &PushRequest) -> Result<bool, LinkError> {
        let _ = self.entered.send(());
        let _ = self.release.lock().expect("lock").recv();
        self.sent.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }
}

#[test]
fn a_commit_is_taken_locally_at_once_and_sent_by_the_worker() {
    let locals = Arc::new(Locals(Mutex::new(Vec::new())));
    let (entered, entered_rx) = sync_channel(512);
    let (release_tx, release) = sync_channel(512);
    let sent = Arc::new(AtomicUsize::new(0));
    let link = Gate {
        entered,
        release: Mutex::new(release),
        sent: Arc::clone(&sent),
    };
    let push = CommitPush::start(
        PushSender::new(One, link, true),
        Arc::clone(&locals) as Arc<dyn LocalTake>,
        true,
    )
    .expect("starts");
    push.published("logs", "u1", 3, b"commit", false);
    // The local take came before the send, which is still held.
    assert_eq!(*locals.0.lock().expect("lock"), [("logs".to_owned(), 3)]);
    entered_rx.recv().expect("the worker is sending");
    assert_eq!(sent.load(Ordering::SeqCst), 0);
    release_tx.send(()).expect("released");
    push.stop();
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

#[test]
fn a_full_queue_drops_the_commit_and_the_publisher_never_waits() {
    let locals = Arc::new(Locals(Mutex::new(Vec::new())));
    let (entered, entered_rx) = sync_channel(512);
    let (release_tx, release) = sync_channel(512);
    let link = Gate {
        entered,
        release: Mutex::new(release),
        sent: Arc::new(AtomicUsize::new(0)),
    };
    let push = CommitPush::start(
        PushSender::new(One, link, true),
        Arc::clone(&locals) as Arc<dyn LocalTake>,
        true,
    )
    .expect("starts");
    // One in the worker's hands, the queue full behind it, then three over.
    push.published("logs", "u1", 0, b"c", false);
    entered_rx.recv().expect("the worker holds one");
    for shard in 1..=u32::try_from(QUEUE + 3).expect("fits") {
        push.published("logs", "u1", shard, b"c", false);
    }
    assert_eq!(push.dropped(), 3);
    // Every one was taken locally, dropped or not.
    assert_eq!(locals.0.lock().expect("lock").len(), QUEUE + 4);
    for _ in 0..=QUEUE {
        release_tx.send(()).expect("released");
    }
    push.stop();
}

#[test]
fn after_stop_nothing_is_queued() {
    let locals = Arc::new(Locals(Mutex::new(Vec::new())));
    let (entered, _entered_rx) = sync_channel(512);
    let (_release_tx, release) = sync_channel(512);
    let link = Gate {
        entered,
        release: Mutex::new(release),
        sent: Arc::new(AtomicUsize::new(0)),
    };
    let push = CommitPush::start(
        PushSender::new(One, link, true),
        Arc::clone(&locals) as Arc<dyn LocalTake>,
        true,
    )
    .expect("starts");
    push.stop();
    push.published("logs", "u1", 0, b"c", false);
    assert_eq!(push.dropped(), 0, "stopped is not full");
}

#[test]
fn without_the_send_only_a_takeovers_first_commit_goes_to_the_readers() {
    let locals = Arc::new(Locals(Mutex::new(Vec::new())));
    let (entered, _entered_rx) = sync_channel(512);
    let (release_tx, release) = sync_channel(512);
    let sent = Arc::new(AtomicUsize::new(0));
    let link = Gate {
        entered,
        release: Mutex::new(release),
        sent: Arc::clone(&sent),
    };
    let push = CommitPush::start(
        PushSender::new(One, link, true),
        Arc::clone(&locals) as Arc<dyn LocalTake>,
        false,
    )
    .expect("starts");
    push.published("logs", "u1", 0, b"routine", false);
    push.published("logs", "u1", 1, b"taken", true);
    // Both were taken by the node's own reader; one was queued.
    assert_eq!(locals.0.lock().expect("lock").len(), 2);
    release_tx.send(()).expect("released");
    push.stop();
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}
