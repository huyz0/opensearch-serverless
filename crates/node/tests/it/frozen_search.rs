//! `_search` over a point in time: the commit a view froze, whatever the live
//! commit has become.

use std::sync::Arc;

use oss_blob::{BlobStore, ByteRange, Emulator, PooledStore, S3Op};
use oss_format::{CommitManifest, PitFile, PitShard, PointInTime, Register, ShardRef};
use oss_meta::{PitClock, PointInTimeStore};
use oss_reader::ShardFiles;

use crate::support::{Node, counts, request, start};

const ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const INDEX: &str = "compat-term";
const QUERY: &[u8] = br#"{"query":{"term":{"title":"brown"}}}"#;
/// A clock far from any deadline.
const NOW: u64 = 1_000;

fn clock() -> PitClock {
    Arc::new(|| NOW)
}

/// The view of the index's live commit, as capture would record it.
fn view(store: &PooledStore<Emulator>, id: &str, shards: usize, expires: u64) -> PointInTime {
    let (_, meta) = ShardFiles::open_with_meta(store, "", INDEX, 0).expect("open");
    let at = ShardRef {
        index: INDEX,
        uuid: &meta.uuid,
        shard: 0,
    };
    let mut bytes = Vec::new();
    store
        .get(
            &at.manifest_key("").expect("key"),
            ByteRange::Full,
            &mut |chunk: &[u8]| {
                bytes.extend_from_slice(chunk);
                Ok(())
            },
        )
        .expect("read the register");
    let manifest =
        CommitManifest::parse(Register::parse(&bytes).expect("register").value).expect("manifest");
    let files: Vec<PitFile> = manifest
        .files()
        .iter()
        .map(|f| PitFile {
            name: f.name.to_string(),
            location: f.location.encode(),
            length: f.length,
        })
        .collect();
    PointInTime {
        id: id.to_owned(),
        index: INDEX.to_owned(),
        index_uuid: Some(meta.uuid),
        expires_at_millis: expires,
        capturing: false,
        shards: (0..shards)
            .map(|shard| PitShard {
                shard: u32::try_from(shard).expect("small"),
                term: manifest.term,
                files: files.clone(),
            })
            .collect(),
    }
}

struct Rig {
    node: Node,
    pits: Arc<Emulator>,
}

/// A node whose point-in-time store is a separate emulator holding one view.
fn rig(shards: usize, expires: u64) -> Rig {
    let node = start();
    let pits = Arc::new(Emulator::new(64 << 20, 1));
    let record = view(&node.store, ID, shards, expires);
    let store = PointInTimeStore::new(Arc::clone(&pits) as _, "", clock()).expect("store");
    store
        .create(&record, &mut vec![0u8; 1 << 20])
        .expect("create");
    assert!(node.service.set_point_in_time(
        PointInTimeStore::new(Arc::clone(&pits) as _, "", clock()).expect("store"),
        clock(),
        1 << 20,
    ));
    Rig { node, pits }
}

fn search_pit(node: &Node, id: &str) -> crate::support::Reply {
    let body = format!(r#"{{"pit":{{"id":"{id}"}},"query":{{"term":{{"title":"brown"}}}}}}"#);
    request(node.addr(), "POST", "/_search", Some(body.as_bytes()))
}

fn without_took(text: &str) -> String {
    let at = text.find("\"took\":").expect("took");
    let end = text[at..].find(',').expect("comma") + at;
    format!("{}{}", &text[..at], &text[end + 1..])
}

fn pit_ops(rig: &Rig) -> u64 {
    S3Op::ALL.iter().map(|op| rig.pits.meter().count(*op)).sum()
}

#[test]
fn a_search_of_a_view_answers_what_a_live_search_does() {
    let rig = rig(1, u64::MAX / 2);
    let live = request(rig.node.addr(), "POST", "/compat-term/_search", Some(QUERY));
    let frozen = search_pit(&rig.node, ID);
    assert_eq!(frozen.status, 200, "{}", frozen.text());
    let text = frozen.text();
    assert!(
        text.starts_with(&format!("{{\"pit_id\":\"{ID}\",\"took\":")),
        "{text}"
    );
    let stripped = without_took(&text).replacen(&format!("\"pit_id\":\"{ID}\","), "", 1);
    assert_eq!(stripped, without_took(&live.text()));
}

#[test]
fn a_search_of_a_view_does_not_read_the_live_commit() {
    let rig = rig(1, u64::MAX / 2);
    let at = ShardRef {
        index: INDEX,
        uuid: &ShardFiles::open_with_meta(&rig.node.store, "", INDEX, 0)
            .expect("open")
            .1
            .uuid,
        shard: 0,
    };
    let key = at.manifest_key("").expect("key");
    let read = rig
        .node
        .store
        .store()
        .read_register(&key, &mut vec![0u8; 1 << 20])
        .expect("read")
        .expect("there");
    assert!(
        rig.node
            .store
            .store()
            .delete_register(&key, read.generation)
            .expect("delete the register")
    );
    let live = request(rig.node.addr(), "POST", "/compat-term/_search", Some(QUERY));
    assert_ne!(live.status, 200, "the live commit is gone");
    let frozen = search_pit(&rig.node, ID);
    assert_eq!(frozen.status, 200, "{}", frozen.text());
    assert!(
        frozen.text().contains("\"total\":{\"value\":2"),
        "{}",
        frozen.text()
    );
}

#[test]
fn a_second_search_of_a_view_reads_no_index_and_holds_one_entry() {
    let rig = rig(1, u64::MAX / 2);
    assert_eq!(search_pit(&rig.node, ID).status, 200);
    let before = counts(&rig.node.store);
    let pit_before = pit_ops(&rig);
    assert_eq!(search_pit(&rig.node, ID).status, 200);
    assert_eq!(counts(&rig.node.store), before);
    assert!(pit_ops(&rig) > pit_before, "the view is read every search");
    assert_eq!(rig.node.service.open_indexes(), 1);
}

#[test]
fn an_unknown_released_or_expired_view_is_missing() {
    let rig = rig(1, u64::MAX / 2);
    let r = search_pit(&rig.node, "BBBBBBBBBBBBBBBBBBBBBB");
    assert_eq!(r.status, 404, "{}", r.text());
    assert!(
        r.text().contains("search_context_missing_exception"),
        "{}",
        r.text()
    );
    let r = search_pit(&rig.node, "not an id");
    assert_eq!(r.status, 404, "{}", r.text());
    // Released.
    let store = PointInTimeStore::new(Arc::clone(&rig.pits) as _, "", clock()).expect("store");
    assert!(store.release(ID, &mut vec![0u8; 1 << 20]).expect("release"));
    let r = search_pit(&rig.node, ID);
    assert_eq!(r.status, 404, "{}", r.text());
    // Expired: the deadline is at the clock.
    let expired = rig_expired();
    let r = search_pit(&expired.node, ID);
    assert_eq!(r.status, 404, "{}", r.text());
}

fn rig_expired() -> Rig {
    rig(1, NOW)
}

#[test]
fn keep_alive_moves_the_deadline_and_over_an_hour_is_refused() {
    let rig = rig(1, NOW + 10);
    let body = format!(r#"{{"pit":{{"id":"{ID}","keep_alive":"2m"}},"size":1}}"#);
    let r = request(rig.node.addr(), "POST", "/_search", Some(body.as_bytes()));
    assert_eq!(r.status, 200, "{}", r.text());
    let store = PointInTimeStore::new(Arc::clone(&rig.pits) as _, "", clock()).expect("store");
    let held = store
        .read(ID, &mut vec![0u8; 1 << 20])
        .expect("read")
        .expect("live");
    assert_eq!(held.expires_at_millis, NOW + 120_000);
    let body = format!(r#"{{"pit":{{"id":"{ID}","keep_alive":"1h"}},"size":1}}"#);
    let r = request(rig.node.addr(), "POST", "/_search", Some(body.as_bytes()));
    assert_eq!(r.status, 200, "an hour is the most: {}", r.text());
    let body = format!(r#"{{"pit":{{"id":"{ID}","keep_alive":"2h"}}}}"#);
    let r = request(rig.node.addr(), "POST", "/_search", Some(body.as_bytes()));
    assert_eq!(r.status, 400, "{}", r.text());
}

#[test]
fn what_cannot_be_served_is_refused_before_any_read() {
    let rig = rig(1, u64::MAX / 2);
    let before = counts(&rig.node.store);
    let pit_before = pit_ops(&rig);
    let body = format!(r#"{{"pit":{{"id":"{ID}"}}}}"#);
    let r = request(
        rig.node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    assert_eq!(r.status, 400, "index and pit: {}", r.text());
    let r = request(rig.node.addr(), "POST", "/_search", Some(QUERY));
    assert_eq!(r.status, 400, "no pit: {}", r.text());
    assert_eq!(counts(&rig.node.store), before);
    assert_eq!(pit_ops(&rig), pit_before);

    let many = self::rig(2, u64::MAX / 2);
    let r = search_pit(&many.node, ID);
    assert_eq!(r.status, 400, "two shards: {}", r.text());

    let bare = start();
    let r = search_pit(&bare, ID);
    assert_eq!(r.status, 400, "no store: {}", r.text());
}

#[test]
fn a_pool_without_room_for_the_view_buffer_answers_429_first() {
    let node = start();
    let pits = Arc::new(Emulator::new(64 << 20, 1));
    assert!(node.service.set_point_in_time(
        PointInTimeStore::new(Arc::clone(&pits) as _, "", clock()).expect("store"),
        clock(),
        64 << 20,
    ));
    let r = search_pit(&node, ID);
    assert_eq!(r.status, 429, "{}", r.text());
    let spent: u64 = S3Op::ALL.iter().map(|op| pits.meter().count(*op)).sum();
    assert_eq!(spent, 0);
}
