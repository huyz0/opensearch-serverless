//! Opening a bucketed index's frozen commit: its files are in node packs,
//! which resolve without a shard's own storage.

use oss_blob::{Emulator, PooledStore, S3Op, SliceSource};
use oss_format::{IndexRecord, PitFile, PitShard, PointInTime, descriptor_key, node_pack_key};
use oss_reader::{ReaderError, ShardFiles};

use crate::{BASE, counts, since, store_with_fixtures};

const NODE: &str = "nodeA";

fn bucketed(store: &PooledStore<Emulator>) {
    let record = IndexRecord {
        bucket: Some("b1".into()),
        ..IndexRecord::new("bucketed", "u-bucketed", 1)
    };
    let mut buf = vec![0u8; 4096];
    let n = record.write(&mut buf).expect("write");
    // A register is its generation, then the value.
    let mut blob = 1u64.to_be_bytes().to_vec();
    blob.extend_from_slice(&buf[..n]);
    store
        .put(
            &descriptor_key(BASE, "bucketed").expect("key"),
            &mut SliceSource::new(&blob),
        )
        .expect("create");
}

fn view(location: &str) -> PointInTime {
    PointInTime {
        id: "AAAAAAAAAAAAAAAAAAAAAA".to_owned(),
        index: "bucketed".to_owned(),
        index_uuid: Some("u-bucketed".to_owned()),
        expires_at_millis: u64::MAX / 2,
        capturing: false,
        shards: vec![PitShard {
            shard: 0,
            term: 4,
            files: vec![PitFile {
                name: "_0.cfs".to_owned(),
                location: location.to_owned(),
                length: Some(40),
            }],
        }],
    }
}

#[test]
fn a_bucketed_views_node_packed_files_resolve_with_one_get() {
    let store = store_with_fixtures();
    bucketed(&store);
    let location = format!("@{NODE}/e={:020}/{:020}#8:40", 3, 2);
    let before = counts(&store);
    let (files, meta) =
        ShardFiles::open_frozen(&store, BASE, "bucketed", &view(&location), 0).expect("open");
    assert_eq!(meta.uuid, "u-bucketed");
    assert_eq!(files.term, 4);
    let range = files.get("_0.cfs").expect("file");
    assert_eq!(range.key, node_pack_key(BASE, NODE, 3, 2).expect("key"));
    assert_eq!((range.offset, range.length), (8, 40));
    assert_eq!(since(&store, before), [(S3Op::Get, 1)]);
}

#[test]
fn a_bucketed_view_with_a_file_outside_the_node_packs_is_refused() {
    let store = store_with_fixtures();
    bucketed(&store);
    for location in ["t=1/s0#8:40", "t=1"] {
        let err = ShardFiles::open_frozen(&store, BASE, "bucketed", &view(location), 0)
            .expect_err(location);
        assert!(
            matches!(
                err,
                ReaderError::Format(oss_format::FormatError::NotFollowable { .. })
            ),
            "{location}: {err}"
        );
    }
}
