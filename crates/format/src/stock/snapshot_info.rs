//! A snapshot's own blob (`snap-<uuid>.dat`): what a restore checks before
//! it reads any index of it.
//!
//! Ported from `SnapshotInfo.fromXContentInternal`, reading the state, the
//! shard counts and failures, whether the global state is included, whether
//! the copy is a remote-store shallow one and the indices. Everything else is
//! skipped.

use crate::error::{FormatError, corrupt};
use crate::json::{Reader, Token};

const WHAT: &str = "stock snapshot";

/// The most indices one snapshot may list.
pub const MAX_INDICES: usize = 10_000;

/// OpenSearch writes a version id with this bit set so that it never equals
/// an Elasticsearch one.
const OPENSEARCH_MASK: i64 = 0x0800_0000;
/// The oldest supported version, 3.0.0, without the mask.
const OLDEST: i64 = 3_000_099;
/// The newest supported version, 3.10.99, without the mask.
const NEWEST: i64 = 3_109_999;

/// A snapshot's own record, as a restore reads it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StockSnapshotInfo {
    /// The snapshot's name.
    pub name: String,
    /// The snapshot's uuid.
    pub uuid: String,
    /// The OpenSearch version id that wrote it, the mask included.
    pub version_id: i64,
    /// `SUCCESS`, `PARTIAL`, `FAILED` or `IN_PROGRESS`.
    pub state: String,
    /// Whether the cluster's global state was included, when stated.
    pub include_global_state: Option<bool>,
    /// Whether the copy is a remote-store shallow one.
    pub remote_store_shallow: bool,
    /// The names of the indices it holds.
    pub indices: Vec<String>,
    /// How many shards it covered.
    pub total_shards: u64,
    /// How many of them it holds.
    pub successful_shards: u64,
    /// How many shard failures it lists.
    pub failures: usize,
}

impl StockSnapshotInfo {
    /// Reads the JSON of a snapshot blob.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a shape the Java does not write and
    /// [`FormatError::TooLarge`] past [`MAX_INDICES`].
    pub fn parse(json: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(json, WHAT);
        r.start_object("a snapshot blob is an object")?;
        let mut out = Self::default();
        let mut seen = false;
        while let Some(field) = r.next_field()? {
            if field != "snapshot" || seen {
                r.skip_value()?;
                continue;
            }
            seen = true;
            r.start_object("a snapshot is an object")?;
            while let Some(key) = r.next_field()? {
                match key.as_ref() {
                    "name" => out.name = string(&mut r)?,
                    "uuid" => out.uuid = string(&mut r)?,
                    "state" => out.state = string(&mut r)?,
                    "version_id" => out.version_id = int(&mut r)?,
                    "total_shards" => out.total_shards = count(&mut r)?,
                    "successful_shards" => out.successful_shards = count(&mut r)?,
                    "include_global_state" => out.include_global_state = flag(&mut r)?,
                    "remote_store_index_shallow_copy" => {
                        out.remote_store_shallow = flag(&mut r)? == Some(true);
                    }
                    "indices" => out.indices = names(&mut r)?,
                    "failures" => out.failures = array_len(&mut r)?,
                    _ => r.skip_value()?,
                }
            }
        }
        r.finish()?;
        if !seen || out.name.is_empty() || out.uuid.is_empty() || out.state.is_empty() {
            return Err(corrupt(WHAT, "a snapshot without a name, uuid or state"));
        }
        Ok(out)
    }

    /// Why a restore refuses this snapshot, when it does.
    #[must_use]
    pub fn refusal(&self) -> Option<&'static str> {
        if self.state != "SUCCESS" {
            return Some("the snapshot is not complete (a partial, failed or running snapshot)");
        }
        if self.failures > 0 || self.successful_shards != self.total_shards {
            return Some("the snapshot has shards that failed");
        }
        if self.include_global_state == Some(true) {
            return Some("the snapshot includes the global state");
        }
        if self.remote_store_shallow {
            return Some("the snapshot is a remote-store shallow copy");
        }
        let id = self.version_id ^ OPENSEARCH_MASK;
        // An id without the mask set lands far above NEWEST once it is flipped.
        if !(OLDEST..=NEWEST).contains(&id) {
            return Some(
                "the snapshot was written by an OpenSearch version this node does not read",
            );
        }
        None
    }
}

fn string(r: &mut Reader<'_>) -> Result<String, FormatError> {
    match r.expect_token()? {
        // ALLOC: one name, bounded by the decoded blob.
        Token::Str(s) => Ok(s.into_owned()),
        _ => Err(r.error("a snapshot field is a string")),
    }
}

fn int(r: &mut Reader<'_>) -> Result<i64, FormatError> {
    match r.expect_token()? {
        Token::Int(n) => Ok(n),
        _ => Err(r.error("a snapshot field is an integer")),
    }
}

fn count(r: &mut Reader<'_>) -> Result<u64, FormatError> {
    u64::try_from(int(r)?).map_err(|_| r.error("a shard count is negative"))
}

fn flag(r: &mut Reader<'_>) -> Result<Option<bool>, FormatError> {
    match r.expect_token()? {
        Token::Bool(b) => Ok(Some(b)),
        Token::Null => Ok(None),
        _ => Err(r.error("a snapshot flag is a boolean")),
    }
}

fn names(r: &mut Reader<'_>) -> Result<Vec<String>, FormatError> {
    if r.expect_token()? != Token::StartArray {
        return Err(r.error("a snapshot's indices are an array"));
    }
    // ALLOC: one name an index, at most MAX_INDICES.
    let mut out = Vec::new();
    loop {
        match r.expect_token()? {
            Token::EndArray => return Ok(out),
            Token::Str(s) => {
                if out.len() >= MAX_INDICES {
                    return Err(FormatError::TooLarge {
                        what: WHAT,
                        limit: MAX_INDICES as u64,
                        actual: (MAX_INDICES as u64).saturating_add(1),
                    });
                }
                // ALLOC: one name, bounded by the decoded blob.
                out.push(s.into_owned());
            }
            _ => return Err(r.error("an index name is a string")),
        }
    }
}

/// The length of an array of anything, each element skipped.
fn array_len(r: &mut Reader<'_>) -> Result<usize, FormatError> {
    if r.expect_token()? != Token::StartArray {
        return Err(r.error("failures is an array"));
    }
    let mut n = 0usize;
    loop {
        let first = r.expect_token()?;
        if first == Token::EndArray {
            return Ok(n);
        }
        r.skip_after(&first)?;
        n = n.saturating_add(1);
    }
}
