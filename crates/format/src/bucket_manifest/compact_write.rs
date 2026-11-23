//! Writing the compact encoding of a bucket manifest: the string table, the
//! member index, then each member's entry.
//!
//! Ported from `CompactManifestFormat` (`body`, `writeCommit`, `encode`,
//! `encodeAuto`, `frame`, `Location.of`, `Pack.of`) on huyz0/OpenSearch
//! `feature/serverlessshell`. The bytes are the shell's for the same
//! manifest, except that the pruning digest is not carried and a file with no
//! recorded length but a range is written with the range's length.

use std::collections::BTreeMap;

use super::codec::{self, ManifestCodec};
use super::cursor::MAX_STRING_CHARS;
use super::{BucketManifest, BucketMember, compact};
use crate::error::{FormatError, corrupt, len64};
use crate::manifest::CommitManifest;

const WHAT: &str = "bucket manifest";

const LOCATION_PREFIX: u64 = 0;
const LOCATION_RANGE: u64 = 1;
const LOCATION_RAW: u64 = 2;
const LENGTH_NONE: u64 = 0;
const LENGTH_OF_RANGE: u64 = 1;
const LENGTH_GIVEN: u64 = 2;

/// Below this many body bytes `auto` writes uncompressed.
const AUTO_MIN_BYTES: usize = 32 * 1024;

pub(super) fn encode(
    manifest: &BucketManifest,
    codec: ManifestCodec,
    only_if_it_pays: bool,
) -> Result<Vec<u8>, FormatError> {
    let raw = body(manifest)?;
    codec::check_body_length(raw.len())?;
    let (codec, packed) =
        if codec == ManifestCodec::None || (only_if_it_pays && raw.len() < AUTO_MIN_BYTES) {
            (ManifestCodec::None, None)
        } else {
            let packed = codec::compress(codec, &raw)?;
            // A compressed body is kept only if it is at most four fifths of the plain one.
            if only_if_it_pays && packed.len().saturating_mul(5) > raw.len().saturating_mul(4) {
                (ManifestCodec::None, None)
            } else {
                (codec, Some(packed))
            }
        };
    let packed = packed.as_deref().unwrap_or(&raw);
    // ALLOC: the register's bytes, the header and the body, once a swap.
    let mut out = Vec::with_capacity(packed.len().saturating_add(compact::HEADER));
    out.extend_from_slice(compact::MAGIC);
    out.push(compact::FORMAT);
    out.push(codec.id());
    let length = u32::try_from(raw.len()).map_err(|_| corrupt(WHAT, "a body is past 4 GiB"))?;
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(packed);
    Ok(out)
}

/// Each repeated string once, in the order the entries first use them.
#[derive(Default)]
struct Strings {
    ids: BTreeMap<String, u64>,
    table: Vec<String>,
}

impl Strings {
    fn id(&mut self, text: &str) -> Result<u64, FormatError> {
        if let Some(id) = self.ids.get(text) {
            return Ok(*id);
        }
        check_length(text)?;
        let id = len64(self.table.len());
        // ALLOC: one string twice (table and index), at most 1,024 characters each, a string a commit part.
        self.table.push(text.to_owned());
        self.ids.insert(text.to_owned(), id);
        Ok(id)
    }
}

fn check_length(text: &str) -> Result<(), FormatError> {
    if text
        .encode_utf16()
        .take(MAX_STRING_CHARS.saturating_add(1))
        .count()
        > MAX_STRING_CHARS
    {
        return Err(corrupt(WHAT, "a string is past 1024 characters"));
    }
    Ok(())
}

fn body(manifest: &BucketManifest) -> Result<Vec<u8>, FormatError> {
    let mut strings = Strings::default();
    // ALLOC: one entry a member, at most MAX_BUCKET_MEMBERS.
    let mut entries: Vec<Vec<u8>> = Vec::with_capacity(manifest.len());
    for (_, member) in manifest.members() {
        entries.push(entry(member, &mut strings)?);
    }
    // ALLOC: the body, once a swap; the strings and entries it is made of are already in memory.
    let mut out = Vec::new();
    push_string(&mut out, manifest.bucket())?;
    push_vlong(&mut out, manifest.term());
    push_vlong(&mut out, len64(strings.table.len()));
    for text in &strings.table {
        push_string(&mut out, text)?;
    }
    push_vlong(&mut out, len64(entries.len()));
    for ((uuid, _), entry) in manifest.members().zip(&entries) {
        push_string(&mut out, uuid)?;
        push_vlong(&mut out, len64(entry.len()));
    }
    for entry in entries {
        out.extend_from_slice(&entry);
    }
    Ok(out)
}

fn entry(member: &BucketMember, strings: &mut Strings) -> Result<Vec<u8>, FormatError> {
    // ALLOC: one member's entry, once a swap.
    let mut out = Vec::new();
    push_string(&mut out, &member.index_name)?;
    push_vlong(&mut out, member.version);
    out.push(u8::from(member.commit.is_some()));
    if let Some(json) = &member.commit {
        commit(json, strings, &mut out)?;
    }
    Ok(out)
}

/// A file's location taken apart: only a canonical range is.
enum Location<'a> {
    Prefix(&'a str),
    Range {
        prefix: &'a str,
        offset: u64,
        length: u64,
    },
    Raw(&'a str),
}

impl Location<'_> {
    fn tag(&self) -> u64 {
        match self {
            Location::Prefix(_) => LOCATION_PREFIX,
            Location::Range { .. } => LOCATION_RANGE,
            Location::Raw(_) => LOCATION_RAW,
        }
    }
}

fn locate(encoded: &str) -> Location<'_> {
    let Some(hash) = encoded.rfind('#') else {
        return if encoded.contains(':') {
            Location::Raw(encoded)
        } else {
            Location::Prefix(encoded)
        };
    };
    let (prefix, rest) = encoded.split_at(hash);
    let Some((offset_text, length_text)) = rest.get(1..).and_then(|r| r.split_once(':')) else {
        return Location::Raw(encoded);
    };
    // Only a canonical range is taken apart, so writing it back gives the same string.
    match (canonical(offset_text), canonical(length_text)) {
        (Some(offset), Some(length)) => Location::Range {
            prefix,
            offset,
            length,
        },
        _ => Location::Raw(encoded),
    }
}

/// A non-negative decimal with no sign and no leading zero, in an `i64`.
fn canonical(text: &str) -> Option<u64> {
    let plain = !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text.len() == 1 || !text.starts_with('0'));
    if !plain {
        return None;
    }
    text.parse::<i64>().ok().map(i64::unsigned_abs)
}

/// A node pack taken apart, kept only if putting it back gives the same text.
fn node_pack(prefix: &str) -> Option<(&str, i64, i64)> {
    let rest = prefix.strip_prefix('@')?;
    let (node, rest) = rest.split_once('/')?;
    let (epoch, ordinal) = rest.strip_prefix("e=")?.split_once('/')?;
    if node.is_empty() {
        return None;
    }
    let (epoch, ordinal) = (epoch.parse::<i64>().ok()?, ordinal.parse::<i64>().ok()?);
    if epoch < 0 || ordinal < 0 {
        return None;
    }
    // ALLOC: one pack path to compare, under a hundred bytes.
    let back = format!("@{node}/e={epoch:020}/{ordinal:020}");
    (back == prefix).then_some((node, epoch, ordinal))
}

fn commit(json: &[u8], strings: &mut Strings, out: &mut Vec<u8>) -> Result<(), FormatError> {
    let parsed = CommitManifest::parse(json)?;
    push_vlong(out, parsed.term);
    push_vlong(
        out,
        match parsed.writer.as_deref() {
            Some(writer) => strings.id(writer)?.saturating_add(1),
            None => 0,
        },
    );
    // Absent is -1 and written as 0, so a value is written one up.
    push_vlong(
        out,
        u64::try_from(parsed.wal_ordinal.saturating_add(1)).unwrap_or(0),
    );
    push_vlong(
        out,
        u64::try_from(parsed.stage_sequence.saturating_add(1)).unwrap_or(0),
    );
    // ALLOC: one location text a file, at most MAX_FILES.
    let encoded: Vec<String> = parsed.files().iter().map(|f| f.location.encode()).collect();
    // ALLOC: one reference a file, at most MAX_FILES, sorted by name.
    let mut files: Vec<(&str, Location<'_>, Option<u64>)> = Vec::with_capacity(encoded.len());
    for (file, text) in parsed.files().iter().zip(&encoded) {
        files.push((file.name.as_ref(), locate(text), file.length));
    }
    files.sort_unstable_by(|a, b| a.0.cmp(b.0));
    // The member's packs once each, in the order its files first use them.
    // ALLOC: one entry a pack, at most one a file.
    let mut pack_ids: BTreeMap<&str, u64> = BTreeMap::new();
    // ALLOC: one pack path a pack, at most one a file.
    let mut packs: Vec<&str> = Vec::new();
    for (_, location, _) in &files {
        if let Location::Range { prefix, .. } = location
            && !pack_ids.contains_key(prefix)
        {
            pack_ids.insert(prefix, len64(packs.len()));
            packs.push(prefix);
        }
    }
    push_vlong(out, len64(packs.len()));
    let (mut epoch, mut ordinal) = (0i64, 0i64);
    for pack in &packs {
        if let Some((node, e, o)) = node_pack(pack) {
            push_vlong(out, strings.id(node)? << 1 | 1);
            push_zlong(
                out,
                e.checked_sub(epoch)
                    .ok_or(corrupt(WHAT, "a pack's epoch overflows"))?,
            );
            push_zlong(
                out,
                o.checked_sub(ordinal)
                    .ok_or(corrupt(WHAT, "a pack's ordinal overflows"))?,
            );
            (epoch, ordinal) = (e, o);
        } else {
            push_vlong(out, strings.id(pack)? << 1);
        }
    }
    push_vlong(out, len64(files.len()));
    for (name, location, length) in &files {
        let (stem, extension) = name.split_at(name.find('.').unwrap_or(name.len()));
        push_vlong(out, strings.id(stem)?);
        push_vlong(out, strings.id(extension)?);
        let length_tag = match (length, location) {
            (None, _) => LENGTH_NONE,
            (Some(l), Location::Range { length: range, .. }) if l == range => LENGTH_OF_RANGE,
            (Some(_), _) => LENGTH_GIVEN,
        };
        push_vlong(out, location.tag() << 2 | length_tag);
        match location {
            Location::Range {
                prefix,
                offset,
                length,
            } => {
                push_vlong(out, pack_ids.get(prefix).copied().unwrap_or(0));
                push_vlong(out, *offset);
                push_vlong(out, *length);
            }
            Location::Prefix(text) | Location::Raw(text) => push_vlong(out, strings.id(text)?),
        }
        if let (LENGTH_GIVEN, Some(length)) = (length_tag, length) {
            push_vlong(out, *length);
        }
    }
    // No lengths for files the commit does not name, and no pruning digest.
    push_vlong(out, 0);
    push_vlong(out, 0);
    Ok(())
}

pub(super) fn push_vlong(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(u8::try_from(value & 0x7F).unwrap_or(0) | 0x80);
        value >>= 7;
    }
    out.push(u8::try_from(value).unwrap_or(0));
}

fn push_zlong(out: &mut Vec<u8>, value: i64) {
    let bits = u64::from_ne_bytes(value.to_ne_bytes());
    let sign = u64::from_ne_bytes((value >> 63).to_ne_bytes());
    push_vlong(out, (bits << 1) ^ sign);
}

/// A `writeString`: the character count, then each UTF-16 unit as one, two
/// or three bytes.
pub(super) fn push_string(out: &mut Vec<u8>, text: &str) -> Result<(), FormatError> {
    check_length(text)?;
    push_vlong(out, len64(text.encode_utf16().count()));
    for unit in text.encode_utf16() {
        if unit <= 0x7F {
            out.push(u8::try_from(unit).unwrap_or(0));
        } else if unit > 0x7FF {
            out.push(0xE0 | u8::try_from(unit >> 12).unwrap_or(0));
            out.push(0x80 | u8::try_from((unit >> 6) & 0x3F).unwrap_or(0));
            out.push(0x80 | u8::try_from(unit & 0x3F).unwrap_or(0));
        } else {
            out.push(0xC0 | u8::try_from(unit >> 6).unwrap_or(0));
            out.push(0x80 | u8::try_from(unit & 0x3F).unwrap_or(0));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_canonical_range_is_taken_apart() {
        assert!(matches!(
            locate("p#5:11"),
            Location::Range {
                prefix: "p",
                offset: 5,
                length: 11
            }
        ));
        assert!(matches!(locate("t=3"), Location::Prefix("t=3")));
        for raw in [
            "x:y", "p#x:y", "p#05:7", "p#5:07", "p#-1:2", "p#1", "p#+1:2", "p#1:2:3",
        ] {
            assert!(matches!(locate(raw), Location::Raw(t) if t == raw), "{raw}");
        }
        // The last '#' splits, as the shell's lastIndexOf does.
        assert!(matches!(
            locate("a#b#1:2"),
            Location::Range {
                prefix: "a#b",
                offset: 1,
                length: 2
            }
        ));
        assert!(matches!(
            locate("p#0:0"),
            Location::Range {
                offset: 0,
                length: 0,
                ..
            }
        ));
    }

    #[test]
    fn a_node_pack_is_taken_apart_only_when_it_puts_back_the_same() {
        let pack = format!("@nodeA/e={:020}/{:020}", 1_794_710_559_389_i64, 4);
        assert_eq!(node_pack(&pack), Some(("nodeA", 1_794_710_559_389, 4)));
        assert_eq!(node_pack("@n/e=1/2"), None, "not padded");
        assert_eq!(node_pack("t=3/s1"), None);
        assert_eq!(
            node_pack(&format!("@/e={:020}/{:020}", 1, 1)),
            None,
            "no node"
        );
        assert_eq!(node_pack(&format!("@n/x={:020}/{:020}", 1, 1)), None);
        assert_eq!(
            node_pack(&format!("@n/e={:020}/{:020}", -1, 1)),
            None,
            "negative"
        );
        assert_eq!(
            node_pack(&format!("@n/e={:020}/{:020}", 0, 0)),
            Some(("n", 0, 0))
        );
    }

    #[test]
    fn variable_length_integers_are_the_shells() {
        let mut out = Vec::new();
        push_vlong(&mut out, 0);
        push_vlong(&mut out, 127);
        push_vlong(&mut out, 128);
        push_vlong(&mut out, u64::MAX);
        assert_eq!(
            out,
            [
                0, 127, 0x80, 1, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 1
            ]
        );
        let mut z = Vec::new();
        for v in [0i64, -1, 1, -2, 63, -64, 64, i64::MAX, i64::MIN] {
            push_zlong(&mut z, v);
        }
        assert_eq!(
            z,
            [
                0, 1, 2, 3, 126, 127, 0x80, 1, 0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
                0xFF, 1, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 1
            ]
        );
    }

    #[test]
    fn a_string_is_its_unit_count_then_one_two_or_three_bytes_a_unit() {
        let bytes = |text: &str| {
            let mut out = Vec::new();
            push_string(&mut out, text).unwrap();
            out
        };
        assert_eq!(bytes("a"), [1, 0x61]);
        assert_eq!(bytes("\u{7f}"), [1, 0x7F]);
        assert_eq!(bytes("\u{80}"), [1, 0xC2, 0x80]);
        assert_eq!(bytes("\u{7ff}"), [1, 0xDF, 0xBF]);
        assert_eq!(bytes("\u{800}"), [1, 0xE0, 0xA0, 0x80]);
        assert_eq!(bytes("\u{ffff}"), [1, 0xEF, 0xBF, 0xBF]);
        // A character past the plane is two units of three bytes each.
        assert_eq!(bytes("\u{1f600}"), [2, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80]);
    }

    #[test]
    fn a_body_past_the_reader_cap_is_refused() {
        assert!(codec::check_body_length(64 << 20).is_ok());
        assert!(matches!(
            codec::check_body_length((64 << 20) + 1),
            Err(FormatError::TooLarge { .. })
        ));
    }
}
