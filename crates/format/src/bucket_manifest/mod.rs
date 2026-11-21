//! A bucket's manifest register: every member index's latest commit, in one
//! register `buckets/<id>/manifest`, read one member at a time.
//!
//! Ported from `BucketManifest` (`parse`, `parseMember`, `fromStream`),
//! `BucketStore` (`ROOT`, `MANIFEST`) and `BucketIds.validate` on
//! huyz0/OpenSearch `feature/serverlessshell`. The register's value is either
//! JSON (`{"bucket":…,"term":…,"members":{"<uuid>":{"index":…,"version":…,
//! "commit":{…}}}}`) or the compact binary form (`OSBM`, see [`compact`]),
//! which the header tells apart; both give a member's commit as the JSON a
//! shard's commit register holds, for `CommitManifest::parse`.

mod codec;
mod compact;
mod cursor;

use crate::error::{FormatError, corrupt};
use crate::json::{Reader, Token};
use crate::manifest::CommitManifest;

const WHAT: &str = "bucket manifest";

/// Where under a deployment's base the buckets are (`BucketStore.ROOT`).
pub const BUCKETS_PREFIX: &str = "buckets/";

/// The longest bucket id (`BucketIds.MAX_LENGTH`).
pub const MAX_BUCKET_ID_LENGTH: usize = 64;

/// One member of a bucket: an index and its latest commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketMember {
    /// The index's name, for operators.
    pub index_name: String,
    /// The entry's version: 0 with no commit, positive with one.
    pub version: u64,
    /// The latest commit as the JSON of a shard's commit register, or
    /// `None` before the member's first.
    pub commit: Option<Vec<u8>>,
}

/// What a bucket manifest says about one index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketEntry {
    /// The bucket the manifest is of.
    pub bucket: String,
    /// The bucket head term the owner wrote it at.
    pub term: u64,
    /// The index's entry, or `None` when it is not a member.
    pub member: Option<BucketMember>,
}

/// The key of a bucket's manifest register: `buckets/<id>/manifest`.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is not empty and does not end in
/// `/`, or a bucket id that is empty, over [`MAX_BUCKET_ID_LENGTH`], or holds
/// anything but `a-z`, `0-9`, `-` and `_`.
pub fn bucket_manifest_key(base: &str, bucket: &str) -> Result<String, FormatError> {
    if !base.is_empty() && !base.ends_with('/') {
        return Err(corrupt("blob key", "a base is empty or ends in a slash"));
    }
    check_bucket_id(bucket)?;
    // ALLOC: one key, the length of its parts.
    Ok([base, BUCKETS_PREFIX, bucket, "/", crate::keys::MANIFEST].concat())
}

pub(crate) fn check_bucket_id(id: &str) -> Result<(), FormatError> {
    let valid = !id.is_empty()
        && id.len() <= MAX_BUCKET_ID_LENGTH
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if valid {
        Ok(())
    } else {
        Err(corrupt(WHAT, "a bucket id is 1 to 64 of a-z, 0-9, - and _"))
    }
}

/// Reads the entry of the index with `index_uuid` from a bucket manifest
/// register's value (the bytes after the generation), in either encoding.
/// The other members are skipped, not decoded.
///
/// # Errors
///
/// [`FormatError::Truncated`], [`FormatError::Corrupt`],
/// [`FormatError::Unsupported`] (a format or codec this reader does not
/// know) or [`FormatError::TooLarge`] for bytes that are not a manifest.
pub fn parse_member(value: &[u8], index_uuid: &str) -> Result<BucketEntry, FormatError> {
    if value.starts_with(compact::MAGIC) {
        compact::member(value, index_uuid)
    } else {
        json_member(value, index_uuid)
    }
}

fn json_member(value: &[u8], index_uuid: &str) -> Result<BucketEntry, FormatError> {
    let mut json = Reader::new(value, WHAT);
    json.start_object("expected an object")?;
    let (mut bucket, mut term, mut member) = (None, None, None);
    while let Some(field) = json.next_field()? {
        if field == "members" && json.next_is_object() {
            json.start_object("members must be an object")?;
            while let Some(uuid) = json.next_field()? {
                if !json.next_is_object() {
                    return Err(json.error("a member must be an object"));
                }
                if uuid == index_uuid {
                    member = Some(json_entry(&mut json)?);
                } else {
                    json.skip_value()?;
                }
            }
            continue;
        }
        let token = json.expect_token()?;
        match (field.as_ref(), token) {
            ("bucket", Token::Str(text)) => bucket = Some(text.into_owned()),
            ("term", Token::Int(number)) => term = u64::try_from(number).ok(),
            (_, other) => json.skip_after(&other)?,
        }
    }
    json.finish()?;
    let (Some(bucket), Some(term)) = (bucket, term) else {
        return Err(corrupt(WHAT, "missing bucket or term"));
    };
    check_bucket_id(&bucket)?;
    Ok(BucketEntry {
        bucket,
        term,
        member,
    })
}

fn json_entry(json: &mut Reader<'_>) -> Result<BucketMember, FormatError> {
    json.start_object("a member must be an object")?;
    let (mut index_name, mut version, mut commit) = (None, None, None);
    while let Some(field) = json.next_field()? {
        if field == "commit" && json.next_is_object() {
            let raw = json.raw_object("a commit must be an object")?;
            CommitManifest::parse(raw)?;
            // ALLOC: one commit's JSON, once a read.
            commit = Some(raw.to_vec());
            continue;
        }
        let token = json.expect_token()?;
        match (field.as_ref(), token) {
            ("index", Token::Str(text)) => index_name = Some(text.into_owned()),
            ("version", Token::Int(number)) => {
                version = Some(
                    u64::try_from(number)
                        .map_err(|_| corrupt(WHAT, "a member's version is negative"))?,
                );
            }
            (_, other) => json.skip_after(&other)?,
        }
    }
    let Some(index_name) = index_name else {
        return Err(corrupt(WHAT, "a member names no index"));
    };
    let version = version.unwrap_or(u64::from(commit.is_some()));
    if commit.is_none() != (version == 0) {
        return Err(corrupt(WHAT, "a member's version and commit disagree"));
    }
    Ok(BucketMember {
        index_name,
        version,
        commit,
    })
}
