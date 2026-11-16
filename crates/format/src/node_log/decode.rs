//! Reading a node-log slot: the header's length from its prefix, then the
//! header or fence.

use std::borrow::Cow;

use super::{
    FENCE_MAGIC, MAX_HEADER_BYTES, MIN_INDEX_BYTES, MIN_NAME_BYTES, NodeLogEntry, NodeLogFence,
    NodeLogHeader, OrdinalRange, PREFIX_BYTES, RECORDS_MAGIC, RUN_BYTES, SLICE_BYTES, ShardKey,
    Slice, VERSION, WHAT,
};
use crate::error::{FormatError, corrupt, len64};
use crate::mutf8;

/// How many leading bytes a blob's prefix and header take, `Ok(None)` when
/// more bytes are needed to tell. A fence is complete in whatever bytes are
/// given, as in the Java.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a magic of neither kind or a negative header
/// length, [`FormatError::TooLarge`] past [`MAX_HEADER_BYTES`].
pub fn header_end(leading: &[u8]) -> Result<Option<usize>, FormatError> {
    let magic = leading.first_chunk::<4>().map(|m| u32::from_be_bytes(*m));
    if magic == Some(FENCE_MAGIC) {
        return Ok(Some(leading.len()));
    }
    let Some(prefix) = leading.first_chunk::<PREFIX_BYTES>() else {
        return Ok(None);
    };
    if magic != Some(RECORDS_MAGIC) {
        return Err(corrupt(WHAT, "not a node-log blob: wrong magic"));
    }
    let [.., a, b, c, d] = *prefix;
    let length = u32::try_from(i32::from_be_bytes([a, b, c, d]))
        .map_err(|_| corrupt(WHAT, "negative header length"))?;
    if length > MAX_HEADER_BYTES {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: MAX_HEADER_BYTES.into(),
            actual: length.into(),
        });
    }
    usize::try_from(length)
        .ok()
        .and_then(|l| l.checked_add(PREFIX_BYTES))
        .map(Some)
        .ok_or(corrupt(WHAT, "header length"))
}

/// Decodes what a slot holds from the blob's leading bytes, which must
/// cover the prefix and header; more are ignored.
///
/// # Errors
///
/// As [`header_end`]; [`FormatError::Unsupported`] for another version;
/// [`FormatError::Truncated`] with `needed` set when the header is longer
/// than `leading`; [`FormatError::Corrupt`] for a field past the header, bad
/// modified UTF-8, a name position out of range, or a rule in the module
/// documentation broken.
pub fn decode(leading: &[u8]) -> Result<NodeLogEntry<'_>, FormatError> {
    let mut at = Cursor {
        bytes: leading,
        at: 0,
    };
    let (Ok(magic), Ok(version)) = (at.u32(), at.u8()) else {
        return Err(FormatError::Truncated {
            what: WHAT,
            needed: len64(PREFIX_BYTES),
            have: len64(leading.len()),
        });
    };
    if version != VERSION {
        return Err(FormatError::Unsupported {
            what: WHAT,
            version: version.into(),
        });
    }
    if magic == FENCE_MAGIC {
        let fenced_by = at.utf()?;
        let term = at.i64()?;
        return Ok(NodeLogEntry::Fence(NodeLogFence { fenced_by, term }));
    }
    let Some(end) = header_end(leading)? else {
        return Err(FormatError::Truncated {
            what: WHAT,
            needed: len64(PREFIX_BYTES),
            have: len64(leading.len()),
        });
    };
    let Some(header) = leading.get(..end) else {
        return Err(FormatError::Truncated {
            what: WHAT,
            needed: len64(end),
            have: len64(leading.len()),
        });
    };
    let mut at = Cursor {
        bytes: header,
        at: PREFIX_BYTES,
    };
    let node_id = at.utf()?;
    let epoch = at.non_negative()?;
    let ordinal = at.non_negative()?;
    let trim_point = at.non_negative()?;
    if trim_point > ordinal {
        return Err(corrupt(WHAT, "the trim point is past the blob's ordinal"));
    }
    let names = at.count(MIN_NAME_BYTES)?;
    // ALLOC: one entry an index named, at most header length / MIN_NAME_BYTES (checked by count).
    let mut named = Vec::with_capacity(names);
    for _ in 0..names {
        let uuid = at.utf()?;
        let name = at.utf()?;
        named.push((uuid, (!name.is_empty()).then_some(name)));
    }
    let slice_count = at.count(SLICE_BYTES)?;
    // ALLOC: one entry a slice, at most header length / SLICE_BYTES (checked by count).
    let mut slices = Vec::with_capacity(slice_count);
    let mut next_free = 0u64;
    for _ in 0..slice_count {
        let shard = at.shard(&named)?;
        let term = at.i64()?;
        let offset = at.non_negative()?;
        let length = at.non_negative_i32()?;
        let records = at.non_negative_i32()?;
        let max_seq_no = at.i64()?;
        if offset < next_free {
            return Err(corrupt(WHAT, "a slice starts inside the slice before it"));
        }
        next_free = offset
            .checked_add(length.into())
            .ok_or(corrupt(WHAT, "a slice's range overflows"))?;
        slices.push(Slice {
            shard,
            term,
            offset,
            length,
            records,
            max_seq_no,
        });
    }
    let index_count = at.count(MIN_INDEX_BYTES)?;
    // ALLOC: one entry a shard indexed, at most header length / MIN_INDEX_BYTES (checked by count).
    let mut index: Vec<(ShardKey<'_>, Vec<OrdinalRange>)> = Vec::with_capacity(index_count);
    for _ in 0..index_count {
        let shard = at.shard(&named)?;
        if index.iter().any(|(k, _)| *k == shard) {
            return Err(corrupt(WHAT, "a shard is indexed twice"));
        }
        let run_count = at.count(RUN_BYTES)?;
        // ALLOC: one entry a run, at most header length / RUN_BYTES (checked by count).
        let mut runs = Vec::with_capacity(run_count);
        let mut floor = 0u64;
        for first in (0..run_count).map(|i| i == 0) {
            let from = at.non_negative()?;
            let to = at.non_negative()?;
            if to < from || (!first && from <= floor) {
                return Err(corrupt(
                    WHAT,
                    "a shard's runs are empty, overlap or go back",
                ));
            }
            floor = to;
            runs.push(OrdinalRange { from, to });
        }
        index.push((shard, runs));
    }
    if at.at != end {
        return Err(corrupt(WHAT, "bytes left over in the header"));
    }
    Ok(NodeLogEntry::Records(NodeLogHeader {
        node_id,
        epoch,
        ordinal,
        trim_point,
        body_offset: len64(end),
        slices,
        index,
    }))
}

/// Big-endian reads over the header, each refusing to run past its end.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], FormatError> {
        let end = self
            .at
            .checked_add(n)
            .ok_or(corrupt(WHAT, "a field runs past the header"))?;
        let taken = self
            .bytes
            .get(self.at..end)
            .ok_or(corrupt(WHAT, "a field runs past the header"))?;
        self.at = end;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], FormatError> {
        self.take(N)?
            .try_into()
            .map_err(|_| corrupt(WHAT, "a field runs past the header"))
    }

    fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(u8::from_be_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, FormatError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, FormatError> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    fn i64(&mut self) -> Result<i64, FormatError> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    fn non_negative(&mut self) -> Result<u64, FormatError> {
        u64::try_from(self.i64()?)
            .map_err(|_| corrupt(WHAT, "a negative epoch, ordinal, offset or run"))
    }

    fn non_negative_i32(&mut self) -> Result<u32, FormatError> {
        u32::try_from(self.i32()?).map_err(|_| corrupt(WHAT, "a negative shard, count or length"))
    }

    /// A count of entries of at least `min` bytes each, bounded by the
    /// bytes left in the header.
    fn count(&mut self, min: usize) -> Result<usize, FormatError> {
        let n = usize::try_from(self.non_negative_i32()?).map_err(|_| corrupt(WHAT, "count"))?;
        let room = self
            .bytes
            .len()
            .saturating_sub(self.at)
            .checked_div(min)
            .unwrap_or(0);
        if n > room {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: len64(room),
                actual: len64(n),
            });
        }
        Ok(n)
    }

    fn shard(
        &mut self,
        named: &[(Cow<'a, str>, Option<Cow<'a, str>>)],
    ) -> Result<ShardKey<'a>, FormatError> {
        let position = self.i32()?;
        let (uuid, name) = usize::try_from(position)
            .ok()
            .and_then(|p| named.get(p))
            .ok_or(corrupt(WHAT, "a shard names an index past the names"))?;
        let shard = self.non_negative_i32()?;
        Ok(ShardKey {
            index_uuid: uuid.clone(),
            shard,
            index_name: name.clone(),
        })
    }

    fn utf(&mut self) -> Result<Cow<'a, str>, FormatError> {
        let length = u16::from_be_bytes(self.array()?);
        let bytes = self.take(length.into())?;
        mutf8::decode(bytes).ok_or(corrupt(WHAT, "a string is not modified UTF-8"))
    }
}
