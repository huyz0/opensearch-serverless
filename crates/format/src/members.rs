//! The registers beside the node leases: the members index, which names
//! every member so a refresh is a read rather than a listing, and the lease
//! digest, every live lease as one node last read them.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/membership/BlobLeaseMembership.java`
//! (`LEASE_PREFIX`, `MEMBERS`, `encodeIds`, `decodeIds`),
//! `membership/LeaseDigest.java` (`NAME`, `encode`, `decode`) and
//! `metadata/RegisterMap.java` (`members`). Every key is under the
//! deployment's base, which is empty or ends in `/`:
//!
//! | key | what |
//! |---|---|
//! | `cluster/members/lease-<node id>` | a node's lease, written by that node |
//! | `cluster/members/members` | the members index: ids sorted, one a line |
//! | `cluster/members/members-digest` | the lease digest |
//!
//! The digest is binary, big-endian as Java's `DataOutputStream`: a version
//! byte (1), the time it was written (`i64`), the lease count (`i32`), then
//! each lease as its length (`i32`) and its JSON.

use crate::error::{FormatError, corrupt, len64};
use crate::node_lease::{MAX_LEASE_BYTES, NodeLease, java_order};

const KEY: &str = "members key";
const INDEX: &str = "members index";
const DIGEST: &str = "lease digest";

/// Where under a deployment's base the membership registers are.
pub const MEMBERS_PREFIX: &str = "cluster/members/";

/// The prefix of a lease's blob name in [`MEMBERS_PREFIX`].
pub const LEASE_PREFIX: &str = "lease-";

/// The longest node id: ids are path segments and lines of the index.
pub const MAX_NODE_ID_BYTES: usize = 128;

/// The digest's format version.
const DIGEST_VERSION: u8 = 1;

/// The digest's fixed part: version, time and count.
const DIGEST_HEAD: usize = 13;

/// Checks that `node_id` can name a lease: not empty, at most
/// [`MAX_NODE_ID_BYTES`], no `/` (a path segment on the filesystem store),
/// and no character the members index would split or trim on (at most
/// U+0020, as Java's `String.trim`).
///
/// # Errors
///
/// [`FormatError::Corrupt`] for an id that cannot.
pub fn check_node_id(node_id: &str) -> Result<(), FormatError> {
    if node_id.is_empty() || node_id.len() > MAX_NODE_ID_BYTES {
        return Err(corrupt(KEY, "a node id is 1 to 128 bytes"));
    }
    if node_id.bytes().any(|b| b == b'/' || b <= b' ') {
        return Err(corrupt(
            KEY,
            "a node id holds no '/', space or control character",
        ));
    }
    Ok(())
}

fn check_base(base: &str) -> Result<(), FormatError> {
    if base.is_empty() || base.ends_with('/') {
        Ok(())
    } else {
        Err(corrupt(KEY, "a base is empty or ends in '/'"))
    }
}

/// The key of `node_id`'s lease (`cluster/members/lease-<node id>`).
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is not empty and does not end in
/// `/`, or an id [`check_node_id`] refuses.
pub fn lease_key(base: &str, node_id: &str) -> Result<String, FormatError> {
    check_base(base)?;
    check_node_id(node_id)?;
    // ALLOC: one key, the length of its parts.
    Ok([base, MEMBERS_PREFIX, LEASE_PREFIX, node_id].concat())
}

/// The key of the members index (`cluster/members/members`).
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is not empty and does not end in
/// `/`.
pub fn members_index_key(base: &str) -> Result<String, FormatError> {
    check_base(base)?;
    // ALLOC: one key, the length of its parts.
    Ok([base, MEMBERS_PREFIX, "members"].concat())
}

/// The key of the lease digest (`cluster/members/members-digest`).
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is not empty and does not end in
/// `/`.
pub fn lease_digest_key(base: &str) -> Result<String, FormatError> {
    check_base(base)?;
    // ALLOC: one key, the length of its parts.
    Ok([base, MEMBERS_PREFIX, "members-digest"].concat())
}

/// Reads the members index as the Java's `decodeIds`: each line trimmed,
/// blank lines skipped, sorted and without repeats.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for bytes that are not UTF-8 or a line that is
/// not a node id.
pub fn decode_members(value: &[u8]) -> Result<Vec<String>, FormatError> {
    let text = std::str::from_utf8(value).map_err(|_| corrupt(INDEX, "not UTF-8"))?;
    // ALLOC: one id a member, bounded by the register's length.
    let mut ids = Vec::new();
    for line in text.split('\n') {
        let id = line.trim_matches(|c: char| c <= ' ');
        if id.is_empty() {
            continue;
        }
        check_node_id(id).map_err(|_| corrupt(INDEX, "a line is not a node id"))?;
        // ALLOC: as above.
        ids.push(id.to_owned());
    }
    ids.sort_by(|a, b| java_order(a, b));
    ids.dedup();
    Ok(ids)
}

/// Writes the members index as the Java's `encodeIds`: the ids sorted as
/// Java's `TreeSet` and joined by `\n`, into the front of `buf`; returns
/// its length. `ids` must be sorted by [`java_order`] and without repeats,
/// as [`decode_members`] returns them.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for ids out of order or one [`check_node_id`]
/// refuses; [`FormatError::TooLarge`] when the index is longer than `buf`.
pub fn encode_members(ids: &[String], buf: &mut [u8]) -> Result<usize, FormatError> {
    let mut len = 0usize;
    for (i, id) in ids.iter().enumerate() {
        check_node_id(id)?;
        if i > 0 {
            if ids
                .get(i.saturating_sub(1))
                .is_some_and(|prev| java_order(prev, id).is_ge())
            {
                return Err(corrupt(INDEX, "ids are sorted and distinct"));
            }
            len = put(buf, len, b"\n");
        }
        len = put(buf, len, id.as_bytes());
    }
    if len > buf.len() {
        return Err(FormatError::TooLarge {
            what: INDEX,
            limit: len64(buf.len()),
            actual: len64(len),
        });
    }
    Ok(len)
}

/// Copies `bytes` to `buf` at `at` if they fit, and returns where they end
/// whether or not they did, so a short buffer learns how long one must be.
fn put(buf: &mut [u8], at: usize, bytes: &[u8]) -> usize {
    let end = at.saturating_add(bytes.len());
    if let Some(room) = buf.get_mut(at..end) {
        room.copy_from_slice(bytes);
    }
    end
}

/// Writes the lease digest of `leases`, read at `written_at_millis`, as the
/// Java's `LeaseDigest.encode`, into the front of `buf`; returns its
/// length.
///
/// # Errors
///
/// [`FormatError::TooLarge`] when the digest is longer than `buf`, or a
/// lease longer than [`MAX_LEASE_BYTES`].
pub fn encode_digest(
    written_at_millis: i64,
    leases: &[NodeLease],
    buf: &mut [u8],
) -> Result<usize, FormatError> {
    let count = i32::try_from(leases.len()).map_err(|_| FormatError::TooLarge {
        what: DIGEST,
        limit: i32::MAX as u64,
        actual: len64(leases.len()),
    })?;
    let mut at = put(buf, 0, &[DIGEST_VERSION]);
    at = put(buf, at, &written_at_millis.to_be_bytes());
    at = put(buf, at, &count.to_be_bytes());
    // ALLOC: none; one lease at a time on the stack.
    let mut one = [0u8; MAX_LEASE_BYTES];
    for lease in leases {
        let len = lease.write(&mut one)?;
        let prefix = i32::try_from(len).unwrap_or(i32::MAX);
        at = put(buf, at, &prefix.to_be_bytes());
        at = put(buf, at, one.get(..len).unwrap_or_default());
    }
    if at > buf.len() {
        return Err(FormatError::TooLarge {
            what: DIGEST,
            limit: len64(buf.len()),
            actual: len64(at),
        });
    }
    Ok(at)
}

/// A decoded lease digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseDigest {
    /// When the writer read the leases, epoch milliseconds.
    pub written_at_millis: i64,
    /// The leases it read live.
    pub leases: Vec<NodeLease>,
}

/// Reads a lease digest as the Java's `LeaseDigest.decode`.
///
/// # Errors
///
/// [`FormatError::Unsupported`] for another version;
/// [`FormatError::Truncated`] for bytes that end early;
/// [`FormatError::Corrupt`] for a negative count or length, a lease that
/// does not parse, or bytes after the last lease. A lease longer than
/// [`MAX_LEASE_BYTES`] is skipped, not refused.
pub fn decode_digest(value: &[u8]) -> Result<LeaseDigest, FormatError> {
    let truncated = || FormatError::Truncated {
        what: DIGEST,
        needed: 0,
        have: len64(value.len()),
    };
    let head = value.get(..DIGEST_HEAD).ok_or_else(truncated)?;
    let (version, rest) = head.split_first().ok_or_else(truncated)?;
    if *version != DIGEST_VERSION {
        return Err(FormatError::Unsupported {
            what: DIGEST,
            version: u64::from(*version),
        });
    }
    let (time, count) = rest.split_at(8);
    let written_at_millis = i64::from_be_bytes(time.try_into().map_err(|_| truncated())?);
    let count = i32::from_be_bytes(count.try_into().map_err(|_| truncated())?);
    let count = usize::try_from(count).map_err(|_| corrupt(DIGEST, "a negative lease count"))?;
    let mut at = DIGEST_HEAD;
    // Each lease takes at least its four-byte length, so a count past what
    // the bytes could hold is refused before anything is reserved for it.
    if count > value.len().saturating_sub(at) / 4 {
        return Err(truncated());
    }
    // ALLOC: one lease a member, bounded by the register's length.
    let mut leases = Vec::with_capacity(count);
    for _ in 0..count {
        let len_end = at.saturating_add(4);
        let len = value.get(at..len_end).ok_or_else(truncated)?;
        let len = i32::from_be_bytes(len.try_into().map_err(|_| truncated())?);
        let len = usize::try_from(len).map_err(|_| corrupt(DIGEST, "a negative lease length"))?;
        let end = len_end.saturating_add(len);
        let lease = value.get(len_end..end).ok_or_else(truncated)?;
        at = end;
        // The Java bounds no lease, so one past ours is left out rather than
        // costing every other member its digest; a reader then reads that
        // lease alone, and refuses it there.
        if lease.len() > MAX_LEASE_BYTES {
            continue;
        }
        leases.push(NodeLease::parse(lease)?);
    }
    if at != value.len() {
        // The Java stops after the count and ignores the rest; no Java writes
        // a digest with more.
        return Err(corrupt(DIGEST, "bytes follow the last lease"));
    }
    Ok(LeaseDigest {
        written_at_millis,
        leases,
    })
}
