//! Per-index usage: every request lands on one line
//! (docs/specs/store-usage/SPEC.md).

use std::sync::Arc;

use crate::{MIB, get_all, pattern, put};
use oss_blob::usage::{Classifier, Ledger, NODE, ONE_REQUEST, OTHER};
use oss_blob::{BlobStore, ByteRange, Emulator, S3Op, UsageStore};

/// `i/<index>/<rest>` belongs to `<index>`; anything else to no index.
struct Prefixed;

impl Classifier for Prefixed {
    fn index_of<'a>(&self, key: &'a str) -> Option<&'a str> {
        key.strip_prefix("i/")?.split('/').next()
    }
}

fn store(max_indices: usize) -> (UsageStore<Emulator>, Arc<Ledger>) {
    let ledger = Arc::new(Ledger::new(max_indices));
    let store = UsageStore::new(
        Emulator::new(64 * MIB, 1),
        Arc::clone(&ledger),
        Arc::new(Prefixed),
    );
    (store, ledger)
}

fn line(ledger: &Ledger, name: &str) -> Arc<oss_blob::usage::IndexUsage> {
    ledger.of(name)
}

// AC-1
#[test]
fn a_request_is_charged_to_the_index_its_key_names() {
    let (s, ledger) = store(8);
    put(&s, "i/a/one", b"x");
    put(&s, "i/a/two", b"x");
    put(&s, "i/b/one", b"x");
    put(&s, "leases/n1", b"x");
    assert_eq!(line(&ledger, "a").requests(S3Op::Put), 2 * ONE_REQUEST);
    assert_eq!(line(&ledger, "b").requests(S3Op::Put), ONE_REQUEST);
    assert_eq!(line(&ledger, NODE).requests(S3Op::Put), ONE_REQUEST);
    assert_eq!(line(&ledger, OTHER).requests(S3Op::Put), 0);
}

// AC-2
#[test]
fn the_lines_add_up_to_what_the_meter_counted() {
    let (s, ledger) = store(8);
    put(&s, "i/a/one", &pattern(100));
    put(&s, "i/b/one", &pattern(50));
    put(&s, "leases/n1", b"x");
    get_all(&s, "i/a/one", ByteRange::Full).unwrap();
    get_all(&s, "i/b/one", ByteRange::Full).unwrap();
    s.delete("i/b/one").unwrap();
    for op in S3Op::ALL {
        assert_eq!(
            ledger.total_requests(op),
            s.meter().count(op) * ONE_REQUEST,
            "{op:?}"
        );
    }
    assert_eq!(
        ledger.total_bytes(),
        (s.meter().bytes_read(), s.meter().bytes_written())
    );
}

// AC-3
#[test]
fn bytes_are_charged_to_the_index_that_moved_them() {
    let (s, ledger) = store(8);
    put(&s, "i/a/one", &pattern(100));
    put(&s, "i/b/one", &pattern(7));
    get_all(&s, "i/a/one", ByteRange::Full).unwrap();
    let a = line(&ledger, "a");
    assert_eq!((a.bytes_written(), a.bytes_read()), (100, 100));
    let b = line(&ledger, "b");
    assert_eq!((b.bytes_written(), b.bytes_read()), (7, 0));
}

// AC-4
#[test]
fn a_shared_upload_is_split_by_weight_and_the_heaviest_takes_the_remainder() {
    let (s, ledger) = store(8);
    ledger.split(&[("a", 1), ("b", 2)], || {
        put(&s, "packs/n1/e=1/1", &pattern(10))
    });
    let (a, b) = (line(&ledger, "a"), line(&ledger, "b"));
    assert_eq!(a.requests(S3Op::Put), 333);
    assert_eq!(b.requests(S3Op::Put), 667);
    assert_eq!(a.bytes_written(), 3);
    assert_eq!(b.bytes_written(), 7);
    assert_eq!(line(&ledger, NODE).requests(S3Op::Put), 0);
    // Outside the call the thread works for no one.
    put(&s, "packs/n1/e=1/2", b"x");
    assert_eq!(line(&ledger, NODE).requests(S3Op::Put), ONE_REQUEST);
}

// AC-4
#[test]
fn a_split_with_no_weight_charges_the_node_and_a_key_beats_a_split() {
    let (s, ledger) = store(8);
    ledger.split(&[("a", 0)], || put(&s, "packs/n1/e=1/1", b"x"));
    assert_eq!(line(&ledger, NODE).requests(S3Op::Put), ONE_REQUEST);
    ledger.split(&[("a", 5)], || put(&s, "i/b/one", b"x"));
    assert_eq!(line(&ledger, "b").requests(S3Op::Put), ONE_REQUEST);
    assert_eq!(line(&ledger, "a").requests(S3Op::Put), 0);
}

// AC-5
#[test]
fn indices_past_the_limit_are_counted_together() {
    let (s, ledger) = store(2);
    for name in ["a", "b", "c", "d"] {
        put(&s, &format!("i/{name}/x"), b"x");
    }
    assert_eq!(line(&ledger, OTHER).requests(S3Op::Put), 2 * ONE_REQUEST);
    let names: Vec<String> = ledger.lines().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["a", "b", NODE, OTHER]);
    assert_eq!(ledger.total_requests(S3Op::Put), 4 * ONE_REQUEST);
}

// AC-6
#[test]
fn the_price_of_a_line_is_a_put_a_get_at_eight_hundredths_and_a_free_delete() {
    let (s, ledger) = store(8);
    put(&s, "i/a/x", b"x");
    get_all(&s, "i/a/x", ByteRange::Full).unwrap();
    s.delete("i/a/x").unwrap();
    assert_eq!(line(&ledger, "a").cost_milli_units(), 1_000 + 80);
}

// AC-1
#[test]
fn every_kind_of_call_is_charged_to_its_keys_index() {
    let (s, ledger) = store(8);
    let mut buf = [0u8; 64];
    s.create_if_absent("i/a/c", &mut oss_blob::SliceSource::new(b"x"), &mut buf)
        .unwrap();
    s.head("i/a/c").unwrap();
    s.create_register("i/a/r", b"v").unwrap();
    s.read_register("i/a/r", &mut buf).unwrap().unwrap();
    s.list_prefix("i/a/", None, 10, &mut |_, _| {}).unwrap(); // LIST: one page of one prefix, to see its charge.
    let a = line(&ledger, "a");
    for op in S3Op::ALL {
        assert_eq!(a.requests(op), s.meter().count(op) * ONE_REQUEST, "{op:?}");
    }
    assert!(a.requests(S3Op::List) > 0 && a.requests(S3Op::Head) > 0);
}

// AC-1
#[test]
fn a_batch_delete_is_the_index_its_keys_all_name_or_the_nodes() {
    let (s, ledger) = store(8);
    s.delete_many(&["i/a/1", "i/a/2"]).unwrap();
    assert_eq!(line(&ledger, "a").requests(S3Op::Delete), ONE_REQUEST);
    s.delete_many(&["i/a/1", "i/b/1"]).unwrap();
    s.delete_many(&["i/a/1", "leases/n"]).unwrap();
    assert_eq!(line(&ledger, NODE).requests(S3Op::Delete), 2 * ONE_REQUEST);
    s.delete_many(&[]).unwrap();
    assert_eq!(ledger.total_requests(S3Op::Delete), 3 * ONE_REQUEST);
}

#[test]
fn a_usage_store_names_itself_without_its_contents() {
    let (s, _) = store(1);
    assert_eq!(format!("{s:?}"), "UsageStore");
}

// AC-1
#[test]
fn a_call_gives_the_thread_back_to_no_index_when_it_ends() {
    let (s, ledger) = store(8);
    put(&s, "i/a/one", b"x");
    // A request the meter sees outside any call is the node's, not the
    // last call's index.
    s.meter().record(S3Op::Put);
    assert_eq!(line(&ledger, "a").requests(S3Op::Put), ONE_REQUEST);
    assert_eq!(line(&ledger, NODE).requests(S3Op::Put), ONE_REQUEST);
}
