//! docs/specs/node-forward-memory: a forwarded group is held once. Its own
//! binary, because the counting allocator is process-wide.

#![cfg(feature = "writer")]
// ARITH: test sizes are small constants; an overflow panics the test,
// which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::sync::Arc;

use oss_format::NodeLease;
use oss_node::cluster_owner::{BulkSend, ClusterRoutes, IndexSource};
use oss_node::forward::{ForwardError, MAX_FORWARD_BYTES};
use oss_node::push_link::Leases;
use oss_node::writer::Routes;
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};
use oss_transport::actions::write::{BulkRequest, BulkResponse};
use oss_transport::stream::Output;
use oss_writer::bulk::{Action, IndexInfo, ItemResult, Kind, Op, Planned};
use oss_writer::index::Expect;

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const ITEMS: usize = 4;
const SOURCE: usize = 4 << 20;

struct Fixed(Arc<[NodeLease]>);

impl Leases for Fixed {
    fn current(&self) -> Arc<[NodeLease]> {
        Arc::clone(&self.0)
    }
}

struct Indexes;

impl IndexSource for Indexes {
    fn index(&self, _: &str) -> Option<IndexInfo> {
        Some(info())
    }
}

fn info() -> IndexInfo {
    IndexInfo {
        name: "logs".into(),
        uuid: "u1".into(),
        shards: 4,
        partition_size: 1,
        routing_shards: 4,
        routing_required: false,
    }
}

/// Encodes the request as the forwarder does, into one buffer sized by the
/// request's own hint, and answers an outcome for every item.
struct Encoding;

impl BulkSend for Encoding {
    fn bulk(&self, node: &str, request: &BulkRequest<'_>) -> Result<BulkResponse, ForwardError> {
        let mut message = Vec::with_capacity(request.encoded_size_hint().min(MAX_FORWARD_BYTES));
        request
            .write(&mut Output::new(&mut message, MAX_FORWARD_BYTES))
            .map_err(|_| ForwardError::TooLarge)?;
        assert!(message.len() > ITEMS * SOURCE);
        Ok(BulkResponse {
            owner_node_id: node.to_owned(),
            outcomes: request
                .operations
                .iter()
                .map(|op| oss_transport::actions::write::BulkOutcome {
                    id: op.id.clone(),
                    ..Default::default()
                })
                .collect(),
            refreshed: false,
        })
    }
}

#[test]
fn forwarding_a_large_group_holds_its_sources_about_once() {
    let routes = ClusterRoutes::new(Indexes, Fixed(Vec::new().into()), Encoding, "me");
    let ops: Vec<Op> = (0..ITEMS)
        .map(|i| Op {
            action: Action {
                kind: Kind::Index,
                index: Some("logs".into()),
                id: Some(format!("id-{i}")),
                routing: None,
                create: false,
                expect: Expect::default(),
                pipeline: None,
                require_alias: false,
            },
            source: vec![b'a'; SOURCE],
        })
        .collect();
    let ids: Vec<String> = (0..ITEMS).map(|i| format!("id-{i}")).collect();
    let group: Vec<Planned<'_>> = ops
        .iter()
        .zip(&ids)
        .enumerate()
        .map(|(position, (op, id))| Planned { position, op, id })
        .collect();
    let window = PeakWindow::start(&ALLOC);
    let results = routes.forward("n2", &info(), 0, &group);
    let peak = window.peak_bytes();
    assert!(
        results
            .iter()
            .all(|r| matches!(r, ItemResult::Written { .. }))
    );
    let group_bytes = ITEMS * SOURCE;
    // The message is the one new copy; before, the owned sources and the
    // growing buffer made it about three groups.
    assert!(
        peak <= group_bytes + group_bytes / 4,
        "peak {peak} over the {group_bytes} bytes of sources"
    );
}
