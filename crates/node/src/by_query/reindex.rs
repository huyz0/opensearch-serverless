//! The change a reindex makes: each hit is read from the source index, run
//! through the script if there is one, and written to the destination.

use oss_coordinator::docs::by_query::{Applied, Change, Hit};
use oss_coordinator::docs::update::ScriptOp;
use oss_coordinator::docs::update_script::run_update;
use oss_rest::{Exception, RestError};
use oss_writer::bulk::{Action, Bulk, IdGenerator, IndexInfo, Kind, Op, doc_status};
use oss_writer::index::{Expect, MATCH_ANY, VersionType};

use crate::update_body::ScriptDef;
use crate::writer::{RoleTarget, WriterRole};

/// Copies hits of `source` into `dest` through the `_bulk` machinery.
pub(crate) struct ReindexChange<'a> {
    role: &'a WriterRole,
    source: &'a IndexInfo,
    dest: &'a IndexInfo,
    create: bool,
    script: Option<&'a ScriptDef>,
    ids: &'a mut IdGenerator,
    now_millis: u64,
    /// Documents written to the destination that were not there.
    pub(crate) created: u64,
    /// Destination documents overwritten.
    pub(crate) updated: u64,
    /// Destination documents the script deleted.
    pub(crate) deleted: u64,
}

impl<'a> ReindexChange<'a> {
    /// Copies `source` into `dest` on `role`, through `script` when given,
    /// stamped `now_millis`; `create` makes an existing document a conflict.
    pub(crate) fn new(
        role: &'a WriterRole,
        (source, dest): (&'a IndexInfo, &'a IndexInfo),
        create: bool,
        script: Option<&'a ScriptDef>,
        ids: &'a mut IdGenerator,
        now_millis: u64,
    ) -> Self {
        Self {
            role,
            source,
            dest,
            create,
            script,
            ids,
            now_millis,
            created: 0,
            updated: 0,
            deleted: 0,
        }
    }
}

impl Change for ReindexChange<'_> {
    type Error = RestError;

    fn apply(&mut self, hit: &Hit, refresh: bool) -> Result<Applied, RestError> {
        let shard = crate::get_route::shard_of(self.source, &hit.id, None)?;
        let found = self
            .role
            .get_owned(self.source, shard, &hit.id, true)
            .map_err(|reason| {
                RestError::exception(
                    503,
                    Exception::new("no_shard_available_action_exception", reason),
                )
            })?;
        // The document was deleted after the search: nothing to copy.
        let Some(found) = found else {
            return Ok(Applied::Noop);
        };
        let dest = self.dest.name.to_string(); // ALLOC: one name a hit, bounded by the page.
        let (kind, source) = match self.script {
            None => (Kind::Index, found.source),
            Some(script) => {
                let out = run_update(
                    &script.source,
                    &script.params,
                    true,
                    &dest,
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
                match out.op {
                    ScriptOp::Index | ScriptOp::Create => (Kind::Index, out.source),
                    ScriptOp::Delete => (Kind::Delete, Vec::new()),
                    ScriptOp::Noop => return Ok(Applied::Noop),
                }
            }
        };
        let action = Action {
            kind,
            index: Some(dest),
            id: Some(hit.id.clone()), // ALLOC: one id a hit, bounded by the page.
            routing: None,
            create: self.create && kind == Kind::Index,
            expect: Expect {
                version: MATCH_ANY,
                version_type: VersionType::Internal,
                if_seq_no: oss_writer::index::UNASSIGNED_SEQ_NO,
                if_primary_term: 0,
            },
            pipeline: None,
            require_alias: false,
        };
        let mut target = RoleTarget::new(self.role);
        let mut run = Bulk::new(&mut target, self.ids, self.now_millis, refresh);
        run.slice(&[Op { action, source }])?;
        let items = run.finish()?;
        let Some(item) = items.first() else {
            return Err(RestError::simple(
                500,
                "the reindex write produced no result",
            ));
        };
        match (&item.result, doc_status(item)) {
            (oss_writer::bulk::ItemResult::Written { .. }, 201) => {
                self.created = self.created.saturating_add(1);
                Ok(Applied::Changed)
            }
            (oss_writer::bulk::ItemResult::Written { .. }, _) if kind == Kind::Delete => {
                self.deleted = self.deleted.saturating_add(1);
                Ok(Applied::Changed)
            }
            (oss_writer::bulk::ItemResult::Written { .. }, _) => {
                self.updated = self.updated.saturating_add(1);
                Ok(Applied::Changed)
            }
            (oss_writer::bulk::ItemResult::Failed { error, .. }, 409) => {
                Ok(Applied::Conflict(error.reason().to_owned())) // ALLOC: error path; one message.
            }
            // Only a bulk `update` item is a no-op; a reindex writes index or delete items.
            (oss_writer::bulk::ItemResult::Noop { .. }, _) => Err(RestError::simple(
                500,
                "the reindex write produced no result",
            )),
            (oss_writer::bulk::ItemResult::Failed { status, error }, _) => {
                Err(RestError::simple(
                    *status,
                    // ALLOC: error path; one message.
                    format!("[{}] {}", error.kind(), error.reason()),
                ))
            }
        }
    }
}
