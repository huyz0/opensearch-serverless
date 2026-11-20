//! What an owner runs for a bulk another node forwarded to it
//! (`internal:serverless/document/bulk`).
//!
//! The answering side of the shell's forwarded bulk: the group is applied to
//! the shard this node writes exactly as a local group is, and each item's
//! outcome goes back in the group's order.

use std::sync::Arc;

use oss_transport::actions::write::{BulkOperation, BulkOutcome, BulkRequest, BulkResponse};
use oss_transport::exception::RemoteException;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output};
use oss_writer::bulk::{Action, ItemResult, Kind, Op, Planned};
use oss_writer::index::{Expect, Outcome, VersionType};

use crate::writer::{Routes, WriterRole};

/// Answers `internal:serverless/document/bulk` on an owner.
pub struct BulkHandler {
    role: Arc<WriterRole>,
    routes: Arc<dyn Routes>,
    node_id: String,
}

impl BulkHandler {
    /// A handler applying groups to `role`, finding indexes through
    /// `routes`, naming `node_id` in its answers.
    pub fn new(role: Arc<WriterRole>, routes: Arc<dyn Routes>, node_id: &str) -> Self {
        Self {
            role,
            routes,
            // ALLOC: one id, once a node.
            node_id: node_id.to_owned(),
        }
    }
}

fn refuse(why: &str, status: &str) -> RemoteException {
    RemoteException::status(why, status)
}

/// The engine's input for one wire operation.
fn op(index: &str, wire: &BulkOperation) -> Op {
    let kind = if wire.deletion {
        Kind::Delete
    } else {
        Kind::Index
    };
    Op {
        action: Action {
            kind,
            // ALLOC: one name an operation.
            index: Some(index.to_owned()),
            // ALLOC: one id an operation.
            id: Some(wire.id.clone()),
            routing: wire.routing.clone(),
            create: wire.require_absent && !wire.deletion,
            expect: Expect {
                if_seq_no: wire.if_seq_no,
                if_primary_term: wire.if_primary_term,
                version_type: VersionType::Internal,
                ..Expect::default()
            },
            pipeline: None,
            require_alias: false,
        },
        // ALLOC: one copy of the source an operation.
        source: wire.source.clone().into_bytes(),
    }
}

/// An item's result as the wire carries it.
fn outcome(id: &str, result: ItemResult) -> BulkOutcome {
    match result {
        ItemResult::Written { written, .. } => BulkOutcome {
            id: id.to_owned(),
            failure: None,
            conflict: false,
            retryable: false,
            deletion: matches!(written.outcome, Outcome::Deleted | Outcome::NotFound),
            found: written.outcome == Outcome::Deleted,
            seq_no: written.seq_no,
            primary_term: written.primary_term,
            version: written.version,
            created: written.outcome == Outcome::Created,
        },
        ItemResult::Failed { status, error } => BulkOutcome {
            id: id.to_owned(),
            failure: Some(error.reason().to_owned()),
            conflict: status == 409,
            retryable: status == 429,
            ..BulkOutcome::default()
        },
    }
}

impl Handler for BulkHandler {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        let bulk = BulkRequest::read(&mut Input::new(request.message))
            .map_err(|_| refuse("malformed forwarded bulk", "BAD_REQUEST"))?;
        let index = self
            .routes
            .index(&bulk.index)
            .ok_or_else(|| refuse("no such index", "NOT_FOUND"))?;
        if bulk.index_uuid.as_ref().is_some_and(|u| *u != index.uuid) {
            return Err(refuse(
                "the index is not the one the sender knew",
                "CONFLICT",
            ));
        }
        let shard = u32::try_from(bulk.shard)
            .ok()
            .filter(|s| *s < index.shards)
            .ok_or_else(|| refuse("shard out of range", "BAD_REQUEST"))?;
        // ALLOC: one input an operation, bounded by the frame the request came in.
        let ops: Vec<Op> = bulk.operations.iter().map(|w| op(&bulk.index, w)).collect();
        let group: Vec<Planned<'_>> = ops
            .iter()
            .zip(&bulk.operations)
            .enumerate()
            .map(|(position, (op, wire))| Planned {
                position,
                op,
                id: wire.id.as_str(),
            })
            // ALLOC: one entry an operation.
            .collect();
        let results = self.role.apply_group(&index, shard, &group);
        let refreshed = bulk.refresh && self.role.refresh(&index, shard).is_ok();
        let response = BulkResponse {
            owner_node_id: self.node_id.clone(),
            outcomes: bulk
                .operations
                .iter()
                .zip(results)
                .map(|(wire, result)| outcome(&wire.id, result))
                // ALLOC: one outcome an operation.
                .collect(),
            refreshed,
        };
        response.write(out).map_err(|_| {
            refuse(
                "the answer is larger than a transport frame",
                "INTERNAL_SERVER_ERROR",
            )
        })
    }
}
