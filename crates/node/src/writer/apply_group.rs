//! Applying one admitted group to a shard's engine and building the log
//! records of what it took.

use oss_format::WalRecord;
use oss_writer::bulk::{Kind, Planned};
use oss_writer::index::Written;

use super::Failure;
use super::slot::ShardSlot;

/// What a group's apply left: an outcome an item, and a record for each the
/// engine took.
pub(super) struct Applied<'g> {
    pub(super) outcomes: Vec<Result<Written, Failure>>,
    pub(super) records: Vec<WalRecord<'g>>,
    /// The index in `outcomes` of each item that has a record.
    pub(super) staged: Vec<usize>,
}

/// Applies `group` to `slot`'s engine, item by item.
pub(super) fn apply_group<'g>(slot: &mut ShardSlot, group: &'g [Planned<'g>]) -> Applied<'g> {
    // ALLOC: one outcome and one record an item of the group.
    let mut outcomes: Vec<Result<Written, Failure>> = Vec::with_capacity(group.len());
    let mut records: Vec<WalRecord<'_>> = Vec::with_capacity(group.len());
    let mut staged: Vec<usize> = Vec::with_capacity(group.len());
    for item in group {
        let a = &item.op.action;
        let source = match std::str::from_utf8(&item.op.source) {
            Ok(s) => s,
            Err(_) if a.kind == Kind::Delete => "",
            Err(_) => {
                outcomes.push(Err(Failure::Unavailable(
                    // ALLOC: error path; one message.
                    "the document source is not valid UTF-8".to_owned(),
                )));
                continue;
            }
        };
        let routing = a.routing.as_deref().filter(|r| !r.is_empty());
        // Checked before the engine runs: a write the engine took must
        // always have a record to stage.
        if let Err(e) = WalRecord::index(item.id, source, 0, 0, 0).with_routing(routing) {
            outcomes.push(Err(Failure::Unavailable(e.to_string())));
            continue;
        }
        match slot.apply(
            a.kind,
            item.id,
            &item.op.source,
            routing,
            a.create,
            &a.expect,
        ) {
            Ok(written) => {
                let record = if a.kind == Kind::Delete {
                    WalRecord::deletion(
                        item.id,
                        written.seq_no,
                        written.primary_term,
                        written.version,
                    )
                } else {
                    WalRecord::index(
                        item.id,
                        source,
                        written.seq_no,
                        written.primary_term,
                        written.version,
                    )
                };
                // The routing was checked above, so this keeps the record.
                let record = record.with_routing(routing).unwrap_or_else(|_| {
                    WalRecord::index(
                        item.id,
                        source,
                        written.seq_no,
                        written.primary_term,
                        written.version,
                    )
                });
                staged.push(outcomes.len());
                records.push(record);
                outcomes.push(Ok(written));
            }
            Err(refused) => outcomes.push(Err(Failure::Refused(refused))),
        }
    }
    Applied {
        outcomes,
        records,
        staged,
    }
}
