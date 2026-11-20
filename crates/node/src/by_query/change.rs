//! The change a delete-by-query makes: one delete a hit, conditional on the
//! sequence number and primary term the search saw.

use oss_coordinator::docs::by_query::{Applied, Change, Hit};
use oss_rest::RestError;
use oss_writer::bulk::{Action, Bulk, IdGenerator, IndexInfo, ItemResult, Kind, Op};
use oss_writer::index::{Expect, MATCH_ANY, VersionType};

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
            Some(ItemResult::Written { .. }) => Ok(Applied::Changed),
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
