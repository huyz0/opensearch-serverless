//! `_search` over a point in time: the commit a view froze, whatever the live
//! commit has become.

use std::sync::Arc;

use oss_blob::{BlobStore, ByteRange, Emulator, PooledStore, S3Op};
use oss_format::{CommitManifest, PitFile, PitShard, PointInTime, Register, ShardRef};
use oss_meta::{PitClock, PointInTimeStore};
use oss_reader::ShardFiles;

use crate::support::{Node, counts, put_shard_copy, put_variant, request, start};

const ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const INDEX: &str = "compat-term";
const QUERY: &[u8] = br#"{"query":{"term":{"title":"brown"}}}"#;
/// A clock far from any deadline.
const NOW: u64 = 1_000;

fn clock() -> PitClock {
    Arc::new(|| NOW)
}

/// The files of shard `shard`'s live commit, as capture would record them.
fn shard_of_view(store: &PooledStore<Emulator>, uuid: &str, shard: u32) -> PitShard {
    let at = ShardRef {
        index: INDEX,
        uuid,
        shard,
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
    PitShard {
        shard,
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
    }
}

/// The view of the index's live commits, as capture would record them.
fn view(store: &PooledStore<Emulator>, id: &str, shards: usize, expires: u64) -> PointInTime {
    let (_, meta) = ShardFiles::open_with_meta(store, "", INDEX, 0).expect("open");
    let uuid = meta.uuid.clone();
    PointInTime {
        id: id.to_owned(),
        index: INDEX.to_owned(),
        index_uuid: Some(meta.uuid),
        expires_at_millis: expires,
        capturing: false,
        shards: (0..shards)
            .map(|shard| shard_of_view(store, &uuid, u32::try_from(shard).expect("small")))
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
    if shards > 1 {
        // The index has the shards its view names: its descriptor says so
        // and each shard past the first holds the first's blobs.
        let more = format!(r#""number_of_shards":{shards}"#);
        put_variant(&node.store, INDEX, INDEX, |j| {
            j.replace(r#""number_of_shards":1"#, &more)
        });
        for shard in 1..shards {
            put_shard_copy(
                &node.store,
                INDEX,
                "compat-term-uuid",
                u32::try_from(shard).expect("small"),
            );
        }
    }
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
    // With its register gone the live shard reads as one nobody wrote to.
    assert_eq!(live.status, 200, "{}", live.text());
    assert!(
        live.text().contains("\"total\":{\"value\":0"),
        "the live commit is gone: {}",
        live.text()
    );
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
fn closing_a_view_frees_its_shards_and_the_next_search_opens_them_afresh() {
    let rig = rig(2, u64::MAX / 2);
    assert_eq!(search_pit(&rig.node, ID).status, 200);
    assert_eq!(rig.node.service.open_indexes(), 2);
    assert_eq!(rig.node.service.close_view("not-this-view"), 0);
    assert_eq!(
        rig.node.service.open_indexes(),
        2,
        "another view's id closes none"
    );
    assert_eq!(rig.node.service.close_view(ID), 2);
    assert_eq!(rig.node.service.open_indexes(), 0, "the charge is freed");
    assert_eq!(
        rig.node.service.close_view(ID),
        0,
        "closing twice is harmless"
    );
    let before = counts(&rig.node.store);
    assert_eq!(search_pit(&rig.node, ID).status, 200);
    assert_ne!(counts(&rig.node.store), before, "the shards open again");
    assert_eq!(rig.node.service.open_indexes(), 2);
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

    let bare = start();
    let r = search_pit(&bare, ID);
    assert_eq!(r.status, 400, "no store: {}", r.text());
}

#[test]
fn routing_and_preference_do_not_go_with_a_view() {
    let rig = rig(1, u64::MAX / 2);
    let before = counts(&rig.node.store);
    let pit_before = pit_ops(&rig);
    let body = format!(r#"{{"pit":{{"id":"{ID}"}}}}"#);
    for param in ["routing=a", "preference=_local"] {
        let r = request(
            rig.node.addr(),
            "POST",
            &format!("/_search?{param}"),
            Some(body.as_bytes()),
        );
        assert_eq!(r.status, 400, "{param}: {}", r.text());
        let name = param.split('=').next().expect("a name");
        assert!(
            r.text()
                .contains(&format!("[{name}] cannot be used with point in time")),
            "{param}: {}",
            r.text()
        );
    }
    assert_eq!(counts(&rig.node.store), before);
    assert_eq!(pit_ops(&rig), pit_before);
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

#[test]
fn a_search_of_a_view_of_two_shards_answers_what_a_live_search_of_them_does() {
    let rig = rig(2, u64::MAX / 2);
    let live = request(rig.node.addr(), "POST", "/compat-term/_search", Some(QUERY));
    assert!(
        live.text().contains(r#""_shards":{"total":2,"#),
        "the live index is two shards: {}",
        live.text()
    );
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
fn a_second_search_of_a_view_of_two_shards_reads_no_index_and_holds_an_entry_each() {
    let rig = rig(2, u64::MAX / 2);
    assert_eq!(search_pit(&rig.node, ID).status, 200);
    let after_first = counts(&rig.node.store);
    assert_eq!(search_pit(&rig.node, ID).status, 200);
    assert_eq!(counts(&rig.node.store), after_first, "the shards are open");
    assert_eq!(rig.node.service.open_indexes(), 2, "one entry a shard");
}

#[test]
fn a_view_of_two_shards_does_not_read_the_live_commits() {
    let rig = rig(2, u64::MAX / 2);
    for shard in 0..2 {
        let at = ShardRef {
            index: INDEX,
            uuid: "compat-term-uuid",
            shard,
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
    }
    let r = request(rig.node.addr(), "POST", "/compat-term/_search", Some(QUERY));
    // With their registers gone the live shards read as ones nobody wrote to.
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(
        r.text().contains("\"total\":{\"value\":0"),
        "the live commits are gone: {}",
        r.text()
    );
    let r = search_pit(&rig.node, ID);
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.text().contains(r#""_shards":{"total":2,"successful":2"#));
}

// docs/specs/search-pipelines AC-9: a view's search takes the same pipeline.
#[test]
fn a_search_of_a_view_takes_a_search_pipeline() {
    use crate::search_pipeline::{CUT, Held};
    let rig = rig(1, u64::MAX / 2);
    assert!(rig.node.service.set_search_pipelines(Arc::new(Held(
        std::collections::HashMap::from([("cut", CUT)])
    ))));
    let body = format!(r#"{{"pit":{{"id":"{ID}"}},"query":{{"match_all":{{}}}},"size":10}}"#);
    let all = request(rig.node.addr(), "POST", "/_search", Some(body.as_bytes()));
    let cut = request(
        rig.node.addr(),
        "POST",
        "/_search?search_pipeline=cut",
        Some(body.as_bytes()),
    );
    assert_eq!(cut.status, 200, "{}", cut.text());
    assert_eq!(all.text().matches(r#""_index":"#).count(), 5);
    assert_eq!(
        cut.text().matches(r#""_index":"#).count(),
        1,
        "{}",
        cut.text()
    );
}
