//! The change a delete-by-query makes: one delete a hit, conditional on the
//! sequence number and primary term the search saw.

use oss_coordinator::docs::by_query::{Applied, Change, Hit};
use oss_coordinator::docs::update::ScriptOp;
use oss_coordinator::docs::update_script::run_update;
use oss_rest::{Exception, RestError};
use oss_writer::bulk::{Action, Bulk, IdGenerator, IndexInfo, ItemResult, Kind, Op};
use oss_writer::index::{Expect, MATCH_ANY, VersionType};

use crate::update_body::ScriptDef;
use crate::writer::{RoleTarget, WriterRole};

/// Deletes hits of one index through the `_bulk` machinery, so planning,
/// the version rules and the write log are the bulk path's.
pub struct DeleteChange<'a> {
    role: &'a WriterRole,
    index: &'a IndexInfo,
    ids: &'a mut IdGenerator,
    now_millis: u64,
}

impl<'a> DeleteChange<'a> {
    /// Deletes from `index` on `role`, stamped `now_millis`.
    pub fn new(
        role: &'a WriterRole,
        index: &'a IndexInfo,
        ids: &'a mut IdGenerator,
        now_millis: u64,
    ) -> Self {
        Self {
            role,
            index,
            ids,
            now_millis,
        }
    }
}

impl Change for DeleteChange<'_> {
    type Error = RestError;

    fn apply(&mut self, hit: &Hit, refresh: bool) -> Result<Applied, RestError> {
        let action = Action {
            kind: Kind::Delete,
            // ALLOC: one name and id a hit, bounded by the page.
            index: Some(self.index.name.to_string()),
            id: Some(hit.id.clone()),
            routing: None,
            create: false,
            expect: Expect {
                version: MATCH_ANY,
                version_type: VersionType::Internal,
                if_seq_no: hit.seq_no,
                if_primary_term: hit.primary_term,
            },
            pipeline: None,
            require_alias: false,
        };
        let mut target = RoleTarget::new(self.role);
        let mut run = Bulk::new(&mut target, self.ids, self.now_millis, refresh);
        run.slice(&[Op {
            action,
            source: Vec::new(),
        }])?;
        let items = run.finish()?;
        match items.first().map(|item| &item.result) {
            Some(ItemResult::Written { .. } | ItemResult::Noop { .. }) => Ok(Applied::Changed),
            Some(ItemResult::Failed {
                status: 409 | 404,
                error,
            }) => Ok(Applied::Conflict(error.reason().to_owned())),
            Some(ItemResult::Failed { status, error }) => Err(RestError::simple(
                *status,
                // ALLOC: error path; one message.
                format!("[{}] {}", error.kind(), error.reason()),
            )),
            None => Err(RestError::simple(500, "the delete produced no result")),
        }
    }
}

/// Updates hits of one index with a Painless script: the document is read,
/// the script runs over it, and the result is written through the `_bulk`
/// machinery conditional on the sequence number and primary term the search
/// saw, so a document rewritten since is a conflict.
pub(crate) struct UpdateChange<'a> {
    role: &'a WriterRole,
    index: &'a IndexInfo,
    script: &'a ScriptDef,
    ids: &'a mut IdGenerator,
    now_millis: u64,
    /// Documents rewritten.
    pub(crate) updated: u64,
    /// Documents the script deleted.
    pub(crate) deleted: u64,
}

impl<'a> UpdateChange<'a> {
    /// Updates `index` on `role` with `script`, stamped `now_millis`.
    pub(crate) fn new(
        role: &'a WriterRole,
        index: &'a IndexInfo,
        script: &'a ScriptDef,
        ids: &'a mut IdGenerator,
        now_millis: u64,
    ) -> Self {
        Self {
            role,
            index,
            script,
            ids,
            now_millis,
            updated: 0,
            deleted: 0,
        }
    }
}

impl Change for UpdateChange<'_> {
    type Error = RestError;

    fn apply(&mut self, hit: &Hit, refresh: bool) -> Result<Applied, RestError> {
        let name = self.index.name.to_string(); // ALLOC: one name a hit, bounded by the page.
        let shard = crate::get_route::shard_of(self.index, &hit.id, None)?;
        let found = self
            .role
            .get_owned(self.index, shard, &hit.id, true)
            .map_err(|reason| {
                RestError::exception(
                    503,
                    Exception::new("no_shard_available_action_exception", reason),
                )
            })?;
        let Some(found) = found else {
            return Ok(Applied::Conflict(format!(
                "[{}]: version conflict, document missing",
                hit.id
            )));
        };
        let out = run_update(
            &self.script.source,
            &self.script.params,
            true,
            &name,
            &hit.id,
            found.version,
            &found.source,
        )
        .map_err(|reason| {
            RestError::exception(
                400,
                // ALLOC: error path; one message.
                Exception::new("script_exception", reason),
            )
        })?;
        let (kind, source) = match out.op {
            ScriptOp::Index => (Kind::Index, out.source),
            ScriptOp::Delete => (Kind::Delete, Vec::new()),
            ScriptOp::Create | ScriptOp::Noop => return Ok(Applied::Noop),
        };
        let action = Action {
            kind,
            index: Some(name),
            id: Some(hit.id.clone()), // ALLOC: one id a hit, bounded by the page.
            routing: None,
            create: false,
            expect: Expect {
                version: MATCH_ANY,
                version_type: VersionType::Internal,
                if_seq_no: hit.seq_no,
                if_primary_term: hit.primary_term,
            },
            pipeline: None,
            require_alias: false,
        };
        let mut target = RoleTarget::new(self.role);
        let mut run = Bulk::new(&mut target, self.ids, self.now_millis, refresh);
        run.slice(&[Op { action, source }])?;
        let items = run.finish()?;
        match items.first().map(|item| &item.result) {
            Some(ItemResult::Written { .. }) => {
                if kind == Kind::Delete {
                    self.deleted = self.deleted.saturating_add(1);
                } else {
                    self.updated = self.updated.saturating_add(1);
                }
                Ok(Applied::Changed)
            }
            Some(ItemResult::Failed {
                status: 409 | 404,
                error,
            }) => Ok(Applied::Conflict(error.reason().to_owned())), // ALLOC: error path; one message.
            Some(ItemResult::Failed { status, error }) => Err(RestError::simple(
                *status,
                // ALLOC: error path; one message.
                format!("[{}] {}", error.kind(), error.reason()),
            )),
            // Only a bulk `update` item is a no-op; this writes an index or a delete.
            Some(ItemResult::Noop { .. }) | None => {
                Err(RestError::simple(500, "the update produced no result"))
            }
        }
    }
}
