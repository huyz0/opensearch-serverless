//! docs/specs/point-in-time-capture/SPEC.md: freezing an index, one suite over
//! every backend with request counts, and against an index replaced mid-capture.

use std::sync::{Arc, OnceLock};

use oss_blob::BlobStore;
use oss_blob::S3Op::{Get, GetRange, Head, List, Put};
use oss_format::{
    FileLocation, IndexRecord, ManifestEntry, ShardRef, encode_manifest, looks_like_an_id,
};
use oss_meta::{AliasRecord, CaptureError, DeleteMode, DescriptorStore, PitFile};

use crate::point_in_time::{buf, store_of};
use crate::probe::Quirky;
use crate::{BASE, Backend, all, counted};

const KEEP_ALIVE: u64 = 60_000;
const N_BEFORE_CHECK: u32 = 5;

fn publish(store: &dyn BlobStore, index: &str, uuid: &str, shard: u32, term: u64) {
    let at = ShardRef { index, uuid, shard };
    let entries = [
        ManifestEntry {
            name: "_0.si",
            location: FileLocation::Whole { term_dir: "t=1" }.encode(),
            length: 7,
        },
        ManifestEntry {
            name: "_0.cfs",
            location: FileLocation::Packed {
                term_dir: "t=1",
                blob: "s0",
                offset: 10,
                length: 5,
            }
            .encode(),
            length: 5,
        },
    ];
    let value = encode_manifest(term, Some("n1"), &entries, -1, -1).unwrap();
    store
        .create_register(&at.manifest_key(BASE).unwrap(), &value)
        .unwrap();
}

fn index(d: &DescriptorStore, name: &str, uuid: &str, shards: u32) {
    d.create(&IndexRecord::new(name, uuid, shards), 1, &mut buf())
        .unwrap();
}

fn files() -> Vec<PitFile> {
    vec![
        PitFile {
            name: "_0.cfs".to_owned(),
            location: "t=1/s0#10:5".to_owned(),
            length: Some(5),
        },
        PitFile {
            name: "_0.si".to_owned(),
            location: "t=1".to_owned(),
            length: Some(7),
        },
    ]
}

fn setup(b: &Backend) -> (DescriptorStore, oss_meta::PointInTimeStore) {
    let d = b.descriptors(DeleteMode::Conditional);
    let (_, p) = store_of(Arc::clone(&b.store));
    (d, p)
}

#[test]
fn every_shards_commit_is_frozen_under_a_fresh_id_and_the_view_reads_back() {
    for b in all() {
        let (d, p) = setup(&b);
        index(&d, "logs", "u1", 2);
        publish(&*b.store, "logs", "u1", 0, 3);
        publish(&*b.store, "logs", "u1", 1, 4);
        let (view, sent) = counted(b.meter(), || {
            p.capture(&d, "logs", KEEP_ALIVE, &mut buf()).unwrap()
        });
        assert!(looks_like_an_id(&view.id));
        assert_eq!(view.index, "logs");
        assert_eq!(view.index_uuid.as_deref(), Some("u1"));
        assert_eq!(
            view.expires_at_millis,
            1_000 + KEEP_ALIVE,
            "the store's clock"
        );
        assert!(!view.capturing);
        assert_eq!(
            view.shards
                .iter()
                .map(|s| (s.shard, s.term))
                .collect::<Vec<_>>(),
            [(0, 3), (1, 4)]
        );
        assert!(view.shards.iter().all(|s| s.files == files()));
        assert_eq!(p.read(&view.id, &mut buf()).unwrap(), Some(view.clone()));
        assert!(sent.iter().all(|(op, _)| *op != List), "{sent:?}");
    }
}

#[test]
fn a_shard_that_has_published_nothing_refuses_the_capture_and_leaves_nothing_pinned() {
    for b in all() {
        let (d, p) = setup(&b);
        index(&d, "logs", "u1", 2);
        publish(&*b.store, "logs", "u1", 0, 3);
        match p.capture(&d, "logs", KEEP_ALIVE, &mut buf()) {
            Err(CaptureError::NothingPublished(1)) => {}
            other => panic!("{other:?}"),
        }
        assert!(
            p.live(&mut buf()).unwrap().is_empty(),
            "the marker was released"
        );
    }
}

#[test]
fn a_name_that_is_no_concrete_index_cannot_be_frozen() {
    for b in all() {
        let (d, p) = setup(&b);
        assert!(matches!(
            p.capture(&d, "absent", KEEP_ALIVE, &mut buf()),
            Err(CaptureError::NoSuchIndex)
        ));
        index(&d, "logs", "u1", 1);
        d.create_alias(&AliasRecord::new("a", &["logs"]), 1, &mut buf())
            .unwrap();
        assert!(matches!(
            p.capture(&d, "a", KEEP_ALIVE, &mut buf()),
            Err(CaptureError::IsAlias)
        ));
    }
}

fn bucketed_index(d: &DescriptorStore, name: &str, uuid: &str, bucket: &str) {
    let record = IndexRecord {
        bucket: Some(bucket.into()),
        ..IndexRecord::new(name, uuid, 1)
    };
    d.create(&record, 1, &mut buf()).unwrap();
}

/// The commit a bucketed member publishes: two files in one node pack.
fn node_commit(term: u64, packed: bool) -> String {
    let location = |offset, length| {
        if packed {
            FileLocation::NodePacked {
                node: "nodeA",
                epoch: 3,
                ordinal: 2,
                offset,
                length,
            }
            .encode()
        } else {
            FileLocation::Packed {
                term_dir: "t=1",
                blob: "s0",
                offset,
                length,
            }
            .encode()
        }
    };
    let entries = [
        ManifestEntry {
            name: "_0.cfs",
            location: location(0, 40),
            length: 40,
        },
        ManifestEntry {
            name: "_0.si",
            location: location(40, 7),
            length: 7,
        },
    ];
    String::from_utf8(encode_manifest(term, Some("n1"), &entries, -1, -1).unwrap()).unwrap()
}

/// A bucket's manifest register in its JSON form, `others` members first.
fn publish_bucket(store: &dyn BlobStore, bucket: &str, members: &[(&str, Option<&str>)]) {
    let mut json = format!("{{\"bucket\":\"{bucket}\",\"term\":1,\"members\":{{");
    for (i, (uuid, commit)) in members.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&format!("\"{uuid}\":{{\"index\":\"i-{uuid}\""));
        if let Some(commit) = commit {
            json.push_str(&format!(",\"version\":1,\"commit\":{commit}"));
        }
        json.push('}');
    }
    json.push_str("}}");
    let key = oss_format::bucket_manifest_key(BASE, bucket).unwrap();
    store.create_register(&key, json.as_bytes()).unwrap();
}

#[test]
fn a_bucketed_index_freezes_its_members_commit_for_the_cost_of_a_shards() {
    for b in all() {
        let (d, p) = setup(&b);
        index(&d, "plain", "u0", 1);
        publish(&*b.store, "plain", "u0", 0, 3);
        let (_, plain) = counted(b.meter(), || {
            p.capture(&d, "plain", KEEP_ALIVE, &mut buf()).unwrap()
        });
        bucketed_index(&d, "logs", "u1", "b1");
        let commit = node_commit(5, true);
        publish_bucket(
            &*b.store,
            "b1",
            &[
                ("u-other", Some(&node_commit(9, true))),
                ("u1", Some(&commit)),
            ],
        );
        let (view, sent) = counted(b.meter(), || {
            p.capture(&d, "logs", KEEP_ALIVE, &mut buf()).unwrap()
        });
        assert_eq!(
            sent, plain,
            "the bucket's register costs a shard's: {:?}",
            b.kind
        );
        assert_eq!(view.index_uuid.as_deref(), Some("u1"));
        assert_eq!(view.shards.len(), 1);
        let shard = &view.shards[0];
        assert_eq!((shard.shard, shard.term), (0, 5));
        let names: Vec<_> = shard.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["_0.cfs", "_0.si"]);
        assert_eq!(
            shard.files[0].location,
            "@nodeA/e=00000000000000000003/00000000000000000002#0:40"
        );
        assert_eq!(shard.files[1].length, Some(7));
        assert_eq!(p.read(&view.id, &mut buf()).unwrap(), Some(view.clone()));
    }
}

#[test]
fn a_bucketed_capture_that_cannot_freeze_is_refused_and_leaves_nothing_pinned() {
    type Case = (
        &'static str,
        Option<&'static str>,
        fn(&CaptureError) -> bool,
    );
    let cases: [Case; 4] = [
        ("no member", None, |e| matches!(e, CaptureError::Replaced)),
        ("no commit yet", Some(""), |e| {
            matches!(e, CaptureError::NothingPublished(0))
        }),
        ("a term container", Some("t"), |e| {
            matches!(e, CaptureError::Unsupported(_))
        }),
        ("no register", None, |e| {
            matches!(e, CaptureError::NothingPublished(0))
        }),
    ];
    for b in all() {
        for (n, (what, kind, expected)) in cases.iter().enumerate() {
            let (d, p) = setup(&b);
            let (name, bucket) = (format!("logs{n}"), format!("b{n}"));
            bucketed_index(&d, &name, "u1", &bucket);
            let commit = node_commit(5, false);
            match (*what, *kind) {
                ("no register", _) => {}
                (_, None) => publish_bucket(&*b.store, &bucket, &[("u-other", Some(&commit))]),
                (_, Some("")) => publish_bucket(&*b.store, &bucket, &[("u1", None)]),
                (_, Some(_)) => publish_bucket(&*b.store, &bucket, &[("u1", Some(&commit))]),
            }
            let got = p.capture(&d, &name, KEEP_ALIVE, &mut buf());
            match got {
                Err(ref e) if expected(e) => {}
                other => panic!("{what}: {other:?}"),
            }
            assert!(p.live(&mut buf()).unwrap().is_empty(), "{what}: released");
        }
    }
}

#[test]
fn a_register_that_names_another_bucket_is_not_the_members_bucket() {
    for b in all() {
        let (d, p) = setup(&b);
        bucketed_index(&d, "logs", "u1", "b1");
        let json = format!(
            "{{\"bucket\":\"b2\",\"term\":1,\"members\":{{\"u1\":{{\"index\":\"logs\",\"version\":1,\"commit\":{}}}}}}}",
            node_commit(5, true)
        );
        let key = oss_format::bucket_manifest_key(BASE, "b1").unwrap();
        b.store.create_register(&key, json.as_bytes()).unwrap();
        let got = p.capture(&d, "logs", KEEP_ALIVE, &mut buf());
        assert!(matches!(got, Err(CaptureError::Replaced)), "{got:?}");
        assert!(p.live(&mut buf()).unwrap().is_empty());
    }
}

#[test]
fn a_bucket_register_larger_than_the_buffer_fails_the_capture_and_leaves_nothing_pinned() {
    for b in all() {
        let (d, p) = setup(&b);
        bucketed_index(&d, "logs", "u1", "b1");
        let members: Vec<(String, String)> = (0..40)
            .map(|i| (format!("u-{i:02}"), node_commit(9, true)))
            .collect();
        let mine = node_commit(5, true);
        let refs: Vec<(&str, Option<&str>)> = members
            .iter()
            .map(|(u, c)| (u.as_str(), Some(c.as_str())))
            .chain([("u1", Some(mine.as_str()))])
            .collect();
        publish_bucket(&*b.store, "b1", &refs);
        let got = p.capture(&d, "logs", KEEP_ALIVE, &mut vec![0u8; 4096]);
        assert!(matches!(got, Err(CaptureError::Meta(_))), "{got:?}");
        assert!(p.live(&mut buf()).unwrap().is_empty(), "released");
    }
}

#[test]
fn an_index_moved_to_another_bucket_while_the_capture_began_is_refused_and_released() {
    let slot: Arc<OnceLock<Arc<dyn BlobStore>>> = Arc::new(OnceLock::new());
    let seen = Arc::clone(&slot);
    let quirky = Arc::new(Quirky::hooked(move |_: &dyn BlobStore| {
        let d = DescriptorStore::new(
            Arc::clone(seen.get().unwrap()),
            BASE,
            DeleteMode::Conditional,
        )
        .unwrap();
        d.delete("logs").unwrap();
        bucketed_index(&d, "logs", "u1", "b2");
    }));
    let inner = Arc::clone(&quirky.inner);
    slot.set(Arc::clone(&inner)).ok().unwrap();
    let store: Arc<dyn BlobStore> = quirky.clone();
    let d = DescriptorStore::new(Arc::clone(&store), BASE, DeleteMode::Conditional).unwrap();
    let (_, p) = store_of(store);
    bucketed_index(&d, "logs", "u1", "b1");
    publish_bucket(&*inner, "b1", &[("u1", Some(&node_commit(5, true)))]);
    publish_bucket(&*inner, "b2", &[("u1", Some(&node_commit(6, true)))]);
    quirky.arm(N_BEFORE_CHECK);
    let got = p.capture(&d, "logs", KEEP_ALIVE, &mut buf());
    assert!(matches!(got, Err(CaptureError::Replaced)), "{got:?}");
    assert!(p.live(&mut buf()).unwrap().is_empty(), "released");
}

#[test]
fn an_index_replaced_while_the_capture_began_is_refused_and_released() {
    let slot: Arc<OnceLock<Arc<dyn BlobStore>>> = Arc::new(OnceLock::new());
    let seen = Arc::clone(&slot);
    let quirky = Arc::new(Quirky::hooked(move |_: &dyn BlobStore| {
        let d = DescriptorStore::new(
            Arc::clone(seen.get().unwrap()),
            BASE,
            DeleteMode::Conditional,
        )
        .unwrap();
        d.delete("logs").unwrap();
        index(&d, "logs", "u2", 1);
    }));
    let inner = Arc::clone(&quirky.inner);
    slot.set(Arc::clone(&inner)).ok().unwrap();
    let store: Arc<dyn BlobStore> = quirky.clone();
    let d = DescriptorStore::new(Arc::clone(&store), BASE, DeleteMode::Conditional).unwrap();
    let (_, p) = store_of(store);
    index(&d, "logs", "u1", 1);
    publish(&*inner, "logs", "u1", 0, 3);
    // The descriptor read, the marker's register create and write, then the
    // incarnation check's read: the index is replaced just before it.
    quirky.arm(N_BEFORE_CHECK);
    assert!(matches!(
        p.capture(&d, "logs", KEEP_ALIVE, &mut buf()),
        Err(CaptureError::Replaced)
    ));
    assert!(
        p.live(&mut buf()).unwrap().is_empty(),
        "the marker was released"
    );
}

#[test]
fn the_same_name_made_again_after_the_capture_is_not_the_captured_view() {
    for b in all() {
        let (d, p) = setup(&b);
        index(&d, "logs", "u1", 1);
        publish(&*b.store, "logs", "u1", 0, 3);
        let view = p.capture(&d, "logs", KEEP_ALIVE, &mut buf()).unwrap();
        d.delete("logs").unwrap();
        index(&d, "logs", "u2", 1);
        let read = p.read(&view.id, &mut buf()).unwrap().unwrap();
        assert!(read.pins("logs", Some("u1")));
        assert!(
            !read.pins("logs", Some("u2")),
            "the new index's files are not held"
        );
    }
}

#[test]
fn a_capture_costs_a_constant_and_one_read_a_shard() {
    for b in all() {
        for shards in [1u32, 3] {
            let (d, p) = setup(&b);
            let name = format!("logs{shards}");
            index(&d, &name, "u1", shards);
            for shard in 0..shards {
                publish(&*b.store, &name, "u1", shard, 3);
            }
            let (_, sent) = counted(b.meter(), || {
                p.capture(&d, &name, KEEP_ALIVE, &mut buf()).unwrap()
            });
            // The descriptor twice, a manifest a shard and the view's finish.
            let gets = u64::from(shards) + 3;
            let want = b.budget(
                &[(Get, gets), (Head, 1), (Put, 3)],
                &[(Get, gets), (GetRange, 1), (Head, 1), (Put, 3)],
            );
            assert_eq!(sent, want, "{:?}, {shards} shards", b.kind);
        }
    }
}

#[test]
fn a_refusal_before_the_marker_writes_nothing() {
    for b in all() {
        let (d, p) = setup(&b);
        let (got, sent) = counted(b.meter(), || {
            p.capture(&d, "absent", KEEP_ALIVE, &mut buf())
        });
        assert!(matches!(got, Err(CaptureError::NoSuchIndex)));
        assert_eq!(sent, [(Get, 1)], "{:?}", b.kind);
    }
}

/// Two live units, 1 and 3, in four slots; 0 and 2 are the halves of a split of
/// 3 still pending, so a capture that counted them would ask for their commits.
const LAYOUT: &[u8] = br#"{"mode":"adaptive","declared_shards":2,"next_unit_id":4,"units":[{"id":3,"first":"8000000000000000","last":"ffffffffffffffff"},{"id":1,"first":"0","last":"7fffffffffffffff"}],"pending_split":[{"id":3,"first":"8000000000000000","last":"ffffffffffffffff"},{"id":2,"first":"8000000000000000","last":"bfffffffffffffff"},{"id":0,"first":"c000000000000000","last":"ffffffffffffffff"}]}"#;

fn units(d: &DescriptorStore) {
    let record = IndexRecord {
        unit_layout: Some(LAYOUT),
        ..IndexRecord::new("logs", "u1", 4)
    };
    d.create(&record, 1, &mut buf()).unwrap();
}

#[test]
fn an_index_in_units_freezes_its_live_units_and_no_retired_slot() {
    for b in all() {
        let (d, p) = setup(&b);
        units(&d);
        publish(&*b.store, "logs", "u1", 1, 3);
        publish(&*b.store, "logs", "u1", 3, 4);
        let view = p.capture(&d, "logs", KEEP_ALIVE, &mut buf()).unwrap();
        assert_eq!(
            view.shards
                .iter()
                .map(|s| (s.shard, s.term))
                .collect::<Vec<_>>(),
            [(1, 3), (3, 4)],
            "{:?}",
            b.kind
        );
        assert_eq!(p.read(&view.id, &mut buf()).unwrap(), Some(view));
    }
}

#[test]
fn a_live_unit_that_has_published_nothing_refuses_an_index_in_units() {
    for b in all() {
        let (d, p) = setup(&b);
        units(&d);
        publish(&*b.store, "logs", "u1", 1, 3);
        match p.capture(&d, "logs", KEEP_ALIVE, &mut buf()) {
            Err(CaptureError::NothingPublished(3)) => {}
            other => panic!("{other:?}"),
        }
        assert!(p.live(&mut buf()).unwrap().is_empty());
    }
}

#[test]
fn a_layout_that_cannot_be_read_is_refused_before_anything_is_written() {
    for b in all() {
        let (d, p) = setup(&b);
        let record = IndexRecord {
            unit_layout: Some(b"{}"),
            ..IndexRecord::new("logs", "u1", 2)
        };
        d.create(&record, 1, &mut buf()).unwrap();
        let (got, sent) = counted(b.meter(), || p.capture(&d, "logs", KEEP_ALIVE, &mut buf()));
        assert!(matches!(got, Err(CaptureError::Meta(_))), "{got:?}");
        assert_eq!(sent, [(Get, 1)], "{:?}", b.kind);
    }
}
