//! docs/specs/index-lifecycle/SPEC.md: creating and deleting an index.

use std::sync::{Arc, Mutex};

use oss_blob::S3Op::{Delete, Get, GetRange, List, Put};
use oss_blob::{BlobError, BlobStore, SliceSource};
use oss_format::{IndexRecord, ReclaimIntent};
use oss_meta::{
    AliasRecord, DeleteMode, Deleted, DescriptorStore, HeadAt, IndexLifecycle, MetaError, Purged,
    ReclaimQueue, ShardHeads, ShardPins, Target,
};

use crate::alias_mutations::{NOW, at_now, mutations, names};
use crate::{BASE, Kind, all, counted};

/// Shard heads that answer as configured and record what they were asked.
#[derive(Default)]
pub(crate) struct Heads {
    pub(crate) read: Vec<HeadAt>,
    pub(crate) reappeared: usize,
    pub(crate) calls: Mutex<Vec<String>>,
}

impl Heads {
    pub(crate) fn owned(shard: u32) -> Self {
        Self {
            read: vec![HeadAt {
                shard,
                generation: 7,
                owned: true,
            }],
            ..Self::default()
        }
    }

    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn call(&self, what: String) {
        self.calls.lock().unwrap().push(what);
    }
}

impl ShardHeads for Heads {
    fn read_all_of(&self, index: &str, shards: u32, uuid: &str) -> Result<Vec<HeadAt>, MetaError> {
        self.call(format!("read {index} {shards} {uuid}"));
        Ok(self.read.clone())
    }
    fn delete_if_unchanged(&self, index: &str, heads: &[HeadAt]) -> Result<(), MetaError> {
        let at: Vec<_> = heads
            .iter()
            .map(|h| format!("{}@{}", h.shard, h.generation))
            .collect();
        self.call(format!("delete {index} {}", at.join(",")));
        Ok(())
    }
    fn release_all_of(&self, index: &str, shards: u32, uuid: &str) -> Result<(), MetaError> {
        self.call(format!("release {index} {shards} {uuid}"));
        Ok(())
    }
    fn delete_all_of(&self, index: &str, shards: u32, uuid: &str) -> Result<usize, MetaError> {
        self.call(format!("delete all {index} {shards} {uuid}"));
        Ok(self.reappeared)
    }
}

/// No heads: every shard unowned, none reappearing.
pub(crate) struct NoHeads;

impl ShardHeads for NoHeads {
    fn read_all_of(&self, _: &str, _: u32, _: &str) -> Result<Vec<HeadAt>, MetaError> {
        Ok(Vec::new())
    }
    fn delete_if_unchanged(&self, _: &str, _: &[HeadAt]) -> Result<(), MetaError> {
        Ok(())
    }
    fn release_all_of(&self, _: &str, _: u32, _: &str) -> Result<(), MetaError> {
        Ok(())
    }
    fn delete_all_of(&self, _: &str, _: u32, _: &str) -> Result<usize, MetaError> {
        Ok(0)
    }
}

/// No pins.
pub(crate) static NO_PINS: Pins = Pins(Some(Vec::new()));

/// Pins on fixed shards, or pins that cannot be read.
pub(crate) struct Pins(pub(crate) Option<Vec<u32>>);

impl ShardPins for Pins {
    fn pinned(&self, _: &str, _: &str, _: u32) -> Result<Vec<u32>, MetaError> {
        self.0
            .clone()
            .ok_or(MetaError::Blob(BlobError::Protocol("pins unreadable")))
    }
}

pub(crate) fn lifecycle<'a>(
    d: &'a DescriptorStore,
    queue: &'a ReclaimQueue,
    heads: &'a dyn ShardHeads,
    pins: &'a dyn ShardPins,
) -> IndexLifecycle<'a> {
    IndexLifecycle::new(d, mutations(d, &at_now), queue, heads, pins, 4096)
}

pub(crate) fn queue(store: &Arc<dyn BlobStore>) -> ReclaimQueue {
    ReclaimQueue::new(Arc::clone(store), BASE).unwrap()
}

/// Writes a blob into a shard's storage.
pub(crate) fn shard_blob(store: &dyn BlobStore, index: &str, uuid: &str, shard: u32, file: &str) {
    let key = format!("{BASE}segments/{index}#{uuid}#{shard}/{file}");
    store
        .put(&key, &mut SliceSource::new(b"bytes"), &mut [0u8; 16])
        .unwrap();
}

/// The keys under `prefix`, below the base, in order.
pub(crate) fn keys(store: &dyn BlobStore, prefix: &str) -> Vec<String> {
    let mut found = Vec::new();
    store
        // LIST: a test reading what is left.
        .list_prefix(&format!("{BASE}{prefix}"), None, 1000, &mut |k, _| {
            found.push(k[BASE.len()..].to_owned());
        })
        .unwrap();
    found
}

fn uuid_of(d: &DescriptorStore, name: &str) -> Option<String> {
    let mut buf = [0u8; 4096];
    d.get(name, &mut buf).unwrap().map(|r| r.uuid.into_owned())
}

#[test]
fn a_create_writes_the_descriptor_in_one_request() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        let mut l = lifecycle(&d, &q, &NoHeads, &NO_PINS);
        let (g, n) = counted(b.meter(), || {
            l.create_index(&IndexRecord::new("logs", "u1", 3), NOW)
        });
        let g = g.unwrap();
        assert_eq!(n, vec![(Put, 1)], "{:?}", b.kind);
        assert_eq!(d.generation_of("logs", &mut [0u8; 4096]).unwrap(), g);
        assert_eq!(uuid_of(&d, "logs").as_deref(), Some("u1"));
    }
}

#[test]
fn a_retried_create_answers_its_own_record_and_no_other() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        let mut l = lifecycle(&d, &q, &NoHeads, &NO_PINS);
        let record = IndexRecord::new("logs", "u1", 3);
        let g = l.create_index(&record, NOW).unwrap();
        // The first response was lost: the same create, sent again.
        let (again, n) = counted(b.meter(), || l.create_index(&record, NOW));
        assert_eq!(again.unwrap(), g, "{:?}", b.kind);
        // The refused create, the tombstone-sized read, and the read of the
        // record that proves it is this create's.
        let refused = b.budget(&[(Get, 2), (Put, 1)], &[(Get, 2), (Put, 1)]);
        assert_eq!(n, refused, "{:?}", b.kind);
        // Another create of the name, with its own uuid, is refused.
        assert!(matches!(
            l.create_index(&IndexRecord::new("logs", "u2", 3), NOW),
            Err(MetaError::AlreadyExists)
        ));
        assert_eq!(uuid_of(&d, "logs").as_deref(), Some("u1"));
        // An alias holding the name is no index of that uuid.
        let mut m = mutations(&d, &at_now);
        m.create_alias(&AliasRecord::new("a", &["logs"])).unwrap();
        assert!(matches!(
            l.create_index(&IndexRecord::new("a", "u1", 1), NOW),
            Err(MetaError::AlreadyExists)
        ));
    }
}

#[test]
fn a_create_refuses_a_bad_name_or_unusable_layout_before_any_request() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        let mut l = lifecycle(&d, &q, &NoHeads, &NO_PINS);
        // A 250-byte name is valid, but `<name>#<uuid>#<shard>` is one key
        // component, and a filesystem's are at most 255 bytes.
        let long = "x".repeat(250);
        let (made, n) = counted(b.meter(), || {
            l.create_index(&IndexRecord::new(&long, "uuid-1", 10), NOW)
        });
        if b.kind == Kind::Fs {
            assert!(matches!(
                made,
                Err(MetaError::Blob(BlobError::InvalidArgument(_)))
            ));
            assert!(n.is_empty());
        } else {
            made.unwrap();
        }
        // The highest shard decides: one shard fits where ten do not.
        let short = "y".repeat(250);
        l.create_index(&IndexRecord::new(&short, "u", 1), NOW)
            .unwrap();
        let (bad, n) = counted(b.meter(), || {
            l.create_index(&IndexRecord::new("Upper", "u", 1), NOW)
        });
        assert!(matches!(bad, Err(MetaError::InvalidName(_))));
        assert!(n.is_empty());
        let (bad, n) = counted(b.meter(), || {
            l.create_index(&IndexRecord::new("ok", "u#1", 1), NOW)
        });
        assert!(matches!(bad, Err(MetaError::Format(_))), "{bad:?}");
        assert!(n.is_empty());
    }
}

#[test]
fn a_conditional_delete_files_its_intent_first_then_removes_everything() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        let heads = Heads {
            read: vec![HeadAt {
                shard: 1,
                generation: 9,
                owned: false,
            }],
            ..Heads::default()
        };
        let mut l = lifecycle(&d, &q, &heads, &NO_PINS);
        l.create_index(&IndexRecord::new("logs", "u1", 2), NOW)
            .unwrap();
        shard_blob(&*b.store, "logs", "u1", 0, "manifest");
        shard_blob(&*b.store, "logs", "u1", 1, "t=1/_0.cfs");
        shard_blob(&*b.store, "logs", "u2", 0, "manifest");
        let (deleted, n) = counted(b.meter(), || l.delete_index("logs", None, NOW));
        assert_eq!(
            deleted.unwrap(),
            Some(Deleted {
                uuid: "u1".into(),
                bucket: None,
                aliases_left: 0,
                purged: Some(Purged::default()),
            }),
            "{:?}",
            b.kind
        );
        // The read, the intent, the conditional delete, and a LIST and the blobs' deletes a
        // shard. The filesystem's conditional delete reads the generation first.
        let budget = b.budget(
            &[(Get, 1), (Put, 1), (List, 2), (Delete, 3)],
            &[(Get, 1), (GetRange, 1), (Put, 1), (List, 2), (Delete, 3)],
        );
        assert_eq!(n, budget, "{:?}", b.kind);
        assert_eq!(uuid_of(&d, "logs"), None);
        assert_eq!(heads.calls(), ["read logs 2 u1", "delete logs 1@9"]);
        // Another incarnation's bytes stay.
        assert_eq!(
            keys(&*b.store, "segments/"),
            ["segments/logs#u2#0/manifest"]
        );
        let due = (NOW + 120_000) / 60_000 * 60_000;
        let bucket = String::from_utf8(oss_format::reclaim_bucket(due).unwrap().to_vec()).unwrap();
        assert_eq!(
            keys(&*b.store, "reclaim/"),
            [format!("reclaim/{bucket}/logs#u1")]
        );
    }
}

#[test]
fn the_intent_is_written_before_the_descriptor_goes() {
    // Request 1 creates, 2 reads, 3 is the intent and 4 the delete. Failing
    // the intent stops the delete with nothing filed; failing the delete
    // leaves the intent already filed. Either way the index stays whole.
    for (fail, filed) in [(3, 0), (4, 1)] {
        let store: Arc<dyn BlobStore> = Arc::new(crate::probe::Quirky::new(
            crate::probe::Quirk::FailRequest(fail),
        ));
        let d = DescriptorStore::new(Arc::clone(&store), BASE, DeleteMode::Conditional).unwrap();
        let q = queue(&store);
        let mut l = lifecycle(&d, &q, &NoHeads, &NO_PINS);
        l.create_index(&IndexRecord::new("logs", "u1", 1), NOW)
            .unwrap();
        assert!(matches!(
            l.delete_index("logs", None, NOW),
            Err(MetaError::Blob(_))
        ));
        assert_eq!(uuid_of(&d, "logs").as_deref(), Some("u1"), "{fail}");
        assert_eq!(keys(&*store, "reclaim/").len(), filed, "{fail}");
    }
}

#[test]
fn a_shard_with_a_writer_leaves_its_bytes_to_the_reclaim() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        let heads = Heads::owned(0);
        let mut l = lifecycle(&d, &q, &heads, &NO_PINS);
        l.create_index(&IndexRecord::new("logs", "u1", 1), NOW)
            .unwrap();
        shard_blob(&*b.store, "logs", "u1", 0, "manifest");
        let (deleted, n) = counted(b.meter(), || l.delete_index("logs", None, NOW));
        assert_eq!(
            deleted.unwrap(),
            Some(Deleted {
                uuid: "u1".into(),
                bucket: None,
                aliases_left: 0,
                purged: None,
            })
        );
        let budget = b.budget(
            &[(Get, 1), (Put, 1), (Delete, 1)],
            &[(Get, 1), (GetRange, 1), (Put, 1), (Delete, 1)],
        );
        assert_eq!(n, budget, "{:?}", b.kind);
        assert_eq!(
            keys(&*b.store, "segments/"),
            ["segments/logs#u1#0/manifest"]
        );
        let mut taken = Vec::new();
        q.drain(NOW + 180_000, &|_| true, &mut |i| {
            taken.push(i.clone().into_owned_intent());
            Ok(())
        })
        .unwrap();
        assert_eq!(taken, [("logs".to_owned(), "u1".to_owned(), 1, true)]);
    }
}

#[test]
fn a_tombstone_delete_releases_heads_and_purges_without_an_intent() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let q = queue(&b.store);
        let heads = Heads::owned(0);
        let mut l = lifecycle(&d, &q, &heads, &NO_PINS);
        l.create_index(&IndexRecord::new("logs", "u1", 1), NOW)
            .unwrap();
        shard_blob(&*b.store, "logs", "u1", 0, "manifest");
        let deleted = l.delete_index("logs", None, NOW).unwrap().unwrap();
        assert_eq!(deleted.purged, Some(Purged::default()));
        assert_eq!(heads.calls(), ["release logs 1 u1"]);
        assert!(keys(&*b.store, "reclaim/").is_empty(), "{:?}", b.kind);
        assert!(keys(&*b.store, "segments/").is_empty());
    }
}

#[test]
fn a_delete_of_no_such_incarnation_reads_once_and_writes_nothing() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        let heads = Heads::default();
        let mut l = lifecycle(&d, &q, &heads, &NO_PINS);
        l.create_index(&IndexRecord::new("logs", "u1", 1), NOW)
            .unwrap();
        let mut m = mutations(&d, &at_now);
        m.create_alias(&AliasRecord::new("a", &["logs"])).unwrap();
        for (name, uuid) in [("logs", Some("u0")), ("none", None), ("a", None)] {
            let (deleted, n) = counted(b.meter(), || l.delete_index(name, uuid, NOW));
            assert_eq!(deleted.unwrap(), None, "{name}");
            assert_eq!(n, vec![(Get, 1)], "{name} {:?}", b.kind);
        }
        assert!(heads.calls().is_empty());
        let deleted = l.delete_index("logs", Some("u1"), NOW).unwrap();
        assert!(deleted.is_some());
    }
}

#[test]
fn a_delete_that_loses_a_race_leaves_only_an_intent_the_reclaim_drops() {
    let quirky = Arc::new(crate::probe::Quirky::hooked(|inner| {
        // Another writer updates the descriptor between the read and the
        // delete.
        let key = format!("{BASE}indices/logs");
        let mut buf = [0u8; 4096];
        let read = inner.read_register(&key, &mut buf).unwrap().unwrap();
        let mut value = [0u8; 4096];
        let record = IndexRecord {
            mapping_version: 2,
            ..IndexRecord::new("logs", "u1", 1)
        };
        let len = record.write(&mut value).unwrap();
        inner
            .swap_register(&key, read.generation, &value[..len])
            .unwrap();
    }));
    let store: Arc<dyn BlobStore> = Arc::clone(&quirky) as Arc<dyn BlobStore>;
    let d = DescriptorStore::new(Arc::clone(&store), BASE, DeleteMode::Conditional).unwrap();
    let q = queue(&store);
    let heads = Heads::default();
    let mut l = lifecycle(&d, &q, &heads, &NO_PINS);
    l.create_index(&IndexRecord::new("logs", "u1", 1), NOW)
        .unwrap();
    shard_blob(&*store, "logs", "u1", 0, "manifest");
    // The read, the intent, then the delete: the hook runs before it.
    quirky.arm(3);
    assert_eq!(l.delete_index("logs", None, NOW).unwrap(), None);
    assert_eq!(uuid_of(&d, "logs").as_deref(), Some("u1"));
    assert_eq!(heads.calls(), ["read logs 1 u1"]);
    assert_eq!(keys(&*store, "segments/"), ["segments/logs#u1#0/manifest"]);
    // The intent names a live index: the reclaim touches nothing.
    let mut l2 = lifecycle(&d, &q, &heads, &NO_PINS);
    let taken = q
        .drain(NOW + 180_000, &|_| true, &mut |i| l2.reclaim(i))
        .unwrap();
    assert_eq!(taken, 1);
    assert_eq!(heads.calls(), ["read logs 1 u1"]);
    assert_eq!(keys(&*store, "segments/"), ["segments/logs#u1#0/manifest"]);
    assert!(keys(&*store, "reclaim/").is_empty());
}

#[test]
fn a_delete_detaches_the_index_from_its_aliases() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        let mut m = mutations(&d, &at_now);
        let mut l = lifecycle(&d, &q, &NoHeads, &NO_PINS);
        l.create_index(&IndexRecord::new("logs", "u1", 1), NOW)
            .unwrap();
        l.create_index(&IndexRecord::new("old", "u2", 1), NOW)
            .unwrap();
        m.create_alias(&AliasRecord::new("a", &["logs", "old"]))
            .unwrap();
        let (deleted, n) = counted(b.meter(), || l.delete_index("logs", None, NOW));
        assert_eq!(deleted.unwrap().unwrap().aliases_left, 0);
        // The read, the intent, the delete, the alias's read and swap, and
        // the shard's LIST.
        let budget = b.budget(
            &[(Get, 2), (Put, 2), (List, 1), (Delete, 1)],
            &[(Get, 2), (GetRange, 2), (Put, 2), (List, 1), (Delete, 1)],
        );
        assert_eq!(n, budget, "{:?}", b.kind);
        let mut buf = [0u8; 4096];
        let Target::Alias(alias) = d.resolve("a", &mut buf).unwrap().target else {
            panic!("an alias");
        };
        assert_eq!(alias.indices, names(&["old"]), "{:?}", b.kind);
    }
}

#[test]
fn pinned_shards_keep_their_bytes_and_unreadable_pins_purge_nothing() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let q = queue(&b.store);
        let pins = Pins(Some(vec![1]));
        let mut l = lifecycle(&d, &q, &NoHeads, &pins);
        l.create_index(&IndexRecord::new("logs", "u1", 2), NOW)
            .unwrap();
        shard_blob(&*b.store, "logs", "u1", 0, "manifest");
        shard_blob(&*b.store, "logs", "u1", 1, "manifest");
        let deleted = l.delete_index("logs", None, NOW).unwrap().unwrap();
        assert_eq!(
            deleted.purged,
            Some(Purged {
                pinned: 1,
                failed: 0
            })
        );
        assert_eq!(
            keys(&*b.store, "segments/"),
            ["segments/logs#u1#1/manifest"]
        );

        let unreadable = Pins(None);
        let mut l = lifecycle(&d, &q, &NoHeads, &unreadable);
        l.create_index(&IndexRecord::new("other", "u3", 1), NOW)
            .unwrap();
        shard_blob(&*b.store, "other", "u3", 0, "manifest");
        assert!(l.delete_index("other", None, NOW).is_err());
        assert!(keys(&*b.store, "segments/other").len() == 1, "{:?}", b.kind);
    }
}

/// An intent's fields, owned, for comparing.
trait Owned {
    fn into_owned_intent(self) -> (String, String, u32, bool);
}

impl Owned for ReclaimIntent<'_> {
    fn into_owned_intent(self) -> (String, String, u32, bool) {
        (
            self.name.into_owned(),
            self.uuid.into_owned(),
            self.shards,
            self.had_writer,
        )
    }
}
