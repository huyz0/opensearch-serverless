//! The writer's push over a real socket: placement picks the readers, the
//! link signs the push, and a guarded reader-side handler verifies it.

#![cfg(feature = "writer")]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use oss_cluster::placement::{ROLE_SEARCH, ShardKey, readers_for};
use oss_format::NodeLease;
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::push_link::{Leases, PlacementTargets, TransportPushLink};
use oss_node::push_sender::{PushLink, PushTargets};
use oss_transport::actions::push::{PUSH_ACTION, PushRequest, PushResponse};
use oss_transport::auth::{AuthConfig, Authenticator, Guarded, SecretSource, Secrets, identity};
use oss_transport::client::{Client, ClientConfig};
use oss_transport::exception::RemoteException;
use oss_transport::node::{DiscoveryNode, NodeRole, TransportAddress};
use oss_transport::server::{Handler, Handlers, Request, Server, ServerConfig};
use oss_transport::stream::{Input, Output};
use oss_transport::version::WireVersion;

const NOW: i64 = 1_000_000;

struct Fixed(Arc<[NodeLease]>);

impl Leases for Fixed {
    fn current(&self) -> Arc<[NodeLease]> {
        Arc::clone(&self.0)
    }
}

struct Secret(String);

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

fn auth(who: &str, secret: &str) -> Arc<Authenticator> {
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

fn lease(id: &str, roles: &[&str], address: &str, revoked: bool) -> NodeLease {
    let mut lease = NodeLease::new(id, "eph", address, NOW + 60_000);
    lease.set_roles(roles.iter().map(|r| (*r).to_owned()));
    lease.revoked = revoked;
    lease
}

fn leases(all: Vec<NodeLease>) -> Fixed {
    Fixed(all.into())
}

/// Records the push it is given and answers that the shard is open.
struct Taking(Arc<Mutex<Vec<PushRequest>>>);

impl Handler for Taking {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        let pushed = PushRequest::read(&mut Input::new(request.message)).expect("decodes");
        self.0.lock().expect("lock").push(pushed);
        PushResponse { open: true }.write(out).expect("writes");
        Ok(())
    }
}

fn reader(secret: &str) -> (Server, Arc<Mutex<Vec<PushRequest>>>) {
    let taken = Arc::new(Mutex::new(Vec::new()));
    let mut handlers: Handlers = HashMap::new();
    handlers.insert(
        PUSH_ACTION.into(),
        Arc::new(Guarded::new(
            auth(&identity("reader-1", "eph"), secret),
            Taking(Arc::clone(&taken)),
        )),
    );
    let budget = MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::Transport, 64 << 20)
            .expect("fits"),
    );
    let node = DiscoveryNode {
        name: "reader-1".into(),
        id: "reader-1".into(),
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
    let server = Server::bind(
        "127.0.0.1:0".parse().expect("addr"),
        ServerConfig::new(node, "cluster", 4),
        budget,
        handlers,
    )
    .expect("binds");
    (server, taken)
}

fn client() -> Arc<Client> {
    let budget = MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::Transport, 64 << 20)
            .expect("fits"),
    );
    Arc::new(Client::new(ClientConfig::default(), budget))
}

fn request() -> PushRequest {
    PushRequest {
        index_name: "logs".into(),
        index_uuid: "u1".into(),
        shard: 2,
        commit: b"commit".to_vec(),
        mapping_version: 4,
        settings_version: 3,
        ..PushRequest::default()
    }
}

const SECRET: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const OTHER: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

fn link(address: SocketAddr, secret: &str, revoked: bool) -> TransportPushLink<Fixed> {
    TransportPushLink::new(
        client(),
        auth(&identity("writer-1", "eph"), secret),
        leases(vec![lease(
            "reader-1",
            &[ROLE_SEARCH],
            &address.to_string(),
            revoked,
        )]),
    )
}

#[test]
fn a_signed_push_reaches_the_reader_and_its_answer_comes_back() {
    let (server, taken) = reader(SECRET);
    let link = link(server.address(), SECRET, false);
    assert_eq!(link.send("reader-1", &request()), Ok(true));
    assert_eq!(*taken.lock().expect("lock"), [request()]);
}

#[test]
fn a_push_that_cannot_be_delivered_or_verified_is_a_link_error() {
    let (server, taken) = reader(SECRET);
    // Signed with another secret: the reader refuses it.
    let wrong = link(server.address(), OTHER, false);
    assert!(wrong.send("reader-1", &request()).is_err());
    // A node the leases do not name, and one whose lease was revoked.
    let right = link(server.address(), SECRET, false);
    assert!(right.send("reader-2", &request()).is_err());
    let revoked = link(server.address(), SECRET, true);
    assert!(revoked.send("reader-1", &request()).is_err());
    // A lease whose address is no address, and one nobody listens on.
    let mut bad = lease("reader-1", &[ROLE_SEARCH], "not an address", false);
    let nowhere = TransportPushLink::new(
        client(),
        auth(&identity("writer-1", "eph"), SECRET),
        leases(vec![bad.clone()]),
    );
    assert!(nowhere.send("reader-1", &request()).is_err());
    bad.address = "127.0.0.1:1".into();
    let closed = TransportPushLink::new(
        client(),
        auth(&identity("writer-1", "eph"), SECRET),
        leases(vec![bad]),
    );
    assert!(closed.send("reader-1", &request()).is_err());
    assert!(taken.lock().expect("lock").is_empty(), "nothing was taken");
}

#[test]
fn the_targets_are_the_ranked_search_nodes_other_than_this_one() {
    let all: Vec<NodeLease> = (1..=6)
        .map(|n| lease(&format!("n{n}"), &[ROLE_SEARCH], "127.0.0.1:9", false))
        .chain([lease("ingest", &["ingest"], "127.0.0.1:9", false)])
        .collect();
    let ranked = |limit: usize| -> Vec<String> {
        readers_for(ShardKey::new("logs", 5), all.iter(), ROLE_SEARCH, limit)
            .into_iter()
            .map(str::to_owned)
            .collect()
    };
    let top = ranked(2);
    // This node is the best-ranked one: the others follow it, none is it.
    let me = top[0].clone();
    let targets = PlacementTargets::new(leases(all.clone()), &me, 1);
    assert_eq!(targets.readers("logs", 5), top[1..], "at least two ranked");
    // A hot shard's wider spread.
    let wide = PlacementTargets::new(leases(all.clone()), &me, 4);
    assert_eq!(wide.readers("logs", 5), ranked(4)[1..]);
    // Another node asks: it is the one left out, and the ingest node never
    // appears.
    let other = top[1].clone();
    let from_other = PlacementTargets::new(leases(all), &other, 1).readers("logs", 5);
    assert_eq!(from_other, [top[0].clone()]);
    // No leases, no readers.
    assert!(
        PlacementTargets::new(leases(vec![]), "n1", 3)
            .readers("logs", 5)
            .is_empty()
    );
}
