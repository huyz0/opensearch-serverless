//! A shard's head: the register that says who owns the shard, and at what
//! term.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/ShardHead.java`
//! (`toBytes`, `fromStream`, `isHeldAt`). The Java writes compact JSON with
//! `XContentBuilder`: `index`, `shard`, `term`, the owner's node and
//! ephemeral ids (null when unowned), the stamp, then `index_uuid` and
//! `writers` only when set. Unknown fields are skipped, for a head written
//! by a newer node.
//!
//! Where the Java coerces a JSON type it never writes there (a number as
//! text), or keeps the last of a repeated field, this refuses: such a head
//! came from no shell node.

use crate::error::FormatError;
use crate::json::out::Out;
use crate::json::{Reader, Token};

const WHAT: &str = "shard head";

/// The longest head read or written: room for the names and a long list of
/// writers. A longer one is refused.
pub const MAX_SHARD_HEAD_BYTES: usize = 16 * 1024;

/// The most writers a head names. A take adds the owner it takes from, so
/// the list would otherwise grow with every dead owner; past this the
/// oldest are dropped, which only costs a successor a log it would have
/// read for records long since published.
pub const MAX_WRITERS: usize = 64;

/// A shard head, as the Java's `ShardHead` holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardHead {
    /// The index's name.
    pub index: String,
    /// The shard number.
    pub shard: u32,
    /// The term ownership was acquired at; positive.
    pub term: i64,
    /// The owning node, or `None` when unowned.
    pub owner_node_id: Option<String>,
    /// The owner's ephemeral id.
    pub owner_ephemeral_id: Option<String>,
    /// When the owner's claim lapses, epoch milliseconds.
    pub lease_expires_at_millis: i64,
    /// The incarnation of the index the head was acquired for.
    pub index_uuid: Option<String>,
    /// The nodes whose logs may hold the shard's unpublished records,
    /// oldest first.
    pub writers: Vec<String>,
}

impl ShardHead {
    /// Whether the owner's claim is live by its own stamp: owned and not
    /// yet expired at `now_millis`.
    pub fn is_held_at(&self, now_millis: i64) -> bool {
        self.owner_node_id.is_some() && now_millis < self.lease_expires_at_millis
    }

    /// Whether the head names this process as its owner. A head with no
    /// ephemeral id names the node alone.
    pub fn owned_by(&self, node_id: &str, ephemeral_id: &str) -> bool {
        self.owner_node_id.as_deref() == Some(node_id)
            && self
                .owner_ephemeral_id
                .as_deref()
                .is_none_or(|e| e == ephemeral_id)
    }

    /// This head naming `writer` as the newest node whose log may hold the
    /// shard's unpublished records (`withWriter`): moved to the end if it
    /// was already named.
    #[must_use]
    pub fn with_writer(mut self, writer: Option<&str>) -> Self {
        if let Some(writer) = writer {
            self.writers.retain(|w| w != writer);
            // ALLOC: one name, within MAX_SHARD_HEAD_BYTES.
            self.writers.push(writer.to_owned());
            let excess = self.writers.len().saturating_sub(MAX_WRITERS);
            self.writers.drain(..excess);
        }
        self
    }

    /// Writes the head as the Java's `toBytes`, into the front of `buf`,
    /// and returns its length.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when `buf` is shorter than the head.
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        let mut out = Out::new(buf);
        out.start_object();
        out.string_field("index", &self.index);
        out.int_field("shard", i64::from(self.shard));
        out.int_field("term", self.term);
        match &self.owner_node_id {
            Some(id) => out.string_field("owner_node_id", id),
            None => out.null_field("owner_node_id"),
        }
        match &self.owner_ephemeral_id {
            Some(id) => out.string_field("owner_ephemeral_id", id),
            None => out.null_field("owner_ephemeral_id"),
        }
        out.int_field("lease_expires_at_millis", self.lease_expires_at_millis);
        if let Some(uuid) = &self.index_uuid {
            out.string_field("index_uuid", uuid);
        }
        if !self.writers.is_empty() {
            out.start_array_field("writers");
            for writer in &self.writers {
                out.string(writer);
            }
            out.end_array();
        }
        out.end_object();
        out.finish(WHAT)
    }

    /// The head's bytes, as a register value.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] past [`MAX_SHARD_HEAD_BYTES`]: a head that
    /// long would not read back, so none is written.
    pub fn to_bytes(&self) -> Result<Vec<u8>, FormatError> {
        // ALLOC: one register value, at most MAX_SHARD_HEAD_BYTES.
        let mut buf = vec![0u8; MAX_SHARD_HEAD_BYTES];
        let n = self.write(&mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    /// Parses a head as the Java's `fromStream` does.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] past [`MAX_SHARD_HEAD_BYTES`];
    /// [`FormatError::Corrupt`] for malformed JSON, content after the head,
    /// a missing `index`, a missing or negative `shard`, a term below 1, a
    /// repeated field, or a field of a type the Java never writes there.
    pub fn parse(value: &[u8]) -> Result<Self, FormatError> {
        if value.len() > MAX_SHARD_HEAD_BYTES {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: MAX_SHARD_HEAD_BYTES as u64,
                actual: u64::try_from(value.len()).unwrap_or(u64::MAX),
            });
        }
        let mut json = Reader::new(value, WHAT);
        json.start_object("a shard head is an object")?;
        let mut head = Fields::default();
        while let Some(field) = json.next_field()? {
            head.read(&mut json, &field)?;
        }
        json.finish()?;
        head.head(&json)
    }
}

/// The fields read so far.
#[derive(Default)]
struct Fields {
    seen: u8,
    index: Option<String>,
    shard: Option<i64>,
    term: i64,
    owner: Option<String>,
    ephemeral: Option<String>,
    expires: i64,
    uuid: Option<String>,
    writers: Vec<String>,
}

const FIELDS: [&str; 8] = [
    "index",
    "shard",
    "term",
    "owner_node_id",
    "owner_ephemeral_id",
    "lease_expires_at_millis",
    "index_uuid",
    "writers",
];

impl Fields {
    fn read(&mut self, json: &mut Reader<'_>, field: &str) -> Result<(), FormatError> {
        let Some(at) = FIELDS.iter().position(|f| *f == field) else {
            return json.skip_value();
        };
        let bit = 1u8 << at;
        if self.seen & bit != 0 {
            return Err(json.error("a shard head field is repeated"));
        }
        self.seen |= bit;
        match at {
            0 => self.index = text_or_null(json)?,
            1 => self.shard = Some(int(json)?),
            2 => self.term = int(json)?,
            3 => self.owner = text_or_null(json)?,
            4 => self.ephemeral = text_or_null(json)?,
            5 => self.expires = int(json)?,
            6 => self.uuid = text_or_null(json)?,
            _ => self.writers = writers(json)?,
        }
        Ok(())
    }

    fn head(self, json: &Reader<'_>) -> Result<ShardHead, FormatError> {
        let (Some(index), Some(shard)) = (self.index, self.shard) else {
            return Err(json.error("a shard head needs index and shard"));
        };
        let Ok(shard) = u32::try_from(shard) else {
            return Err(json.error("a shard head's shard is not negative"));
        };
        if self.term < 1 {
            return Err(json.error("a shard head's term is positive"));
        }
        Ok(ShardHead {
            index,
            shard,
            term: self.term,
            owner_node_id: self.owner,
            owner_ephemeral_id: self.ephemeral,
            lease_expires_at_millis: self.expires,
            index_uuid: self.uuid,
            writers: self.writers,
        })
    }
}

fn text_or_null(json: &mut Reader<'_>) -> Result<Option<String>, FormatError> {
    match json.expect_token()? {
        Token::Str(s) => Ok(Some(s.into_owned())),
        Token::Null => Ok(None),
        _ => Err(json.error("a shard head's text field is a string")),
    }
}

fn int(json: &mut Reader<'_>) -> Result<i64, FormatError> {
    match json.expect_token()? {
        Token::Int(n) => Ok(n),
        _ => Err(json.error("a shard head's number is an integer")),
    }
}

fn writers(json: &mut Reader<'_>) -> Result<Vec<String>, FormatError> {
    if json.expect_token()? != Token::StartArray {
        return Err(json.error("writers is an array"));
    }
    // ALLOC: the head's writers, bounded by MAX_SHARD_HEAD_BYTES.
    let mut writers = Vec::new();
    loop {
        match json.expect_token()? {
            Token::EndArray => return Ok(writers),
            Token::Str(s) => writers.push(s.into_owned()),
            _ => return Err(json.error("a writer is a string")),
        }
    }
}
