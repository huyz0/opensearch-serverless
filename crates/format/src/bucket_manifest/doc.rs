//! A whole bucket manifest as a value: built, edited one member at a time,
//! and written as the JSON `BucketManifest.toBytes()` writes.
//!
//! Ported from `BucketManifest` (`Member`, `empty`, `withMember`,
//! `withoutMember`, `atTerm`, `dataBytes`, `toBytes()`, `fromStream`) on
//! huyz0/OpenSearch `feature/serverlessshell`. A member's commit is kept as
//! the JSON bytes of a shard's commit register, checked by
//! `CommitManifest::parse` on the way in.

use std::collections::BTreeMap;

use super::{WHAT, check_bucket_id, json_entry};
use crate::error::{FormatError, corrupt, len64};
use crate::json::{Reader, Token};
use crate::json_write::{push_string, push_u64};
use crate::manifest::CommitManifest;

/// The most members one bucket manifest holds (the shell's bucket cap).
pub const MAX_BUCKET_MEMBERS: usize = 1_024;

impl super::BucketMember {
    /// A member at its first version: 0 with no commit, 1 with one.
    ///
    /// # Errors
    ///
    /// [`FormatError`] when the commit is not a commit manifest.
    pub fn first(index_name: &str, commit: Option<Vec<u8>>) -> Result<Self, FormatError> {
        if let Some(commit) = &commit {
            CommitManifest::parse(commit)?;
        }
        let version = u64::from(commit.is_some());
        Ok(Self {
            // ALLOC: one index name, once a member entry.
            index_name: index_name.to_owned(),
            version,
            commit,
        })
    }

    /// This member with a new commit, at the next version.
    ///
    /// # Errors
    ///
    /// [`FormatError`] when the commit is not a commit manifest.
    pub fn advance(&self, commit: Vec<u8>) -> Result<Self, FormatError> {
        CommitManifest::parse(&commit)?;
        Ok(Self {
            index_name: self.index_name.clone(),
            version: self.version.saturating_add(1),
            commit: Some(commit),
        })
    }
}

/// A bucket's manifest: the bucket, the head term it was written at, and
/// every member by index uuid, in uuid order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketManifest {
    bucket: String,
    term: u64,
    members: BTreeMap<String, super::BucketMember>,
}

impl BucketManifest {
    /// An empty manifest for a new bucket, at term zero.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a bucket id that breaks the id rule.
    pub fn empty(bucket: &str) -> Result<Self, FormatError> {
        check_bucket_id(bucket)?;
        Ok(Self {
            // ALLOC: the bucket id, at most 64 bytes.
            bucket: bucket.to_owned(),
            term: 0,
            members: BTreeMap::new(),
        })
    }

    /// The bucket's id.
    #[must_use]
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// The bucket head term this was written at.
    #[must_use]
    pub fn term(&self) -> u64 {
        self.term
    }

    /// How many members the bucket holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the bucket holds no member.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// One member's entry.
    #[must_use]
    pub fn member(&self, index_uuid: &str) -> Option<&super::BucketMember> {
        self.members.get(index_uuid)
    }

    /// Every member by uuid, in uuid order.
    pub fn members(&self) -> impl Iterator<Item = (&str, &super::BucketMember)> {
        self.members.iter().map(|(uuid, m)| (uuid.as_str(), m))
    }

    /// This manifest with a member added or its entry replaced.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for an empty uuid; [`FormatError::TooLarge`]
    /// when adding would pass [`MAX_BUCKET_MEMBERS`].
    pub fn with_member(
        mut self,
        index_uuid: &str,
        member: super::BucketMember,
    ) -> Result<Self, FormatError> {
        if index_uuid.is_empty() {
            return Err(corrupt(WHAT, "a member's uuid is empty"));
        }
        if !self.members.contains_key(index_uuid) {
            check_count(self.members.len().saturating_add(1))?;
        }
        // ALLOC: one entry a member, at most MAX_BUCKET_MEMBERS (checked above).
        self.members.insert(index_uuid.to_owned(), member);
        Ok(self)
    }

    /// This manifest without one member; unchanged when it is not a member.
    #[must_use]
    pub fn without_member(mut self, index_uuid: &str) -> Self {
        self.members.remove(index_uuid);
        self
    }

    /// This manifest written at a bucket head term.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a term past a signed 64-bit integer.
    pub fn at_term(mut self, term: u64) -> Result<Self, FormatError> {
        check_term(term)?;
        self.term = term;
        Ok(self)
    }

    /// The sum of the recorded file lengths of every member's latest
    /// commit: what fills a bucket.
    ///
    /// # Errors
    ///
    /// [`FormatError`] when a member's commit no longer parses.
    pub fn data_bytes(&self) -> Result<u64, FormatError> {
        let mut total = 0u64;
        for member in self.members.values() {
            if let Some(commit) = &member.commit {
                for file in CommitManifest::parse(commit)?.files() {
                    total = total.saturating_add(file.length.unwrap_or(0));
                }
            }
        }
        Ok(total)
    }

    /// Writes the JSON encoding: `bucket`, `term`, then `members` by uuid,
    /// each with `index`, `version` when positive and `commit` when there is
    /// one.
    ///
    /// # Errors
    ///
    /// [`FormatError`] when a member's commit no longer parses.
    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        let commits = self
            .members
            .values()
            .filter_map(|m| m.commit.as_ref())
            .map(Vec::len)
            .fold(0usize, usize::saturating_add);
        // ALLOC: the manifest, its commits plus about a hundred bytes a member, once a swap.
        let mut out = Vec::with_capacity(
            commits
                .saturating_add(self.members.len().saturating_mul(128))
                .saturating_add(64),
        );
        out.extend_from_slice(b"{\"bucket\":");
        push_string(&mut out, &self.bucket);
        out.extend_from_slice(b",\"term\":");
        push_u64(&mut out, self.term);
        out.extend_from_slice(b",\"members\":{");
        for (i, (uuid, member)) in self.members.iter().enumerate() {
            if i > 0 {
                out.push(b',');
            }
            push_string(&mut out, uuid);
            out.extend_from_slice(b":{\"index\":");
            push_string(&mut out, &member.index_name);
            if member.version > 0 {
                out.extend_from_slice(b",\"version\":");
                push_u64(&mut out, member.version);
            }
            if let Some(commit) = &member.commit {
                CommitManifest::parse(commit)?;
                out.extend_from_slice(b",\"commit\":");
                out.extend_from_slice(commit);
            }
            out.push(b'}');
        }
        out.extend_from_slice(b"}}");
        Ok(out)
    }

    /// Writes the compact encoding (`OSBM`) with `codec`, whatever it saves.
    ///
    /// # Errors
    ///
    /// [`FormatError`] for a commit that no longer parses, a string past
    /// 1,024 characters or a body past 64 MiB.
    pub fn encode_compact(&self, codec: super::ManifestCodec) -> Result<Vec<u8>, FormatError> {
        super::compact_write::encode(self, codec, false)
    }

    /// Writes the compact encoding, compressed with `codec` only when that
    /// pays: from 32 KiB of body, and only when the result is at most four
    /// fifths of it. Otherwise the body is written as it is.
    ///
    /// # Errors
    ///
    /// As [`encode_compact`](Self::encode_compact).
    pub fn encode_compact_auto(&self, codec: super::ManifestCodec) -> Result<Vec<u8>, FormatError> {
        super::compact_write::encode(self, codec, true)
    }

    /// Parses a register's value whole, in either encoding.
    ///
    /// # Errors
    ///
    /// As [`parse_member`](super::parse_member), plus
    /// [`FormatError::TooLarge`] past [`MAX_BUCKET_MEMBERS`].
    pub fn parse(value: &[u8]) -> Result<Self, FormatError> {
        if value.starts_with(super::compact::MAGIC) {
            let whole = super::compact::all(value)?;
            check_term(whole.term)?;
            let mut manifest = Self::empty(&whole.bucket)?.at_term(whole.term)?;
            for (uuid, member) in whole.members {
                if let Some(commit) = &member.commit {
                    CommitManifest::parse(commit)?;
                }
                manifest = manifest.with_member(&uuid, member)?;
            }
            return Ok(manifest);
        }
        let mut json = Reader::new(value, WHAT);
        json.start_object("expected an object")?;
        let (mut bucket, mut term) = (None, None);
        // ALLOC: one entry a member, at most MAX_BUCKET_MEMBERS (checked before each insert).
        let mut members = BTreeMap::new();
        while let Some(field) = json.next_field()? {
            if field == "members" && json.next_is_object() {
                json.start_object("members must be an object")?;
                while let Some(uuid) = json.next_field()? {
                    if !json.next_is_object() {
                        return Err(json.error("a member must be an object"));
                    }
                    let uuid = uuid.into_owned();
                    let entry = json_entry(&mut json)?;
                    if !members.contains_key(&uuid) {
                        check_count(members.len().saturating_add(1))?;
                    }
                    // ALLOC: one entry a member, at most MAX_BUCKET_MEMBERS (checked above).
                    members.insert(uuid, entry);
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
        Ok(Self {
            bucket,
            term,
            members,
        })
    }
}

fn check_term(term: u64) -> Result<(), FormatError> {
    if i64::try_from(term).is_err() {
        return Err(corrupt(WHAT, "the term is past a signed 64-bit integer"));
    }
    Ok(())
}

fn check_count(count: usize) -> Result<(), FormatError> {
    if count > MAX_BUCKET_MEMBERS {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: len64(MAX_BUCKET_MEMBERS),
            actual: len64(count),
        });
    }
    Ok(())
}
