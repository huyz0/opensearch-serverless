//! docs/specs/extension-analyzer AC-13: a search of a point in time of an
//! index that pins an analyzer parses its text with the module, as a live
//! search does, and never with the built-in analyzer.
#![cfg(feature = "extensions")]

use std::sync::Arc;

use lucene_store::ByteBuffersDirectory;
use oss_blob::{BlobStore, ByteRange, FsStore};
use oss_format::{CommitManifest, PitFile, PitShard, PointInTime, Register, ShardRef};
use oss_meta::{PitClock, PointInTimeStore};
use oss_testkit::TempDir;

use super::pinned_read::{module, modules, rig};
use crate::support::request;

const ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";

fn clock() -> PitClock {
    Arc::new(|| 1_000)
}

/// The view of the published shard, as a capture would record it.
fn view(store: &FsStore) -> PointInTime {
    let at = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    let mut bytes = Vec::new();
    store
        .get(
            &at.manifest_key("").expect("key"),
            ByteRange::Full,
            &mut [0_u8; 8192],
            &mut |chunk: &[u8]| {
                bytes.extend_from_slice(chunk);
                Ok(())
            },
        )
        .expect("the register");
    let manifest =
        CommitManifest::parse(Register::parse(&bytes).expect("register").value).expect("manifest");
    PointInTime {
        id: ID.to_owned(),
        index: "logs".to_owned(),
        index_uuid: Some("u1".to_owned()),
        expires_at_millis: u64::MAX / 2,
        capturing: false,
        shards: vec![PitShard {
            shard: 0,
            term: manifest.term,
            files: manifest
                .files()
                .iter()
                .map(|f| PitFile {
                    name: f.name.to_string(),
                    location: f.location.encode(),
                    length: f.length,
                })
                .collect(),
        }],
    }
}

// AC-13 (the frozen path)
#[test]
fn a_search_of_a_view_analyses_with_the_module_the_index_pins() {
    let root = TempDir::new("pinned-frozen");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    assert!(r.service.set_modules(modules(root.path())));
    let pits = Arc::new(FsStore::new(TempDir::new("pinned-frozen-pits").path()).expect("store"));
    let record = view(&FsStore::new(root.path()).expect("store"));
    let store = PointInTimeStore::new(Arc::clone(&pits) as _, "", clock()).expect("store");
    store
        .create(&record, &mut vec![0u8; 1 << 20])
        .expect("create");
    assert!(r.service.set_point_in_time(
        PointInTimeStore::new(pits as _, "", clock()).expect("store"),
        clock(),
        1 << 20,
    ));
    let body = format!(r#"{{"pit":{{"id":"{ID}"}},"query":{{"match":{{"t":"zzzzzzz"}}}}}}"#);
    let reply = request(
        r.server.local_addr(),
        "POST",
        "/_search",
        Some(body.as_bytes()),
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert!(reply.text().contains(r#""value":1"#), "{}", reply.text());
}
