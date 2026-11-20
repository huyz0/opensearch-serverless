//! docs/specs/node-read-forward: a document get forwarded to the node that
//! holds the shard, over a real socket and signed.

#![cfg(feature = "writer")]
#![allow(clippy::disallowed_methods)]

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_format::{NodeLease, NodeLogEpoch, ShardKey};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::doc_route::{DocService, routes as doc_routes};
use oss_node::push_link::Leases;
use oss_node::read_forward::{GetForwarder, GetHandler};
use oss_node::writer::{Routes, WriterRole};
use oss_rest::{Router, Server as Rest};
use oss_transport::actions::read::{GET_ACTION, GetRequest};
use oss_transport::auth::{AuthConfig, Authenticator, Guarded, SecretSource, Secrets, identity};
use oss_transport::client::{CallError, Client, ClientConfig};
use oss_transport::node::{DiscoveryNode, NodeRole, TransportAddress};
use oss_transport::server::{Handlers, Server, ServerConfig};
use oss_transport::version::WireVersion;
use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned};
use oss_writer::log::{
    CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Refusal, ShardAdmission,
};
use oss_writer::mapping::Mapping;

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

fn info() -> IndexInfo {
    IndexInfo {
        name: "docs".into(),
        uuid: "docs-uuid".into(),
        shards: 2,
        partition_size: 1,
        routing_shards: 2,
        routing_required: false,
    }
}

struct Cluster;

impl Routes for Cluster {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        (name == "docs").then(info)
    }

    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn forward(&self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
        unreachable!("no write is forwarded")
    }
}

/// The owner: a role holding shard 0 of `docs`, a REST door to write
/// through and the transport server answering gets.
struct Owning {
    rest: Rest,
    server: Server,
}

fn owning() -> Owning {
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let log = NodeLogStore::new(store as Arc<dyn BlobStore>, "", "owner").expect("names");
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
    role.open_shard(
        &info(),
        0,
        3,
        Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping"),
    )
    .expect("opens");
    let router = doc_routes(
        Router::new(),
        Arc::new(DocService::new(Arc::clone(&role), [0; 6])),
    )
    .expect("doc routes");
    let rest = Rest::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    let mut handlers: Handlers = HashMap::new();
    handlers.insert(
        GET_ACTION.into(),
        Arc::new(Guarded::new(
            auth(&identity("owner", "eph")),
            GetHandler::new(role, "owner"),
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
    let budget = MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::Transport, 64 << 20)
            .expect("fits"),
    );
    let server = Server::bind(
        "127.0.0.1:0".parse().expect("addr"),
        ServerConfig::new(node, "cluster", 4),
        budget,
        handlers,
    )
    .expect("binds");
    Owning { rest, server }
}

fn forwarder(owner: &Owning, signer: &str) -> GetForwarder<Fixed> {
    let address = owner.server.address().to_string();
    let budget = MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::Transport, 64 << 20)
            .expect("fits"),
    );
    GetForwarder::new(
        Arc::new(Client::new(ClientConfig::default(), budget)),
        auth(signer),
        Fixed(vec![NodeLease::new("owner", "eph", &address, 2_000_000)].into()),
    )
}

fn get(index: &str, uuid: Option<&str>, shard: i32, id: &str) -> GetRequest {
    GetRequest {
        index: index.into(),
        index_uuid: uuid.map(str::to_owned),
        shard,
        id: id.into(),
        ..GetRequest::default()
    }
}

fn put(owner: &Owning, id: &str, body: &str) {
    let reply = support::request(
        owner.rest.local_addr(),
        "PUT",
        &format!("/docs/_doc/{id}"),
        Some(body.as_bytes()),
    );
    assert!(reply.status < 300, "{}", reply.text());
}

#[test]
fn a_forwarded_get_returns_an_acknowledged_unpublished_write() {
    let owner = owning();
    put(&owner, "a", r#"{"n":1}"#);
    put(&owner, "a", r#"{"n":2}"#);
    let answer = forwarder(&owner, "coordinator")
        .get("owner", &get("docs", Some("docs-uuid"), 0, "a"))
        .expect("answers");
    assert!(answer.found);
    assert_eq!(answer.owner_node_id, "owner");
    assert_eq!(answer.id, "a");
    assert_eq!(answer.source.as_deref(), Some(r#"{"n":2}"#));
    assert_eq!(
        (answer.version, answer.seq_no, answer.primary_term),
        (2, 1, 3)
    );
}

#[test]
fn a_forwarded_get_of_a_missing_id_is_not_found() {
    let owner = owning();
    let answer = forwarder(&owner, "coordinator")
        .get("owner", &get("docs", None, 0, "nope"))
        .expect("answers");
    assert!(!answer.found);
    assert_eq!(answer.owner_node_id, "owner");
    assert_eq!(answer.source, None);
    assert_eq!(answer.id, "nope");
}

#[test]
fn a_get_of_an_unknown_index_a_stale_incarnation_or_a_shard_not_held_fails() {
    let owner = owning();
    let f = forwarder(&owner, "coordinator");
    for request in [
        get("other", None, 0, "a"),
        get("docs", Some("old-uuid"), 0, "a"),
        get("docs", None, 1, "a"),
        get("docs", None, -1, "a"),
    ] {
        let err = f.get("owner", &request).expect_err("refused");
        assert!(
            matches!(
                err,
                oss_node::forward::ForwardError::Failed(CallError::Remote(_))
            ),
            "{request:?}: {err}"
        );
    }
}

#[test]
fn a_get_to_a_node_no_lease_names_sends_nothing() {
    let owner = owning();
    let err = forwarder(&owner, "coordinator")
        .get("elsewhere", &get("docs", None, 0, "a"))
        .expect_err("no peer");
    assert!(matches!(err, oss_node::forward::ForwardError::NoPeer));
}
