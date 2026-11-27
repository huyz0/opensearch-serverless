//! docs/specs/node-tasks AC-9: a task id of another node is answered by that
//! node over the transport, signed.

// ARITH: offsets into a reply are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_format::NodeLease;
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::by_query::{TaskForwarder, TaskHandler};
use oss_node::push_link::Leases;
use oss_transport::actions::tasks::TASKS_ACTION;
use oss_transport::auth::{AuthConfig, Authenticator, Guarded, SecretSource, Secrets, identity};
use oss_transport::client::{Client, ClientConfig};
use oss_transport::node::{DiscoveryNode, NodeRole, TransportAddress};
use oss_transport::server::{Handlers, Server, ServerConfig};
use oss_transport::version::WireVersion;

use super::by_query_rig::{QUERY, Rig, call, put, rig_as};
use oss_coordinator::docs::by_query::Hit;

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

/// The transport door of `owner`'s tasks, as node `owner`.
fn door(owner: &Rig) -> Server {
    let mut handlers: Handlers = HashMap::new();
    handlers.insert(
        TASKS_ACTION.into(),
        Arc::new(Guarded::new(
            auth(&identity("owner", "eph")),
            TaskHandler::new(Arc::clone(&owner.tasks)),
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

fn asker_for(door: Option<&Server>) -> Rig {
    let asker = rig_as("asker");
    if let Some(door) = door {
        let lease = NodeLease::new("owner", "eph", &door.address().to_string(), 2_000_000);
        asker.tasks.set_forward(Arc::new(TaskForwarder::new(
            Arc::new(Client::new(ClientConfig::default(), transport_budget())),
            auth("asker"),
            Fixed(vec![lease].into()),
        )));
    }
    asker
}

fn start_task(owner: &Rig) -> String {
    let hits: Vec<Hit> = ["a", "b"].iter().map(|id| put(owner, id)).collect();
    *owner.matches.hits.lock().unwrap() = hits;
    owner.matches.close();
    let reply = call(
        owner,
        "POST",
        "/docs/_delete_by_query?wait_for_completion=false",
        Some(QUERY),
    );
    let text = reply.text();
    let at = text.find(r#""task":""#).expect("task") + 8;
    text[at..].split('"').next().expect("id").to_owned()
}

// docs/specs/node-tasks AC-9
#[test]
fn another_nodes_task_is_read_and_cancelled_through_it() {
    let owner = rig_as("owner");
    let door = door(&owner);
    let asker = asker_for(Some(&door));
    let id = start_task(&owner);
    assert!(id.starts_with("owner:"), "{id}");
    let running = call(&asker, "GET", &format!("/_tasks/{id}"), None);
    assert_eq!(running.status, 200, "{}", running.text());
    assert!(
        running.text().contains(r#""completed":false"#),
        "{}",
        running.text()
    );
    assert!(
        running.text().contains(r#""node":"owner""#),
        "{}",
        running.text()
    );
    let cancel = call(&asker, "POST", &format!("/_tasks/{id}/_cancel"), None);
    assert_eq!(cancel.status, 200, "{}", cancel.text());
    owner.matches.open();
    let until = Instant::now() + Duration::from_secs(10);
    let done = loop {
        let text = call(&asker, "GET", &format!("/_tasks/{id}"), None).text();
        if text.contains(r#""completed":true"#) {
            break text;
        }
        assert!(Instant::now() < until, "{text}");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(done.contains(r#""canceled":"by user request""#), "{done}");
    // The owner's refusal comes back as it gave it, with its status.
    let gone = call(&asker, "POST", &format!("/_tasks/{id}/_cancel"), None);
    assert_eq!(gone.status, 404, "{}", gone.text());
    let unknown = call(&asker, "GET", "/_tasks/owner:5", None);
    assert_eq!(unknown.status, 404, "{}", unknown.text());
    assert!(unknown.text().contains("isn't found"), "{}", unknown.text());
}

// docs/specs/node-tasks AC-9
#[test]
fn a_node_that_cannot_be_reached_is_not_found() {
    let nowhere = asker_for(None);
    let reply = call(&nowhere, "GET", "/_tasks/owner:5", None);
    assert_eq!(reply.status, 404, "{}", reply.text());
    assert!(reply.text().contains("cannot reach"), "{}", reply.text());
    // A lease for another node: no live node holds the id's node.
    let owner = rig_as("owner");
    let door = door(&owner);
    let asker = asker_for(Some(&door));
    let other = call(&asker, "GET", "/_tasks/stranger:5", None);
    assert_eq!(other.status, 404, "{}", other.text());
    assert!(other.text().contains("cannot reach"), "{}", other.text());
}
