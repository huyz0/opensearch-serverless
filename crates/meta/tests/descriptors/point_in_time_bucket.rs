//! docs/specs/point-in-time-capture/SPEC.md, AC-5 and AC-10: freezing a
//! bucketed index from its bucket's manifest register.

use std::sync::{Arc, OnceLock};

use oss_blob::BlobStore;
use oss_blob::S3Op::{Get, GetRange, Head, Put};
use oss_format::{FileLocation, IndexRecord, ManifestEntry, encode_manifest};
use oss_meta::{CaptureError, DeleteMode, DescriptorStore};

use crate::point_in_time::{buf, store_of};
use crate::point_in_time_capture::{KEEP_ALIVE, N_BEFORE_CHECK, index, publish, setup};
use crate::probe::Quirky;
use crate::{BASE, all, counted};

pub(crate) fn bucketed_index(d: &DescriptorStore, name: &str, uuid: &str, bucket: &str) {
    let record = IndexRecord {
        bucket: Some(bucket.into()),
        ..IndexRecord::new(name, uuid, 1)
    };
    d.create(&record, 1, &mut buf()).unwrap();
}

/// The commit a bucketed member publishes: two files in one node pack.
pub(crate) fn node_commit(term: u64, packed: bool) -> String {
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
pub(crate) fn publish_bucket(
    store: &dyn BlobStore,
    bucket: &str,
    members: &[(&str, Option<&str>)],
) {
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
        // The descriptor twice, the bucket's register and the view's finish.
        let want = b.budget(
            &[(Get, 4), (Head, 1), (Put, 3)],
            &[(Get, 4), (GetRange, 1), (Head, 1), (Put, 3)],
        );
        assert_eq!(sent, want, "{:?}", b.kind);
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
    let cases: [Case; 3] = [
        ("no member", None, |e| matches!(e, CaptureError::Replaced)),
        ("no commit yet", Some(""), |e| {
            matches!(e, CaptureError::NothingPublished(0))
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
            let commit = node_commit(5, true);
            match (*what, *kind) {
                ("no register", _) => {}
                (_, None) => publish_bucket(&*b.store, &bucket, &[("u-other", Some(&commit))]),
                (_, Some(_)) => publish_bucket(&*b.store, &bucket, &[("u1", None)]),
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
fn a_members_files_in_its_own_shard_path_are_frozen_as_recorded() {
    for b in all() {
        let (d, p) = setup(&b);
        bucketed_index(&d, "logs", "u1", "b1");
        publish_bucket(&*b.store, "b1", &[("u1", Some(&node_commit(5, false)))]);
        let view = p.capture(&d, "logs", KEEP_ALIVE, &mut buf()).unwrap();
        let locations: Vec<_> = view.shards[0]
            .files
            .iter()
            .map(|f| f.location.as_str())
            .collect();
        assert_eq!(locations, ["t=1/s0#0:40", "t=1/s0#40:7"]);
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
