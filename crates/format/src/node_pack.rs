//! A node pack: one blob holding segment blobs of many shards that one node
//! published together.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/store/NodePackBlob.java`
//! (`headerEnd`, `decode`); keys from `NodePackStore.java` (`name`,
//! `epochDir`). The blob is `packs/<node>/e=<epoch:020>/<ordinal:020>` and starts:
//!
//! | bytes | field |
//! |---|---|
//! | 4 | magic `0x4E504B31` (`NPK1`), big-endian |
//! | 1 | version: 2, or 1 for packs without index names |
//! | 4 | header length, big-endian signed |
//! | header | node id, epoch (i64), ordinal (i64), block count (i32), then per block: index uuid, index name (version 2 only), shard (i32), blob name, offset (i64), length (i64) |
//!
//! Strings are Java `DataOutput.writeUTF`: a big-endian u16 byte length, then
//! modified UTF-8. Each block is a whole segment blob, header included, at
//! its offset from the pack's start. A commit manifest already records each
//! file's range of the pack, so a reader needs this header only to check a
//! pack, or for collection.
//!
//! Stricter than the Java: blocks must lie past the header without
//! overlapping, and the header must end where its length says.

use std::borrow::Cow;

use crate::error::{FormatError, corrupt, len64};
use crate::mutf8;

const WHAT: &str = "node pack header";

/// The magic a node pack starts with.
pub const MAGIC: u32 = 0x4E50_4B31;
/// The current version.
pub const VERSION: u8 = 2;
/// The older version, whose blocks carry no index name.
pub const VERSION_UUID_ONLY: u8 = 1;
/// Bytes before the header: magic, version, header length.
pub const PREFIX_BYTES: usize = 4 + 1 + 4;
/// The longest header accepted. A pack holds one node's publishes of one
/// linger window; at about 100 bytes a block this is some 40,000 blocks.
pub const MAX_HEADER_BYTES: u32 = 4 * 1024 * 1024;
/// The bytes a block takes at the least: three empty strings (two in version
/// 1), the shard, the offset and the length. Bounds the block count by the
/// header's length before anything is allocated.
const MIN_BLOCK_BYTES: usize = 2 + 2 + 4 + 2 + 8 + 8;

/// One segment blob in a pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block<'a> {
    /// The index's uuid.
    pub index_uuid: Cow<'a, str>,
    /// The index's name, empty in a version 1 pack.
    pub index_name: Cow<'a, str>,
    /// The shard.
    pub shard: u32,
    /// The segment blob's name.
    pub name: Cow<'a, str>,
    /// Where the block starts, from the pack's first byte.
    pub offset: u64,
    /// The block's length.
    pub length: u64,
}

impl Block<'_> {
    /// Whether `offset..offset+length` lies inside this block.
    pub fn contains(&self, offset: u64, length: u64) -> bool {
        let end = self.offset.saturating_add(self.length);
        offset >= self.offset && offset.checked_add(length).is_some_and(|e| e <= end)
    }
}

/// A parsed node pack header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePackHeader<'a> {
    /// The node that wrote the pack.
    pub node: Cow<'a, str>,
    /// The pack's epoch.
    pub epoch: u64,
    /// The pack's slot in its epoch.
    pub ordinal: u64,
    /// The format version read.
    pub version: u8,
    /// Where the body (the first block) starts.
    pub body_start: u64,
    blocks: Vec<Block<'a>>,
}

/// How many bytes from the pack's start its header takes, from its first
/// [`PREFIX_BYTES`].
///
/// # Errors
///
/// [`FormatError::Truncated`] for a shorter prefix, [`FormatError::Corrupt`]
/// for a wrong magic or a negative length, [`FormatError::Unsupported`] for
/// an unknown version, [`FormatError::TooLarge`] past [`MAX_HEADER_BYTES`].
pub fn header_end(leading: &[u8]) -> Result<usize, FormatError> {
    let mut at = Cursor {
        bytes: leading,
        at: 0,
    };
    let (Ok(magic), Ok(version), Ok(length)) = (at.u32(), at.u8(), at.i32()) else {
        return Err(FormatError::Truncated {
            what: WHAT,
            needed: len64(PREFIX_BYTES),
            have: len64(leading.len()),
        });
    };
    if magic != MAGIC {
        return Err(corrupt(WHAT, "not a node pack: wrong magic"));
    }
    if version != VERSION && version != VERSION_UUID_ONLY {
        return Err(FormatError::Unsupported {
            what: WHAT,
            version: version.into(),
        });
    }
    let length = u32::try_from(length).map_err(|_| corrupt(WHAT, "negative header length"))?;
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
        .ok_or(corrupt(WHAT, "header length"))
}

impl<'a> NodePackHeader<'a> {
    /// Parses the header from the pack's leading bytes, which must hold at
    /// least [`header_end`] of them; more are ignored.
    ///
    /// # Errors
    ///
    /// As [`header_end`]; [`FormatError::Truncated`] with `needed` set when
    /// `leading` is shorter than the header; [`FormatError::Corrupt`] for a
    /// field that runs past the header, a negative number, bad modified
    /// UTF-8, an empty uuid or blob name, overlapping blocks, or bytes left
    /// over in the header.
    pub fn parse(leading: &'a [u8]) -> Result<Self, FormatError> {
        let end = header_end(leading)?;
        let Some(header) = leading.get(..end) else {
            return Err(FormatError::Truncated {
                what: WHAT,
                needed: len64(end),
                have: len64(leading.len()),
            });
        };
        let version = header.get(4).copied().unwrap_or_default();
        let mut at = Cursor {
            bytes: header,
            at: PREFIX_BYTES,
        };
        let node = at.utf()?;
        let epoch = at.non_negative()?;
        let ordinal = at.non_negative()?;
        let count =
            usize::try_from(at.i32()?).map_err(|_| corrupt(WHAT, "negative block count"))?;
        let room = end.saturating_sub(at.at) / MIN_BLOCK_BYTES;
        if count > room {
            // More blocks than the header has bytes for.
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: len64(room),
                actual: len64(count),
            });
        }
        let body_start = len64(end);
        // ALLOC: one entry a block, at most header length / MIN_BLOCK_BYTES (checked above).
        let mut blocks = Vec::with_capacity(count);
        let mut next_free = body_start;
        for _ in 0..count {
            let index_uuid = at.utf()?;
            let index_name = if version == VERSION_UUID_ONLY {
                Cow::Borrowed("")
            } else {
                at.utf()?
            };
            let shard = u32::try_from(at.i32()?).map_err(|_| corrupt(WHAT, "negative shard"))?;
            let name = at.utf()?;
            let offset = at.non_negative()?;
            let length = at.non_negative()?;
            if index_uuid.is_empty() || name.is_empty() {
                return Err(corrupt(WHAT, "a block has no index uuid or no name"));
            }
            if offset < next_free {
                return Err(corrupt(
                    WHAT,
                    "a block starts inside the header or the block before it",
                ));
            }
            next_free = offset
                .checked_add(length)
                .ok_or(corrupt(WHAT, "a block's range overflows"))?;
            blocks.push(Block {
                index_uuid,
                index_name,
                shard,
                name,
                offset,
                length,
            });
        }
        if at.at != end {
            return Err(corrupt(WHAT, "bytes left over in the header"));
        }
        Ok(Self {
            node,
            epoch,
            ordinal,
            version,
            body_start,
            blocks,
        })
    }

    /// The blocks, in pack order.
    pub fn blocks(&self) -> &[Block<'a>] {
        &self.blocks
    }

    /// The block holding `offset..offset+length`, if one does.
    pub fn block_containing(&self, offset: u64, length: u64) -> Option<&Block<'a>> {
        self.blocks.iter().find(|b| b.contains(offset, length))
    }

    /// The pack's length implied by the header: the end of its last block.
    pub fn pack_length(&self) -> u64 {
        self.blocks
            .last()
            .map_or(self.body_start, |b| b.offset.saturating_add(b.length))
    }
}

/// One block a pack will hold, before its offset is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedBlock<'a> {
    /// The index's uuid.
    pub index_uuid: &'a str,
    /// The index's name, which with the uuid locates the shard's manifest.
    pub index_name: &'a str,
    /// The shard.
    pub shard: u32,
    /// The segment blob's name as the shard would have named it.
    pub name: &'a str,
    /// The blob's length.
    pub length: u64,
}

/// A laid-out pack's leading bytes, and where each block then starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPack {
    /// The prefix and header; the blocks follow in the order planned.
    pub bytes: Vec<u8>,
    /// Where each block starts from the pack's first byte, in planned order.
    pub offsets: Vec<u64>,
}

/// Lays out a pack as `NodePackBlob.encode` does (version 2): the prefix and
/// header that go before the blocks, and each block's offset.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for no block, an empty uuid or name, an epoch,
/// ordinal or range past `i64`, or a string past `writeUTF`'s 65,535 bytes;
/// [`FormatError::TooLarge`] for a header past [`MAX_HEADER_BYTES`].
pub fn encode(
    node: &str,
    epoch: u64,
    ordinal: u64,
    planned: &[PlannedBlock<'_>],
) -> Result<EncodedPack, FormatError> {
    if planned.is_empty() {
        return Err(corrupt(WHAT, "a pack holds at least one block"));
    }
    let signed = |v: u64| i64::try_from(v).map_err(|_| corrupt(WHAT, "a number past i64"));
    let count = i32::try_from(planned.len()).map_err(|_| corrupt(WHAT, "too many blocks"))?;
    // ALLOC: the header, at most MAX_HEADER_BYTES (checked below), once a pack.
    let mut header: Vec<u8> = Vec::new();
    let utf = |text: &str, out: &mut Vec<u8>| {
        mutf8::write(text, out).ok_or(corrupt(WHAT, "a string past writeUTF's length"))
    };
    utf(node, &mut header)?;
    header.extend_from_slice(&signed(epoch)?.to_be_bytes());
    header.extend_from_slice(&signed(ordinal)?.to_be_bytes());
    header.extend_from_slice(&count.to_be_bytes());
    // The prefix and header length are not known until every block is
    // written; the offsets are fixed-width, so they are patched after.
    // ALLOC: one slot a block, at most MAX_HEADER_BYTES / MIN_BLOCK_BYTES.
    let mut patches = Vec::with_capacity(planned.len());
    for block in planned {
        if block.index_uuid.is_empty() || block.name.is_empty() {
            return Err(corrupt(WHAT, "a block has no index uuid or no name"));
        }
        signed(block.length)?;
        let shard = i32::try_from(block.shard).map_err(|_| corrupt(WHAT, "shard past i32"))?;
        utf(block.index_uuid, &mut header)?;
        utf(block.index_name, &mut header)?;
        header.extend_from_slice(&shard.to_be_bytes());
        utf(block.name, &mut header)?;
        patches.push(header.len());
        header.extend_from_slice(&[0; 8]);
        header.extend_from_slice(&signed(block.length)?.to_be_bytes());
    }
    let size = u32::try_from(header.len()).unwrap_or(u32::MAX);
    if size > MAX_HEADER_BYTES {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: MAX_HEADER_BYTES.into(),
            actual: len64(header.len()),
        });
    }
    let body_start = len64(PREFIX_BYTES.saturating_add(header.len()));
    // ALLOC: one offset a block, bounded as the slots are.
    let mut offsets = Vec::with_capacity(planned.len());
    let mut next = body_start;
    for (block, at) in planned.iter().zip(patches) {
        offsets.push(next);
        let slot = header
            .get_mut(at..at.saturating_add(8))
            .ok_or(corrupt(WHAT, "an offset slot is out of range"))?;
        slot.copy_from_slice(&signed(next)?.to_be_bytes());
        next = next
            .checked_add(block.length)
            .ok_or(corrupt(WHAT, "a block's range overflows"))?;
    }
    signed(next)?;
    // ALLOC: the prefix and header, at most MAX_HEADER_BYTES, once a pack.
    let mut bytes = Vec::with_capacity(PREFIX_BYTES.saturating_add(header.len()));
    bytes.extend_from_slice(&MAGIC.to_be_bytes());
    bytes.push(VERSION);
    bytes.extend_from_slice(&size.to_be_bytes());
    bytes.extend_from_slice(&header);
    Ok(EncodedPack { bytes, offsets })
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

    fn non_negative(&mut self) -> Result<u64, FormatError> {
        u64::try_from(i64::from_be_bytes(self.array()?))
            .map_err(|_| corrupt(WHAT, "a negative epoch, ordinal or range"))
    }

    fn utf(&mut self) -> Result<Cow<'a, str>, FormatError> {
        let length = u16::from_be_bytes(self.array()?);
        let bytes = self.take(length.into())?;
        mutf8::decode(bytes).ok_or(corrupt(WHAT, "a string is not modified UTF-8"))
    }
}
