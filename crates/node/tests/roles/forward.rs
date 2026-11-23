//! docs/specs/forward: a bulk forwarded to its owner over a real socket,
//! and the refusals that may revoke the owner's lease early.

#![cfg(feature = "writer")]
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_cluster::early_revoke::MIN_RENEWED_AGO_MILLIS;
use oss_cluster::{ConnectProbe, EarlyRevoke, LeaseClock, LeaseConfig, Membership};
use oss_format::NodeLease;
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::cluster_owner::BulkSend;
use oss_node::forward::{BulkForwarder, ForwardError, MAX_FORWARD_BYTES};
use oss_node::push_link::Leases;
use oss_transport::actions::write::{
    BULK_ACTION, BulkOperation, BulkOutcome, BulkRequest, BulkResponse,
};
use oss_transport::auth::{AuthConfig, Authenticator, Guarded, SecretSource, Secrets, identity};
use oss_transport::client::{CallError, Client, ClientConfig};
use oss_transport::exception::RemoteException;
use oss_transport::node::{DiscoveryNode, NodeRole, TransportAddress};
use oss_transport::server::{Handler, Handlers, Request, Server, ServerConfig};
use oss_transport::stream::{Input, Output};
use oss_transport::version::WireVersion;

const T0: i64 = 1_000_000;
const TTL: i64 = 30_000;
const SECRET: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

/// A clock the test moves; a pause moves it.
struct Manual(AtomicI64);

impl LeaseClock for Manual {
    fn now_millis(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }

    fn pause(&self, pause: Duration) {
        self.0
            .fetch_add(i64::try_from(pause.as_millis()).unwrap(), Ordering::SeqCst);
    }
}

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
        Arc::new(|| T0),
    ))
}

struct Fixed(Arc<[NodeLease]>);

impl Leases for Fixed {
    fn current(&self) -> Arc<[NodeLease]> {
        Arc::clone(&self.0)
    }
}

#[derive(Default)]
struct Probe {
    refused: AtomicBool,
    calls: AtomicU32,
}

impl ConnectProbe for Probe {
    fn refused(&self, _: SocketAddr) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.refused.load(Ordering::SeqCst)
    }
}

fn budget() -> MemoryBudget {
    MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::Transport, 64 << 20)
            .expect("fits"),
    )
}

fn request() -> BulkRequest {
    BulkRequest {
        index: "logs".into(),
        shard: 1,
        operations: vec![BulkOperation {
            id: "1".into(),
            source: "{}".into(),
            ..BulkOperation::default()
        }],
        ..BulkRequest::default()
    }
}

/// An address nothing listens on.
fn closed() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let address = listener.local_addr().expect("address");
    drop(listener);
    address
}

struct Owner(Arc<Mutex<Vec<BulkRequest>>>);

impl Handler for Owner {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        let bulk = BulkRequest::read(&mut Input::new(request.message)).expect("decodes");
        let outcomes = vec![BulkOutcome::default(); bulk.operations.len()];
        self.0.lock().expect("lock").push(bulk);
        BulkResponse {
            owner_node_id: "owner".into(),
            outcomes,
            refreshed: false,
        }
        .write(out)
        .expect("writes");
        Ok(())
    }
}

fn owner() -> (Server, Arc<Mutex<Vec<BulkRequest>>>) {
    let taken = Arc::new(Mutex::new(Vec::new()));
    (serve(Owner(Arc::clone(&taken))), taken)
}

/// Answers every bulk with a failure.
struct Failing;

impl Handler for Failing {
    fn handle(&self, _: &Request<'_>, _: &mut Output<'_>) -> Result<(), RemoteException> {
        Err(RemoteException::status(
            "the owner failed",
            "INTERNAL_SERVER_ERROR",
        ))
    }
}

/// Never answers within a test's patience.
struct Silent;

impl Handler for Silent {
    fn handle(&self, _: &Request<'_>, _: &mut Output<'_>) -> Result<(), RemoteException> {
        std::thread::sleep(Duration::from_millis(800));
        Ok(())
    }
}

fn serve(handler: impl Handler + 'static) -> Server {
    let mut handlers: Handlers = HashMap::new();
    handlers.insert(
        BULK_ACTION.into(),
        Arc::new(Guarded::new(auth(&identity("owner", "eph")), handler)),
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
        budget(),
        handlers,
    )
    .expect("binds")
}

struct World {
    store: Arc<Emulator>,
    clock: Arc<Manual>,
    probe: Arc<Probe>,
    forwarder: BulkForwarder<Fixed>,
}

fn lease_at(address: &str) -> NodeLease {
    NodeLease::new("owner", "eph", address, T0 + TTL)
}

/// The owner's lease is in the store (renewed at the start) and in the
/// leases the forwarder holds, at `address`; early revoke is `early`.
fn world(address: &str, early: bool, advance: i64) -> World {
    let store = Arc::new(Emulator::new(64 << 20, 1));
    let clock = Arc::new(Manual(AtomicI64::new(T0)));
    let dynstore: Arc<dyn BlobStore> = store.clone();
    let dynclock: Arc<dyn LeaseClock> = clock.clone();
    let config = LeaseConfig {
        ttl_millis: TTL,
        max_members: 16,
        conditional_deletes: false,
    };
    let peer = Membership::new(Arc::clone(&dynstore), "", Arc::clone(&dynclock), config)
        .expect("membership");
    let mut lease = NodeLease::new("owner", "eph", address, 0);
    lease.set_roles(["data".to_owned()]);
    peer.renew(&lease).expect("enrolled");
    clock.0.fetch_add(advance, Ordering::SeqCst);
    let detector =
        Membership::new(dynstore, "", Arc::clone(&dynclock), config).expect("membership");
    let probe = Arc::new(Probe::default());
    probe.refused.store(true, Ordering::SeqCst);
    let early = Arc::new(EarlyRevoke::new(Arc::new(detector), probe.clone(), early));
    let client = Arc::new(Client::new(ClientConfig::default(), budget()));
    let forwarder = BulkForwarder::new(
        client,
        auth(&identity("coordinator", "ce")),
        Fixed(vec![lease_at(address)].into()),
        early,
        dynclock,
    );
    World {
        store,
        clock,
        probe,
        forwarder,
    }
}

fn settled() -> i64 {
    MIN_RENEWED_AGO_MILLIS + TTL / 32 + 1
}

fn stored(world: &World) -> NodeLease {
    let mut buf = vec![0u8; 1 << 16];
    let key = oss_format::lease_key("", "owner").expect("key");
    let read = world
        .store
        .read_register(&key, &mut buf)
        .expect("reads")
        .expect("present");
    NodeLease::parse(&buf[..read.len]).expect("parses")
}

#[test]
fn a_signed_bulk_reaches_the_owner_and_its_answer_comes_back() {
    let (server, taken) = owner();
    let world = world(&server.address().to_string(), true, settled());
    let response = world.forwarder.bulk("owner", &request()).expect("answered");
    assert_eq!(response.owner_node_id, "owner");
    assert_eq!(response.outcomes.len(), 1);
    assert_eq!(*taken.lock().expect("lock"), [request()]);
    assert_eq!(world.probe.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_refusal_with_early_revoke_off_reads_nothing() {
    let world = world(&closed().to_string(), false, settled());
    let before = world.store.meter().total();
    let error = world.forwarder.bulk("owner", &request()).unwrap_err();
    assert!(matches!(error, ForwardError::Failed(CallError::Refused)));
    assert_eq!(world.store.meter().total(), before);
    assert_eq!(world.probe.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_confirmed_refusal_revokes_the_lease_and_answers_gone() {
    let world = world(&closed().to_string(), true, settled());
    let (gets, puts) = (
        world.store.meter().count(S3Op::Get),
        world.store.meter().count(S3Op::Put),
    );
    let error = world.forwarder.bulk("owner", &request()).unwrap_err();
    assert!(matches!(error, ForwardError::Gone), "{error:?}");
    assert_eq!(world.probe.calls.load(Ordering::SeqCst), 1);
    assert_eq!(world.store.meter().count(S3Op::Get), gets + 1);
    assert_eq!(world.store.meter().count(S3Op::Put), puts + 1);
    assert!(stored(&world).revoked);
}

#[test]
fn a_refusal_early_revoke_declines_is_a_failed_call() {
    // Renewed just now: inside the settle time.
    let world = world(&closed().to_string(), true, 0);
    let puts = world.store.meter().count(S3Op::Put);
    let error = world.forwarder.bulk("owner", &request()).unwrap_err();
    assert!(matches!(error, ForwardError::Failed(CallError::Refused)));
    assert!(!stored(&world).revoked);
    assert_eq!(world.store.meter().count(S3Op::Put), puts);
}

/// The world's forwarder over other leases, early revoke on, as before.
fn over(world: &World, leases: Vec<NodeLease>) -> BulkForwarder<Fixed> {
    let detector = Membership::new(
        world.store.clone() as Arc<dyn BlobStore>,
        "",
        world.clock.clone() as Arc<dyn LeaseClock>,
        LeaseConfig {
            ttl_millis: TTL,
            max_members: 16,
            conditional_deletes: false,
        },
    )
    .expect("membership");
    BulkForwarder::new(
        Arc::new(Client::new(ClientConfig::default(), budget())),
        auth(&identity("coordinator", "ce")),
        Fixed(leases.into()),
        Arc::new(EarlyRevoke::new(
            Arc::new(detector),
            world.probe.clone(),
            true,
        )),
        world.clock.clone() as Arc<dyn LeaseClock>,
    )
}

#[test]
fn other_failures_never_reach_early_revoke() {
    let (server, _) = owner();
    let address = server.address().to_string();
    let world = world(&address, true, settled());
    assert!(matches!(
        world.forwarder.bulk("nobody", &request()),
        Err(ForwardError::NoPeer)
    ));
    let unparsable = over(&world, vec![lease_at("not an address")]);
    assert!(matches!(
        unparsable.bulk("owner", &request()),
        Err(ForwardError::NoPeer)
    ));
    // A revoked lease is not called.
    let mut revoked = lease_at(&address);
    revoked.revoked = true;
    let none = over(&world, vec![revoked]);
    assert!(matches!(
        none.bulk("owner", &request()),
        Err(ForwardError::NoPeer)
    ));
    // The owner answers a failure: a remote exception, not a refusal.
    let broken = serve(Failing);
    let failing = world_at(&broken.address().to_string());
    let error = failing.forwarder.bulk("owner", &request()).unwrap_err();
    assert!(
        matches!(error, ForwardError::Failed(CallError::Remote(_))),
        "{error:?}"
    );
    assert_eq!(failing.probe.calls.load(Ordering::SeqCst), 0);
    assert!(!stored(&failing).revoked);
}

fn world_at(address: &str) -> World {
    world(address, true, settled())
}

#[test]
fn a_timeout_never_reaches_early_revoke() {
    let server = serve(Silent);
    let mut slow = world_at(&server.address().to_string());
    slow.forwarder = std::mem::replace(&mut slow.forwarder, over_none())
        .with_timeout(Duration::from_millis(100));
    let error = slow.forwarder.bulk("owner", &request()).unwrap_err();
    assert!(
        matches!(error, ForwardError::Failed(CallError::Timeout)),
        "{error:?}"
    );
    assert_eq!(slow.probe.calls.load(Ordering::SeqCst), 0);
    assert!(!stored(&slow).revoked);
}

fn over_none() -> BulkForwarder<Fixed> {
    let world = world("127.0.0.1:1", true, 0);
    world.forwarder
}

#[test]
fn a_bulk_too_large_to_send_is_refused_without_a_call() {
    let (server, taken) = owner();
    let world = world_at(&server.address().to_string());
    let mut big = request();
    big.operations[0].source = "x".repeat(MAX_FORWARD_BYTES + 1);
    assert!(matches!(
        world.forwarder.bulk("owner", &big),
        Err(ForwardError::TooLarge)
    ));
    assert!(taken.lock().expect("lock").is_empty());
}

#[test]
fn every_error_says_what_happened() {
    for (error, says) in [
        (ForwardError::NoPeer, "no live lease"),
        (ForwardError::TooLarge, "too large"),
        (ForwardError::BadAnswer, "did not decode"),
        (ForwardError::Gone, "lease is gone"),
        (ForwardError::Failed(CallError::Timeout), "Timeout"),
    ] {
        assert!(error.to_string().contains(says), "{error}");
    }
}

#[test]
fn the_forwarder_is_the_send_a_cluster_route_makes() {
    let (server, taken) = owner();
    let world = world(&server.address().to_string(), true, settled());
    let response = BulkSend::bulk(&world.forwarder, "owner", &request()).expect("answered");
    assert_eq!(response.owner_node_id, "owner");
    assert_eq!(response.outcomes.len(), 1);
    assert_eq!(taken.lock().expect("lock").len(), 1);
    let missing = BulkSend::bulk(&world.forwarder, "nobody", &request()).unwrap_err();
    assert!(matches!(missing, ForwardError::NoPeer));
}
