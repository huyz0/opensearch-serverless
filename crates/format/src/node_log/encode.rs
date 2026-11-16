//! Writing a node-log blob: one window's records with the running index,
//! or a fence.

use std::borrow::Cow;
use std::collections::HashMap;

use super::{
    EncodedNodeLog, FENCE_MAGIC, MAX_HEADER_BYTES, MAX_SLICE_BYTES, MIN_INDEX_BYTES, NodeLogHeader,
    OrdinalRange, PREFIX_BYTES, RECORDS_MAGIC, RUN_BYTES, SLICE_BYTES, ShardBatch, ShardKey, Slice,
    VERSION, WHAT, trim_below, with_ordinal,
};
use crate::error::{FormatError, corrupt, len64};
use crate::mutf8;
use crate::wal_record::UNASSIGNED_SEQ_NO;

/// Encodes one window's records as the Java's `encode` does.
///
/// `prior` is the running index the node held before this window, sorted
/// or not; this blob's ordinal is added to every shard it holds a slice of,
/// every run below the trim point is dropped, and a batch with no records
/// is skipped.
///
/// # Errors
///
/// [`FormatError::Corrupt`], writing nothing, where the Java throws: a trim
/// point past the ordinal; a shard in two batches, or whose prior runs reach
/// this ordinal; a node id or index uuid or name past 65,535 bytes of
/// modified UTF-8; an epoch or ordinal past `i64::MAX` or a shard past
/// `i32::MAX`. [`FormatError::TooLarge`] for a slice past
/// [`MAX_SLICE_BYTES`] or a header past [`MAX_HEADER_BYTES`].
pub fn encode(
    node_id: &str,
    epoch: u64,
    ordinal: u64,
    trim_point: u64,
    batches: &[ShardBatch<'_>],
    prior: &[(ShardKey, Vec<OrdinalRange>)],
) -> Result<EncodedNodeLog, FormatError> {
    if trim_point > ordinal {
        return Err(corrupt(WHAT, "the trim point is past the blob's ordinal"));
    }
    let as_i64 =
        |n: u64| i64::try_from(n).map_err(|_| corrupt(WHAT, "an epoch or ordinal past a long"));
    let (epoch_out, ordinal_out, trim_out) =
        (as_i64(epoch)?, as_i64(ordinal)?, as_i64(trim_point)?);

    // The running index: prior runs trimmed, then this ordinal for each shard written.
    // ALLOC: one entry a shard with unpublished records; keys share the caller's strings.
    let mut index: Vec<(ShardKey, Vec<OrdinalRange>)> = Vec::with_capacity(prior.len());
    for (shard, runs) in prior {
        let kept = trim_below(runs, trim_point);
        if !kept.is_empty() {
            index.push((shard.clone(), kept));
        }
    }
    index.sort_by(|a, b| a.0.cmp(&b.0));
    if index
        .windows(2)
        .any(|w| w.first().map(|a| &a.0) == w.get(1).map(|b| &b.0))
    {
        return Err(corrupt(WHAT, "a shard is in the prior index twice"));
    }
    // ALLOC: one entry a non-empty batch of the window.
    let mut slices: Vec<Slice> = Vec::with_capacity(batches.len());
    let mut body_len = 0u64;
    for batch in batches.iter().filter(|b| !b.records.is_empty()) {
        if batch.shard.shard > i32::MAX as u32 {
            return Err(corrupt(WHAT, "a shard past an int"));
        }
        let mut length = 0u64;
        let mut max_seq_no = UNASSIGNED_SEQ_NO;
        for record in batch.records {
            length = length.saturating_add(len64(record.bytes().len()));
            max_seq_no = max_seq_no.max(record.seq_no());
        }
        // One newline between records.
        length = length.saturating_add(len64(batch.records.len()).saturating_sub(1));
        within(length, MAX_SLICE_BYTES)?;
        let shard = batch.shard.clone();
        match index.binary_search_by(|(k, _)| k.cmp(&shard)) {
            Ok(at) => {
                let runs = index
                    .get_mut(at)
                    .map(|(_, r)| r)
                    .ok_or(corrupt(WHAT, "index"))?;
                *runs = with_ordinal(runs, ordinal)?;
            }
            Err(at) => index.insert(
                at,
                (
                    shard.clone(),
                    // ALLOC: one run for a shard first written in this window.
                    vec![OrdinalRange {
                        from: ordinal,
                        to: ordinal,
                    }],
                ),
            ),
        }
        slices.push(Slice {
            shard,
            term: batch.term,
            offset: body_len,
            length: u32::try_from(length).map_err(|_| corrupt(WHAT, "slice length"))?,
            records: u32::try_from(batch.records.len())
                .map_err(|_| corrupt(WHAT, "record count"))?,
            max_seq_no,
        });
        body_len = body_len.saturating_add(length);
    }

    // Each index named once, in first appearance; a later known name replaces an unknown one.
    // ALLOC: one entry an index named, at most one a slice or index entry.
    let mut names: Vec<(&str, Option<&str>)> =
        Vec::with_capacity(slices.len().saturating_add(index.len()));
    // ALLOC: as above: each uuid's position in `names`.
    let mut positions: HashMap<&str, usize> = HashMap::with_capacity(names.capacity());
    for shard in slices
        .iter()
        .map(|s| &s.shard)
        .chain(index.iter().map(|(k, _)| k))
    {
        let uuid: &str = &shard.index_uuid;
        match positions.get(uuid).and_then(|&p| names.get_mut(p)) {
            Some(entry) => {
                if let Some(name) = shard.index_name.as_deref() {
                    entry.1 = Some(name);
                }
            }
            None => {
                positions.insert(uuid, names.len());
                names.push((uuid, shard.index_name.as_deref()));
            }
        }
    }
    let position = |shard: &ShardKey| -> Result<i32, FormatError> {
        positions
            .get(&*shard.index_uuid)
            .and_then(|&p| i32::try_from(p).ok())
            .ok_or(corrupt(WHAT, "a shard's index is not named"))
    };

    let mut header_len = 2usize
        .saturating_add(mutf8::encoded_len(node_id))
        .saturating_add(8 * 3 + 4 + 4 + 4);
    for (uuid, name) in &names {
        header_len = header_len
            .saturating_add(4)
            .saturating_add(mutf8::encoded_len(uuid))
            .saturating_add(mutf8::encoded_len(name.unwrap_or("")));
    }
    header_len = header_len.saturating_add(slices.len().saturating_mul(SLICE_BYTES));
    for (_, runs) in &index {
        header_len = header_len
            .saturating_add(MIN_INDEX_BYTES)
            .saturating_add(runs.len().saturating_mul(RUN_BYTES));
    }
    within(len64(header_len), MAX_HEADER_BYTES.into())?;
    let total = usize::try_from(body_len)
        .ok()
        .and_then(|b| b.checked_add(header_len))
        .and_then(|b| b.checked_add(PREFIX_BYTES))
        .ok_or(corrupt(WHAT, "blob length"))?;
    // ALLOC: the blob, exactly its length: the window's records, bounded by the committer's window.
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&RECORDS_MAGIC.to_be_bytes());
    out.push(VERSION);
    out.extend_from_slice(&i32::try_from(header_len).unwrap_or(i32::MAX).to_be_bytes());
    let utf = |text: &str, out: &mut Vec<u8>| {
        mutf8::write(text, out).ok_or(corrupt(
            WHAT,
            "a string past 65,535 bytes of modified UTF-8",
        ))
    };
    utf(node_id, &mut out)?;
    for n in [epoch_out, ordinal_out, trim_out] {
        out.extend_from_slice(&n.to_be_bytes());
    }
    out.extend_from_slice(&count(names.len()).to_be_bytes());
    for (uuid, name) in &names {
        utf(uuid, &mut out)?;
        utf(name.unwrap_or(""), &mut out)?;
    }
    out.extend_from_slice(&count(slices.len()).to_be_bytes());
    for slice in &slices {
        out.extend_from_slice(&position(&slice.shard)?.to_be_bytes());
        out.extend_from_slice(&count(slice.shard.shard as usize).to_be_bytes());
        out.extend_from_slice(&slice.term.to_be_bytes());
        out.extend_from_slice(&as_i64(slice.offset)?.to_be_bytes());
        out.extend_from_slice(&count(slice.length as usize).to_be_bytes());
        out.extend_from_slice(&count(slice.records as usize).to_be_bytes());
        out.extend_from_slice(&slice.max_seq_no.to_be_bytes());
    }
    out.extend_from_slice(&count(index.len()).to_be_bytes());
    for (shard, runs) in &index {
        if shard.shard > i32::MAX as u32 {
            return Err(corrupt(WHAT, "a shard past an int"));
        }
        out.extend_from_slice(&position(shard)?.to_be_bytes());
        out.extend_from_slice(&count(shard.shard as usize).to_be_bytes());
        out.extend_from_slice(&count(runs.len()).to_be_bytes());
        for run in runs {
            out.extend_from_slice(&as_i64(run.from)?.to_be_bytes());
            out.extend_from_slice(&as_i64(run.to)?.to_be_bytes());
        }
    }
    let body_offset = out.len();
    if body_offset != PREFIX_BYTES.saturating_add(header_len) {
        return Err(corrupt(WHAT, "the header's measured length"));
    }
    for batch in batches.iter().filter(|b| !b.records.is_empty()) {
        for (i, record) in batch.records.iter().enumerate() {
            if i > 0 {
                out.push(b'\n');
            }
            out.extend_from_slice(record.bytes());
        }
    }
    // ALLOC: the node id, for the returned header; shard keys share their strings.
    let node_id: Cow<'static, str> = Cow::Owned(node_id.to_owned());
    Ok(EncodedNodeLog {
        bytes: out,
        header: NodeLogHeader {
            node_id,
            epoch,
            ordinal,
            trim_point,
            body_offset: len64(body_offset),
            slices,
            index,
        },
    })
}

/// `TooLarge` when `actual` is past `limit`.
fn within(actual: u64, limit: u64) -> Result<(), FormatError> {
    if actual > limit {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit,
            actual,
        });
    }
    Ok(())
}

/// An `int` field of a count already bounded to fit one.
fn count(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// Encodes a fence as the Java's `encodeFence` does.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a node id past 65,535 bytes of modified UTF-8.
pub fn encode_fence(fenced_by: &str, term: i64) -> Result<Vec<u8>, FormatError> {
    // ALLOC: a fence, a node id and fourteen bytes.
    let mut out = Vec::with_capacity(mutf8::encoded_len(fenced_by).saturating_add(15));
    out.extend_from_slice(&FENCE_MAGIC.to_be_bytes());
    out.push(VERSION);
    mutf8::write(fenced_by, &mut out).ok_or(corrupt(
        WHAT,
        "a string past 65,535 bytes of modified UTF-8",
    ))?;
    out.extend_from_slice(&term.to_be_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_length_at_its_limit_passes_and_one_past_is_too_large() {
        assert_eq!(within(MAX_SLICE_BYTES, MAX_SLICE_BYTES), Ok(()));
        assert_eq!(within(0, 0), Ok(()));
        assert_eq!(
            within(MAX_SLICE_BYTES + 1, MAX_SLICE_BYTES),
            Err(FormatError::TooLarge {
                what: WHAT,
                limit: MAX_SLICE_BYTES,
                actual: MAX_SLICE_BYTES + 1
            })
        );
    }
}
