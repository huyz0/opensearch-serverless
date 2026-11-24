//! `_bulk` as stock OpenSearch answers it, streamed: the body read a slice
//! at a time inside the node's bulk budget, each slice's operations checked
//! as `BulkRequest.validate` checks them, given ids and shards, applied by
//! shard, and every item answered in request order.

mod action;
mod budget;
mod ids;
mod reader;
mod response;
mod run;
mod validate;

use std::io::Read;
use std::time::Duration;

use oss_rest::RestError;

pub use action::{Action, Defaults, Kind};
pub use budget::{BulkBudget, Grant};
pub use ids::IdGenerator;
pub use reader::{BodyReader, Op};
pub use response::{Item, ItemResult, doc_status, write as write_response, write_doc};
pub use run::{Bulk, IndexInfo, Ingest, IngestError, Owner, Planned, Target, item_result};
pub use validate::MAX_ID_BYTES;

/// `serverless.write.bulk.slice_bytes`'s default: one node-log blob's worth.
pub const DEFAULT_SLICE_BYTES: usize = 8 << 20;

/// `serverless.write.bulk.max_wait`'s default.
pub const DEFAULT_MAX_WAIT: Duration = Duration::from_secs(120);

/// How a node streams bulk bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamConfig {
    /// How much of a body is parsed and applied at once.
    pub slice_bytes: usize,
    /// How long a slice waits for its share of the budget.
    pub max_wait: Duration,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            slice_bytes: DEFAULT_SLICE_BYTES,
            max_wait: DEFAULT_MAX_WAIT,
        }
    }
}

/// Reads `body` through `buffer`, a slice at a time, each slice granted
/// from `budget` before its bytes are read, and applies it to `bulk`.
///
/// # Errors
///
/// A body refused as the Java refuses it, a slice that waited past its
/// time for the budget (429), or the body's read failing.
pub fn stream<T: Target>(
    body: &mut dyn Read,
    buffer: &mut [u8],
    defaults: &Defaults,
    budget: &BulkBudget,
    config: StreamConfig,
    bulk: &mut Bulk<'_, T>,
) -> Result<(), RestError> {
    let mut reader = BodyReader::new(defaults);
    let mut grant = budget.acquire(config.slice_bytes, config.max_wait)?;
    loop {
        let n = match body.read(buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                // ALLOC: error path; one message.
                return Err(RestError::simple(
                    400,
                    format!("the bulk body could not be read: {e}"),
                ));
            }
        };
        reader.feed(buffer.get(..n).unwrap_or_default())?;
        if reader.held() >= grant.bytes() {
            let ops = reader.take();
            bulk.slice(&ops)?;
            drop(ops);
            // Back in line for the next slice, behind whoever waits.
            drop(grant);
            grant = budget.acquire(config.slice_bytes, config.max_wait)?;
        }
    }
    reader.finish()?;
    let ops = reader.take();
    if !ops.is_empty() {
        bulk.slice(&ops)?;
    }
    drop(grant);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_documented_ones() {
        assert_eq!(DEFAULT_SLICE_BYTES, 8 * 1024 * 1024);
        assert_eq!(DEFAULT_MAX_WAIT, Duration::from_secs(120));
        let c = StreamConfig::default();
        assert_eq!(
            (c.slice_bytes, c.max_wait),
            (DEFAULT_SLICE_BYTES, DEFAULT_MAX_WAIT)
        );
    }
}
