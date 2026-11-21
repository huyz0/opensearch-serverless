//! The compact encoding of a bucket manifest: a header, then a body of one
//! string table and every member's entry, which a reader after one member
//! skips the others of unparsed.
//!
//! Ported from `CompactManifestFormat` (`open`, `decodeMember`,
//! `readMember`, `readCommit`) on huyz0/OpenSearch `feature/serverlessshell`:
//! `OSBM`, format 2, codec id, the body's length as four big-endian bytes,
//! then the body.

use super::codec;
use super::cursor::{Cursor, string_at};
use super::{BucketEntry, BucketMember, MAX_BUCKET_MEMBERS, check_bucket_id};
use crate::error::{FormatError, corrupt, len64};
use crate::json_write::{push_string, push_u64};
use crate::manifest::MAX_FILES;

const WHAT: &str = "bucket manifest";

/// What starts a compact manifest.
pub(super) const MAGIC: &[u8; 4] = b"OSBM";
pub(super) const FORMAT: u8 = 2;
pub(super) const HEADER: usize = 10;

/// The most bytes of file names and locations one commit decodes to.
const MAX_COMMIT_TEXT_BYTES: usize = 8 << 20;

const LOCATION_RANGE: usize = 1;
const LENGTH_NONE: usize = 0;
const LENGTH_OF_RANGE: usize = 1;
const LENGTH_GIVEN: usize = 2;

/// The body a compact manifest's header describes, decompressed.
fn open(bytes: &[u8]) -> Result<std::borrow::Cow<'_, [u8]>, FormatError> {
    let (Some(&format), Some(&codec_id), Some(length)) =
        (bytes.get(4), bytes.get(5), bytes.get(6..HEADER))
    else {
        return Err(FormatError::Truncated {
            what: WHAT,
            needed: len64(HEADER),
            have: len64(bytes.len()),
        });
    };
    if format != FORMAT {
        return Err(FormatError::Unsupported {
            what: WHAT,
            version: u64::from(format),
        });
    }
    let raw_length = u32::from_be_bytes(length.try_into().unwrap_or_default());
    let raw_length = usize::try_from(raw_length).unwrap_or(usize::MAX);
    let packed = bytes.get(HEADER..).unwrap_or_default();
    codec::body(codec_id, packed, raw_length)
}

/// One member's entry from a compact manifest.
pub(super) fn member(bytes: &[u8], uuid: &str) -> Result<BucketEntry, FormatError> {
    read_body(&open(bytes)?, uuid)
}

/// Every member of a compact manifest, in uuid order of the index, with the
/// bucket and its term.
pub(super) fn all(bytes: &[u8]) -> Result<Whole, FormatError> {
    let body = open(bytes)?;
    let mut cursor = Cursor::new(&body);
    let bucket = cursor.string()?;
    check_bucket_id(&bucket)?;
    let term = cursor.vlong()?;
    let strings = Strings::read(&mut cursor, &body)?;
    let count = cursor.vint()?;
    if count > MAX_BUCKET_MEMBERS {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: len64(MAX_BUCKET_MEMBERS),
            actual: len64(count),
        });
    }
    if count > cursor.remaining() / MIN_MEMBER_BYTES {
        return Err(corrupt(WHAT, "more members than the body holds"));
    }
    // ALLOC: one uuid and entry length a member, at most MAX_BUCKET_MEMBERS.
    let mut index: Vec<(String, usize)> = Vec::with_capacity(count);
    for _ in 0..count {
        let uuid = cursor.string()?;
        index.push((uuid, cursor.vint()?));
    }
    // ALLOC: one entry a member, at most MAX_BUCKET_MEMBERS.
    let mut members = Vec::with_capacity(count);
    for (uuid, length) in index {
        let entry = cursor.take(length)?;
        members.push((uuid, read_member(entry, &strings)?));
    }
    if cursor.remaining() != 0 {
        return Err(corrupt(WHAT, "a body has bytes past its last member"));
    }
    Ok(Whole {
        bucket,
        term,
        members,
    })
}

/// The least a member takes of a body: an empty uuid and an entry length in
/// the index, and in the entry an empty index name, a version and a commit flag.
const MIN_MEMBER_BYTES: usize = 5;

/// A compact manifest read whole.
pub(super) struct Whole {
    pub(super) bucket: String,
    pub(super) term: u64,
    pub(super) members: Vec<(String, BucketMember)>,
}

fn read_body(body: &[u8], uuid: &str) -> Result<BucketEntry, FormatError> {
    let mut cursor = Cursor::new(body);
    let bucket = cursor.string()?;
    check_bucket_id(&bucket)?;
    let term = cursor.vlong()?;
    let strings = Strings::read(&mut cursor, body)?;
    let count = cursor.vint()?;
    // Each member takes two bytes at least: a uuid's count and an entry length.
    if count > cursor.remaining() / 2 {
        return Err(corrupt(WHAT, "more members than the body holds"));
    }
    // The first entry for a uuid is the one the Java returns.
    let mut found: Option<(usize, usize)> = None;
    let mut skipped = 0usize;
    for _ in 0..count {
        let at = cursor.skip_string()?;
        let length = cursor.vint()?;
        if found.is_none() && string_at(body, at)? == uuid {
            found = Some((skipped, length));
        }
        skipped = skipped
            .checked_add(length)
            .ok_or(corrupt(WHAT, "member entries are past the body"))?;
    }
    if skipped > cursor.remaining() {
        return Err(corrupt(WHAT, "member entries are past the body"));
    }
    let Some((before, length)) = found else {
        return Ok(BucketEntry {
            bucket,
            term,
            member: None,
        });
    };
    cursor.take(before)?;
    let entry = cursor.take(length)?;
    let member = read_member(entry, &strings)?;
    Ok(BucketEntry {
        bucket,
        term,
        member: Some(member),
    })
}

/// The body's strings, found but not decoded: a member's entry decodes the
/// few it uses.
struct Strings<'a> {
    body: &'a [u8],
    starts: Vec<u32>,
}

impl<'a> Strings<'a> {
    fn read(cursor: &mut Cursor<'_>, body: &'a [u8]) -> Result<Self, FormatError> {
        let count = cursor.vint()?;
        if count > cursor.remaining() {
            return Err(corrupt(WHAT, "more strings than the body holds"));
        }
        // ALLOC: one offset a string, at most one a body byte.
        let mut starts = Vec::with_capacity(count);
        for _ in 0..count {
            // The body is at most MAX_BODY_BYTES, which fits a u32.
            starts.push(u32::try_from(cursor.skip_string()?).unwrap_or(u32::MAX));
        }
        Ok(Self { body, starts })
    }

    fn get(&self, id: usize) -> Result<String, FormatError> {
        let at = *self
            .starts
            .get(id)
            .ok_or(corrupt(WHAT, "a string index is past the table"))?;
        string_at(self.body, usize::try_from(at).unwrap_or(usize::MAX))
    }
}

fn read_member(entry: &[u8], strings: &Strings<'_>) -> Result<BucketMember, FormatError> {
    let mut cursor = Cursor::new(entry);
    let index_name = cursor.string()?;
    let version = cursor.vlong()?;
    let commit = match cursor.byte()? {
        0 => None,
        1 => Some(read_commit(&mut cursor, strings)?),
        _ => return Err(corrupt(WHAT, "a member's commit flag is not a boolean")),
    };
    if cursor.remaining() != 0 {
        return Err(corrupt(WHAT, "a member's entry is not its recorded length"));
    }
    if commit.is_none() != (version == 0) {
        return Err(corrupt(WHAT, "a member's version and commit disagree"));
    }
    Ok(BucketMember {
        index_name,
        version,
        commit,
    })
}

/// A commit as the JSON `CommitManifest::parse` reads.
fn read_commit(cursor: &mut Cursor<'_>, strings: &Strings<'_>) -> Result<Vec<u8>, FormatError> {
    let term = cursor.vlong()?;
    let writer = match cursor.vint()? {
        0 => None,
        id => Some(strings.get(id.saturating_sub(1))?),
    };
    let wal_ordinal = signed(cursor.vlong()?)?.saturating_sub(1);
    let stage_sequence = signed(cursor.vlong()?)?.saturating_sub(1);
    let packs = read_packs(cursor, strings)?;
    let file_count = cursor.vint()?;
    if file_count > MAX_FILES {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: len64(MAX_FILES),
            actual: len64(file_count),
        });
    }
    // The decoded names and locations together stay under this: strings are
    // capped, but one referenced by every file would still multiply.
    let mut budget = MAX_COMMIT_TEXT_BYTES;
    // ALLOC: one file's text and its length, at most MAX_FILES.
    let mut files: Vec<(String, String, Option<u64>)> = Vec::with_capacity(file_count);
    for _ in 0..file_count {
        let name = [strings.get(cursor.vint()?)?, strings.get(cursor.vint()?)?].concat();
        let tag = cursor.vint()?;
        let (location_tag, length_tag) = (tag >> 2, tag & 3);
        let (location, ranged) = if location_tag == LOCATION_RANGE {
            let pack = packs
                .get(cursor.vint()?)
                .ok_or(corrupt(WHAT, "a file names a pack that is not listed"))?;
            let offset = cursor.vlong()?;
            let length = cursor.vlong()?;
            // ALLOC: one location's text, a few dozen bytes, at most MAX_FILES.
            (format!("{pack}#{offset}:{length}"), Some(length))
        } else {
            (strings.get(cursor.vint()?)?, None)
        };
        let length = match length_tag {
            LENGTH_NONE => None,
            LENGTH_OF_RANGE => Some(ranged.ok_or(corrupt(
                WHAT,
                "a file's length is its range's but it has no range",
            ))?),
            LENGTH_GIVEN => Some(cursor.vlong()?),
            _ => return Err(corrupt(WHAT, "a file's length tag is unknown")),
        };
        budget = budget
            .checked_sub(name.len().saturating_add(location.len()))
            .ok_or(corrupt(
                WHAT,
                "a commit's file names and locations are past 8 MiB",
            ))?;
        files.push((name, location, length));
    }
    skip_extra_lengths(cursor)?;
    let digest = cursor.vint()?;
    cursor.take(digest)?;
    Ok(commit_json(
        term,
        writer.as_deref(),
        &files,
        wal_ordinal,
        stage_sequence,
    ))
}

fn signed(value: u64) -> Result<i64, FormatError> {
    i64::try_from(value).map_err(|_| corrupt(WHAT, "a number is past a signed 64-bit integer"))
}

/// A member's packs: a string, or a node, epoch and ordinal, the last two
/// as deltas from the pack before.
fn read_packs(cursor: &mut Cursor<'_>, strings: &Strings<'_>) -> Result<Vec<String>, FormatError> {
    let count = cursor.vint()?;
    // A pack no file uses is no use, so a commit has as many as it has files.
    if count > cursor.remaining() || count > MAX_FILES {
        return Err(corrupt(WHAT, "more packs than the entry holds"));
    }
    // ALLOC: one pack path a pack, at most one a body byte.
    let mut packs = Vec::with_capacity(count);
    let (mut epoch, mut ordinal) = (0i64, 0i64);
    for _ in 0..count {
        let reference = cursor.vint()?;
        let name = strings.get(reference >> 1)?;
        if reference & 1 == 0 {
            packs.push(name);
            continue;
        }
        epoch = epoch
            .checked_add(cursor.zlong()?)
            .ok_or(corrupt(WHAT, "a pack's epoch overflows"))?;
        ordinal = ordinal
            .checked_add(cursor.zlong()?)
            .ok_or(corrupt(WHAT, "a pack's ordinal overflows"))?;
        if epoch < 0 || ordinal < 0 {
            return Err(corrupt(WHAT, "a pack's epoch or ordinal is negative"));
        }
        // ALLOC: one pack path, under a hundred bytes, at most one a body byte.
        packs.push(format!("@{name}/e={epoch:020}/{ordinal:020}"));
    }
    Ok(packs)
}

/// Lengths of files the commit does not name: read, bounded, and dropped.
fn skip_extra_lengths(cursor: &mut Cursor<'_>) -> Result<(), FormatError> {
    let count = cursor.vint()?;
    if count > cursor.remaining() {
        return Err(corrupt(WHAT, "more lengths than the entry holds"));
    }
    for _ in 0..count {
        cursor.skip_string()?;
        cursor.vlong()?;
    }
    Ok(())
}

fn commit_json(
    term: u64,
    writer: Option<&str>,
    files: &[(String, String, Option<u64>)],
    wal_ordinal: i64,
    stage_sequence: i64,
) -> Vec<u8> {
    // ALLOC: the commit's JSON, about a hundred bytes a file, once a read.
    let mut out: Vec<u8> = Vec::with_capacity(files.len().saturating_mul(96).saturating_add(64));
    out.extend_from_slice(b"{\"term\":");
    push_u64(&mut out, term);
    if let Some(writer) = writer {
        out.extend_from_slice(b",\"writer\":");
        push_string(&mut out, writer);
    }
    out.extend_from_slice(b",\"files\":{");
    for (i, (name, location, _)) in files.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        push_string(&mut out, name);
        out.push(b':');
        push_string(&mut out, location);
    }
    out.extend_from_slice(b"},\"lengths\":{");
    let mut first = true;
    for (name, _, length) in files {
        let Some(length) = length else { continue };
        if !first {
            out.push(b',');
        }
        first = false;
        push_string(&mut out, name);
        out.push(b':');
        push_u64(&mut out, *length);
    }
    out.push(b'}');
    for (name, value) in [("wal_ordinal", wal_ordinal), ("stage_seq", stage_sequence)] {
        if let Ok(value) = u64::try_from(value) {
            out.extend_from_slice(b",\"");
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(b"\":");
            push_u64(&mut out, value);
        }
    }
    out.push(b'}');
    out
}

#[cfg(test)]
mod tests {
    use super::super::compact_write::{push_string, push_vlong};
    use super::*;

    /// A body of `count` members, each as small as one can be, with the
    /// count the index claims.
    fn body_of(count: usize, claimed: usize) -> Vec<u8> {
        let mut body = Vec::new();
        push_string(&mut body, "b").unwrap();
        push_vlong(&mut body, 1);
        push_vlong(&mut body, 0);
        push_vlong(&mut body, claimed as u64);
        for _ in 0..count {
            push_string(&mut body, "").unwrap();
            push_vlong(&mut body, 3);
        }
        for _ in 0..count {
            push_string(&mut body, "").unwrap();
            push_vlong(&mut body, 0);
            body.push(0);
        }
        body
    }

    fn framed(body: &[u8]) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend([FORMAT, 0]);
        out.extend(u32::try_from(body.len()).unwrap().to_be_bytes());
        out.extend(body);
        out
    }

    #[test]
    fn members_as_small_as_they_can_be_are_all_read() {
        for count in [0, 1, 3, MAX_BUCKET_MEMBERS] {
            let whole = all(&framed(&body_of(count, count))).unwrap();
            assert_eq!(whole.members.len(), count);
        }
    }

    #[test]
    fn a_count_past_the_ceiling_is_too_large_and_past_the_body_is_corrupt() {
        let over = MAX_BUCKET_MEMBERS + 1;
        assert!(matches!(
            all(&framed(&body_of(over, over))),
            Err(FormatError::TooLarge { .. })
        ));
        // 1,000 members claimed, three present.
        assert!(matches!(
            all(&framed(&body_of(3, 1000))),
            Err(FormatError::Corrupt { .. })
        ));
    }
}
