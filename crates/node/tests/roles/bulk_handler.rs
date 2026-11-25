//! docs/specs/node-bulk-handler: the owner's answer to a forwarded bulk.

#![cfg(feature = "writer")]

use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_format::{NodeLogEpoch, ShardKey};
use oss_node::bulk_handler::BulkHandler;
use oss_node::writer::{Routes, WriterRole};
use oss_transport::actions::write::{BulkOperation, BulkRequest, BulkResponse};
use oss_transport::exception::RemoteException;
use oss_transport::frame::VariableHeader;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output};
use oss_transport::version::WireVersion;
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

fn info() -> IndexInfo {
    IndexInfo {
        name: "docs".into(),
        uuid: "docs-uuid".into(),
        shards: 2,
        partition_size: 1,
        routing_shards: 2,
        routing_required: false,
        write_block: Default::default(),
        default_pipeline: None,
        final_pipeline: None,
    }
}

struct Indexes;

impl Routes for Indexes {
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

fn handler() -> (BulkHandler, Arc<WriterRole>) {
    let store = Arc::new(Emulator::new(1 << 28, 9));
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
    let routes: Arc<dyn Routes> = Arc::new(Indexes);
    let role = Arc::new(WriterRole::new(committer, Arc::clone(&routes)));
    role.open_shard(
        &info(),
        0,
        3,
        Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping"),
    )
    .expect("opens");
    (BulkHandler::new(Arc::clone(&role), routes, "owner-1"), role)
}

fn op(id: &str, source: &str) -> BulkOperation<'static> {
    BulkOperation {
        id: id.into(),
        source: source.to_owned().into(),
        if_seq_no: -2,
        ..BulkOperation::default()
    }
}

fn request(shard: i32, operations: Vec<BulkOperation>) -> BulkRequest {
    BulkRequest {
        index: "docs".into(),
        shard,
        operations,
        index_uuid: Some("docs-uuid".into()),
        ..BulkRequest::default()
    }
}

fn call(handler: &BulkHandler, message: &[u8]) -> Result<BulkResponse, RemoteException> {
    let variable = VariableHeader::default();
    let mut answer = Vec::new();
    handler.handle(
        &Request {
            action: "internal:serverless/document/bulk",
            variable: &variable,
            message,
            version: WireVersion::SHELL,
        },
        &mut Output::new(&mut answer, 1 << 20),
    )?;
    Ok(BulkResponse::read(&mut Input::new(&answer)).expect("decodes"))
}

fn send(handler: &BulkHandler, request: &BulkRequest) -> Result<BulkResponse, RemoteException> {
    let mut message = Vec::new();
    request
        .write(&mut Output::new(&mut message, 1 << 20))
        .expect("encodes");
    call(handler, &message)
}

// AC-1
#[test]
fn a_group_is_applied_in_order_and_each_outcome_is_the_engines() {
    let (handler, _) = handler();
    let created = send(
        &handler,
        &request(0, vec![op("a", r#"{"n":1}"#), op("b", r#"{"n":2}"#)]),
    )
    .expect("answered");
    assert_eq!(created.owner_node_id, "owner-1");
    assert_eq!(created.outcomes.len(), 2);
    assert_eq!(created.outcomes[0].id, "a");
    assert!(created.outcomes[0].created && created.outcomes[0].failure.is_none());
    assert_eq!(created.outcomes[0].version, 1);
    assert_eq!(created.outcomes[0].primary_term, 3);
    assert!(created.outcomes[1].seq_no > created.outcomes[0].seq_no);

    let next = send(
        &handler,
        &request(
            0,
            vec![
                op("a", r#"{"n":3}"#),
                BulkOperation {
                    id: "b".into(),
                    deletion: true,
                    if_seq_no: -2,
                    ..BulkOperation::default()
                },
                BulkOperation {
                    id: "nope".into(),
                    deletion: true,
                    if_seq_no: -2,
                    ..BulkOperation::default()
                },
            ],
        ),
    )
    .expect("answered");
    let [updated, deleted, missing] = &next.outcomes[..] else {
        panic!("three outcomes");
    };
    assert!(!updated.created && !updated.deletion && updated.version == 2);
    assert!(deleted.deletion && deleted.found);
    assert!(missing.deletion && !missing.found);
}

// AC-2
#[test]
fn a_lost_condition_is_a_conflict_and_the_rest_of_the_group_goes_on() {
    let (handler, _) = handler();
    send(&handler, &request(0, vec![op("a", r#"{"n":1}"#)])).expect("answered");
    let created_again = BulkOperation {
        require_absent: true,
        ..op("a", r#"{"n":9}"#)
    };
    let stale = BulkOperation {
        if_seq_no: 99,
        if_primary_term: 3,
        ..op("a", r#"{"n":9}"#)
    };
    let answer = send(
        &handler,
        &request(0, vec![created_again, stale, op("c", r#"{"n":5}"#)]),
    )
    .expect("answered");
    assert!(answer.outcomes[0].conflict && answer.outcomes[0].failure.is_some());
    assert!(answer.outcomes[1].conflict && answer.outcomes[1].failure.is_some());
    assert!(answer.outcomes[2].created && answer.outcomes[2].failure.is_none());
    assert!(!answer.outcomes[2].retryable);
}

// AC-3
#[test]
fn what_cannot_be_applied_is_refused_and_the_shard_is_untouched() {
    let (handler, role) = handler();
    let refused = |request: &BulkRequest| send(&handler, request).expect_err("refused");
    let mut unknown = request(0, vec![op("a", "{}")]);
    unknown.index = "nowhere".into();
    refused(&unknown);
    let mut moved = request(0, vec![op("a", "{}")]);
    moved.index_uuid = Some("another".into());
    refused(&moved);
    refused(&request(2, vec![op("a", "{}")]));
    refused(&request(-1, vec![op("a", "{}")]));
    assert!(call(&handler, &[0xff, 0xff, 0xff]).is_err());
    role.refresh(&info(), 0).expect("refreshes");
    assert!(role.get(&info(), 0, "a", true).expect("reads").is_none());
}

// AC-4
#[test]
fn a_refresh_is_answered_and_makes_the_documents_visible() {
    let (handler, role) = handler();
    let mut with = request(0, vec![op("a", r#"{"n":1}"#)]);
    with.refresh = true;
    assert!(send(&handler, &with).expect("answered").refreshed);
    assert!(role.get(&info(), 0, "a", false).expect("reads").is_some());
    let without = send(&handler, &request(0, vec![op("b", r#"{"n":1}"#)])).expect("answered");
    assert!(!without.refreshed);
    assert!(role.get(&info(), 0, "b", false).expect("reads").is_none());
}

// AC-5
#[test]
fn a_shard_that_cannot_be_opened_fails_its_items_not_the_request() {
    let (handler, _) = handler();
    let answer = send(&handler, &request(1, vec![op("a", "{}"), op("b", "{}")])).expect("answered");
    assert_eq!(answer.outcomes.len(), 2);
    assert!(answer.outcomes.iter().all(|o| o.failure.is_some()));
}
