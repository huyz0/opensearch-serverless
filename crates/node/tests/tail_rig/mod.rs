//! Shared by the tail link tests: signed peers on loopback.

#![allow(dead_code)]

use std::sync::Arc;

use oss_format::NodeLease;
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::push_link::Leases;
use oss_transport::actions::tail::{Record, ShardRef};
use oss_transport::auth::{AuthConfig, Authenticator, SecretSource, Secrets};
use oss_transport::client::{Client, ClientConfig};
use oss_transport::frame::VariableHeader;
use oss_transport::node::{DiscoveryNode, NodeRole, TransportAddress};
use oss_transport::server::{Handlers, Request, Server, ServerConfig};
use oss_transport::version::WireVersion;

pub const NOW: i64 = 1_000_000;
pub const SECRET: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
pub const OTHER: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

pub struct Secret(pub String);

impl SecretSource for Secret {
    fn secrets(&self) -> Option<Arc<Secrets>> {
        Some(Arc::new(Secrets {
            generation: 1,
            current: self.0.clone(),
            previous: None,
            rotated_at_millis: 0,
        }))
    }

    fn refresh(&self) {}
}

pub fn auth(who: &str, secret: &str) -> Arc<Authenticator> {
    Arc::new(Authenticator::new(
        AuthConfig {
            identity: who.to_owned(),
            window_millis: 60_000,
            replay_capacity: 64,
        },
        Arc::new(Secret(secret.to_owned())),
        Arc::new(|| NOW),
    ))
}

pub fn budget() -> MemoryBudget {
    MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::Transport, 64 << 20)
            .expect("fits"),
    )
}

pub fn client() -> Arc<Client> {
    Arc::new(Client::new(ClientConfig::default(), budget()))
}

pub fn serve(id: &str, handlers: Handlers) -> Server {
    let node = DiscoveryNode {
        name: id.into(),
        id: id.into(),
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
        budget(),
        handlers,
    )
    .expect("binds")
}

pub struct Fixed(pub Arc<[NodeLease]>);

impl Leases for Fixed {
    fn current(&self) -> Arc<[NodeLease]> {
        Arc::clone(&self.0)
    }
}

pub fn lease(id: &str, address: &str, revoked: bool) -> NodeLease {
    let mut lease = NodeLease::new(id, "eph", address, NOW + 60_000);
    lease.revoked = revoked;
    lease
}

pub fn shard_ref(uuid: &str) -> ShardRef {
    ShardRef {
        index_name: "logs".into(),
        index_uuid: uuid.into(),
        shard: 0,
    }
}

pub fn record(id: &str, seq: i64) -> Record {
    Record {
        id: id.into(),
        deletion: false,
        source: r#"{"n":1}"#.into(),
        seq_no: seq,
        primary_term: 1,
        version: seq + 1,
        routing: None,
    }
}

pub fn request<'a>(
    action: &'a str,
    message: &'a [u8],
    variable: &'a VariableHeader,
) -> Request<'a> {
    Request {
        action,
        variable,
        message,
        version: WireVersion::SHELL,
    }
}
