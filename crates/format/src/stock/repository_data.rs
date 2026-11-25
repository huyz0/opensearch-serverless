//! `RepositoryData`: the `index-N` blob that lists a stock repository's
//! snapshots and indices.
//!
//! Ported from `RepositoryData.snapshotsFromXContent`. Plain JSON, not
//! enveloped. Every id that later becomes part of a blob key is checked
//! against [`is_blob_name`] here, so no key is built from a hostile name.

use std::borrow::Cow;

use crate::error::FormatError;
use crate::json::{Reader, Token};

const WHAT: &str = "stock repository index blob";

/// The most entries read across every list of one `index-N`.
const MAX_ENTRIES: usize = 1_000_000;
/// The longest index name: OpenSearch's own limit.
const MAX_INDEX_NAME_BYTES: usize = 255;
/// The longest id read into a blob key.
const MAX_BLOB_NAME_BYTES: usize = 128;

/// Whether `s` may be part of a blob key: 1 to 128 of `[A-Za-z0-9._-]`, and
/// not a path step.
pub fn is_blob_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_BLOB_NAME_BYTES
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// One snapshot of the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockSnapshot {
    /// The name a restore asks for.
    pub name: String,
    /// The uuid its blobs are named by.
    pub uuid: String,
    /// The snapshot state's code (1 is `SUCCESS`), when written.
    pub state: Option<i64>,
    /// The version of OpenSearch that took it, when written.
    pub version: Option<String>,
    /// Index id to the generation of that index's metadata in this snapshot.
    metadata: Vec<(String, String)>,
}

/// One index the repository holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockIndex {
    /// The index's name when it was snapshotted.
    pub name: String,
    /// The id its blobs are under (`indices/<id>/`).
    pub id: String,
    /// How shard blobs are laid out: 0 is `indices/<id>/<shard>/`.
    pub shard_path_type: i64,
    /// The uuids of the snapshots that hold it.
    pub snapshots: Vec<String>,
    /// The current generation of each shard's blob list, when written.
    pub shard_generations: Vec<Option<String>>,
}

/// A parsed `index-N`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepositoryData {
    /// The snapshots, in file order.
    pub snapshots: Vec<StockSnapshot>,
    /// The indices, in file order.
    pub indices: Vec<StockIndex>,
    /// Metadata generation to the identifier its blob is named by.
    identifiers: Vec<(String, String)>,
}

impl RepositoryData {
    /// Reads `bytes`, the whole of an `index-N`.
    pub fn parse(bytes: &[u8]) -> Result<Self, FormatError> {
        Self::parse_with(bytes, MAX_ENTRIES)
    }

    /// Reads `bytes`, refusing more than `max_entries` entries in all.
    pub fn parse_with(bytes: &[u8], max_entries: usize) -> Result<Self, FormatError> {
        let mut json = Reader::new(bytes, WHAT);
        json.start_object("an index blob is an object")?;
        let mut data = Self::default();
        let mut left = max_entries;
        while let Some(field) = json.next_field()? {
            match field.as_ref() {
                "snapshots" => data.snapshots = snapshots(&mut json, &mut left)?,
                "indices" => data.indices = indices(&mut json, &mut left)?,
                "index_metadata_identifiers" => data.identifiers = pairs(&mut json, &mut left)?,
                "min_version" => json.skip_value()?,
                _ => return Err(json.error("an unknown field")),
            }
        }
        json.finish()?;
        Ok(data)
    }

    /// The snapshot named `name`.
    pub fn snapshot(&self, name: &str) -> Option<&StockSnapshot> {
        self.snapshots.iter().find(|s| s.name == name)
    }

    /// The indices `snapshot` holds, in file order.
    pub fn indices_of<'a>(
        &'a self,
        snapshot: &'a StockSnapshot,
    ) -> impl Iterator<Item = &'a StockIndex> {
        self.indices
            .iter()
            .filter(|i| i.snapshots.contains(&snapshot.uuid))
    }

    /// The name, without its `meta-` prefix and `.dat` suffix, of the blob
    /// holding `index`'s metadata in `snapshot`: the identifier of its
    /// generation, or the snapshot's uuid for a repository from before
    /// generations. `None` when the generation has no identifier.
    pub fn metadata_blob_id<'a>(
        &'a self,
        snapshot: &'a StockSnapshot,
        index: &StockIndex,
    ) -> Option<&'a str> {
        match snapshot.metadata.iter().find(|(id, _)| *id == index.id) {
            None => Some(snapshot.uuid.as_str()),
            Some((_, generation)) => self
                .identifiers
                .iter()
                .find(|(g, _)| g == generation)
                .map(|(_, identifier)| identifier.as_str()),
        }
    }
}

/// Spends one entry of the budget.
fn spend(left: &mut usize, json: &Reader<'_>) -> Result<(), FormatError> {
    *left = left
        .checked_sub(1)
        .ok_or_else(|| json.error("more entries than a reader accepts"))?;
    Ok(())
}

/// A string value.
fn string<'a>(json: &mut Reader<'a>, reason: &'static str) -> Result<Cow<'a, str>, FormatError> {
    match json.expect_token()? {
        Token::Str(s) => Ok(s),
        _ => Err(json.error(reason)),
    }
}

/// A string value that becomes part of a blob key.
fn blob_name(json: &mut Reader<'_>, reason: &'static str) -> Result<String, FormatError> {
    let s = string(json, reason)?;
    check_name(json, &s)?;
    // ALLOC: one id of at most 128 bytes, for each entry of the budget.
    Ok(s.into_owned())
}

fn check_name(json: &Reader<'_>, s: &str) -> Result<(), FormatError> {
    if is_blob_name(s) {
        Ok(())
    } else {
        Err(json.error("a name that is not safe in a blob key"))
    }
}

/// An object whose values are blob names, as pairs.
fn pairs(json: &mut Reader<'_>, left: &mut usize) -> Result<Vec<(String, String)>, FormatError> {
    json.start_object("expected an object")?;
    // ALLOC: one pair for each entry of the budget.
    let mut out = Vec::new();
    while let Some(key) = json.next_field()? {
        spend(left, json)?;
        check_name(json, &key)?;
        let key = key.into_owned();
        out.push((key, blob_name(json, "expected a string")?));
    }
    Ok(out)
}

/// An array of strings, each passed to `each`.
fn strings(
    json: &mut Reader<'_>,
    left: &mut usize,
    mut each: impl FnMut(&Reader<'_>, Option<Cow<'_, str>>) -> Result<(), FormatError>,
) -> Result<(), FormatError> {
    match json.expect_token()? {
        Token::StartArray => {}
        _ => return Err(json.error("expected an array")),
    }
    loop {
        match json.expect_token()? {
            Token::EndArray => return Ok(()),
            Token::Str(s) => {
                spend(left, json)?;
                each(json, Some(s))?;
            }
            Token::Null => {
                spend(left, json)?;
                each(json, None)?;
            }
            _ => return Err(json.error("expected a string")),
        }
    }
}

fn snapshots(json: &mut Reader<'_>, left: &mut usize) -> Result<Vec<StockSnapshot>, FormatError> {
    match json.expect_token()? {
        Token::StartArray => {}
        _ => return Err(json.error("snapshots is an array")),
    }
    // ALLOC: one entry for each entry of the budget.
    let mut out = Vec::new();
    loop {
        match json.expect_token()? {
            Token::EndArray => return Ok(out),
            Token::StartObject => {}
            _ => return Err(json.error("a snapshot is an object")),
        }
        spend(left, json)?;
        let (mut name, mut uuid, mut state, mut version) = (None, None, None, None);
        // ALLOC: one pair for each entry of the budget.
        let mut metadata = Vec::new();
        while let Some(field) = json.next_field()? {
            match field.as_ref() {
                "name" => name = Some(string(json, "a name is a string")?.into_owned()),
                "uuid" => uuid = Some(blob_name(json, "a uuid is a string")?),
                "state" => match json.expect_token()? {
                    Token::Int(n) => state = Some(n),
                    _ => return Err(json.error("a state is an integer")),
                },
                "version" => version = Some(string(json, "a version is a string")?.into_owned()),
                "index_metadata_lookup" => metadata = pairs(json, left)?,
                _ => json.skip_value()?,
            }
        }
        let (Some(name), Some(uuid)) = (name, uuid) else {
            return Err(json.error("a snapshot without a name or uuid"));
        };
        out.push(StockSnapshot {
            name,
            uuid,
            state,
            version,
            metadata,
        });
    }
}

fn indices(json: &mut Reader<'_>, left: &mut usize) -> Result<Vec<StockIndex>, FormatError> {
    json.start_object("indices is an object")?;
    // ALLOC: one entry for each entry of the budget.
    let mut out = Vec::new();
    while let Some(name) = json.next_field()? {
        spend(left, json)?;
        if name.is_empty() || name.len() > MAX_INDEX_NAME_BYTES {
            return Err(json.error("an index name of an impossible length"));
        }
        let name = name.into_owned();
        json.start_object("an index is an object")?;
        let (mut id, mut path_type) = (None, 0i64);
        // ALLOC: one entry for each entry of the budget.
        let (mut snaps, mut gens) = (Vec::new(), Vec::new());
        while let Some(field) = json.next_field()? {
            match field.as_ref() {
                "id" => id = Some(blob_name(json, "an id is a string")?),
                "shard_path_type" => match json.expect_token()? {
                    Token::Int(n) => path_type = n,
                    _ => return Err(json.error("a path type is an integer")),
                },
                "snapshots" => strings(json, left, |j, s| {
                    let s = s.ok_or_else(|| j.error("a snapshot uuid is a string"))?;
                    check_name(j, &s)?;
                    snaps.push(s.into_owned());
                    Ok(())
                })?,
                "shard_generations" => strings(json, left, |j, s| {
                    if let Some(s) = &s {
                        check_name(j, s)?;
                    }
                    gens.push(s.map(Cow::into_owned));
                    Ok(())
                })?,
                _ => json.skip_value()?,
            }
        }
        let Some(id) = id else {
            return Err(json.error("an index without an id"));
        };
        out.push(StockIndex {
            name,
            id,
            shard_path_type: path_type,
            snapshots: snaps,
            shard_generations: gens,
        });
    }
    Ok(out)
}
