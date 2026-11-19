//! A frozen view of an index: the record a point in time keeps so the
//! commit it froze stays readable and its files stay undeleted.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/PointInTime.java`
//! (`toBytes`, `fromStream`, `pins`, `isCapturing`, `withExpiry`) and
//! `PointInTimeStore`'s `RELEASED`. The value is JSON, `id`, `index`,
//! `index_uuid` when known, `expires_at`, `capturing` only on a capture
//! marker, then `shards`: one object each with its `shard`, `term`, `files`
//! (name to recorded location) and `lengths` when any is known.
//!
//! Differences from the Java, each stricter: a number the Java would take from
//! a string (`"expires_at":"5"`) is refused, a file named twice in a shard is
//! refused, and a record past [`MAX_POINT_IN_TIME_BYTES`] or a shard past
//! [`MAX_FILES`] files is refused rather than read without a bound. Locations
//! are kept as recorded and not parsed here, as the Java keeps them.

use crate::error::{FormatError, corrupt, len64};
use crate::index_record::MAX_SHARDS;
use crate::json::out::Out;
use crate::json::{Reader, Token};
use crate::manifest::MAX_FILES;

const WHAT: &str = "point in time";

/// The longest record a reader takes. A view of an index of
/// [`MAX_SHARDS`] shards holds a manifest each, so this is generous; a longer
/// one was not written by a store.
pub const MAX_POINT_IN_TIME_BYTES: usize = 16 << 20;

/// The value a released point in time's state register holds, so that a
/// delayed request can neither bring the id back nor release a later view of
/// the same id.
pub const RELEASED: &[u8] = br#"{"_released":true}"#;

/// One file a frozen shard names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PitFile {
    /// The Lucene file name.
    pub name: String,
    /// Where its bytes are, as the commit manifest recorded it.
    pub location: String,
    /// Its length, when the manifest recorded one.
    pub length: Option<u64>,
}

/// One shard's commit as it was when the view was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PitShard {
    /// The shard's number.
    pub shard: u32,
    /// The term the frozen commit was published in.
    pub term: u64,
    /// The commit's files, in name order.
    pub files: Vec<PitFile>,
}

/// A point in time: a view of an index frozen at a moment, and the promise
/// that its bytes stay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointInTime {
    /// The identifier a caller quotes back.
    pub id: String,
    /// The index it froze, by the name the caller spelled.
    pub index: String,
    /// The incarnation it froze, or `None` for a record written before it
    /// was recorded, which matches by name alone.
    pub index_uuid: Option<String>,
    /// When it stops being honoured, in the plane's clock.
    pub expires_at_millis: u64,
    /// Whether this marks a freeze that has begun and not finished: it
    /// names an index, holds no shards, and pins everything of that index.
    pub capturing: bool,
    /// Each shard's frozen commit, in shard order.
    pub shards: Vec<PitShard>,
}

impl PointInTime {
    /// Whether this is the stand-in a listing substitutes for a record it
    /// could not read: it names no index and never expires, and a collector
    /// must read its presence as "delete nothing", since what it holds is
    /// unknowable. [`PointInTime::pins`] does not answer for it.
    pub fn is_placeholder(&self) -> bool {
        self.index.is_empty()
    }

    /// Whether it should no longer be honoured or held at `now_millis`.
    pub fn expired_at(&self, now_millis: u64) -> bool {
        now_millis >= self.expires_at_millis
    }

    /// The same view with a later deadline, everything else unchanged.
    #[must_use]
    pub fn with_expiry(mut self, expires_at_millis: u64) -> Self {
        self.expires_at_millis = expires_at_millis;
        self
    }

    /// Whether this view holds files of the index: by uuid when both sides
    /// know one, by name otherwise, which errs towards pinning.
    pub fn pins(&self, index: &str, uuid: Option<&str>) -> bool {
        self.index == index
            && match (self.index_uuid.as_deref(), uuid) {
                (Some(held), Some(asked)) => held == asked,
                _ => true,
            }
    }

    /// The marker a view's inventory entry holds while the state register
    /// is authoritative: the index it pins, a deadline no collector reads as
    /// past, and no shards.
    pub fn marker(&self) -> Self {
        // ALLOC: the marker's two names, once a create or a keep-alive.
        Self {
            id: self.id.clone(),
            index: self.index.clone(),
            index_uuid: self.index_uuid.clone(),
            expires_at_millis: i64::MAX.unsigned_abs(),
            capturing: true,
            // ALLOC: none, an empty vector holds no memory.
            shards: Vec::new(),
        }
    }

    /// Writes the record as the Java's `toBytes` does into the front of
    /// `buf`, and returns its length.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when it does not fit `buf`, or a deadline
    /// past a signed 64-bit integer.
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        let expiry = i64::try_from(self.expires_at_millis)
            .map_err(|_| corrupt(WHAT, "the deadline is past a signed 64-bit integer"))?;
        let mut out = Out::new(buf);
        out.start_object();
        out.string_field("id", &self.id);
        out.string_field("index", &self.index);
        if let Some(uuid) = &self.index_uuid {
            out.string_field("index_uuid", uuid);
        }
        out.int_field("expires_at", expiry);
        if self.capturing {
            out.bool_field("capturing", true);
        }
        out.start_array_field("shards");
        for shard in &self.shards {
            out.start_object();
            out.int_field("shard", i64::from(shard.shard));
            out.int_field("term", i64::try_from(shard.term).unwrap_or(i64::MAX));
            out.start_object_field("files");
            for file in &shard.files {
                out.string_field(&file.name, &file.location);
            }
            out.end_object();
            if shard.files.iter().any(|f| f.length.is_some()) {
                out.start_object_field("lengths");
                for file in &shard.files {
                    if let Some(length) = file.length {
                        out.int_field(&file.name, i64::try_from(length).unwrap_or(i64::MAX));
                    }
                }
                out.end_object();
            }
            out.end_object();
        }
        out.end_array();
        out.end_object();
        out.finish(WHAT)
    }

    /// Parses a record as the Java's `fromStream`: nonempty `id` and `index`,
    /// an optional nonempty `index_uuid`, a nonnegative `expires_at`, an
    /// optional boolean `capturing`, and `shards`, which are empty exactly
    /// when the record is a capture marker.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for malformed JSON, a repeated field, a
    /// missing or mistyped field, a shard number past [`MAX_SHARDS`] or given
    /// twice, a negative term or length, a length naming no file, or
    /// finished views without shards and capture markers with them;
    /// [`FormatError::TooLarge`] past [`MAX_POINT_IN_TIME_BYTES`] or
    /// [`MAX_FILES`] files in a shard.
    pub fn parse(value: &[u8]) -> Result<Self, FormatError> {
        if value.len() > MAX_POINT_IN_TIME_BYTES {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: len64(MAX_POINT_IN_TIME_BYTES),
                actual: len64(value.len()),
            });
        }
        let mut json = Reader::new(value, WHAT);
        json.start_object("a point in time is an object")?;
        let (mut id, mut index, mut uuid, mut expiry) = (None, None, None, None);
        let (mut capturing, mut shards) = (None, None);
        while let Some(field) = json.next_field()? {
            let repeated = match field.as_ref() {
                "id" => id.replace(nonempty(&mut json)?).is_some(),
                "index" => index.replace(nonempty(&mut json)?).is_some(),
                "index_uuid" => uuid.replace(nonempty(&mut json)?).is_some(),
                "expires_at" => expiry.replace(unsigned(&mut json)?).is_some(),
                "capturing" => {
                    let Token::Bool(flag) = json.expect_token()? else {
                        return Err(json.error("capturing must be a boolean"));
                    };
                    capturing.replace(flag).is_some()
                }
                "shards" => shards.replace(read_shards(&mut json)?).is_some(),
                _ => {
                    json.skip_value()?;
                    false
                }
            };
            if repeated {
                return Err(json.error("a point in time's field is repeated"));
            }
        }
        json.finish()?;
        let capturing = capturing.unwrap_or(false);
        let (Some(id), Some(index), Some(expires_at_millis), Some(shards)) =
            (id, index, expiry, shards)
        else {
            return Err(corrupt(
                WHAT,
                "a point in time names its id, index, deadline and shards",
            ));
        };
        if shards.is_empty() != capturing {
            return Err(corrupt(
                WHAT,
                "finished views must hold shards; capture markers must not",
            ));
        }
        Ok(Self {
            id,
            index,
            index_uuid: uuid,
            expires_at_millis,
            capturing,
            shards,
        })
    }
}

fn nonempty(json: &mut Reader<'_>) -> Result<String, FormatError> {
    match json.expect_token()? {
        Token::Str(s) if !s.is_empty() => Ok(s.into_owned()),
        _ => Err(json.error("a point in time's text field is a nonempty string")),
    }
}

fn unsigned(json: &mut Reader<'_>) -> Result<u64, FormatError> {
    match json.expect_token()? {
        Token::Int(n) => u64::try_from(n).map_err(|_| json.error("a number is negative")),
        _ => Err(json.error("a point in time's number is an integer")),
    }
}

fn read_shards(json: &mut Reader<'_>) -> Result<Vec<PitShard>, FormatError> {
    if json.expect_token()? != Token::StartArray {
        return Err(json.error("shards must be an array"));
    }
    // ALLOC: one entry a shard, at most MAX_SHARDS (checked before each push).
    let mut shards: Vec<PitShard> = Vec::new();
    loop {
        match json.expect_token()? {
            Token::EndArray => break,
            Token::StartObject => {
                let shard = read_shard(json)?;
                if shards.iter().any(|s| s.shard == shard.shard) {
                    return Err(json.error("a shard is given twice"));
                }
                shards.push(shard);
            }
            _ => return Err(json.error("a shard entry is not an object")),
        }
    }
    shards.sort_unstable_by_key(|s| s.shard);
    Ok(shards)
}

fn read_shard(json: &mut Reader<'_>) -> Result<PitShard, FormatError> {
    let (mut number, mut term, mut files, mut lengths) = (None, None, None, None);
    while let Some(field) = json.next_field()? {
        let repeated = match field.as_ref() {
            "shard" => number.replace(unsigned(json)?).is_some(),
            "term" => term.replace(unsigned(json)?).is_some(),
            "files" => files.replace(read_map(json, read_location)?).is_some(),
            "lengths" => lengths.replace(read_map(json, read_length)?).is_some(),
            _ => {
                json.skip_value()?;
                false
            }
        };
        if repeated {
            return Err(json.error("a shard's field is repeated"));
        }
    }
    let shard = number
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n < MAX_SHARDS)
        .ok_or_else(|| json.error("a shard number is 0 to 4095"))?;
    let (Some(term), Some(mut files)) = (term, files) else {
        return Err(json.error("a shard holds its term and files"));
    };
    files.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    if files.windows(2).any(|w| matches!(w, [a, b] if a.0 == b.0)) {
        return Err(json.error("a file is named twice"));
    }
    // ALLOC: one entry a file, at most MAX_FILES (read_map's bound).
    let mut files: Vec<PitFile> = files
        .into_iter()
        .map(|(name, location)| PitFile {
            name,
            location,
            length: None,
        })
        // ALLOC: one entry a file, at most MAX_FILES.
        .collect();
    for (name, length) in lengths.unwrap_or_default() {
        let at = files.binary_search_by(|f| f.name.as_str().cmp(&name));
        let Some(file) = at.ok().and_then(|at| files.get_mut(at)) else {
            return Err(json.error("a length names a file the shard does not"));
        };
        file.length = Some(length);
    }
    Ok(PitShard { shard, term, files })
}

fn read_location(json: &mut Reader<'_>) -> Result<String, FormatError> {
    match json.expect_token()? {
        Token::Str(s) if !s.is_empty() => Ok(s.into_owned()),
        _ => Err(json.error("a file location is a nonempty string")),
    }
}

fn read_length(json: &mut Reader<'_>) -> Result<u64, FormatError> {
    unsigned(json)
}

fn read_map<T>(
    json: &mut Reader<'_>,
    value: fn(&mut Reader<'_>) -> Result<T, FormatError>,
) -> Result<Vec<(String, T)>, FormatError> {
    if json.expect_token()? != Token::StartObject {
        return Err(json.error("files and lengths are objects"));
    }
    // ALLOC: one entry a file, at most MAX_FILES (checked before each push).
    let mut entries: Vec<(String, T)> = Vec::new();
    while let Some(name) = json.next_field()? {
        if name.is_empty() {
            return Err(json.error("a file name is nonempty"));
        }
        if entries.len() >= MAX_FILES {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: len64(MAX_FILES),
                actual: len64(entries.len().saturating_add(1)),
            });
        }
        entries.push((name.into_owned(), value(json)?));
    }
    Ok(entries)
}
