//! docs/specs/async-search AC-10 and AC-12: an id of another node is
//! answered by that node over the transport, signed, for the caller the first
//! node verified.

// ARITH: the test's own clocks and ports; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::collections::HashMap;
use std::sync::Arc;

use oss_format::NodeLease;
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::async_search::{
    AsyncSearchForwarder, AsyncSearchHandler, SearchRunner, Service, Submit,
};
use oss_node::jobs::{Jobs, Limits, Who};
use oss_node::push_link::Leases;
use oss_node::sql_route::{SqlBackend, SqlFailure};
use oss_rest::RestError;
use oss_transport::actions::async_search::ASYNC_SEARCH_ACTION;
use oss_transport::auth::{AuthConfig, Authenticator, Guarded, SecretSource, Secrets, identity};
use oss_transport::client::{Client, ClientConfig};
use oss_transport::node::{DiscoveryNode, NodeRole, TransportAddress};
use oss_transport::server::{Handlers, Server, ServerConfig};
use oss_transport::version::WireVersion;
use serde_json::{Value, json};

const SECRET: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

struct Secret;

impl SecretSource for Secret {
    fn secrets(&self) -> Option<Arc<Secrets>> {
        Some(Arc::new(Secrets {
            generation: 1,
            current: SECRET.to_owned(),
            previous: None,
            rotated_at_millis: 0,
        }))
    }

    fn refresh(&self) {}
}

fn auth(who: &str) -> Arc<Authenticator> {
    Arc::new(Authenticator::new(
        AuthConfig {
            identity: who.to_owned(),
            window_millis: 60_000,
            replay_capacity: 64,
        },
        Arc::new(Secret),
        Arc::new(|| 1_000_000),
    ))
}

struct Fixed(Arc<[NodeLease]>);

impl Leases for Fixed {
    fn current(&self) -> Arc<[NodeLease]> {
        Arc::clone(&self.0)
    }
}

fn transport_budget() -> MemoryBudget {
    MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::Transport, 64 << 20)
            .expect("fits"),
    )
}

struct Backend;

impl SqlBackend for Backend {
    fn mapping(&self, _: &str) -> Result<Value, SqlFailure> {
        Ok(json!({}))
    }

    fn search(&self, _: &[String], _: &Value) -> Result<Vec<u8>, SqlFailure> {
        Ok(json!({"took": 1, "hits": {"hits": []}})
            .to_string()
            .into_bytes())
    }
}

fn service(node: &str) -> Arc<Service> {
    let budget = MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::QueryExecution, 32 << 20)
            .expect("fits"),
    );
    let limits = Limits {
        pool: Pool::QueryExecution,
        ..Limits::default()
    };
    let jobs = Jobs::new(
        node,
        &budget,
        limits,
        Arc::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        }),
    );
    Arc::new(Service::new(
        jobs,
        Arc::new(SearchRunner::new(Arc::new(Backend))),
        "opensearch",
        node,
    ))
}

/// The transport door of `owner`, as node `owner`.
fn door(owner: &Arc<Service>) -> Server {
    let mut handlers: Handlers = HashMap::new();
    handlers.insert(
        ASYNC_SEARCH_ACTION.into(),
        Arc::new(Guarded::new(
            auth(&identity("owner", "eph")),
            AsyncSearchHandler::new(Arc::clone(owner)),
        )),
    );
    let node = DiscoveryNode {
        name: "owner".into(),
        id: "owner".into(),
        ephemeral_id: "eph".into(),
        host_name: "127.0.0.1".into(),
        host_address: "127.0.0.1".into(),
        address: TransportAddress {
            ip: "127.0.0.1".parse().expect("ip"),
            host: "127.0.0.1".into(),
            port: 9300,
        },
        stream_address: None,
        attributes: Vec::new(),
        roles: vec![NodeRole::data()],
        version: WireVersion::SHELL,
    };
    Server::bind(
        "127.0.0.1:0".parse().expect("addr"),
        ServerConfig::new(node, "cluster", 4),
        transport_budget(),
        handlers,
    )
    .expect("binds")
}

fn asker(door: Option<&Server>) -> Arc<Service> {
    let asker = service("asker");
    if let Some(door) = door {
        let lease = NodeLease::new("owner", "eph", &door.address().to_string(), 2_000_000);
        asker.set_relay(Arc::new(AsyncSearchForwarder::new(
            Arc::new(Client::new(ClientConfig::default(), transport_budget())),
            auth("asker"),
            Fixed(vec![lease].into()),
        )));
    }
    asker
}

fn who(user: &str) -> Who {
    Who {
        tenant: "_".into(),
        user: user.into(),
    }
}

fn kept(owner: &Service, user: &str) -> String {
    let query = [("keep_on_completion".to_owned(), "true".to_owned())];
    let w = who(user);
    let reply = owner
        .submit(&Submit {
            index: None,
            path: "/_plugins/_asynchronous_search",
            query: &query,
            body: b"{}",
            who: &w,
        })
        .expect("submits");
    let v: Value = serde_json::from_slice(&reply.body).expect("json");
    v["id"].as_str().expect("id").to_owned()
}

fn get(s: &Service, id: &str, user: &str) -> Result<(u16, Value), RestError> {
    let reply = s.get(&who(user), id, "/_plugins/_asynchronous_search/x", &[])?;
    Ok((
        reply.status,
        serde_json::from_slice(&reply.body).expect("json"),
    ))
}

// AC-10: another node's id is read and deleted through it.
#[test]
fn another_nodes_search_is_read_and_deleted_through_it() {
    let owner = service("owner");
    let door = door(&owner);
    let asker = asker(Some(&door));
    let id = kept(&owner, "alice");
    assert!(id.starts_with("owner:"), "{id}");
    let (status, v) = get(&asker, &id, "alice").expect("relayed");
    assert_eq!((status, v["state"].as_str()), (200, Some("STORE_RESIDENT")));
    let ack = asker.delete(&who("alice"), &id).expect("relayed");
    assert_eq!(
        (ack.status, ack.body.as_slice()),
        (200, br#"{"acknowledged":true}"#.as_slice())
    );
    assert_eq!(
        get(&owner, &id, "alice").expect_err("dropped").status(),
        404
    );
}

// AC-12: the owner answers only the caller the first node named.
#[test]
fn another_user_is_not_found_through_the_relay() {
    let owner = service("owner");
    let door = door(&owner);
    let asker = asker(Some(&door));
    let id = kept(&owner, "alice");
    // The owner's refusal comes back as it gave it, with its status.
    assert_eq!(get(&asker, &id, "mallory").expect("answered").0, 404);
    assert_eq!(
        asker.delete(&who("mallory"), &id).expect("answered").status,
        404
    );
    assert_eq!(get(&asker, &id, "alice").expect("hers").0, 200);
}

// AC-10: no live node, no relay and a malformed id.
#[test]
fn an_unknown_node_is_not_found_naming_it_and_a_malformed_id_is_a_bad_request() {
    let owner = service("owner");
    let door = door(&owner);
    let with_relay = asker(Some(&door));
    let without = asker(None);
    for s in [&with_relay, &without] {
        let err = get(s, "ghost:7", "alice").expect_err("no such node");
        let mut body = Vec::new();
        err.write_body(&mut body).expect("body");
        assert_eq!(err.status(), 404);
        assert!(String::from_utf8_lossy(&body).contains("ghost"), "{body:?}");
        assert_eq!(
            get(s, "not-an-id", "alice")
                .expect_err("malformed")
                .status(),
            400
        );
        assert_eq!(
            s.delete(&who("alice"), "ghost:7")
                .expect_err("no node")
                .status(),
            404
        );
    }
}
