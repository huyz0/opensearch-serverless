//! The by-query and reindex rig: a writer role and server over the emulator
//! with two indexes held here and the search half faked.

// ARITH: offsets into a reply are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use crate::support;

use std::net::TcpListener;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_coordinator::docs::by_query::Hit;
use oss_format::{NodeLogEpoch, ShardKey};
use oss_mem::MemoryBudget;
use oss_node::by_query::{ByQueryService, ShardMatches, Tasks};
use oss_node::doc_route::{self, DocService};
use oss_node::writer::{Routes, WriterRole};
use oss_rest::{RestError, Router, Server};
use oss_writer::bulk::BulkBudget;
use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned};
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

/// Two indexes, `docs` and `dest`, of one shard each held here; any other
/// name is unknown.
struct Local;

fn info() -> IndexInfo {
    info_of("docs")
}

fn info_of(name: &str) -> IndexInfo {
    IndexInfo {
        name: name.into(),
        uuid: format!("{name}-uuid"),
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
        (name == "docs" || name == "dest").then(|| info_of(name))
    }

    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn forward(&self, _: &str, _: &IndexInfo, _: u32, group: &[Planned<'_>]) -> Vec<ItemResult> {
        unreachable!("every shard is local: {}", group.len())
    }
}

/// The search half: the hits the test says match, and the queries asked.
#[derive(Default)]
pub(crate) struct Matches {
    pub(crate) hits: Mutex<Vec<Hit>>,
    pub(crate) queries: Mutex<Vec<String>>,
    /// While true the search waits, holding a walk where a test can see it.
    gate: Mutex<bool>,
    opened: Condvar,
}

impl Matches {
    /// Holds every walk at its first search until [`Matches::open`].
    pub(crate) fn close(&self) {
        *self.gate.lock().unwrap() = true;
    }

    /// Lets the held walks go.
    pub(crate) fn open(&self) {
        *self.gate.lock().unwrap() = false;
        self.opened.notify_all();
    }
}

impl ShardMatches for Matches {
    fn newest(&self, _: &str, _: u32, query: &str) -> Result<Option<i64>, RestError> {
        let mut closed = self.gate.lock().unwrap();
        while *closed {
            closed = self.opened.wait(closed).unwrap();
        }
        drop(closed);
        self.queries.lock().unwrap().push(query.to_owned());
        Ok(self.hits.lock().unwrap().iter().map(|h| h.seq_no).max())
    }

    fn page(
        &self,
        _: &str,
        _: u32,
        _: &str,
        up_to: i64,
        after: Option<i64>,
        size: usize,
    ) -> Result<Vec<Hit>, RestError> {
        let mut hits: Vec<Hit> = self
            .hits
            .lock()
            .unwrap()
            .iter()
            .filter(|h| h.seq_no <= up_to && after.is_none_or(|a| h.seq_no > a))
            .cloned()
            .collect();
        hits.sort_by_key(|h| h.seq_no);
        hits.truncate(size);
        Ok(hits)
    }
}

pub(crate) struct Rig {
    pub(crate) tasks: Arc<Tasks>,
    pub(crate) store: Arc<Emulator>,
    pub(crate) server: Server,
    pub(crate) matches: Arc<Matches>,
}

pub(crate) fn rig() -> Rig {
    rig_as("node-1")
}

/// A rig whose node is called `node`.
pub(crate) fn rig_as(node: &str) -> Rig {
    build(node, None)
}

/// A rig that resolves a script named by `id` through `scripts`.
pub(crate) fn rig_scripts(scripts: Arc<dyn oss_reader::stored_scripts::StoredScripts>) -> Rig {
    build("node-1", Some(scripts))
}

fn build(node: &str, scripts: Option<Arc<dyn oss_reader::stored_scripts::StoredScripts>>) -> Rig {
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let log =
        NodeLogStore::new(Arc::clone(&store) as Arc<dyn BlobStore>, "", "node-1").expect("names");
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
    role.open_shard(
        &info_of("dest"),
        0,
        3,
        Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping"),
    )
    .expect("opens");
    let budget = MemoryBudget::new(&support::budget_config());
    let matches = Arc::new(Matches::default());
    let docs = Arc::new(DocService::new(Arc::clone(&role), [0; 6]));
    let gets = Arc::new(oss_node::get_route::GetService::new(Arc::clone(&role)));
    let tasks = Arc::new(Tasks::new(node, &budget, 1_000));
    let mut service = ByQueryService::new(
        role,
        Arc::clone(&matches) as Arc<dyn ShardMatches>,
        BulkBudget::new(1 << 20, 4),
        [0; 6],
        Arc::clone(&tasks),
    );
    if let Some(scripts) = scripts {
        service = service.with_scripts(scripts);
    }
    let by_query = Arc::new(service);
    let router = doc_route::routes(Router::new(), docs).expect("routes");
    let router = oss_node::get_route::routes(router, gets).expect("routes");
    let router = oss_node::by_query::routes(router, by_query).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        tasks,
        store,
        server,
        matches,
    }
}

pub(crate) fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

/// Indexes `id`, and answers the hit a search would have seen.
pub(crate) fn put(r: &Rig, id: &str) -> Hit {
    let reply = call(r, "PUT", &format!("/docs/_doc/{id}"), Some(r#"{"n":1}"#));
    assert!(
        reply.status == 200 || reply.status == 201,
        "{}",
        reply.text()
    );
    let text = reply.text();
    let after = |key: &str| {
        let at = text.find(key).expect(key) + key.len();
        text[at..]
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .expect("digits")
            .parse::<i64>()
            .expect("number")
    };
    Hit {
        id: id.to_owned(),
        seq_no: after(r#""_seq_no":"#),
        primary_term: after(r#""_primary_term":"#),
    }
}

pub(crate) fn gone(r: &Rig, id: &str) -> bool {
    call(r, "DELETE", &format!("/docs/_doc/{id}"), None)
        .text()
        .contains("not_found")
}

pub(crate) const QUERY: &str = r#"{"query":{"match":{"n":1}}}"#;

pub(crate) fn source_of(r: &Rig, id: &str) -> String {
    call(r, "GET", &format!("/docs/_doc/{id}"), None).text()
}

/// Every blob request the store has seen.
pub(crate) fn requests(r: &Rig) -> u64 {
    S3Op::ALL.iter().map(|op| r.store.meter().count(*op)).sum()
}
