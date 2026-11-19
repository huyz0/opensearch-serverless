//! An index descriptor: what an index is called, its uuid and shard count.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/cluster/IndexDescriptor.java`
//! (`fromStream`) and `metadata/DescriptorStore.java` (`get`, `isAlias`),
//! with the tombstone test from `metadata/Tombstones.java`. A descriptor is
//! the value of the register `indices/<name>`. The same key may instead hold
//! an alias (a root field `alias`) or a tombstone (a value starting
//! `{"tombstone":true`), which `DescriptorStore.get` reads as no index.
//!
//! What locates an index's storage is read: `name`, `uuid`,
//! `number_of_shards`, `bucket`, and whether a `unit_layout` is present; and
//! the mapping's span (a JSON string, always escaped), which a reader
//! decodes when it builds queries. Neither it nor the settings are decoded
//! or copied here.

use std::borrow::Cow;

use crate::error::{FormatError, corrupt};
use crate::json::{Reader, Token};
use crate::keys::ShardRef;

const WHAT: &str = "index descriptor";

/// What starts a tombstone's value.
pub const TOMBSTONE_PREFIX: &[u8] = b"{\"tombstone\":true";

/// What the register under an index name holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DescriptorRecord<'a> {
    /// An index.
    Index(IndexDescriptor<'a>),
    /// An alias: no index by this name.
    Alias,
    /// A deleted index whose register has not been removed yet.
    Tombstone,
}

/// The parts of an index descriptor that locate its storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDescriptor<'a> {
    /// The index's name.
    pub name: Cow<'a, str>,
    /// The index's uuid, part of every shard's storage path.
    pub uuid: Cow<'a, str>,
    /// How many shards it was created with.
    pub number_of_shards: u32,
    /// The bucket a bucketed index lives in, whose commits are in the
    /// bucket's manifest rather than in the shard's own register.
    pub bucket: Option<Cow<'a, str>>,
    /// Whether it was created with a shard mode's unit layout.
    pub has_unit_layout: bool,
    /// The mapping's JSON source (`{"properties":{..}}`) as it is written in
    /// the descriptor: the bytes between the string's quotes, escapes left
    /// in. `None` when the descriptor has none.
    pub mapping_escaped: Option<&'a [u8]>,
    /// The `settings` object's bytes as written; `None` when the descriptor
    /// has none or it is `null`.
    pub settings: Option<&'a [u8]>,
}

impl<'a> DescriptorRecord<'a> {
    /// Parses a descriptor register's value.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for malformed JSON, or a descriptor missing
    /// its name, uuid or a positive shard count.
    pub fn parse(value: &'a [u8]) -> Result<Self, FormatError> {
        if value.starts_with(TOMBSTONE_PREFIX) {
            return Ok(Self::Tombstone);
        }
        let mut json = Reader::new(value, WHAT);
        json.start_object("a descriptor or alias must be an object")?;
        let mut alias = false;
        let mut name = None;
        let mut uuid = None;
        let mut shards = None;
        let mut bucket = None;
        let mut has_unit_layout = false;
        let mut mapping = None;
        let mut settings = None;
        while let Some(field) = json.next_field()? {
            match field.as_ref() {
                "alias" => {
                    alias = true;
                    json.skip_value()?;
                }
                "unit_layout" => {
                    has_unit_layout = json.next_is_object();
                    json.skip_value()?;
                }
                "mapping" => mapping = json.raw_string()?,
                "settings" => {
                    settings = if json.next_is_object() {
                        Some(json.raw_object("settings must be an object")?)
                    } else {
                        match json.expect_token()? {
                            Token::Null => None,
                            _ => return Err(json.error("settings must be an object")),
                        }
                    };
                }
                "name" | "uuid" | "bucket" | "number_of_shards" => {
                    match (field.as_ref(), json.expect_token()?) {
                        ("name", Token::Str(text)) => name = Some(text),
                        ("uuid", Token::Str(text)) => uuid = Some(text),
                        ("bucket", Token::Str(text)) => bucket = Some(text),
                        ("number_of_shards", Token::Int(n)) => {
                            shards = Some(
                                u32::try_from(n)
                                    .ok()
                                    .filter(|s| (1..=i32::MAX as u32).contains(s))
                                    .ok_or(corrupt(
                                        WHAT,
                                        "number_of_shards must be a positive int",
                                    ))?,
                            );
                        }
                        _ => {
                            return Err(json.error(
                                "name, uuid and bucket are strings, number_of_shards an integer",
                            ));
                        }
                    }
                }
                // Anything newer: skipped, never decoded.
                _ => json.skip_value()?,
            }
        }
        json.finish()?;
        if alias {
            return Ok(Self::Alias);
        }
        match (name, uuid, shards) {
            (Some(name), Some(uuid), Some(number_of_shards)) => Ok(Self::Index(IndexDescriptor {
                name,
                uuid,
                number_of_shards,
                bucket,
                has_unit_layout,
                mapping_escaped: mapping,
                settings,
            })),
            _ => Err(corrupt(WHAT, "missing a required field")),
        }
    }
}

impl<'a> DescriptorRecord<'a> {
    /// The index this record describes.
    ///
    /// # Errors
    ///
    /// [`FormatError::NotFollowable`] for an alias or a tombstone, which
    /// `DescriptorStore.get` reads as no index by this name.
    pub fn into_index(self) -> Result<IndexDescriptor<'a>, FormatError> {
        match self {
            Self::Index(index) => Ok(index),
            Self::Alias => Err(FormatError::NotFollowable {
                what: WHAT,
                reason: "the name is an alias",
            }),
            Self::Tombstone => Err(FormatError::NotFollowable {
                what: WHAT,
                reason: "the index is deleted",
            }),
        }
    }
}

impl IndexDescriptor<'_> {
    /// One shard's storage, whose commit register a reader reads next.
    ///
    /// # Errors
    ///
    /// [`FormatError::NotFollowable`] for a bucketed index, whose commits are
    /// in its bucket's manifest, which this version does not read; and for a
    /// shard past `number_of_shards` of an index without a unit layout (one
    /// with a layout numbers its storage by unit, which is not checked here).
    pub fn shard(&self, shard: u32) -> Result<ShardRef<'_>, FormatError> {
        if self.bucket.is_some() {
            return Err(FormatError::NotFollowable {
                what: WHAT,
                reason: "a bucketed index's commits are in its bucket",
            });
        }
        if !self.has_unit_layout && shard >= self.number_of_shards {
            return Err(FormatError::NotFollowable {
                what: WHAT,
                reason: "no such shard",
            });
        }
        Ok(ShardRef {
            index: &self.name,
            uuid: &self.uuid,
            shard,
        })
    }
}
