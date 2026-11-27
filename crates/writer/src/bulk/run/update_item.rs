//! An `update` item of a bulk: what it is applied as once the node's
//! [`Updates`](super::super::update::Updates) has read and decided it.

use oss_rest::Exception;

use super::super::action::{Action, Kind};
use super::super::reader::Op;
use super::super::response::ItemResult;
use super::super::response::{Item, shard_failure};
use super::super::update::Resolved;
use super::{Bulk, IndexInfo, Target};

impl<T: Target> Bulk<'_, T> {
    /// Checks an item bound for `shard` against the documents the slice
    /// already addresses and, for an `update`, resolves it into the write it
    /// becomes. `false` means the item ended with its result set.
    pub(super) fn settle_update(
        &mut self,
        info: &IndexInfo,
        shard: u32,
        op: &Op,
        item: &mut Item,
        source: &mut Option<Vec<u8>>,
        rewrite: &mut Option<Action>,
    ) -> bool {
        if self.updates.is_some() {
            let doc = (item.index.clone(), item.id.clone());
            if self.touched.contains(&doc) {
                item.result = ItemResult::Failed {
                    status: 409,
                    error: shard_failure(
                        "version_conflict_engine_exception",
                        // ALLOC: error path; one message.
                        format!(
                            "[{}]: an earlier item of this bulk addresses the document, so it is not read again; retry it in a later request",
                            item.id
                        ),
                        &info.name,
                        shard,
                        &info.uuid,
                    ),
                };
                return false;
            }
            // ALLOC: one entry an item of the slice.
            self.touched.push(doc);
        }
        if op.action.kind == Kind::Update {
            match self.resolve_update(info, shard, op, &item.id) {
                Ok((made, body)) => {
                    *rewrite = Some(made);
                    *source = Some(body);
                }
                Err(result) => {
                    item.result = result;
                    return false;
                }
            }
        }
        true
    }

    /// What an `update` item is applied as, with the document it writes, or
    /// the result it ends with when it is not applied.
    fn resolve_update(
        &mut self,
        info: &IndexInfo,
        shard: u32,
        op: &Op,
        id: &str,
    ) -> Result<(Action, Vec<u8>), ItemResult> {
        let Some(updates) = self.updates.as_deref_mut() else {
            return Err(ItemResult::Failed {
                status: 501,
                error: Exception::new(
                    "unsupported_operation_exception",
                    "[update] is not supported in a bulk here: it merges into the current document, which needs a realtime get",
                ),
            });
        };
        let resolved = updates
            .resolve(info, shard, &op.action, id, &op.source)
            .map_err(|f| ItemResult::Failed {
                status: f.status,
                error: f.error,
            })?;
        // ALLOC: the action of the write the item became.
        let mut made = op.action.clone();
        made.expect.version = crate::index::MATCH_ANY;
        made.expect.version_type = crate::index::VersionType::Internal;
        match resolved {
            Resolved::Put {
                source,
                create,
                if_seq_no,
                if_primary_term,
            } => {
                made.kind = Kind::Index;
                made.create = create;
                made.expect.if_seq_no = if_seq_no;
                made.expect.if_primary_term = if_primary_term;
                Ok((made, source))
            }
            Resolved::Delete {
                if_seq_no,
                if_primary_term,
            } => {
                made.kind = Kind::Delete;
                made.expect.if_seq_no = if_seq_no;
                made.expect.if_primary_term = if_primary_term;
                // ALLOC: none; an empty vec does not allocate.
                Ok((made, Vec::new()))
            }
            Resolved::Noop {
                version,
                seq_no,
                primary_term,
            } => Err(ItemResult::Noop {
                version,
                seq_no,
                primary_term,
            }),
        }
    }
}
