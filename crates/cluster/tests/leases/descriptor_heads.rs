//! Descriptor-backed bucket heads (docs/specs/bucket-descriptor-heads/SPEC.md).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use oss_cluster::heads::{
    BucketHeads, Described, DescriptorBucketHeads, Descriptors, MEMBERSHIP_CACHE_MILLIS,
};
use oss_cluster::{ClusterError, LeaseClock};

use crate::{T0, TestClock};

/// Descriptors by index name, counting the reads.
#[derive(Default)]
struct Map {
    by_name: Mutex<HashMap<String, Described>>,
    reads: AtomicUsize,
}

impl Map {
    fn set(&self, name: &str, uuid: &str, bucket: Option<&str>) {
        self.by_name.lock().unwrap().insert(
            name.to_owned(),
            Described {
                uuid: uuid.to_owned(),
                bucket: bucket.map(str::to_owned),
            },
        );
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

impl Descriptors for Map {
    fn describe(&self, index: &str) -> Result<Option<Described>, ClusterError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.by_name.lock().unwrap().get(index).cloned())
    }
}

fn rig() -> (Arc<Map>, Arc<TestClock>, DescriptorBucketHeads) {
    let map = Arc::new(Map::default());
    let clock = TestClock::at(T0);
    let clock_dyn: Arc<dyn LeaseClock> = clock.clone();
    let heads = DescriptorBucketHeads::new(map.clone(), clock_dyn);
    (map, clock, heads)
}

fn bucket(heads: &DescriptorBucketHeads, index: &str, uuid: Option<&str>) -> Option<String> {
    heads
        .bucket_of(index, 0, uuid)
        .unwrap()
        .map(|m| format!("{}/{}", m.bucket_id(), m.index_uuid()))
}

#[test]
fn a_non_zero_shard_and_a_hash_name_are_no_members() {
    let (map, _, heads) = rig();
    map.set("logs", "u", Some("s02-1"));
    map.set("#bucket#s02-1", "u", Some("s02-1"));
    assert!(heads.bucket_of("logs", 1, None).unwrap().is_none());
    assert!(heads.bucket_of("#bucket#s02-1", 0, None).unwrap().is_none());
    assert_eq!(map.reads(), 0);
}

#[test]
fn a_bucketed_index_is_a_member_of_its_descriptors_bucket() {
    let (map, _, heads) = rig();
    map.set("a", "ua", Some("s02-1"));
    map.set("plain", "up", None);
    assert_eq!(bucket(&heads, "a", None).as_deref(), Some("s02-1/ua"));
    assert_eq!(bucket(&heads, "plain", None), None);
    assert_eq!(bucket(&heads, "ghost", None), None);
}

#[test]
fn the_answer_is_kept_for_exactly_the_cache_time() {
    let (map, clock, heads) = rig();
    map.set("a", "ua", Some("s02-1"));
    bucket(&heads, "a", None);
    clock.advance(MEMBERSHIP_CACHE_MILLIS - 1);
    bucket(&heads, "a", None);
    assert_eq!(map.reads(), 1);
    clock.advance(1);
    bucket(&heads, "a", None);
    assert_eq!(map.reads(), 2);
}

#[test]
fn a_callers_other_uuid_forces_a_read() {
    let (map, _, heads) = rig();
    map.set("a", "old", Some("s02-1"));
    assert_eq!(
        bucket(&heads, "a", Some("old")).as_deref(),
        Some("s02-1/old")
    );
    // recreated: the descriptor now has another uuid and bucket
    map.set("a", "new", Some("s03-9"));
    assert_eq!(
        bucket(&heads, "a", Some("new")).as_deref(),
        Some("s03-9/new")
    );
    assert_eq!(map.reads(), 2);
    // a uuid the descriptor does not have is no member
    assert_eq!(bucket(&heads, "a", Some("stale")), None);
    assert_eq!(map.reads(), 3);
}

#[test]
fn note_answers_without_a_read_and_forget_reads_again() {
    let (map, _, heads) = rig();
    map.set("a", "ua", Some("s02-1"));
    heads.note("a", "noted", Some("s09-9"));
    assert_eq!(bucket(&heads, "a", None).as_deref(), Some("s09-9/noted"));
    assert_eq!(map.reads(), 0);
    heads.forget("a");
    assert_eq!(bucket(&heads, "a", None).as_deref(), Some("s02-1/ua"));
    assert_eq!(map.reads(), 1);
    // a vanished descriptor forgets the entry
    map.by_name.lock().unwrap().clear();
    heads.forget("a");
    assert_eq!(bucket(&heads, "a", None), None);
}

#[test]
fn others_open_on_judges_the_nodes_open_shards() {
    let (map, _, heads) = rig();
    for (name, uuid, b) in [
        ("a", "ua", Some("s02-1")),
        ("b", "ub", Some("s02-1")),
        ("c", "uc", Some("s03-1")),
        ("p", "up", None),
    ] {
        map.set(name, uuid, b);
    }
    heads.register(
        "n1",
        Box::new(|| {
            vec![
                ("a".into(), 0, "ua".into()),
                ("c".into(), 0, "uc".into()),
                ("p".into(), 0, "up".into()),
                ("b".into(), 1, "ub".into()),
            ]
        }),
    );
    // a is open, b only as shard 1, c is another bucket, p has none
    assert!(!heads.others_open_on("s02-1", "n1", "a").unwrap());
    assert!(heads.others_open_on("s02-1", "n1", "zzz").unwrap());
    assert!(heads.others_open_on("s03-1", "n1", "a").unwrap());
    assert!(!heads.others_open_on("s03-1", "n1", "c").unwrap());
    // an unregistered node holds none
    assert!(!heads.others_open_on("s02-1", "n2", "zzz").unwrap());
    // an open shard of another incarnation than its descriptor is no member
    heads.register("n3", Box::new(|| vec![("b".into(), 0, "stale".into())]));
    assert!(!heads.others_open_on("s02-1", "n3", "zzz").unwrap());
}
