//! docs/specs/restore-background: a restore not asked to wait answers once
//! its marked descriptors exist and copies on a thread, and gives back
//! everything it took when it fails, is cancelled or finishes.

use crate::snapshot_restore::{Rig, requests, restored_uuid, rig_full, seed, send};
use crate::support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_blob::emulator::hold::{Hold, Release};
use oss_blob::{BlobStore, S3Op};
use oss_mem::Pool;
use oss_node::restore_background::Settings;
use oss_node::stock_restore_route::StockRestoreSettings;
use oss_testkit::budget;

const NO_WAIT: &str = "/_snapshot/backups/daily/_restore";
const COPY: &str = r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#;
const WAIT: Duration = Duration::from_secs(10);

fn settings(max: usize, grace_millis: u64) -> Settings {
    Settings {
        enabled: true,
        max,
        grace: Duration::from_millis(grace_millis),
    }
}

fn background(max: usize, grace_millis: u64) -> Rig {
    seed(rig_full(
        true,
        "",
        64 << 20,
        64,
        StockRestoreSettings::off(),
        settings(max, grace_millis),
    ))
}

fn writes(r: &Rig) -> u64 {
    [S3Op::Put, S3Op::Copy]
        .iter()
        .map(|op| r.store.meter().count(*op))
        .sum()
}

fn pool(r: &Rig) -> usize {
    r.budget.in_use(Pool::RequestBodies)
}

/// The pool once the request that was answered has ended too: its own
/// reservation goes when the connection's handler returns. Never below the
/// `floor` the rig itself holds.
fn settled(r: &Rig, floor: usize) -> usize {
    let start = Instant::now();
    while pool(r) > floor && start.elapsed() < WAIT {
        std::thread::sleep(Duration::from_millis(5));
    }
    pool(r)
}

fn present(r: &Rig, name: &str) -> bool {
    r.descriptors
        .get(name, &mut vec![0u8; 1 << 16])
        .unwrap()
        .is_some()
}

/// Waits until no restore thread runs.
fn idle(r: &Rig) {
    let start = Instant::now();
    while r.restores.running() > 0 {
        assert!(start.elapsed() < WAIT, "a restore thread did not end");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn marked(r: &Rig, name: &str) -> bool {
    let mut buf = vec![0u8; 1 << 16];
    let found = r.descriptors.get(name, &mut buf).unwrap().expect("there");
    oss_format::is_restoring(found.settings)
}

/// Holds the first ranged read of a source file, which only the restore's
/// thread issues, and returns the hold.
fn hold_copy(r: &Rig) -> Arc<Hold> {
    r.store.hold_next(S3Op::GetRange, 1).remove(0)
}

fn start(r: &Rig, hold: &Hold) -> support::Reply {
    let reply = send(r, "POST", NO_WAIT, Some(COPY));
    assert!(hold.wait_arrived(WAIT), "the copy began");
    reply
}

// AC-1, AC-4, AC-6: the answer arrives while the copy is held, the index is
// marked and the pool held, and after the release all are as a finished
// restore leaves them.
#[test]
fn the_answer_comes_before_the_copy_and_the_pool_is_held_until_it_ends() {
    let r = background(2, 5_000);
    let before = pool(&r);
    let hold = hold_copy(&r);
    let reply = start(&r, &hold);
    assert_eq!(reply.text(), r#"{"accepted":true}"#);
    assert_eq!(r.restores.running(), 1);
    assert!(marked(&r, "copy"), "unreadable until the copy lands");
    assert!(pool(&r) > before, "the buffers are held while it runs");
    let again = send(&r, "POST", NO_WAIT, Some(COPY));
    assert_eq!(again.status, 400, "{}", again.text());
    assert!(again.text().contains("index_already_exists"));
    hold.release(Release::Land);
    idle(&r);
    assert!(!marked(&r, "copy"));
    assert_eq!(settled(&r, before), before);
    assert_eq!(r.restores.failed(), 0);
    let uuid = restored_uuid(&r, "copy");
    let at = oss_format::ShardRef {
        index: "copy",
        uuid: &uuid,
        shard: 1,
    };
    let blob = format!("{}t=1/_0.si", at.base_path("").unwrap());
    assert_eq!(crate::snapshot_restore::read(&r, &blob), b"payload");
}

// AC-2: what a synchronous restore refuses before a write is refused by the
// request, writing nothing and starting nothing.
#[test]
fn a_refusal_before_the_first_write_writes_nothing_and_starts_no_thread() {
    let r = background(2, 10);
    let before = writes(&r);
    let cases = [
        ("/_snapshot/backups/nope/_restore", "", 404),
        (NO_WAIT, r#"{"indices":"other"}"#, 404),
        (NO_WAIT, "", 400),
    ];
    for (target, body, status) in cases {
        let reply = send(&r, "POST", target, (!body.is_empty()).then_some(body));
        assert_eq!(reply.status, status, "{target} {body}: {}", reply.text());
        assert_eq!(writes(&r), before, "{target} {body}");
        assert_eq!(r.restores.running(), 0);
    }
    assert_eq!(r.restores.failed(), 0);
}

// AC-3: a name taken between the check and the create is 400, and nothing
// of the restore is left.
#[test]
fn a_name_taken_between_the_check_and_the_create_is_400() {
    let r = background(2, 10);
    let floor = pool(&r);
    let puts = r.store.hold_next(S3Op::Put, 1);
    let addr = r.server.local_addr();
    let asked =
        std::thread::spawn(move || support::request(addr, "POST", NO_WAIT, Some(COPY.as_bytes())));
    assert!(puts[0].wait_arrived(WAIT), "the create is under way");
    r.descriptors
        .create(
            &oss_format::IndexRecord::new("copy", "other", 1),
            1,
            &mut vec![0u8; 1 << 16],
        )
        .unwrap();
    puts[0].release(Release::Land);
    let reply = asked.join().expect("answered");
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(reply.text().contains("index_already_exists"));
    assert_eq!(restored_uuid(&r, "copy"), "other", "the other index stands");
    assert_eq!((r.restores.running(), r.restores.failed()), (0, 0));
    assert_eq!(settled(&r, floor), floor);
}

// AC-5: a store failing at any write after the answer deletes what the
// restore created, and frees the pool and the slot.
#[test]
fn a_write_that_fails_after_the_answer_removes_every_index_it_made() {
    // The create answers the request; the next five puts are the copy's: a blob and a manifest for each shard, then the unmark.
    for failing in 1..=5 {
        let r = background(2, 10);
        let floor = pool(&r);
        let holds = r.store.hold_next(S3Op::Put, 8);
        for (at, hold) in holds.iter().enumerate() {
            let how = if at == failing {
                Release::Fail
            } else {
                Release::Land
            };
            hold.release(how);
        }
        let reply = send(&r, "POST", NO_WAIT, Some(COPY));
        assert_eq!(reply.text(), r#"{"accepted":true}"#, "put {failing}");
        idle(&r);
        assert!(!present(&r, "copy"), "put {failing}: the index is gone");
        assert_eq!(r.restores.failed(), 1, "put {failing}");
        assert_eq!(settled(&r, floor), floor, "put {failing}");
    }
}

// AC-5: a ranged read that fails, and a file that is gone.
#[test]
fn a_read_that_fails_after_the_answer_removes_the_index() {
    let r = background(2, 10);
    let floor = pool(&r);
    r.store.fail_next(S3Op::GetRange, 1);
    assert_eq!(
        send(&r, "POST", NO_WAIT, Some(COPY)).text(),
        r#"{"accepted":true}"#
    );
    idle(&r);
    assert!(!present(&r, "copy"));
    assert_eq!((r.restores.failed(), settled(&r, floor)), (1, floor));
}

// AC-6: past `max` running is 429 and writes nothing; a pool too small is
// 429 and writes nothing.
#[test]
fn a_restore_past_the_cap_and_one_the_pool_cannot_hold_are_429() {
    let r = background(1, 10);
    let hold = hold_copy(&r);
    start(&r, &hold);
    let before = writes(&r);
    let over = send(
        &r,
        "POST",
        NO_WAIT,
        Some(r#"{"rename_pattern":"logs","rename_replacement":"two"}"#),
    );
    assert_eq!(over.status, 429, "{}", over.text());
    assert_eq!(writes(&r), before);
    assert!(!present(&r, "two"));
    hold.release(Release::Land);
    idle(&r);
    let small = seed(rig_full(
        true,
        "",
        oss_format::MAX_SNAPSHOT_RECORD_BYTES + (2 << 20),
        4 << 20,
        StockRestoreSettings::off(),
        settings(2, 10),
    ));
    let before = writes(&small);
    let short = send(&small, "POST", NO_WAIT, Some(COPY));
    assert_eq!(short.status, 429, "{}", short.text());
    assert_eq!(writes(&small), before);
    assert_eq!(small.restores.running(), 0);
}

// AC-7: a drain that outlasts its grace leaves the copy running and the
// index marked; once the copy moves on it stops, cleans up and the name
// restores again. A restore asked after the drain began is 503.
#[test]
fn a_drain_cancels_the_copy_and_refuses_new_restores() {
    let r = background(2, 50);
    let floor = pool(&r);
    let hold = hold_copy(&r);
    start(&r, &hold);
    assert!(!r.restores.drain(), "the held copy outlasts the grace");
    assert!(marked(&r, "copy"));
    let before = writes(&r);
    let refused = send(
        &r,
        "POST",
        NO_WAIT,
        Some(r#"{"rename_pattern":"logs","rename_replacement":"late"}"#),
    );
    assert_eq!(refused.status, 503, "{}", refused.text());
    assert_eq!(writes(&r), before);
    hold.release(Release::Land);
    idle(&r);
    assert!(!present(&r, "copy"), "the cancelled copy is deleted");
    assert_eq!((r.restores.failed(), settled(&r, floor)), (1, floor));
    let again = send(
        &r,
        "POST",
        "/_snapshot/backups/daily/_restore?wait_for_completion=true",
        Some(COPY),
    );
    assert_eq!(again.status, 200, "{}", again.text());
}

// AC-7: a drain waits for a copy that ends within the grace.
#[test]
fn a_drain_waits_for_a_copy_that_ends_in_time() {
    let r = background(2, 10_000);
    let hold = hold_copy(&r);
    start(&r, &hold);
    let releasing = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        hold.release(Release::Land);
    });
    assert!(r.restores.drain());
    releasing.join().expect("released");
    assert_eq!(r.restores.running(), 0);
}

// AC-4: a delete of the marked index clears it, the copy that was running
// ends in a failure, and the name restores again.
#[test]
fn a_delete_of_the_marked_index_clears_it_and_the_name_restores_again() {
    let r = background(2, 5_000);
    let hold = hold_copy(&r);
    start(&r, &hold);
    let uuid = restored_uuid(&r, "copy");
    assert!(r.deletes.remove("copy", &uuid).unwrap());
    hold.release(Release::Land);
    idle(&r);
    assert!(!present(&r, "copy"));
    let again = send(
        &r,
        "POST",
        "/_snapshot/backups/daily/_restore?wait_for_completion=true",
        Some(COPY),
    );
    assert_eq!(again.status, 200, "{}", again.text());
    assert!(!marked(&r, "copy"));
}

// AC-8: the same requests as the synchronous restore.
#[test]
fn a_background_restore_costs_what_the_synchronous_one_does() {
    let r = background(2, 10);
    let before = requests(&r);
    assert_eq!(
        send(&r, "POST", NO_WAIT, Some(COPY)).text(),
        r#"{"accepted":true}"#
    );
    idle(&r);
    budget::check(
        "snapshot.restore_two.requests",
        requests(&r) - before,
        "a restore of one index of two shards, in the background",
    );
}

// AC-9: asked to wait, or with the switch off, the restore is synchronous.
#[test]
fn a_restore_asked_to_wait_or_with_the_switch_off_is_synchronous() {
    let r = background(2, 10);
    let full = send(
        &r,
        "POST",
        "/_snapshot/backups/daily/_restore?wait_for_completion=true",
        Some(COPY),
    );
    assert!(
        full.text().contains(r#""indices":["copy"]"#),
        "{}",
        full.text()
    );
    assert_eq!(r.restores.running(), 0);
    assert!(!marked(&r, "copy"));
    let off = seed(rig_full(
        true,
        "",
        64 << 20,
        64,
        StockRestoreSettings::off(),
        Settings::off(),
    ));
    assert_eq!(
        send(&off, "POST", NO_WAIT, Some(COPY)).text(),
        r#"{"accepted":true}"#
    );
    assert!(!marked(&off, "copy"), "done when the answer came");
    assert_eq!(off.restores.running(), 0);
}

// AC-7: dropping the reclaim timer, which the node's shutdown does, drains
// the restores: a copy still running is cancelled and later restores refused.
#[test]
fn dropping_the_drain_timer_cancels_the_running_restores() {
    let r = background(2, 10);
    let timer = oss_node::index_delete::DrainTimer::spawn(
        Arc::clone(&r.deletes),
        Duration::from_secs(3600),
    )
    .with_restores(Some(Arc::clone(&r.restores)));
    assert!(!r.restores.cancelled());
    drop(timer);
    assert!(r.restores.cancelled());
    let refused = send(&r, "POST", NO_WAIT, Some(COPY));
    assert_eq!(refused.status, 503, "{}", refused.text());
}
