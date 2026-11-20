//! The cluster half of the writer's routes: who owns a shard this node does
//! not hold, and how a group reaches that owner.
//!
//! The owner is rendezvous placement over the leases the node already holds,
//! so a lookup reads no blob; every node over the same leases names the same
//! owner, and the shard head's conditional write is what makes one node the
//! owner in fact.

use std::sync::Arc;

use oss_cluster::placement::{ROLE_INGEST, ShardKey, has_role, owner};
use oss_format::NodeLease;
use oss_rest::{Exception, Meta};
use oss_transport::actions::read::GetRequest;
use oss_transport::actions::write::{BulkOperation, BulkOutcome, BulkRequest, BulkResponse};
use oss_writer::bulk::{IndexInfo, ItemResult, Kind, Owner, Planned};
use oss_writer::index::{Fetched, Outcome, Written};

use crate::forward::{BulkForwarder, ForwardError};
use crate::push_link::Leases;
use crate::read_forward::GetSend;
use crate::writer::Routes;

/// Which indexes exist.
pub trait IndexSource: Send + Sync {
    /// The index called `name`, if there is one.
    fn index(&self, name: &str) -> Option<IndexInfo>;
}

/// The call a forward makes to an owner.
pub trait BulkSend: Send + Sync {
    /// Sends `request` to node `node_id` and reads its answer.
    ///
    /// # Errors
    ///
    /// [`ForwardError`]: nothing sent, the owner gone, or the call failed.
    fn bulk(&self, node_id: &str, request: &BulkRequest) -> Result<BulkResponse, ForwardError>;
}

impl<L: Leases> BulkSend for BulkForwarder<L> {
    fn bulk(&self, node_id: &str, request: &BulkRequest) -> Result<BulkResponse, ForwardError> {
        BulkForwarder::bulk(self, node_id, request)
    }
}

/// [`Routes`] over the cluster's leases and the transport.
pub struct ClusterRoutes<S, L, B> {
    indexes: S,
    leases: L,
    send: B,
    self_id: String,
    now: Box<dyn Fn() -> i64 + Send + Sync>,
    reads: Option<Arc<dyn GetSend>>,
}

impl<S: IndexSource, L: Leases, B: BulkSend> ClusterRoutes<S, L, B> {
    /// Routes for node `self_id`: indexes from `indexes`, members from
    /// `leases` judged live at `now` (epoch milliseconds), groups sent with
    /// `send`.
    pub fn new(
        indexes: S,
        leases: L,
        send: B,
        self_id: &str,
        now: Box<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            indexes,
            leases,
            send,
            // ALLOC: one id, once a node.
            self_id: self_id.to_owned(),
            now,
            reads: None,
        }
    }

    /// The same, forwarding a get of a shard another node owns with `reads`.
    #[must_use]
    pub fn with_reads(mut self, reads: Arc<dyn GetSend>) -> Self {
        self.reads = Some(reads);
        self
    }
}

fn live_writer(lease: &NodeLease, now: i64) -> bool {
    !lease.revoked && !lease.is_expired_at(now) && has_role(lease, ROLE_INGEST)
}

fn failed_all(
    group: &[Planned<'_>],
    status: u16,
    kind: &'static str,
    reason: &str,
) -> Vec<ItemResult> {
    group
        .iter()
        .map(|_| ItemResult::Failed {
            status,
            error: Exception::new(kind, reason),
        })
        // ALLOC: one result an item of the group.
        .collect()
}

/// The wire form of one item, or why it cannot be sent.
fn operation(item: &Planned<'_>) -> Result<BulkOperation, &'static str> {
    let a = &item.op.action;
    let deletion = match a.kind {
        Kind::Update => return Err("an update cannot be forwarded"),
        Kind::Delete => true,
        Kind::Index | Kind::Create => false,
    };
    let source = if deletion {
        String::new()
    } else {
        std::str::from_utf8(&item.op.source)
            .map_err(|_| "the document source is not valid UTF-8")?
            // ALLOC: one copy an item, bounded by the bulk budget.
            .to_owned()
    };
    Ok(BulkOperation {
        // ALLOC: one id an item.
        id: item.id.to_owned(),
        source,
        deletion,
        if_seq_no: a.expect.if_seq_no,
        if_primary_term: a.expect.if_primary_term,
        require_absent: a.kind == Kind::Create || a.create,
        routing: a.routing.clone(),
    })
}

fn result_of(outcome: &BulkOutcome, index: &IndexInfo, shard: u32) -> ItemResult {
    if let Some(reason) = &outcome.failure {
        let (status, kind) = if outcome.conflict {
            (409, "version_conflict_engine_exception")
        } else if outcome.retryable {
            (429, "es_rejected_execution_exception")
        } else {
            (500, "forwarded_write_exception")
        };
        return ItemResult::Failed {
            status,
            error: Exception::new(kind, reason.clone())
                // ALLOC: three short fields an item that failed.
                .with_meta("index", Meta::Str(index.name.clone()))
                .with_meta("shard", Meta::Str(shard.to_string()))
                .with_meta("index_uuid", Meta::Str(index.uuid.clone())),
        };
    }
    let kind = match (outcome.deletion, outcome.found, outcome.created) {
        (true, true, _) => Outcome::Deleted,
        (true, false, _) => Outcome::NotFound,
        (false, _, true) => Outcome::Created,
        (false, _, false) => Outcome::Updated,
    };
    ItemResult::Written {
        written: Written {
            outcome: kind,
            version: outcome.version,
            seq_no: outcome.seq_no,
            primary_term: outcome.primary_term,
        },
        forced_refresh: false,
    }
}

impl<S: IndexSource, L: Leases, B: BulkSend> Routes for ClusterRoutes<S, L, B> {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        self.indexes.index(name)
    }

    fn owner(&self, index: &IndexInfo, shard: u32) -> Owner {
        let leases = self.leases.current();
        let now = (self.now)();
        let key = ShardKey::new(&index.name, i32::try_from(shard).unwrap_or(i32::MAX));
        let ids = leases
            .iter()
            .filter(|l| live_writer(l, now))
            .map(|l| l.node_id.as_str());
        match owner(key, ids) {
            Some(id) if id != self.self_id => Owner::Node(id.to_owned()),
            _ => Owner::Local,
        }
    }

    fn refresh_remote(&self, node: &str, index: &IndexInfo, shard: u32) -> Result<(), String> {
        // No operations: the owner refreshes the shard and answers.
        let request = BulkRequest {
            index: index.name.clone(),
            shard: i32::try_from(shard).unwrap_or(i32::MAX),
            index_uuid: Some(index.uuid.clone()),
            refresh: true,
            ..BulkRequest::default()
        };
        match self.send.bulk(node, &request) {
            Ok(response) if response.refreshed => Ok(()),
            Ok(_) => Err("the owner did not refresh the shard".to_owned()),
            Err(e) => Err(format!("the owner could not be asked to refresh: {e:?}")),
        }
    }

    fn get_remote(
        &self,
        node: &str,
        index: &IndexInfo,
        shard: u32,
        id: &str,
    ) -> Result<Option<Fetched>, String> {
        let reads = self
            .reads
            .as_ref()
            .ok_or_else(|| "this node does not forward reads".to_owned())?;
        let request = GetRequest {
            // ALLOC: one request's strings.
            index: index.name.clone(),
            index_uuid: Some(index.uuid.clone()),
            shard: i32::try_from(shard).unwrap_or(i32::MAX),
            id: id.to_owned(),
            ..GetRequest::default()
        };
        // ALLOC: error path; one message.
        let answer = reads
            .get(node, &request)
            .map_err(|e| format!("the owner could not be asked for the document: {e}"))?;
        if !answer.found {
            return Ok(None);
        }
        Ok(Some(Fetched {
            version: answer.version,
            seq_no: answer.seq_no,
            primary_term: answer.primary_term,
            source: answer.source.unwrap_or_default().into_bytes(),
        }))
    }

    fn forward(
        &self,
        node: &str,
        index: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
    ) -> Vec<ItemResult> {
        // ALLOC: one slot an item; `None` for an item not sent.
        let mut refused: Vec<Option<&'static str>> = Vec::with_capacity(group.len());
        // ALLOC: one operation an item sent.
        let mut operations = Vec::with_capacity(group.len());
        for item in group {
            match operation(item) {
                Ok(op) => {
                    operations.push(op);
                    refused.push(None);
                }
                Err(why) => refused.push(Some(why)),
            }
        }
        let sent = operations.len();
        let mut outcomes = Vec::new();
        if !operations.is_empty() {
            let request = BulkRequest {
                index: index.name.clone(),
                shard: i32::try_from(shard).unwrap_or(i32::MAX),
                operations,
                index_uuid: Some(index.uuid.clone()),
                ..BulkRequest::default()
            };
            match self.send.bulk(node, &request) {
                Ok(response) if response.outcomes.len() > sent => {
                    return failed_all(
                        group,
                        502,
                        "forwarded_write_exception",
                        "the owner answered more outcomes than items",
                    );
                }
                Ok(response) => outcomes = response.outcomes,
                Err(ForwardError::TooLarge) => {
                    return failed_all(
                        group,
                        413,
                        "content_too_long_exception",
                        "the bulk is too large to forward",
                    );
                }
                Err(e) => {
                    return failed_all(group, 503, "unavailable_shards_exception", &e.to_string());
                }
            }
        }
        let mut answered = outcomes.iter();
        refused
            .into_iter()
            .map(|why| match why {
                Some(reason) => ItemResult::Failed {
                    status: 400,
                    error: Exception::new("illegal_argument_exception", reason),
                },
                None => answered.next().map_or_else(
                    || ItemResult::Failed {
                        status: 502,
                        error: Exception::new(
                            "forwarded_write_exception",
                            "the owner answered fewer outcomes than items",
                        ),
                    },
                    |o| result_of(o, index, shard),
                ),
            })
            // ALLOC: one result an item of the group.
            .collect()
    }
}
