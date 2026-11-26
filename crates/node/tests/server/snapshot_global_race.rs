//! docs/specs/snapshot-global-state AC-19: a document set that changes while
//! the state is read is read again, and the record holds the newer set once.

use crate::snapshot_global_take::{blobs, put_doc, rig, state_of, take};
use crate::snapshot_take::{index, publish, repository, requests};

use std::time::Duration;

use oss_blob::S3Op;
use oss_blob::emulator::hold::Release;
use oss_format::{DocKind, StateSet};
use oss_meta::DocSet;

const WAIT: Duration = Duration::from_secs(10);

fn seeded() -> crate::snapshot_take::Rig {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 1);
    publish(&r, "logs", "u1", 0);
    for n in ["a", "b", "c"] {
        put_doc(
            &r,
            DocKind::StoredScript,
            n,
            r#"{"script":{"lang":"painless","source":"1"}}"#,
        );
    }
    r
}

const ASK: &str = r#"{"indices":"logs","include_global_state":true}"#;

fn scripts(r: &crate::snapshot_take::Rig) -> Vec<String> {
    state_of(r, "g")
        .expect("recorded")
        .documents(StateSet::StoredScripts)
        .iter()
        .map(|(n, _)| n.clone())
        .collect()
}

fn delete_c(r: &crate::snapshot_take::Rig) {
    DocSet::new(blobs(r), "", DocKind::StoredScript)
        .unwrap()
        .delete("c", &mut vec![0u8; 1 << 16])
        .unwrap();
}

// AC-19: a script deleted after the walk has read the marker and begun on
// the contents is not in the record, once the walk starts again from the
// newer marker; the take still answers 200 and holds each script once.
#[test]
fn a_set_that_changes_mid_walk_is_read_again_and_each_document_is_held_once() {
    // The same take with the script already gone: what the restart must cost more than.
    let clean = seeded();
    delete_c(&clean);
    let before = requests(&clean);
    assert_eq!(take(&clean, "g", ASK).status, 200);
    let plain = requests(&clean) - before;
    assert_eq!(scripts(&clean), ["a", "b"]);

    // The 8th whole-object read of the take is the contents of "b", the 2nd
    // script: hold it, delete "c" while it waits, let it go.
    let r = seeded();
    let holds = r.store.hold_next(S3Op::Get, 8);
    let before = requests(&r);
    let reply = std::thread::scope(|s| {
        let t = s.spawn(|| take(&r, "g", ASK));
        for h in &holds[..7] {
            assert!(h.wait_arrived(WAIT));
            h.release(Release::Land);
        }
        assert!(holds[7].wait_arrived(WAIT), "the walk reached the contents");
        delete_c(&r);
        holds[7].release(Release::Land);
        t.join().unwrap()
    });
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(scripts(&r), ["a", "b"], "each held once, none stale");
    assert!(
        requests(&r) - before > plain,
        "the walk started again: {} requests against {plain}",
        requests(&r) - before
    );
}
