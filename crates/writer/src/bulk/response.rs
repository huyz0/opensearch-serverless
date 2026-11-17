//! `BulkResponse` as stock OpenSearch renders it: `took`, `errors`, and
//! each item's outcome or failure, keyed by its action.

use std::io::{self, Write};

use oss_rest::{Exception, Meta, write_json_str};

use crate::index::{Outcome, Written};
use crate::mapping::MappingError;

/// What became of one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemResult {
    /// The shard wrote it.
    Written {
        /// The engine's answer.
        written: Written,
        /// Whether the request refreshed its shard (`?refresh=true`).
        forced_refresh: bool,
    },
    /// It failed, alone: the rest of the request went on.
    Failed {
        /// The item's HTTP status.
        status: u16,
        /// The failure, as `error`.
        error: Exception,
    },
}

/// One item of the response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The action name it is keyed by: an `index` with `op_type=create` is
    /// a `create`.
    pub action: &'static str,
    /// `_index`.
    pub index: String,
    /// `_id`.
    pub id: String,
    /// The outcome.
    pub result: ItemResult,
}

impl Item {
    /// Whether the item failed.
    pub fn failed(&self) -> bool {
        matches!(self.result, ItemResult::Failed { .. })
    }
}

/// A failure of the item's own shard (a version conflict), which names
/// the index, the shard and the index's uuid.
pub fn shard_failure(
    kind: &'static str,
    reason: String,
    index: &str,
    shard: u32,
    uuid: &str,
) -> Exception {
    Exception::new(kind, reason)
        // ALLOC: three short fields an item that failed.
        .with_meta("index", Meta::Str(index.to_owned()))
        .with_meta("shard", Meta::Str(shard.to_string()))
        .with_meta("index_uuid", Meta::Str(uuid.to_owned()))
}

/// A document or mapping refusal as its exception chain.
pub fn mapping_failure(e: &MappingError) -> Exception {
    // ALLOC: the refusal's message; an item that failed.
    let mut out = Exception::new(e.ty, e.reason.clone());
    if let Some(cause) = &e.caused_by {
        out = out.caused_by(mapping_failure(cause));
    }
    out
}

/// The status OpenSearch gives a refusal of this type.
pub fn mapping_status(e: &MappingError) -> u16 {
    match e.ty {
        "illegal_state_exception" => 500,
        _ => 400,
    }
}

fn result_name(outcome: Outcome) -> &'static str {
    outcome.name()
}

fn status_of(outcome: Outcome) -> u16 {
    match outcome {
        Outcome::Created => 201,
        Outcome::Updated | Outcome::Deleted => 200,
        Outcome::NotFound => 404,
    }
}

/// Writes the response body for `items`, `took` milliseconds after the
/// request arrived.
///
/// # Errors
///
/// The writer's.
pub fn write(out: &mut dyn Write, took: u64, items: &[Item]) -> io::Result<()> {
    let errors = items.iter().any(Item::failed);
    write!(out, "{{\"took\":{took},\"errors\":{errors},\"items\":[")?;
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        write_item(out, item)?;
    }
    out.write_all(b"]}")
}

fn write_item(out: &mut dyn Write, item: &Item) -> io::Result<()> {
    out.write_all(b"{")?;
    write_json_str(out, item.action)?;
    out.write_all(b":{\"_index\":")?;
    write_json_str(out, &item.index)?;
    out.write_all(b",\"_id\":")?;
    write_json_str(out, &item.id)?;
    match &item.result {
        ItemResult::Written {
            written,
            forced_refresh,
        } => {
            write!(out, ",\"_version\":{},\"result\":", written.version)?;
            write_json_str(out, result_name(written.outcome))?;
            if *forced_refresh {
                out.write_all(b",\"forced_refresh\":true")?;
            }
            write!(
                out,
                ",\"_shards\":{{\"total\":1,\"successful\":1,\"failed\":0}},\"_seq_no\":{},\"_primary_term\":{},\"status\":{}",
                written.seq_no,
                written.primary_term,
                status_of(written.outcome)
            )?;
        }
        ItemResult::Failed { status, error } => {
            write!(out, ",\"status\":{status},\"error\":{{")?;
            error.write_members(out, true)?;
            out.write_all(b"}")?;
        }
    }
    out.write_all(b"}}")
}
