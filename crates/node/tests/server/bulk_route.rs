//! docs/specs/node-bulk-route: `_bulk` over a writer node's own shards.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_format::{NodeLogEpoch, ShardKey};
use oss_mem::MemoryBudget;
use oss_node::bulk_route::{BulkService, routes};
use oss_node::writer::{Routes, WriterRole};
use oss_rest::{Router, Server};
use oss_writer::bulk::{BulkBudget, IndexInfo, ItemResult, Owner, Planned, StreamConfig};
use oss_writer::log::{
    CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Refusal, ShardAdmission,
};
use oss_writer::mapping::Mapping;

struct Lease;

impl EpochLease for Lease {
    fn record(&self, _: &[NodeLogEpoch]) -> Result<(), Refusal> {
        Ok(())
    }
}

struct Admit;

impl ShardAdmission for Admit {
    fn admit(&self, _: &ShardKey, _: i64, _: u64) -> Result<(), Refusal> {
        Ok(())
    }
}

/// One index, `docs`, of one shard held here; any other name is unknown.
struct Local;

fn info() -> IndexInfo {
    IndexInfo {
        name: "docs".into(),
        uuid: "docs-uuid".into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
        write_block: Default::default(),
        default_pipeline: None,
        final_pipeline: None,
    }
}

impl Routes for Local {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        (name == "docs").then(info)
    }

    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn forward(&self, _: &str, _: &IndexInfo, _: u32, group: &[Planned<'_>]) -> Vec<ItemResult> {
        unreachable!("every shard is local: {}", group.len())
    }
}

struct Rig {
    server: Server,
    budget: BulkBudget,
}

fn rig(slice_bytes: usize) -> Rig {
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let log = NodeLogStore::new(store as Arc<dyn BlobStore>, "", "node-1").expect("names");
    let committer = Arc::new(NodeLogCommitter::new(
        log,
        7,
        Box::new(Arc::new(Lease)),
        Box::new(Arc::new(Admit)),
        CommitterConfig {
            window: Duration::from_millis(0),
            ..CommitterConfig::default()
        },
    ));
    let role = Arc::new(WriterRole::new(committer, Arc::new(Local)));
    role.open_shard(
        &info(),
        0,
        3,
        Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping"),
    )
    .expect("opens");
    let budget = BulkBudget::new(1 << 20, 2);
    let service = Arc::new(BulkService::new(
        role,
        budget.clone(),
        StreamConfig {
            slice_bytes,
            max_wait: Duration::from_secs(5),
        },
        [0; 6],
    ));
    let router = routes(Router::new(), service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server = Server::start(
        listener,
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    Rig { server, budget }
}

fn post(r: &Rig, method: &str, target: &str, body: &str) -> support::Reply {
    support::request(r.server.local_addr(), method, target, Some(body.as_bytes()))
}

const TWO: &str =
    "{\"index\":{\"_id\":\"1\"}}\n{\"n\":1}\n{\"index\":{\"_id\":\"2\"}}\n{\"n\":2}\n";

#[test]
fn a_bulk_applies_its_items_by_post_and_put_with_and_without_an_index() {
    let r = rig(1 << 20);
    for (method, target, body) in [
        ("POST", "/docs/_bulk", TWO.to_owned()),
        ("PUT", "/docs/_bulk", TWO.to_owned()),
        (
            "POST",
            "/_bulk",
            TWO.replace("{\"_id\"", "{\"_index\":\"docs\",\"_id\""),
        ),
    ] {
        let reply = post(&r, method, target, &body);
        let text = reply.text();
        assert_eq!(reply.status, 200, "{text}");
        assert!(text.contains(r#""errors":false,"items":["#), "{text}");
        assert!(
            text.contains(r#""_index":"docs","_id":"1""#) && text.contains(r#""_id":"2""#),
            "{text}"
        );
        assert!(
            text.find(r#""_id":"1""#) < text.find(r#""_id":"2""#),
            "in order: {text}"
        );
    }
}

#[test]
fn a_refused_item_fails_alone() {
    let r = rig(1 << 20);
    let body =
        "{\"index\":{\"_id\":\"1\"}}\n{\"n\":\"abc\"}\n{\"index\":{\"_id\":\"2\"}}\n{\"n\":2}\n";
    let text = post(&r, "POST", "/docs/_bulk", body).text();
    assert!(text.contains(r#""errors":true"#), "{text}");
    assert!(text.contains(r#""_id":"2","_version":1"#), "{text}");
    assert!(text.contains(r#""_id":"1","status":400"#), "{text}");
}

#[test]
fn refresh_follows_the_parameter() {
    let r = rig(1 << 20);
    for refresh in ["true", "", "wait_for"] {
        let text = post(&r, "POST", &format!("/docs/_bulk?refresh={refresh}"), TWO).text();
        assert!(
            text.contains(r#""forced_refresh":true"#),
            "{refresh}: {text}"
        );
    }
    for target in ["/docs/_bulk?refresh=false", "/docs/_bulk"] {
        let text = post(&r, "POST", target, TWO).text();
        assert!(!text.contains("forced_refresh"), "{text}");
    }
    let bad = post(&r, "POST", "/docs/_bulk?refresh=maybe", TWO);
    assert_eq!(bad.status, 400, "{}", bad.text());
    assert!(bad.text().contains("illegal_argument_exception"));
}

#[test]
fn a_bad_body_and_an_unknown_index_are_answered() {
    let r = rig(1 << 20);
    let bad = post(&r, "POST", "/docs/_bulk", "{not json\n");
    assert_eq!(bad.status, 400, "{}", bad.text());
    let text = post(&r, "POST", "/nope/_bulk", TWO).text();
    assert!(text.contains(r#""errors":true"#), "{text}");
    assert!(text.contains(r#""_index":"nope""#), "{text}");
}

#[test]
fn a_body_of_several_slices_is_applied_whole() {
    let r = rig(512);
    let body: String = (0..300)
        .map(|i| format!("{{\"index\":{{\"_id\":\"d{i}\"}}}}\n{{\"n\":{i}}}\n"))
        .collect();
    assert!(body.len() > 4 * 512);
    let text = post(&r, "POST", "/docs/_bulk", &body).text();
    assert_eq!(
        text.matches(r#""result":"created""#).count(),
        300,
        "{text:.300}"
    );
    assert_eq!(r.budget.in_use(), (0, 0), "the budget is free again");
}

/// The millis a time-based id was made at: its bytes lie as
/// `IdGenerator::next` lays them out.
fn id_millis(id: &str) -> u64 {
    let value = |c: char| {
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
            .find(c)
            .expect("base64url") as u32
    };
    let chars: Vec<u32> = id.chars().map(value).collect();
    let bytes: Vec<u8> = chars
        .chunks(4)
        .flat_map(|c| {
            let n = (c[0] << 18) | (c[1] << 12) | (c[2] << 6) | c[3];
            [(n >> 16) as u8, (n >> 8) as u8, n as u8]
        })
        .collect();
    let at = |i: usize| u64::from(bytes[i]);
    at(14) | at(12) << 8 | at(2) << 16 | at(3) << 24 | at(4) << 32 | at(5) << 40
}

#[test]
fn an_item_without_an_id_gets_a_time_based_one() {
    let r = rig(1 << 20);
    let text = post(&r, "POST", "/docs/_bulk", "{\"index\":{}}\n{\"n\":1}\n").text();
    let id = text
        .split(r#""_id":""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("an id")
        .to_owned();
    assert_eq!(id.len(), 20, "{text}");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64;
    let made = id_millis(&id);
    assert!(made <= now && now - made < 60_000, "{made} against {now}");
}

#[test]
fn require_alias_refuses_an_index_that_is_not_one() {
    let r = rig(1 << 20);
    let text = post(&r, "POST", "/docs/_bulk?require_alias=true", TWO).text();
    assert!(text.contains(r#""errors":true"#), "{text}");
    let text = post(&r, "POST", "/docs/_bulk?require_alias=false", TWO).text();
    assert!(text.contains(r#""errors":false"#), "{text}");
}
