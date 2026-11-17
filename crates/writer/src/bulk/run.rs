//! A bulk request's slices planned and applied: each item checked, given an
//! id and a shard, the items grouped by shard in request order as
//! `BulkDispatch` groups them, each group applied where its shard lives,
//! and every item's outcome kept in request order.

use std::collections::HashMap;

use oss_cluster::routing::{DocumentRouting, RoutingError};
use oss_rest::{Exception, Meta, RestError};

use crate::index::{Refused, Written, validation_failed};

use super::action::Kind;
use super::ids::IdGenerator;
use super::reader::Op;
use super::response::{Item, ItemResult, mapping_failure, mapping_status, shard_failure};
use super::validate;

/// What the writer knows of an index a bulk names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexInfo {
    /// Its name.
    pub name: String,
    /// `index.uuid`.
    pub uuid: String,
    /// `index.number_of_shards`.
    pub shards: u32,
    /// `index.routing_partition_size`, 1 when unset.
    pub partition_size: u32,
    /// `index.number_of_routing_shards`, the shard count when unset.
    pub routing_shards: u32,
    /// Whether its mapping sets `_routing.required`.
    pub routing_required: bool,
}

/// Who applies a shard's group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    /// This node.
    Local,
    /// Another node, by id.
    Node(String),
}

/// One item bound for a shard, with its place in the request.
#[derive(Clone, Copy, Debug)]
pub struct Planned<'o> {
    /// Its position in the request.
    pub position: usize,
    /// The operation.
    pub op: &'o Op,
    /// Its id, generated when the action gave none.
    pub id: &'o str,
}

/// Where a bulk's items go: the indexes, their owners and their shards.
pub trait Target {
    /// The index called `name`, if there is one.
    fn index(&mut self, name: &str) -> Option<IndexInfo>;

    /// Who owns `shard` of `index`.
    fn owner(&mut self, index: &IndexInfo, shard: u32) -> Owner;

    /// Applies one item to a shard this node owns.
    ///
    /// # Errors
    ///
    /// The engine's refusal, which fails that item alone.
    fn apply(
        &mut self,
        index: &IndexInfo,
        shard: u32,
        item: &Planned<'_>,
    ) -> Result<Written, Refused>;

    /// Refreshes a shard this node owns.
    ///
    /// # Errors
    ///
    /// The engine's, which fails the request.
    fn refresh(&mut self, index: &IndexInfo, shard: u32) -> Result<(), String>;

    /// Sends a group to the node that owns its shard, once, and answers each
    /// item's outcome in the group's order.
    fn forward(
        &mut self,
        node: &str,
        index: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
    ) -> Vec<ItemResult>;
}

/// A bulk request in progress.
pub struct Bulk<'t, T: Target> {
    target: &'t mut T,
    ids: &'t mut IdGenerator,
    now_millis: u64,
    refresh: bool,
    items: Vec<Item>,
    indexes: HashMap<String, Option<IndexInfo>>,
    written: Vec<(String, u32)>,
}

impl<'t, T: Target> Bulk<'t, T> {
    /// A request over `target`, giving ids from `ids` at `now_millis`, and
    /// refreshing the shards it wrote when `refresh` (`?refresh=true`).
    pub fn new(
        target: &'t mut T,
        ids: &'t mut IdGenerator,
        now_millis: u64,
        refresh: bool,
    ) -> Self {
        Self {
            target,
            ids,
            now_millis,
            refresh,
            // ALLOC: one item an operation; a few dozen bytes each, bounded by the body.
            items: Vec::new(),
            // ALLOC: one entry an index the request names.
            indexes: HashMap::new(),
            // ALLOC: one entry a shard written.
            written: Vec::new(),
        }
    }

    /// Checks, plans and applies one slice of the body's operations.
    ///
    /// # Errors
    ///
    /// `action_request_validation_exception` for the whole request when
    /// the first slice holds an item that fails `BulkRequest.validate`, as
    /// the Java refuses the request; in a later slice, after others were
    /// applied, such an item fails alone.
    pub fn slice(&mut self, ops: &[Op]) -> Result<(), RestError> {
        let first = self.items.is_empty();
        if first {
            let errors = validate::errors(ops);
            if !errors.is_empty() {
                return Err(validation(&errors));
            }
        }
        let base = self.items.len();
        // ALLOC: an id for each operation of the slice.
        let mut ids: Vec<String> = Vec::with_capacity(ops.len());
        // Groups in the order their first item comes: (index, shard) and the
        // items' slice positions.
        // ALLOC: one group a shard the slice writes, one entry an operation.
        let mut groups: Vec<((String, u32), Vec<usize>)> = Vec::new();
        for op in ops {
            let (item, shard) = self.plan(op, first);
            if let Some(key) = shard {
                let at = ids.len();
                match groups.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, members)) => members.push(at),
                    // ALLOC: one group a shard the slice writes.
                    None => groups.push((key, vec![at])),
                }
            }
            ids.push(item.id.clone());
            self.items.push(item);
        }
        for ((name, shard), members) in groups {
            let Some(Some(info)) = self.indexes.get(&name).cloned() else {
                continue;
            };
            // ALLOC: one entry an item of the group.
            let group: Vec<Planned<'_>> = members
                .iter()
                .filter_map(|&at| {
                    Some(Planned {
                        position: base.saturating_add(at),
                        op: ops.get(at)?,
                        id: ids.get(at)?.as_str(),
                    })
                })
                // ALLOC: one entry an item of the group.
                .collect();
            let results = match self.target.owner(&info, shard) {
                Owner::Local => group
                    .iter()
                    .map(|p| written_or_failed(self.target.apply(&info, shard, p), &info, shard))
                    // ALLOC: one result an item of the group.
                    .collect(),
                Owner::Node(node) => self.target.forward(&node, &info, shard, &group),
            };
            let mut wrote = false;
            for (p, result) in group.iter().zip(results) {
                wrote |= matches!(result, ItemResult::Written { .. });
                if let Some(item) = self.items.get_mut(p.position) {
                    item.result = result;
                }
            }
            if wrote && !self.written.contains(&(name.clone(), shard)) {
                self.written.push((name, shard));
            }
        }
        Ok(())
    }

    /// Refreshes the shards written when the request asked, and answers its
    /// items in request order.
    ///
    /// # Errors
    ///
    /// The validation refusal for a body with no operation, or a refresh
    /// that failed.
    pub fn finish(mut self) -> Result<Vec<Item>, RestError> {
        if self.items.is_empty() {
            return Err(validation(&validate::errors(&[])));
        }
        if self.refresh {
            for (name, shard) in &self.written {
                if let Some(Some(info)) = self.indexes.get(name) {
                    self.target.refresh(info, *shard).map_err(|e| {
                        RestError::exception(
                            500,
                            Exception::new("refresh_failed_engine_exception", e),
                        )
                    })?;
                }
            }
            for item in &mut self.items {
                if let ItemResult::Written { forced_refresh, .. } = &mut item.result {
                    *forced_refresh = true;
                }
            }
        }
        Ok(self.items)
    }

    /// The item for `op` before it is applied, and the shard it goes to
    /// when it passed every check; a failing item carries its failure.
    fn plan(&mut self, op: &Op, first: bool) -> (Item, Option<(String, u32)>) {
        let a = &op.action;
        let action = if a.kind == Kind::Index && a.create {
            "create"
        } else {
            a.kind.name()
        };
        let index = a.index.clone().unwrap_or_default();
        let id = match (&a.id, a.kind) {
            (Some(id), _) => id.clone(),
            (None, Kind::Index | Kind::Create) => self.ids.next(self.now_millis),
            // ALLOC: none; an empty string does not allocate.
            (None, _) => String::new(),
        };
        let fail = |status: u16, error: Exception| ItemResult::Failed { status, error };
        let mut item = Item {
            action,
            index,
            id,
            result: fail(
                500,
                Exception::new("illegal_state_exception", "not applied"),
            ),
        };
        if !first {
            // ALLOC: the item's failed checks; an item that failed.
            let mut errors = Vec::new();
            validate::op_errors(op, &mut errors);
            if !errors.is_empty() {
                item.result = fail(
                    400,
                    Exception::new(
                        "action_request_validation_exception",
                        validation_failed(&errors),
                    ),
                );
                return (item, None);
            }
        }
        if a.kind == Kind::Update {
            item.result = fail(
                501,
                Exception::new(
                    "unsupported_operation_exception",
                    "[update] is not supported in a bulk here: it merges into the current document, which needs a realtime get",
                ),
            );
            return (item, None);
        }
        if a.id.as_deref() == Some("") && a.kind != Kind::Delete {
            item.result = fail(
                400,
                Exception::new(
                    "illegal_argument_exception",
                    "if _id is specified it must not be empty",
                ),
            );
            return (item, None);
        }
        let info = match self.indexes.get(&item.index) {
            Some(known) => known.clone(),
            None => {
                let found = self.target.index(&item.index);
                self.indexes.insert(item.index.clone(), found.clone());
                found
            }
        };
        let Some(info) = info else {
            let reason = if a.require_alias {
                // ALLOC: error path; one message.
                format!(
                    "no such index [{0}] and [require_alias] request flag is [true] and [{0}] is not an alias",
                    item.index
                )
            } else {
                // ALLOC: error path; one message.
                format!("no such index [{}]", item.index)
            };
            let mut e = Exception::new("index_not_found_exception", reason)
                .with_meta("index", Meta::Str(item.index.clone()));
            if !a.require_alias {
                e = e
                    .with_meta("resource.id", Meta::Str(item.index.clone()))
                    .with_meta("resource.type", Meta::Str("index_expression".into()));
            }
            item.result = fail(404, e.with_meta("index_uuid", Meta::Str("_na_".into())));
            return (item, None);
        };
        if a.require_alias {
            item.result = fail(
                404,
                Exception::new(
                    "index_not_found_exception",
                    // ALLOC: error path; one message.
                    format!(
                        "no such index [{0}] and [require_alias] request flag is [true] and [{0}] is not an alias",
                        item.index
                    ),
                )
                .with_meta("index", Meta::Str(item.index.clone()))
                .with_meta("index_uuid", Meta::Str("_na_".into())),
            );
            return (item, None);
        }
        if let Some(p) = a.pipeline.as_deref().filter(|p| *p != "_none") {
            item.result = fail(
                400,
                // ALLOC: error path; one message.
                Exception::new(
                    "illegal_argument_exception",
                    format!("pipeline with id [{p}] does not exist"),
                ),
            );
            return (item, None);
        }
        let routed = if a.routing.is_none() && info.routing_required {
            Err(RoutingError::RoutingMissing)
        } else {
            DocumentRouting::legacy_scaled(info.shards, info.partition_size, info.routing_shards)
                .and_then(|r| r.shard_for(&item.id, a.routing.as_deref()))
        };
        match routed {
            Ok(shard) => {
                let key = (info.name.clone(), shard);
                (item, Some(key))
            }
            Err(_) => {
                item.result = fail(
                    400,
                    Exception::new(
                        "routing_missing_exception",
                        // ALLOC: error path; one message.
                        format!("routing is required for [{}]/[{}]", item.index, item.id),
                    )
                    .with_meta("index", Meta::Str(item.index.clone()))
                    .with_meta("index_uuid", Meta::Str(info.uuid.clone())),
                );
                (item, None)
            }
        }
    }
}

/// The engine's answer for one item as its result.
fn written_or_failed(result: Result<Written, Refused>, info: &IndexInfo, shard: u32) -> ItemResult {
    match result {
        Ok(written) => ItemResult::Written {
            written,
            forced_refresh: false,
        },
        Err(Refused::Conflict(reason)) => ItemResult::Failed {
            status: 409,
            error: shard_failure(
                "version_conflict_engine_exception",
                reason,
                &info.name,
                shard,
                &info.uuid,
            ),
        },
        Err(Refused::Mapping(e)) => ItemResult::Failed {
            status: mapping_status(&e),
            error: mapping_failure(&e),
        },
    }
}

/// The request's validation refusal.
fn validation(errors: &[String]) -> RestError {
    RestError::exception(
        400,
        Exception::new(
            "action_request_validation_exception",
            validation_failed(errors),
        ),
    )
}
