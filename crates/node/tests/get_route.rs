//! docs/specs/node-doc-read-routes: the document get routes and `_refresh`
//! over a writer node's own shards.

#[path = "support/mod.rs"]
mod support;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_format::{NodeLogEpoch, ShardKey};
use oss_mem::MemoryBudget;
use oss_node::doc_route::{DocService, routes as doc_routes};
use oss_node::get_route::{GetService, routes as get_routes};
use oss_node::refresh_route::{RefreshService, routes as refresh_routes};
use oss_node::writer::{Routes, WriterRole};
use oss_rest::{Router, Server};
use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned};
use oss_writer::index::Fetched;
use oss_writer::log::{
    CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Refusal, ShardAdmission,
};
use oss_writer::mapping::Mapping;
use serde_json::{Value, json};

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

fn info(name: &str, shards: u32, routing_required: bool) -> IndexInfo {
    IndexInfo {
        name: name.into(),
        uuid: format!("{name}-uuid"),
        shards,
        partition_size: 1,
        routing_shards: shards,
        routing_required,
    }
}

/// `docs` has one shard, `wide` three, `routed` requires routing, and
/// `remote` has a shard another node owns.
struct Cluster;

impl Routes for Cluster {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        match name {
            "docs" => Some(info("docs", 1, false)),
            "wide" => Some(info("wide", 3, false)),
            "routed" => Some(info("routed", 1, true)),
            "remote" => Some(info("remote", 2, false)),
            "dead" => Some(info("dead", 1, false)),
            _ => None,
        }
    }

    fn owner(&self, index: &IndexInfo, shard: u32) -> Owner {
        if index.name == "remote" && shard == 1 {
            Owner::Node("node-2".into())
        } else if index.name == "dead" {
            Owner::Node("node-3".into())
        } else {
            Owner::Local
        }
    }

    fn refresh_remote(&self, node: &str, _: &IndexInfo, _: u32) -> Result<(), String> {
        (node == "node-2")
            .then_some(())
            .ok_or_else(|| "no answer".to_owned())
    }

    fn get_remote(
        &self,
        node: &str,
        _: &IndexInfo,
        _: u32,
        id: &str,
    ) -> Result<Option<Fetched>, String> {
        match (node, id) {
            ("node-2", "0" | "1" | "2" | "3" | "4" | "5") => Ok(Some(Fetched {
                version: 7,
                seq_no: 4,
                primary_term: 2,
                source: br#"{"far":true}"#.to_vec(),
            })),
            ("node-2", _) => Ok(None),
            _ => Err("no answer".to_owned()),
        }
    }

    fn forward(&self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
        unreachable!("no write is forwarded")
    }
}

struct Rig {
    server: Server,
}

fn rig() -> Rig {
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
    let role = Arc::new(WriterRole::new(committer, Arc::new(Cluster)));
    for (name, shards) in [("docs", 1), ("wide", 3), ("routed", 1), ("remote", 2)] {
        for shard in 0..shards {
            if name == "remote" && shard == 1 {
                continue;
            }
            role.open_shard(
                &info(name, shards, false),
                shard,
                3,
                Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping"),
            )
            .expect("opens");
        }
    }
    let router = doc_routes(
        Router::new(),
        Arc::new(DocService::new(Arc::clone(&role), [0; 6])),
    )
    .expect("doc routes");
    let router = get_routes(router, Arc::new(GetService::new(Arc::clone(&role)))).expect("gets");
    let router =
        refresh_routes(router, Arc::new(RefreshService::new(role))).expect("refresh routes");
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    Rig { server }
}

fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

fn json_of(reply: &support::Reply) -> Value {
    serde_json::from_slice(&reply.body).unwrap_or_else(|e| panic!("{e}: {}", reply.text()))
}

fn put(r: &Rig, target: &str, body: &str) {
    let reply = call(r, "PUT", target, Some(body));
    assert!(reply.status < 300, "{}", reply.text());
}

#[test]
fn a_get_sees_a_write_no_refresh_has_made_visible() {
    let r = rig();
    put(&r, "/docs/_doc/a", r#"{"n":1}"#);
    let reply = call(&r, "GET", "/docs/_doc/a", None);
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        json_of(&reply),
        json!({"_index":"docs","_id":"a","_version":1,"_seq_no":0,"_primary_term":3,
               "found":true,"_source":{"n":1}})
    );
    // A second write is the next version, and `realtime=false` reads the old one.
    put(&r, "/docs/_doc/a", r#"{"n":2}"#);
    let stale = json_of(&call(&r, "GET", "/docs/_doc/a?realtime=false", None));
    assert_eq!(stale["_version"], 1, "{stale}");
    assert_eq!(
        json_of(&call(&r, "GET", "/docs/_doc/a", None))["_version"],
        2
    );
    // `refresh=true` makes the write visible to the get itself.
    put(&r, "/docs/_doc/b", r#"{"n":5}"#);
    let unseen = call(&r, "GET", "/docs/_doc/b?realtime=false", None);
    assert_eq!(unseen.status, 404, "{}", unseen.text());
    let seen = call(&r, "GET", "/docs/_doc/b?realtime=false&refresh=true", None);
    assert_eq!(seen.status, 200, "{}", seen.text());
    assert_eq!(
        call(&r, "GET", "/docs/_doc/b?realtime=maybe", None).status,
        400
    );
}

#[test]
fn a_deleted_or_unknown_id_is_not_found_and_head_has_no_body() {
    let r = rig();
    put(&r, "/docs/_doc/a", r#"{"n":1}"#);
    assert_eq!(call(&r, "DELETE", "/docs/_doc/a", None).status, 200);
    for id in ["a", "never"] {
        let reply = call(&r, "GET", &format!("/docs/_doc/{id}"), None);
        assert_eq!(reply.status, 404);
        assert_eq!(
            json_of(&reply),
            json!({"_index":"docs","_id":id,"found":false})
        );
        let head = call(&r, "HEAD", &format!("/docs/_doc/{id}"), None);
        assert_eq!((head.status, head.body.len()), (404, 0));
    }
    put(&r, "/docs/_doc/live", r#"{"n":1}"#);
    let head = call(&r, "HEAD", "/docs/_doc/live", None);
    assert_eq!((head.status, head.body.len()), (200, 0));
}

#[test]
fn source_false_leaves_the_source_out_and_other_values_are_refused() {
    let r = rig();
    put(&r, "/docs/_doc/a", r#"{"n":1}"#);
    let bare = json_of(&call(&r, "GET", "/docs/_doc/a?_source=false", None));
    assert_eq!(bare.get("_source"), None, "{bare}");
    assert_eq!(bare["found"], true);
    let kept = json_of(&call(&r, "GET", "/docs/_doc/a?_source=true", None));
    assert_eq!(kept["_source"], json!({"n":1}));
    let empty = json_of(&call(&r, "GET", "/docs/_doc/a?_source=", None));
    assert_eq!(empty["_source"], json!({"n":1}), "{empty}");
    assert_eq!(call(&r, "GET", "/docs/_doc/a?_source=n", None).status, 400);
}

#[test]
fn the_source_route_answers_the_source_alone() {
    let r = rig();
    put(&r, "/docs/_doc/a", r#"{"n":1}"#);
    let reply = call(&r, "GET", "/docs/_source/a", None);
    assert_eq!(reply.status, 200);
    assert_eq!(json_of(&reply), json!({"n":1}));
    let none = call(&r, "GET", "/docs/_source/missing", None);
    assert_eq!(none.status, 404);
    assert_eq!(
        json_of(&none)["error"]["reason"],
        "Source not found [docs]/[missing]"
    );
    assert_eq!(call(&r, "HEAD", "/docs/_source/a", None).status, 200);
}

#[test]
fn an_unknown_index_is_not_found_and_routing_picks_the_shard() {
    let r = rig();
    let gone = call(&r, "GET", "/nope/_doc/a", None);
    assert_eq!(gone.status, 404);
    assert_eq!(json_of(&gone)["error"]["type"], "index_not_found_exception");
    // Written with a routing, found only with it.
    put(&r, "/wide/_doc/a?routing=x", r#"{"n":1}"#);
    let found = call(&r, "GET", "/wide/_doc/a?routing=x", None);
    assert_eq!(found.status, 200, "{}", found.text());
    let other = (0..20)
        .map(|i| format!("r{i}"))
        .find(|k| call(&r, "GET", &format!("/wide/_doc/a?routing={k}"), None).status == 404);
    assert!(other.is_some(), "some routing reaches another shard");
    // A routed index refuses a get with none.
    let refused = call(&r, "GET", "/routed/_doc/a", None);
    assert_eq!(refused.status, 400, "{}", refused.text());
    assert_eq!(
        json_of(&refused)["error"]["type"],
        "routing_missing_exception"
    );
}

#[test]
fn refresh_makes_every_shard_visible_and_counts_them() {
    let r = rig();
    for i in 0..9 {
        put(&r, &format!("/wide/_doc/{i}"), r#"{"n":1}"#);
    }
    for i in 0..9 {
        let t = format!("/wide/_doc/{i}?realtime=false");
        assert_eq!(call(&r, "GET", &t, None).status, 404);
    }
    for method in ["POST", "GET"] {
        let reply = call(&r, method, "/wide/_refresh", None);
        assert_eq!(reply.status, 200, "{}", reply.text());
        assert_eq!(
            json_of(&reply),
            json!({"_shards":{"total":3,"successful":3,"failed":0}})
        );
    }
    for i in 0..9 {
        let t = format!("/wide/_doc/{i}?realtime=false");
        assert_eq!(call(&r, "GET", &t, None).status, 200, "{i}");
    }
    assert_eq!(call(&r, "POST", "/nope/_refresh", None).status, 404);
}

#[test]
fn a_get_of_a_shard_another_node_owns_is_answered_by_that_owner() {
    let r = rig();
    let replies: Vec<(usize, support::Reply)> = (0..20)
        .map(|i| (i, call(&r, "GET", &format!("/remote/_doc/{i}"), None)))
        .collect();
    let far: Vec<&(usize, support::Reply)> = replies
        .iter()
        .filter(|(_, x)| json_of(x).get("_source") == Some(&json!({"far":true})))
        .collect();
    assert!(!far.is_empty(), "an id routes to the remote shard");
    let (i, reply) = far[0];
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        json_of(reply),
        json!({"_index":"remote","_id":i.to_string(),"_version":7,"_seq_no":4,
               "_primary_term":2,"found":true,"_source":{"far":true}})
    );
    let source = call(&r, "GET", &format!("/remote/_source/{i}"), None);
    assert_eq!(json_of(&source), json!({"far":true}));
    assert!(
        replies.iter().any(|(_, x)| x.status == 404),
        "a local id, or one the owner does not hold"
    );
}

#[test]
fn a_remote_refresh_counts_the_owners_shard_and_an_unreachable_owner_fails_it() {
    let r = rig();
    let refresh = json_of(&call(&r, "POST", "/remote/_refresh", None));
    assert_eq!(
        refresh,
        json!({"_shards":{"total":2,"successful":2,"failed":0}})
    );
    let dead = json_of(&call(&r, "POST", "/dead/_refresh", None));
    assert_eq!(
        dead,
        json!({"_shards":{"total":1,"successful":0,"failed":1}})
    );
    let unavailable = call(&r, "GET", "/dead/_doc/a", None);
    assert_eq!(unavailable.status, 503, "{}", unavailable.text());
    let error = &json_of(&unavailable)["error"];
    assert_eq!(error["type"], "no_shard_available_action_exception");
    assert!(
        error["reason"]
            .as_str()
            .is_some_and(|m| m.contains("node-3") && m.contains("no answer")),
        "{error}"
    );
}
