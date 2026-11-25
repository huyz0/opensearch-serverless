//! docs/specs/node-tail-wiring: an assembled node serves and uses the tail
//! overlay.

#![cfg(all(feature = "reader", feature = "writer"))]
// ARITH: the shared rig's sequence numbers are small constants.
#![allow(clippy::arithmetic_side_effects)]

#[path = "support/mod.rs"]
mod support;
mod tail_rig;

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oss_blob::{BlobStore, Emulator};
use oss_cluster::lease::LeaseClock;
use oss_format::{NodeLease, NodeLogEpoch, ShardKey};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::assemble::{Config, Running, Writer, assemble};
use oss_node::newest_ask::LiveWriters;
use oss_node::push_link::Leases;
use oss_node::tail_node::{LeaseWriters, TailParts};
use oss_node::writer::Routes;
use oss_transport::actions::tail::{FETCH_ACTION, FetchRequest, NEW, PUSH_ACTION};
use oss_transport::auth::identity;
use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned};
use oss_writer::log::{
    CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Refusal, ShardAdmission,
};
use oss_writer::mapping::Mapping;
use tail_rig::{NOW, SECRET, auth, client, lease, serve, shard_ref};

struct Lessor;

impl EpochLease for Lessor {
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
    fn forward(&self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
        Vec::new()
    }
}

struct Clock;

impl LeaseClock for Clock {
    fn now_millis(&self) -> i64 {
        NOW
    }
    fn pause(&self, _: Duration) {}
}

/// Leases that can be filled in once the peers are listening.
#[derive(Clone, Default)]
struct Late(Arc<Mutex<Arc<[NodeLease]>>>);

impl Late {
    fn set(&self, leases: Vec<NodeLease>) {
        *self.0.lock().expect("lock") = leases.into();
    }
}

impl Leases for Late {
    fn current(&self) -> Arc<[NodeLease]> {
        Arc::clone(&self.0.lock().expect("lock"))
    }
}

fn budget() -> MemoryBudget {
    MemoryBudget::new(
        &BudgetConfig::new(512 << 20)
            .with_pool(Pool::RequestBodies, 256 << 20)
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
            .and_then(|c| c.with_pool(Pool::Transport, 8 << 20))
            .expect("fits"),
    )
}

fn parts(enabled: bool, id: &str, leases: &Late) -> TailParts {
    TailParts::over_transport(
        enabled,
        id,
        client(),
        auth(&identity(id, "eph"), SECRET),
        leases.clone(),
        Arc::new(Clock),
    )
}

fn assembled(id: &str) -> oss_node::assemble::Assembled {
    let store = Arc::new(Emulator::new(1 << 28, 9));
    let log = NodeLogStore::new(Arc::clone(&store) as Arc<dyn BlobStore>, "", id).expect("names");
    let writer = Writer {
        committer: Arc::new(NodeLogCommitter::new(
            log,
            7,
            Box::new(Arc::new(Lessor)),
            Box::new(Arc::new(Admit)),
            CommitterConfig {
                window: Duration::from_millis(0),
                ..CommitterConfig::default()
            },
        )),
        routes: Arc::new(Local),
        opener: None,
        names: None,
        incarnation: None,
        buckets: None,
    };
    let config = Config {
        // Publishes only when the test asks.
        publish_every: Duration::from_secs(3600),
        publish_age_millis: 0,
        ..Config::new("", id, [0; 6])
    };
    let node =
        assemble(&config, store as Arc<dyn BlobStore>, &budget(), writer).expect("assembles");
    node.role
        .open_shard(
            &info(),
            0,
            3,
            Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping"),
        )
        .expect("opens");
    node
}

fn start(node: oss_node::assemble::Assembled) -> Running {
    node.start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        &budget(),
    )
    .expect("starts")
}

fn put(r: &Running, id: &str) {
    let reply = support::request(
        r.addr(),
        "PUT",
        &format!("/docs/_doc/{id}"),
        Some(br#"{"n":1}"#),
    );
    assert_eq!(reply.status, 201, "{}", reply.text());
}

fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_node_is_live_only_while_its_lease_is_unexpired_and_not_revoked() {
    let late = Late::default();
    let writers = LeaseWriters::new(late.clone(), Arc::new(Clock));
    let mut expired = lease("old", "127.0.0.1:9300", false);
    expired.expires_at_millis = NOW;
    late.set(vec![
        lease("up", "127.0.0.1:9301", false),
        lease("gone", "127.0.0.1:9302", true),
        expired,
        lease("bad", "not an address", false),
    ]);
    let live = writers.live("up").expect("live");
    assert_eq!(live.address, "127.0.0.1:9301".parse().expect("addr"));
    assert_eq!(live.ephemeral_id, "eph");
    assert_eq!(writers.live("gone"), None, "revoked");
    assert_eq!(writers.live("old"), None, "expired");
    assert_eq!(writers.live("bad"), None, "no address");
    assert_eq!(writers.live("nobody"), None);
}

#[test]
fn off_by_default_nothing_is_built_and_a_write_keeps_nothing() {
    let late = Late::default();
    let mut node = assembled("writer-1");
    assert!(
        node.wire_tail(parts(false, "writer-1", &late))
            .expect("wires")
            .is_none()
    );
    assert!(!node.search.has_overlay_source());
    let running = start(node);
    put(&running, "1");
    assert!(running.tail().is_none());
}

#[test]
fn on_it_serves_the_fetch_and_push_actions_and_wires_once() {
    let late = Late::default();
    let mut node = assembled("writer-1");
    let registry = node
        .wire_tail(parts(true, "writer-1", &late))
        .expect("wires")
        .expect("on");
    assert_eq!(registry.actions(), [FETCH_ACTION, PUSH_ACTION]);
    assert!(node.search.has_overlay_source());
    assert!(
        node.wire_tail(parts(true, "writer-1", &late))
            .expect("wires")
            .is_none(),
        "a second wire builds nothing"
    );
}

/// A writer and a reader node, each serving its tail actions on a socket and
/// finding the other through its leases.
fn pair() -> (Running, Running, tail_rig_servers::Servers) {
    let writer_leases = Late::default();
    let reader_leases = Late::default();
    let mut writer = assembled("writer-1");
    let writer_handlers = writer
        .wire_tail(parts(true, "writer-1", &writer_leases))
        .expect("wires")
        .expect("on")
        .into_handlers();
    let mut reader = assembled("reader-1");
    let reader_handlers = reader
        .wire_tail(parts(true, "reader-1", &reader_leases))
        .expect("wires")
        .expect("on")
        .into_handlers();
    let writer_socket = serve("writer-1", writer_handlers);
    let reader_socket = serve("reader-1", reader_handlers);
    writer_leases.set(vec![lease(
        "reader-1",
        &reader_socket.address().to_string(),
        false,
    )]);
    reader_leases.set(vec![lease(
        "writer-1",
        &writer_socket.address().to_string(),
        false,
    )]);
    (
        start(writer),
        start(reader),
        tail_rig_servers::Servers {
            _writer: writer_socket,
            _reader: reader_socket,
            reader_leases,
        },
    )
}

mod tail_rig_servers {
    pub struct Servers {
        pub _writer: oss_transport::server::Server,
        pub _reader: oss_transport::server::Server,
        pub reader_leases: super::Late,
    }
}

#[test]
fn an_acknowledged_write_is_kept_fetched_pushed_and_dropped_when_published() {
    let (writer, reader, servers) = pair();
    let kept = &writer.tail().expect("on").keep;
    put(&writer, "1");
    assert_eq!(kept.kept_after("docs-uuid", 0, -1).len(), 1, "kept");

    // The reader asks the writer for what it holds past its commit, over
    // the lease-found address, and tracks the shard.
    let held = &reader.tail().expect("on").wired.held;
    held.track("docs-uuid", 0, -1);
    let asker = oss_node::tail_reader_link::TransportTailFetch::new(
        client(),
        auth(&identity("reader-1", "eph"), SECRET),
        LeaseWriters::new(servers.reader_leases.clone(), Arc::new(Clock)),
    );
    let answer = asker
        .fetch(
            "writer-1",
            &FetchRequest {
                shard: shard_ref("docs-uuid"),
                after_seq_no: -1,
                subscriber: "reader-1".into(),
                subscription: NEW,
                ..FetchRequest::default()
            },
        )
        .expect("answered");
    assert_eq!(answer.records.len(), 1);
    assert!(answer.subscription >= 0, "subscribed");
    held.answered("docs-uuid", 0, "writer-1", NEW, NOW, &answer);
    assert_eq!(held.from("docs-uuid", 0, 0).len(), 1);

    // The next write is pushed to the subscriber by the flush timer.
    put(&writer, "2");
    // A write acknowledged between the fetch's subscription and its read of
    // the keep is both answered and pushed, so the reader may hold it twice;
    // it must hold the new one.
    eventually("the push never reached the reader", || {
        held.from("docs-uuid", 0, 0)
            .iter()
            .any(|(_, record)| record.id == "2")
    });

    // A publish drops what its commit holds from the keep.
    writer.role.publish_due();
    assert!(kept.kept_after("docs-uuid", 0, -1).is_empty(), "published");
}

// docs/specs/node-cluster-visibility AC-4
#[test]
fn a_mode_set_on_the_handle_reaches_the_writer_and_the_reader_together() {
    use oss_node::writer::RefreshVisibility::{Everywhere, Owner, Serving};
    let late = Late::default();
    let mut node = assembled("writer-1");
    assert!(
        node.tail_visibility().is_none(),
        "off, there is nothing to change"
    );
    node.wire_tail(parts(true, "writer-1", &late).with_visibility(Owner))
        .expect("wires")
        .expect("on");
    let handle = node.tail_visibility().expect("on");
    assert_eq!(
        (handle.writer_mode(), handle.reader_mode()),
        (Owner, (false, false))
    );
    for (mode, reader) in [
        (Everywhere, (true, true)),
        (Serving, (false, true)),
        (Owner, (false, false)),
    ] {
        handle.set(mode);
        assert_eq!((handle.writer_mode(), handle.reader_mode()), (mode, reader));
    }
}
