//! docs/specs/writer-newest: the writer answers a reader's ask for the
//! commit it last published.

#![cfg(feature = "writer")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_format::{NodeLogEpoch, ShardKey};
use oss_node::newest::NewestHandler;
use oss_node::writer::{MAX_NEWEST_BYTES, RoleTarget, Routes, WriterRole};
use oss_transport::actions::push::{NEWEST_ACTION, NewestRequest, NewestResponse};
use oss_transport::frame::VariableHeader;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output};
use oss_transport::version::WireVersion;
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, Owner, Planned, StreamConfig,
    stream,
};
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

#[derive(Default)]
struct Admit(AtomicBool);

impl ShardAdmission for Admit {
    fn admit(&self, _: &ShardKey, _: i64, _: u64) -> Result<(), Refusal> {
        if self.0.load(Ordering::SeqCst) {
            return Err(Arc::new(std::io::Error::other("not the owner")));
        }
        Ok(())
    }
}

struct Local;

impl Routes for Local {
    fn index(&self, _: &str) -> Option<IndexInfo> {
        Some(info())
    }

    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn forward(&self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
        Vec::new()
    }
}

fn info() -> IndexInfo {
    IndexInfo {
        name: "logs".into(),
        uuid: "logs-uuid".into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
        default_pipeline: None,
        final_pipeline: None,
    }
}

fn rig() -> (Arc<Emulator>, Arc<WriterRole>, Arc<Admit>) {
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let admit = Arc::new(Admit::default());
    let log =
        NodeLogStore::new(Arc::clone(&store) as Arc<dyn BlobStore>, "", "node-1").expect("names");
    let committer = Arc::new(NodeLogCommitter::new(
        log,
        7,
        Box::new(Lease),
        Box::new(Arc::clone(&admit)),
        CommitterConfig {
            window: Duration::from_millis(0),
            ..CommitterConfig::default()
        },
    ));
    let role = Arc::new(WriterRole::new(committer, Arc::new(Local)));
    open(&role, 0);
    (store, role, admit)
}

fn open(role: &WriterRole, shard: u32) {
    role.open_shard(
        &info(),
        shard,
        3,
        Mapping::parse(br#"{"properties":{}}"#).expect("mapping"),
    )
    .expect("opens");
}

fn request_of(message: &[u8]) -> Request<'_> {
    Request {
        action: NEWEST_ACTION,
        variable: &VARIABLE,
        message,
        version: WireVersion::SHELL,
    }
}

static VARIABLE: std::sync::LazyLock<VariableHeader> =
    std::sync::LazyLock::new(VariableHeader::default);

fn ask(handler: &NewestHandler, uuid: &str, shard: i32) -> Option<Vec<u8>> {
    let request = NewestRequest {
        index_name: "logs".into(),
        index_uuid: uuid.into(),
        shard,
        ..NewestRequest::default()
    };
    let mut message = Vec::new();
    request
        .write(&mut Output::new(&mut message, 1 << 20))
        .expect("encodes");
    let mut answer = Vec::new();
    handler
        .handle(
            &request_of(&message),
            &mut Output::new(&mut answer, 1 << 24),
        )
        .expect("answers");
    NewestResponse::read(&mut Input::new(&answer))
        .expect("decodes")
        .commit
}

fn release_by_a_failed_stage(role: &WriterRole) {
    let defaults = Defaults {
        index: Some("logs".into()),
        ..Defaults::default()
    };
    let budget = BulkBudget::new(1 << 20, 2);
    let config = StreamConfig {
        slice_bytes: 1 << 20,
        max_wait: Duration::from_secs(5),
    };
    let mut target = RoleTarget::new(role);
    let mut ids = IdGenerator::new(0, [0; 6]);
    let mut buffer = [0u8; 4096];
    let mut run = Bulk::new(&mut target, &mut ids, 1, false);
    let mut body: &[u8] = b"{\"index\":{}}\n{}\n";
    stream(&mut body, &mut buffer, &defaults, &budget, config, &mut run).expect("streams");
    run.finish().expect("finishes");
}

#[test]
fn the_handler_answers_the_staged_manifest() {
    let (_, role, _) = rig();
    assert!(role.staged("logs-uuid", 0, b"manifest-1"));
    let handler = NewestHandler::new(Arc::clone(&role));
    assert_eq!(ask(&handler, "logs-uuid", 0), Some(b"manifest-1".to_vec()));
}

#[test]
fn a_shard_never_published_not_held_or_released_answers_none() {
    let (_, role, admit) = rig();
    let handler = NewestHandler::new(Arc::clone(&role));
    assert_eq!(ask(&handler, "logs-uuid", 0), None);
    assert!(role.staged("logs-uuid", 0, b"m"));
    assert_eq!(ask(&handler, "other-uuid", 0), None);
    assert_eq!(ask(&handler, "logs-uuid", 1), None);
    assert_eq!(ask(&handler, "logs-uuid", -1), None);
    // A write whose stage fails releases the shard, and its answer with it.
    admit.0.store(true, Ordering::SeqCst);
    release_by_a_failed_stage(&role);
    assert!(!role.held("logs-uuid", 0));
    assert_eq!(ask(&handler, "logs-uuid", 0), None);
}

#[test]
fn a_later_stage_replaces_and_reopening_drops() {
    let (_, role, _) = rig();
    let handler = NewestHandler::new(Arc::clone(&role));
    assert!(role.staged("logs-uuid", 0, b"one"));
    assert!(role.staged("logs-uuid", 0, b"two"));
    assert_eq!(ask(&handler, "logs-uuid", 0), Some(b"two".to_vec()));
    open(&role, 0);
    assert_eq!(ask(&handler, "logs-uuid", 0), None);
}

#[test]
fn an_oversized_manifest_and_an_unheld_shard_are_refused() {
    let (_, role, _) = rig();
    let handler = NewestHandler::new(Arc::clone(&role));
    assert!(role.staged("logs-uuid", 0, b"kept"));
    assert!(!role.staged("logs-uuid", 0, &vec![0u8; MAX_NEWEST_BYTES + 1]));
    assert!(role.staged("logs-uuid", 0, &vec![1u8; MAX_NEWEST_BYTES]));
    assert!(!role.staged("logs-uuid", 1, b"unheld"));
    assert!(!role.staged("nobody", 0, b"unheld"));
    assert_eq!(
        ask(&handler, "logs-uuid", 0).map(|c| c.len()),
        Some(MAX_NEWEST_BYTES)
    );
}

#[test]
fn a_malformed_ask_is_refused_and_no_request_is_made() {
    let (store, role, _) = rig();
    let handler = NewestHandler::new(Arc::clone(&role));
    let before = store.meter().total();
    let mut out = Vec::new();
    let error = handler
        .handle(&request_of(b"\x01"), &mut Output::new(&mut out, 1 << 10))
        .unwrap_err();
    assert_eq!(error.http_status(), 400);
    let _ = ask(&handler, "logs-uuid", 0);
    assert_eq!(store.meter().total(), before);
}

#[test]
fn a_manifest_staged_while_the_shard_is_released_is_not_kept() {
    let (_, role, admit) = rig();
    let handler = NewestHandler::new(Arc::clone(&role));
    let stop = Arc::new(AtomicBool::new(false));
    let stager = {
        let (role, stop) = (Arc::clone(&role), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let _ = role.staged("logs-uuid", 0, b"m");
            }
        })
    };
    while !role.staged("logs-uuid", 0, b"m") {}
    admit.0.store(true, Ordering::SeqCst);
    release_by_a_failed_stage(&role);
    assert!(!role.held("logs-uuid", 0));
    stop.store(true, Ordering::SeqCst);
    stager.join().expect("joins");
    assert_eq!(ask(&handler, "logs-uuid", 0), None);
    assert_eq!(role.newest_commit("logs-uuid", 0), None);
}
