//! One index's mapping grown by a batch of additions: read, merge all of
//! them, swap once, and on a lost swap wait and go again.

use std::borrow::Cow;
use std::time::Duration;

use oss_meta::{DescriptorStore, Target};

use super::GrowthError;
use crate::mapping::Mapping;

/// How many swaps one growth attempts before failing the write.
pub const ATTEMPTS: u32 = 5;

/// The most a descriptor read keeps.
const MAX_REGISTER_BYTES: usize = 4 << 20;

/// What an attempt's write buffer holds beside the mapping's text: the
/// record's other fields.
const RECORD_SLACK: usize = 8 << 10;

/// The longest wait before attempt `attempt` (counting from 0): none before
/// the first, then `min(500, 20 << min(attempt, 6))` milliseconds.
pub fn wait_bound_millis(attempt: u32) -> u64 {
    if attempt == 0 {
        return 0;
    }
    500u64.min(20u64 << attempt.min(6))
}

/// Merges `additions` into `index`'s mapping and swaps the descriptor, as
/// many times as it takes, at most [`ATTEMPTS`]. `wait` is called with a
/// bound in milliseconds before each attempt after the first.
pub fn grow_by(
    descriptors: &DescriptorStore,
    index: &str,
    uuid: &str,
    additions: &[Mapping],
    wait: &dyn Fn(u64),
) -> Result<Mapping, GrowthError> {
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            wait(wait_bound_millis(attempt));
        }
        // ALLOC: one descriptor read, at most MAX_REGISTER_BYTES, an attempt.
        let mut buf = vec![0u8; MAX_REGISTER_BYTES];
        let found = descriptors
            .resolve(index, &mut buf)
            // ALLOC: error path; one message.
            .map_err(|e| GrowthError::Store(e.to_string()))?;
        let Target::Index(record) = found.target else {
            return Err(deleted(index));
        };
        if record.uuid != uuid {
            return Err(deleted(index));
        }
        let mut merged =
            Mapping::from_descriptor(record.mapping.as_deref()).map_err(GrowthError::Conflict)?;
        for addition in additions {
            merged = merged.merge(addition).map_err(GrowthError::Conflict)?;
        }
        let text = merged.to_json();
        if record.mapping.as_deref() == Some(text.as_str()) {
            // Another node already grew it.
            return Ok(merged);
        }
        // ALLOC: the written record, the mapping's text and its other fields, a swap.
        let mut out = vec![0u8; text.len().saturating_mul(2).saturating_add(RECORD_SLACK)];
        // ALLOC: one copy of the record to change its mapping, a swap.
        let mut updated = record.clone();
        updated.mapping = Some(Cow::Owned(text));
        updated.mapping_version = updated.mapping_version.saturating_add(1);
        let swapped = descriptors
            .update(&updated, found.generation, &mut out)
            // ALLOC: error path; one message.
            .map_err(|e| GrowthError::Store(e.to_string()))?;
        if swapped.is_some() {
            return Ok(merged);
        }
    }
    // ALLOC: error path; one message.
    Err(GrowthError::Concurrent(format!(
        "the mapping of {index} is being changed concurrently; retry the write"
    )))
}

fn deleted(index: &str) -> GrowthError {
    // ALLOC: error path; one message.
    GrowthError::Deleted(format!(
        "cannot grow mapping for {index}: the index was deleted or recreated"
    ))
}

/// A wait of `bound` milliseconds at most, drawn uniformly from `1..=bound`
/// with `draw` (a random number), as the Java's jittered backoff.
pub fn jittered(bound: u64, draw: u64) -> Duration {
    Duration::from_millis(1u64.saturating_add(draw.checked_rem(bound.max(1)).unwrap_or(0)))
}
