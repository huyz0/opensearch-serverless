//! An index descriptor whole: every field the shell writes, read and written
//! back byte for byte.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/cluster/IndexDescriptor.java`
//! (`toBytes`, `fromStream` and the constructor's checks), with the alias
//! test of `metadata/DescriptorStore.java` (`isAlias`). [`crate::DescriptorRecord`]
//! reads only what locates a shard's storage, for a reader; this is the
//! record a metadata writer swaps.
//!
//! Fields are written in `toBytes`'s order, each optional one only when it
//! differs from its default, so a record that never moved a version stays
//! the bytes older shells wrote. The settings and the unit layout are held as
//! the JSON object bytes they were read from and written back unchanged.

use std::borrow::Cow;

use crate::alias_record::{ALIAS_DISCRIMINATOR, AliasRecord};
use crate::descriptor::TOMBSTONE_PREFIX;
use crate::error::{FormatError, corrupt};
use crate::json::out::Out;
use crate::json::{Reader, Token};
use crate::tombstone::Tombstone;

const WHAT: &str = "index descriptor";

/// The most shards an index may have, as the Java's
/// `IndexDescriptor.MAX_SHARDS`: the shard list lives in the descriptor, so
/// nothing has to enumerate shards.
pub const MAX_SHARDS: u32 = 4096;

/// The settings of a record that has none: an empty object.
pub const EMPTY_SETTINGS: &[u8] = b"{}";

/// What the register under a name holds, read whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameRecord<'a> {
    /// An index.
    Index(IndexRecord<'a>),
    /// An alias or a data stream.
    Alias(AliasRecord<'a>),
    /// A deleted index's tombstone.
    Tombstone(Tombstone),
}

/// An index descriptor with every field the Java writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRecord<'a> {
    /// The index's name.
    pub name: Cow<'a, str>,
    /// The index's uuid.
    pub uuid: Cow<'a, str>,
    /// Shard slots, 1 to [`MAX_SHARDS`].
    pub number_of_shards: u32,
    /// The mapping's JSON source, decoded from its string.
    pub mapping: Option<Cow<'a, str>>,
    /// Moves on each mapping update; 1 until the first.
    pub mapping_version: i64,
    /// Moves on each settings update; 1 until the first.
    pub settings_version: i64,
    /// When the index was created, epoch milliseconds, or 0 when not recorded.
    pub created_at_millis: i64,
    /// The id of the OpenSearch version that created it, or 0.
    pub created_version_id: i32,
    /// The aliases believed to name this index: a hint, checked against each
    /// alias's own record by whoever reads it.
    pub aliased_by: Vec<Cow<'a, str>>,
    /// The shard mode's unit layout, as JSON object bytes.
    pub unit_layout: Option<&'a [u8]>,
    /// The bucket a bucketed one-shard index lives in.
    pub bucket: Option<Cow<'a, str>>,
    /// The index's settings, as JSON object bytes in `Settings.toXContent`'s
    /// form.
    pub settings: &'a [u8],
}

// One bit per known field, for refusing a repeat as Jackson's duplicate
// detection does.
const FIELDS: [&str; 12] = [
    "name",
    "uuid",
    "number_of_shards",
    "mapping",
    "mapping_version",
    "settings_version",
    "created_at_millis",
    "created_version_id",
    "aliased_by",
    "unit_layout",
    "bucket",
    "settings",
];

impl<'a> IndexRecord<'a> {
    /// A record with the given identity and every other field at its
    /// default: no mapping, versions 1, nothing recorded, empty settings.
    pub fn new(name: &'a str, uuid: &'a str, number_of_shards: u32) -> Self {
        Self {
            name: Cow::Borrowed(name),
            uuid: Cow::Borrowed(uuid),
            number_of_shards,
            mapping: None,
            mapping_version: 1,
            settings_version: 1,
            created_at_millis: 0,
            created_version_id: 0,
            // ALLOC: none; an empty Vec until an alias is pushed.
            aliased_by: Vec::new(),
            unit_layout: None,
            bucket: None,
            settings: EMPTY_SETTINGS,
        }
    }

    /// Checks what the Java's constructor checks.
    fn check(&self) -> Result<(), FormatError> {
        if !(1..=MAX_SHARDS).contains(&self.number_of_shards) {
            return Err(corrupt(WHAT, "number_of_shards must be 1 to 4096"));
        }
        if self.bucket.is_some() && (self.number_of_shards != 1 || self.unit_layout.is_some()) {
            return Err(corrupt(
                WHAT,
                "a bucket holds one-shard indices without a shard mode",
            ));
        }
        Ok(())
    }

    /// Writes the record as the Java's `toBytes` would, into the front of
    /// `buf`, and returns its length.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when it does not fit `buf`, with the length
    /// it needs; [`FormatError::Corrupt`] for a record the Java would refuse
    /// to build, or settings or a unit layout that are not one JSON object.
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        self.check()?;
        one_object(self.settings)?;
        if let Some(layout) = self.unit_layout {
            one_object(layout)?;
        }
        let mut out = Out::new(buf);
        out.start_object();
        out.string_field("name", &self.name);
        out.string_field("uuid", &self.uuid);
        out.int_field("number_of_shards", i64::from(self.number_of_shards));
        if let Some(mapping) = &self.mapping {
            out.string_field("mapping", mapping);
        }
        if self.mapping_version != 1 {
            out.int_field("mapping_version", self.mapping_version);
        }
        if self.settings_version != 1 {
            out.int_field("settings_version", self.settings_version);
        }
        if self.created_at_millis != 0 {
            out.int_field("created_at_millis", self.created_at_millis);
        }
        if self.created_version_id != 0 {
            out.int_field("created_version_id", i64::from(self.created_version_id));
        }
        if !self.aliased_by.is_empty() {
            out.start_array_field("aliased_by");
            for alias in &self.aliased_by {
                out.string(alias);
            }
            out.end_array();
        }
        if let Some(layout) = self.unit_layout {
            out.raw_field("unit_layout", layout);
        }
        if let Some(bucket) = &self.bucket {
            out.string_field("bucket", bucket);
        }
        out.raw_field("settings", self.settings);
        out.end_object();
        out.finish(WHAT)
    }
}

/// Refuses bytes that are not exactly one JSON object.
fn one_object(json: &[u8]) -> Result<(), FormatError> {
    let mut reader = Reader::new(json, WHAT);
    reader.raw_object("settings and a unit layout must be JSON objects")?;
    reader.finish()
}

impl<'a> NameRecord<'a> {
    /// Parses a name register's value: a tombstone, an alias or an index.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for malformed JSON, a malformed tombstone, or
    /// an index record the Java would refuse; this is also stricter than the
    /// Java on a value of the wrong JSON type, which it coerces.
    pub fn parse(value: &'a [u8]) -> Result<Self, FormatError> {
        if value.starts_with(TOMBSTONE_PREFIX) {
            return Tombstone::parse(value).map(Self::Tombstone);
        }
        if is_alias(value)? {
            return AliasRecord::parse(value).map(Self::Alias);
        }
        parse_index(value).map(Self::Index)
    }
}

impl NameRecord<'_> {
    /// The index a name register holds, or `None` for a tombstone or an
    /// alias, which are recognised and not decoded, as the Java's listing
    /// does: a malformed alias does not stop a walk over indexes.
    ///
    /// # Errors
    ///
    /// As [`NameRecord::parse`], for an index record.
    pub fn index_only(value: &[u8]) -> Result<Option<IndexRecord<'_>>, FormatError> {
        if value.starts_with(TOMBSTONE_PREFIX) || is_alias(value)? {
            return Ok(None);
        }
        parse_index(value).map(Some)
    }
}

/// Whether a record's root fields include the alias discriminator. Values
/// are skipped, not decoded, as the Java's `isAlias`.
fn is_alias(value: &[u8]) -> Result<bool, FormatError> {
    let mut json = Reader::new(value, WHAT);
    json.start_object("a descriptor or alias must be an object")?;
    let mut alias = false;
    while let Some(field) = json.next_field()? {
        alias |= field == ALIAS_DISCRIMINATOR;
        json.skip_value()?;
    }
    json.finish()?;
    Ok(alias)
}

fn parse_index(value: &[u8]) -> Result<IndexRecord<'_>, FormatError> {
    let mut json = Reader::new(value, WHAT);
    json.start_object("a descriptor must be an object")?;
    let mut name = None;
    let mut uuid = None;
    let mut shards = None;
    let mut record = IndexRecord::new("", "", 1);
    let mut seen = 0u16;
    while let Some(field) = json.next_field()? {
        let Some(index) = FIELDS.iter().position(|known| *known == field) else {
            // Forward compatibility: a newer node may write fields this one
            // does not know.
            json.skip_value()?;
            continue;
        };
        let bit = 1u16 << index;
        if seen & bit != 0 {
            return Err(json.error("a field is repeated"));
        }
        seen |= bit;
        match field.as_ref() {
            "aliased_by" => record.aliased_by = strings(&mut json)?,
            "unit_layout" => {
                record.unit_layout = Some(json.raw_object("unit_layout is an object")?)
            }
            "settings" => record.settings = json.raw_object("settings is an object")?,
            known => match (known, json.expect_token()?) {
                ("name", Token::Str(text)) => name = Some(text),
                ("uuid", Token::Str(text)) => uuid = Some(text),
                ("mapping", Token::Str(text)) => record.mapping = Some(text),
                ("bucket", Token::Str(text)) => record.bucket = Some(text),
                ("number_of_shards", Token::Int(n)) => shards = Some(n),
                ("mapping_version", Token::Int(n)) => record.mapping_version = n,
                ("settings_version", Token::Int(n)) => record.settings_version = n,
                ("created_at_millis", Token::Int(n)) => record.created_at_millis = n,
                ("created_version_id", Token::Int(n)) => {
                    record.created_version_id = i32::try_from(n)
                        .map_err(|_| json.error("created_version_id must be an int"))?;
                }
                _ => {
                    return Err(json.error(
                        "names, the mapping and the bucket are strings, counts and times integers",
                    ));
                }
            },
        }
    }
    json.finish()?;
    let (Some(name), Some(uuid), Some(shards)) = (name, uuid, shards) else {
        return Err(corrupt(WHAT, "missing a required field"));
    };
    record.name = name;
    record.uuid = uuid;
    record.number_of_shards = u32::try_from(shards).unwrap_or(0);
    record.check()?;
    Ok(record)
}

/// An array of strings.
fn strings<'a>(json: &mut Reader<'a>) -> Result<Vec<Cow<'a, str>>, FormatError> {
    if json.expect_token()? != Token::StartArray {
        return Err(json.error("aliased_by is an array"));
    }
    // ALLOC: one entry an alias, bounded by the register's length.
    let mut names = Vec::new();
    loop {
        match json.expect_token()? {
            Token::EndArray => return Ok(names),
            Token::Str(name) => names.push(name),
            _ => return Err(json.error("aliased_by holds strings")),
        }
    }
}
